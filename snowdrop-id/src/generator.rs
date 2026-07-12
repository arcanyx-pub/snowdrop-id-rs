use core::fmt;
use core::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use crate::clock::{Clock, SystemClock};
use crate::epoch::Epoch;
use crate::id::{Id, SEQ_MASK, TS_MASK};
use crate::machine::MachineId;

/// A lock-free Snowdrop ID generator.
///
/// `IdGenerator` is `Send + Sync` and generates through `&self`: share one
/// instance across threads or tasks via `Arc` or a `static`. All state fits
/// in a single atomic word updated with a compare-exchange loop — there is
/// no mutex.
///
/// Per SPEC §5, the generator holds its last-used timestamp if the wall
/// clock steps backwards, and waits out the (rare) exhaustion of the 22-bit
/// sequence within one 1024 ms window.
pub struct IdGenerator<C: Clock = SystemClock> {
    machine_id: MachineId,
    epoch: Epoch,
    clock: C,
    /// `0` = nothing issued yet; otherwise `((last_ts + 1) << 22) | last_seq`.
    state: AtomicU64,
}

const STATE_EMPTY: u64 = 0;

const fn pack(ts: u64, seq: u64) -> u64 {
    ((ts + 1) << 22) | seq
}

const fn unpack(state: u64) -> (u64, u64) {
    ((state >> 22) - 1, state & SEQ_MASK)
}

impl IdGenerator<SystemClock> {
    /// Creates a generator with the default epoch and system clock.
    pub fn new(machine_id: MachineId) -> IdGenerator<SystemClock> {
        IdGenerator::builder(machine_id).build()
    }

    /// Starts building a generator with a custom epoch or clock.
    pub fn builder(machine_id: MachineId) -> IdGeneratorBuilder<SystemClock> {
        IdGeneratorBuilder {
            machine_id,
            epoch: Epoch::DEFAULT,
            clock: SystemClock,
        }
    }
}

impl<C: Clock> IdGenerator<C> {
    /// The machine ID this generator stamps into every ID.
    pub fn machine_id(&self) -> MachineId {
        self.machine_id
    }

    /// The epoch this generator measures timestamps from.
    pub fn epoch(&self) -> Epoch {
        self.epoch
    }

    /// Generates the next ID without ever blocking.
    ///
    /// Returns [`TryGenerateError::SequenceExhausted`] if more than 2²² IDs
    /// were issued in the current 1024 ms window (retry after the window
    /// ends), and [`TryGenerateError::EpochExhausted`] once the 31-bit
    /// timestamp field overflows (permanent).
    pub fn try_generate(&self) -> Result<Id, TryGenerateError> {
        loop {
            let now_ms = self.clock.unix_ms();
            let t_now = now_ms.saturating_sub(self.epoch.unix_ms()) >> 10;
            if t_now > TS_MASK {
                return Err(TryGenerateError::EpochExhausted);
            }

            let current = self.state.load(Ordering::Acquire);
            let (ts, seq) = if current == STATE_EMPTY {
                (t_now, 0)
            } else {
                let (last_ts, last_seq) = unpack(current);
                if t_now > last_ts {
                    (t_now, 0)
                } else if last_seq == SEQ_MASK {
                    // Sequence exhausted in the (possibly held) window:
                    // the caller must wait until the wall clock passes the
                    // end of the `last_ts` window.
                    let window_end_ms = self.epoch.unix_ms() + ((last_ts + 1) << 10);
                    let retry_after =
                        Duration::from_millis(window_end_ms.saturating_sub(now_ms).max(1));
                    return Err(TryGenerateError::SequenceExhausted { retry_after });
                } else {
                    // Same window, or the clock stepped backwards (hold rule).
                    (last_ts, last_seq + 1)
                }
            };

            if self
                .state
                .compare_exchange_weak(current, pack(ts, seq), Ordering::AcqRel, Ordering::Relaxed)
                .is_ok()
            {
                return Ok(Id::from_parts_unchecked(
                    ts,
                    self.machine_id.get() as u64,
                    seq,
                ));
            }
            // Lost the race to another caller; recompute and retry.
        }
    }

    /// Generates the next ID, sleeping through sequence exhaustion.
    ///
    /// The only blocking case is more than 2²² IDs in one 1024 ms window
    /// (a sustained ~4M IDs/s), where it sleeps until the window ends —
    /// less than 1.024 s unless the clock is being held (SPEC §5.3). Errors
    /// only when the timestamp field is permanently exhausted.
    pub fn generate(&self) -> Result<Id, GenerateError> {
        loop {
            match self.try_generate() {
                Ok(id) => return Ok(id),
                Err(TryGenerateError::SequenceExhausted { retry_after }) => {
                    std::thread::sleep(retry_after);
                }
                Err(TryGenerateError::EpochExhausted) => {
                    return Err(GenerateError::EpochExhausted);
                }
            }
        }
    }

