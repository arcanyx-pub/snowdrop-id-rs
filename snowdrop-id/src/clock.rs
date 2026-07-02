use std::time::{SystemTime, UNIX_EPOCH};

/// A millisecond wall-clock source.
///
/// [`Generator`](crate::Generator) is generic over this trait so tests can
/// drive the hold rule, sequence exhaustion, and clock-regression behavior
/// deterministically. Production code uses the default [`SystemClock`].
pub trait Clock: Send + Sync {
    /// Returns the current time as Unix milliseconds.
    fn unix_ms(&self) -> u64;
}

/// The system wall clock ([`SystemTime::now`]).
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn unix_ms(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    }
}