    /// Generates the next ID, awaiting instead of blocking the thread on
    /// sequence exhaustion. Semantics are otherwise identical to
    /// [`generate`](Self::generate).
    #[cfg(feature = "tokio")]
    pub async fn generate_async(&self) -> Result<Id, GenerateError> {
        loop {
            match self.try_generate() {
                Ok(id) => return Ok(id),
                Err(TryGenerateError::SequenceExhausted { retry_after }) => {
                    tokio::time::sleep(retry_after).await;
                }
                Err(TryGenerateError::EpochExhausted) => {
                    return Err(GenerateError::EpochExhausted);
                }
            }
        }
    }
}

impl<C: Clock> fmt::Debug for IdGenerator<C> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("IdGenerator")
            .field("machine_id", &self.machine_id)
            .field("epoch", &self.epoch)
            .finish_non_exhaustive()
    }
}

/// Builder for a [`IdGenerator`] with a custom [`Epoch`] or [`Clock`].
#[derive(Debug, Clone)]
pub struct IdGeneratorBuilder<C: Clock = SystemClock> {
    machine_id: MachineId,
    epoch: Epoch,
    clock: C,
}

impl<C: Clock> IdGeneratorBuilder<C> {
    /// Sets the epoch (default: [`Epoch::DEFAULT`]).
    pub fn epoch(mut self, epoch: Epoch) -> IdGeneratorBuilder<C> {
        self.epoch = epoch;
        self
    }

    /// Sets the clock source (default: [`SystemClock`]).
    pub fn clock<D: Clock>(self, clock: D) -> IdGeneratorBuilder<D> {
        IdGeneratorBuilder {
            machine_id: self.machine_id,
            epoch: self.epoch,
            clock,
        }
    }

    /// Builds the generator.
    pub fn build(self) -> IdGenerator<C> {
        IdGenerator {
            machine_id: self.machine_id,
            epoch: self.epoch,
            clock: self.clock,
            state: AtomicU64::new(STATE_EMPTY),
        }
    }
}

/// Error from [`IdGenerator::generate`] / [`IdGenerator::generate_async`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum GenerateError {
    /// The 31-bit timestamp field has overflowed for this epoch; the
    /// generator can never produce another ID. (With the default epoch,
    /// this happens in 2094.)
    EpochExhausted,
}

impl fmt::Display for GenerateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GenerateError::EpochExhausted => {
                f.write_str("timestamp field exhausted for this epoch")
            }
        }
    }
}

impl std::error::Error for GenerateError {}

/// Error from [`IdGenerator::try_generate`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum TryGenerateError {
    /// All 2²² sequence values in the current window are used; retry once
    /// the wall clock has advanced past the end of the window.
    SequenceExhausted {
        /// Time until the current window ends and generation can resume.
        retry_after: Duration,
    },
    /// The 31-bit timestamp field has overflowed for this epoch (permanent).
    EpochExhausted,
}

impl fmt::Display for TryGenerateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TryGenerateError::SequenceExhausted { retry_after } => write!(
                f,
                "sequence exhausted for the current window; retry in {retry_after:?}"
            ),
            TryGenerateError::EpochExhausted => {
                f.write_str("timestamp field exhausted for this epoch")
            }
        }
    }
}

impl std::error::Error for TryGenerateError {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::AtomicU64 as SharedMs;

    /// A test clock whose time is set explicitly.
    #[derive(Clone, Default)]
    struct MockClock(Arc<SharedMs>);

    impl MockClock {
        fn set(&self, unix_ms: u64) {
            self.0.store(unix_ms, Ordering::SeqCst);
        }
    }

    impl Clock for MockClock {
        fn unix_ms(&self) -> u64 {
            self.0.load(Ordering::SeqCst)
        }
    }

    fn mid(v: u16) -> MachineId {
        MachineId::new(v).unwrap()
    }

    fn mock_generator(machine: u16) -> (IdGenerator<MockClock>, MockClock) {
        let clock = MockClock::default();
        clock.set(Epoch::DEFAULT.unix_ms());
        let generator = IdGenerator::builder(mid(machine))
            .clock(clock.clone())
            .build();
        (generator, clock)
    }

    #[test]
    fn stamps_machine_id_and_counts_sequence() {
        let (generator, clock) = mock_generator(7);
        clock.set(Epoch::DEFAULT.unix_ms() + 5 * 1024);
        for expected_seq in 0..100 {
            let id = generator.try_generate().unwrap();
            assert_eq!(id.timestamp(), 5);
            assert_eq!(id.machine_id(), mid(7));
            assert_eq!(id.sequence(), expected_seq);
        }
    }

    #[test]
    fn new_window_resets_sequence() {
        let (generator, clock) = mock_generator(0);
        generator.try_generate().unwrap();
        generator.try_generate().unwrap();
        clock.set(Epoch::DEFAULT.unix_ms() + 1024);
        let id = generator.try_generate().unwrap();
        assert_eq!((id.timestamp(), id.sequence()), (1, 0));
    }

    #[test]
    fn hold_rule_keeps_ids_monotonic_through_clock_regression() {
        let (generator, clock) = mock_generator(0);
        clock.set(Epoch::DEFAULT.unix_ms() + 10 * 1024);
        let before = generator.try_generate().unwrap();
        assert_eq!(before.timestamp(), 10);

        // Clock steps back 8 windows; timestamp must hold at 10.
        clock.set(Epoch::DEFAULT.unix_ms() + 2 * 1024);
        let held = generator.try_generate().unwrap();
        assert_eq!((held.timestamp(), held.sequence()), (10, 1));
        assert!(held > before);

        // Clock recovers past the held window; normal operation resumes.
        clock.set(Epoch::DEFAULT.unix_ms() + 11 * 1024);
        let after = generator.try_generate().unwrap();
        assert_eq!((after.timestamp(), after.sequence()), (11, 0));
        assert!(after > held);
    }

    #[test]
    fn sequence_exhaustion_reports_retry_after() {
        let (generator, clock) = mock_generator(0);
        clock.set(Epoch::DEFAULT.unix_ms() + 512); // mid-window 0
        // Jump the state to one below exhaustion instead of looping 2^22 times.
        generator
            .state
            .store(pack(0, SEQ_MASK - 1), Ordering::SeqCst);

        let id = generator.try_generate().unwrap();
        assert_eq!(id.sequence() as u64, SEQ_MASK);

        let err = generator.try_generate().unwrap_err();
        match err {
            TryGenerateError::SequenceExhausted { retry_after } => {
                assert_eq!(retry_after, Duration::from_millis(512));
            }
            other => panic!("expected SequenceExhausted, got {other:?}"),
        }

        // generate() waits it out (mock clock: advance manually first).
        clock.set(Epoch::DEFAULT.unix_ms() + 1024);
        let id = generator.generate().unwrap();
        assert_eq!((id.timestamp(), id.sequence()), (1, 0));
    }

    #[test]
    fn exhaustion_while_held_reports_full_wait() {
        let (generator, clock) = mock_generator(0);
        clock.set(Epoch::DEFAULT.unix_ms() + 100 * 1024);
        generator.try_generate().unwrap();
        // Clock regresses 50 windows, then the held window exhausts.
        clock.set(Epoch::DEFAULT.unix_ms() + 50 * 1024);
        generator.state.store(pack(100, SEQ_MASK), Ordering::SeqCst);

        match generator.try_generate().unwrap_err() {
            TryGenerateError::SequenceExhausted { retry_after } => {
                // Must wait until the end of window 100: 51 windows away.
                assert_eq!(retry_after, Duration::from_millis(51 * 1024));
            }
            other => panic!("expected SequenceExhausted, got {other:?}"),
        }
    }

    #[test]
    fn epoch_exhaustion_is_permanent() {
        let (generator, clock) = mock_generator(0);
        clock.set(Epoch::DEFAULT.unix_ms() + ((TS_MASK + 1) << 10));
        assert_eq!(
            generator.try_generate(),
            Err(TryGenerateError::EpochExhausted)
        );
        assert_eq!(generator.generate(), Err(GenerateError::EpochExhausted));
    }

    #[test]
    fn last_valid_window_still_generates() {
        let (generator, clock) = mock_generator(0);
        clock.set(Epoch::DEFAULT.unix_ms() + (TS_MASK << 10));
        let id = generator.try_generate().unwrap();
        assert_eq!(id.timestamp() as u64, TS_MASK);
    }

    #[test]
    fn concurrent_generation_is_unique_and_per_thread_monotonic() {
        use std::collections::HashSet;

        let generator = Arc::new(IdGenerator::new(mid(42)));
        let threads = 8;
        let per_thread = 20_000;

        let handles: Vec<_> = (0..threads)
            .map(|_| {
                let generator = Arc::clone(&generator);
                std::thread::spawn(move || {
                    let mut ids = Vec::with_capacity(per_thread);
                    let mut last = None;
                    for _ in 0..per_thread {
                        let id = generator.generate().unwrap();
                        if let Some(prev) = last {
                            assert!(id > prev, "per-caller monotonicity violated");
                        }
                        last = Some(id);
                        ids.push(id);
                    }
                    ids
                })
            })
            .collect();

        let mut all = HashSet::new();
        for handle in handles {
            for id in handle.join().unwrap() {
                assert!(all.insert(id.as_u64()), "duplicate ID generated");
            }
        }
        assert_eq!(all.len(), threads * per_thread);
    }

    #[test]
    fn before_epoch_clamps_to_window_zero() {
        let (generator, clock) = mock_generator(0);
        clock.set(Epoch::DEFAULT.unix_ms().saturating_sub(10_000));
        let id = generator.try_generate().unwrap();
        assert_eq!(id.timestamp(), 0);
    }
}
