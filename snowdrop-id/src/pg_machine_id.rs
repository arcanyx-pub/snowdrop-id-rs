//! Machine-ID leasing via Postgres advisory locks (feature
//! `postgres-machine-id`).
//!
//! Machine IDs must be unique among concurrently active generators. This
//! module leases them from Postgres: a dedicated connection scans the
//! machine-ID range and takes the first free session-level advisory lock —
//! that lock *is* the lease. Session locks release automatically when the
//! connection ends, so a lease can never outlive its process, and scanning
//! from 0 upward hands out the low machine IDs that keep base62 strings
//! short.
//!
//! A background keepalive task pings the connection. If the connection is
//! lost, the task reconnects and re-locks — the *same* machine ID when
//! still free (the common case; generation continues seamlessly), or a new
//! one otherwise. What `generate` does in the window where the lock is not
//! confirmed held is governed by [`LeaseLossPolicy`].
//!
//! Locks are taken with the single-argument (64-bit) form of
//! `pg_try_advisory_lock`, keyed as `namespace + machine_id`. The default
//! namespace is negative, so it cannot collide with application advisory
//! locks keyed by non-negative IDs (such as Snowdrop [`Id`]s themselves).

use core::fmt;
use std::ops::RangeInclusive;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU16, Ordering};
use std::time::Duration;

use sqlx::postgres::{PgConnectOptions, PgConnection};
use sqlx::{ConnectOptions, Connection};

use crate::generator::TryGenerateError;
use crate::{Epoch, GenerateError, Id, IdGenerator, MachineId};

/// Default advisory-lock namespace: a negative base so leases never collide
/// with application advisory locks keyed by non-negative IDs. The magnitude
/// spells "SNOW" (`0x534E4F57`), shifted to leave room for the machine ID.
pub const DEFAULT_NAMESPACE: i64 = -0x534E_4F57_0000;

const RECONNECT_BACKOFF: Duration = Duration::from_secs(1);

/// What [`PgIdGenerator::generate`] does while the advisory lock is not
/// confirmed held (between connection loss and re-acquisition).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum LeaseLossPolicy {
    /// Fail with [`PgGenerateError::MachineIdLeaseLost`] until a lock is
    /// re-held. No collision risk even in partial failures, at the cost of
    /// generation errors during database blips.
    #[default]
    Poison,
    /// Keep generating under the last-held machine ID while the background
    /// task re-acquires. Zero availability impact; a small collision window
    /// exists if only this process's connection failed AND another process
    /// leased the same machine ID AND both generate in the same 1024 ms
    /// window.
    Optimistic,
}

/// Configuration for [`PgMachineIdLease::acquire_with`] /
/// [`PgIdGenerator::acquire_with`].
#[derive(Debug, Clone)]
pub struct PgLeaseConfig {
    namespace: i64,
    machine_ids: RangeInclusive<u16>,
    keepalive_interval: Duration,
    policy: LeaseLossPolicy,
}

impl Default for PgLeaseConfig {
    fn default() -> PgLeaseConfig {
        PgLeaseConfig {
            namespace: DEFAULT_NAMESPACE,
            machine_ids: 0..=1023,
            keepalive_interval: Duration::from_secs(5),
            policy: LeaseLossPolicy::default(),
        }
    }
}

impl PgLeaseConfig {
    /// Creates the default configuration: [`DEFAULT_NAMESPACE`], machine
    /// IDs `0..=1023`, 5-second keepalive, [`LeaseLossPolicy::Poison`].
    pub fn new() -> PgLeaseConfig {
        PgLeaseConfig::default()
    }

    /// Sets the advisory-lock namespace (lock key = `namespace + machine_id`).
    ///
    /// All generators sharing an ID space must use the same namespace;
    /// independent ID spaces in one database cluster should use distinct
    /// namespaces.
    pub fn namespace(mut self, namespace: i64) -> PgLeaseConfig {
        self.namespace = namespace;
        self
    }

    /// Restricts leasing to a subrange of machine IDs (default `0..=1023`),
    /// e.g. to partition the ID space between services.
    ///
    /// # Panics
    ///
    /// Panics if the range end exceeds 1023.
    pub fn machine_ids(mut self, range: RangeInclusive<u16>) -> PgLeaseConfig {
        assert!(
            *range.end() <= 1023,
            "machine ID range end {} exceeds 1023",
            range.end()
        );
        self.machine_ids = range;
        self
    }

    /// Sets how often the lease connection is pinged (default 5 s). Lower
    /// values shorten the window in which a lost lock goes undetected.
    pub fn keepalive_interval(mut self, interval: Duration) -> PgLeaseConfig {
        self.keepalive_interval = interval;
        self
    }

    /// Sets the [`LeaseLossPolicy`] (default [`LeaseLossPolicy::Poison`]).
    pub fn policy(mut self, policy: LeaseLossPolicy) -> PgLeaseConfig {
        self.policy = policy;
        self
    }
}

struct LeaseShared {
    machine_id: AtomicU16,
    poisoned: AtomicBool,
}

/// A leased machine ID, held as a Postgres session advisory lock.
///
/// The lease is a guard: it must be kept alive for as long as any generator
/// uses its machine ID. Dropping it ends the background keepalive task and
/// the dedicated connection, releasing the lock server-side.
///
/// Note that after a connection loss the background task may re-acquire a
/// *different* machine ID; [`machine_id`](Self::machine_id) always returns
/// the current one. A generator built manually from a lease does not follow
/// such changes — use [`PgIdGenerator`] for that — so manual compositions
/// should treat [`is_poisoned`](Self::is_poisoned) as a signal to rebuild.
pub struct PgMachineIdLease {
    shared: Arc<LeaseShared>,
    policy: LeaseLossPolicy,
    task: tokio::task::JoinHandle<()>,
}

impl PgMachineIdLease {
    /// Acquires a lease with the default [`PgLeaseConfig`], using a
    /// dedicated connection built from `options`.
    ///
    /// (A dedicated connection — not a pool — is required: session advisory
    /// locks belong to one server session, and a pooled connection would be
    /// reclaimed and reused.)
    pub async fn acquire(options: PgConnectOptions) -> Result<PgMachineIdLease, PgLeaseError> {
        PgMachineIdLease::acquire_with(options, PgLeaseConfig::default()).await
    }

    /// Acquires a lease with a custom [`PgLeaseConfig`].
    ///
    /// Scans `config.machine_ids` from low to high and leases the first
    /// machine ID whose advisory lock is free. Fails with
    /// [`PgLeaseError::NoMachineIdAvailable`] if every ID in the range is
    /// already leased.
    pub async fn acquire_with(
        options: PgConnectOptions,
        config: PgLeaseConfig,
    ) -> Result<PgMachineIdLease, PgLeaseError> {
        let mut conn = options.connect().await?;
        let Some(mid) = lock_first_free(&mut conn, &config).await? else {
            return Err(PgLeaseError::NoMachineIdAvailable);
        };
        let shared = Arc::new(LeaseShared {
            machine_id: AtomicU16::new(mid),
            poisoned: AtomicBool::new(false),
        });
        let policy = config.policy;
        let task = tokio::spawn(keepalive(conn, options, config, Arc::clone(&shared)));
        Ok(PgMachineIdLease {
            shared,
            policy,
            task,
        })
    }

    /// The currently leased machine ID. May change after a connection loss
    /// if the previous ID could not be re-acquired.
    pub fn machine_id(&self) -> MachineId {
        MachineId::new(self.shared.machine_id.load(Ordering::Acquire))
            .expect("leased machine ID is always in range")
    }

    /// `true` while the advisory lock is not confirmed held (connection
    /// lost, re-acquisition in progress).
    pub fn is_poisoned(&self) -> bool {
        self.shared.poisoned.load(Ordering::Acquire)
    }

    /// The configured [`LeaseLossPolicy`].
    pub fn policy(&self) -> LeaseLossPolicy {
        self.policy
    }
}

impl Drop for PgMachineIdLease {
    fn drop(&mut self) {
        // Ends the keepalive task and drops its connection; the server
        // releases the advisory lock when the session terminates.
        self.task.abort();
    }
}

impl fmt::Debug for PgMachineIdLease {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PgMachineIdLease")
            .field("machine_id", &self.machine_id())
            .field("poisoned", &self.is_poisoned())
            .field("policy", &self.policy)
            .finish()
    }
}

async fn try_lock(conn: &mut PgConnection, key: i64) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar("SELECT pg_try_advisory_lock($1)")
        .bind(key)
        .fetch_one(conn)
        .await
}

async fn lock_first_free(
    conn: &mut PgConnection,
    config: &PgLeaseConfig,
) -> Result<Option<u16>, sqlx::Error> {
    for mid in config.machine_ids.clone() {
        if try_lock(conn, config.namespace + i64::from(mid)).await? {
            return Ok(Some(mid));
        }
    }
    Ok(None)
}

/// Pings the lease connection; on failure, reconnects and re-locks (same
/// machine ID first, any free one otherwise), flipping `poisoned` around
/// the unconfirmed window.
async fn keepalive(
    mut conn: PgConnection,
    options: PgConnectOptions,
    config: PgLeaseConfig,
    shared: Arc<LeaseShared>,
) {
    loop {
        tokio::time::sleep(config.keepalive_interval).await;
        if conn.ping().await.is_ok() {
            continue;
        }
        shared.poisoned.store(true, Ordering::Release);

        'reacquire: loop {
            let Ok(mut new_conn) = options.connect().await else {
                tokio::time::sleep(RECONNECT_BACKOFF).await;
                continue 'reacquire;
            };
            let held = shared.machine_id.load(Ordering::Acquire);
            match try_lock(&mut new_conn, config.namespace + i64::from(held)).await {
                Ok(true) => {
                    conn = new_conn;
                    shared.poisoned.store(false, Ordering::Release);
                    break 'reacquire;
                }
                Ok(false) => match lock_first_free(&mut new_conn, &config).await {
                    Ok(Some(mid)) => {
                        shared.machine_id.store(mid, Ordering::Release);
                        conn = new_conn;
                        shared.poisoned.store(false, Ordering::Release);
                        break 'reacquire;
                    }
                    // Range exhausted or connection failed again: retry.
                    Ok(None) | Err(_) => tokio::time::sleep(RECONNECT_BACKOFF).await,
                },
                Err(_) => tokio::time::sleep(RECONNECT_BACKOFF).await,
            }
        }
    }
}

/// An [`IdGenerator`] whose machine ID is leased via a Postgres advisory
/// lock, for clusters with no static machine-ID assignment.
///
/// Owns a [`PgMachineIdLease`] and follows it: if the lease re-acquires a
/// different machine ID after a connection loss, the generator swaps to it
/// transparently. Behavior while the lock is not confirmed held is set by
/// the configured [`LeaseLossPolicy`] (default: fail with
/// [`PgGenerateError::MachineIdLeaseLost`]).
pub struct PgIdGenerator {
    lease: PgMachineIdLease,
    epoch: Epoch,
    inner: std::sync::RwLock<IdGenerator>,
}

impl PgIdGenerator {
    /// Acquires a machine-ID lease with defaults and wraps it in a
    /// generator using the default [`Epoch`].
    pub async fn acquire(options: PgConnectOptions) -> Result<PgIdGenerator, PgLeaseError> {
        PgIdGenerator::acquire_with(options, PgLeaseConfig::default(), Epoch::DEFAULT).await
    }

    /// Acquires with a custom [`PgLeaseConfig`] and [`Epoch`].
    pub async fn acquire_with(
        options: PgConnectOptions,
        config: PgLeaseConfig,
        epoch: Epoch,
    ) -> Result<PgIdGenerator, PgLeaseError> {
        let lease = PgMachineIdLease::acquire_with(options, config).await?;
        let inner = IdGenerator::builder(lease.machine_id())
            .epoch(epoch)
            .build();
        Ok(PgIdGenerator {
            lease,
            epoch,
            inner: std::sync::RwLock::new(inner),
        })
    }

    /// Generates the next ID (see [`IdGenerator::generate`]).
    ///
    /// With [`LeaseLossPolicy::Poison`], fails with
    /// [`PgGenerateError::MachineIdLeaseLost`] while the advisory lock is
    /// not confirmed held.
    pub fn generate(&self) -> Result<Id, PgGenerateError> {
        self.check_poisoned()?;
        self.with_inner(|inner| inner.generate())
            .map_err(PgGenerateError::from)
    }

    /// Generates the next ID, awaiting instead of blocking the thread on
    /// (rare) sequence exhaustion (see [`IdGenerator::generate_async`]).
    pub async fn generate_async(&self) -> Result<Id, PgGenerateError> {
        loop {
            self.check_poisoned()?;
            match self.with_inner(|inner| inner.try_generate()) {
                Ok(id) => return Ok(id),
                Err(TryGenerateError::SequenceExhausted { retry_after }) => {
                    tokio::time::sleep(retry_after).await;
                }
                Err(TryGenerateError::EpochExhausted) => {
                    return Err(PgGenerateError::EpochExhausted);
                }
            }
        }
    }

    /// The currently leased machine ID (may change after connection loss).
    pub fn machine_id(&self) -> MachineId {
        self.lease.machine_id()
    }

    /// The epoch IDs are generated against.
    pub fn epoch(&self) -> Epoch {
        self.epoch
    }

    /// The underlying lease.
    pub fn lease(&self) -> &PgMachineIdLease {
        &self.lease
    }

    fn check_poisoned(&self) -> Result<(), PgGenerateError> {
        if self.lease.policy == LeaseLossPolicy::Poison && self.lease.is_poisoned() {
            return Err(PgGenerateError::MachineIdLeaseLost);
        }
        Ok(())
    }

    /// Runs `f` on the inner generator, first rebuilding it if the lease's
    /// machine ID has changed. Never holds the lock across an await point.
    fn with_inner<R>(&self, f: impl FnOnce(&IdGenerator) -> R) -> R {
        let mid = self.lease.machine_id();
        {
            let inner = self.inner.read().expect("generator lock poisoned");
            if inner.machine_id() == mid {
                return f(&inner);
            }
        }
        let mut inner = self.inner.write().expect("generator lock poisoned");
        if inner.machine_id() != mid {
            *inner = IdGenerator::builder(mid).epoch(self.epoch).build();
        }
        f(&inner)
    }
}

impl fmt::Debug for PgIdGenerator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PgIdGenerator")
            .field("lease", &self.lease)
            .field("epoch", &self.epoch)
            .finish_non_exhaustive()
    }
}

/// Error acquiring a [`PgMachineIdLease`].
#[derive(Debug)]
#[non_exhaustive]
pub enum PgLeaseError {
    /// The database connection or lock query failed.
    Database(sqlx::Error),
    /// Every machine ID in the configured range is already leased.
    NoMachineIdAvailable,
}

impl fmt::Display for PgLeaseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PgLeaseError::Database(e) => write!(f, "machine-ID lease query failed: {e}"),
            PgLeaseError::NoMachineIdAvailable => {
                f.write_str("no machine ID available: every ID in the range is leased")
            }
        }
    }
}

impl std::error::Error for PgLeaseError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            PgLeaseError::Database(e) => Some(e),
            PgLeaseError::NoMachineIdAvailable => None,
        }
    }
}

impl From<sqlx::Error> for PgLeaseError {
    fn from(e: sqlx::Error) -> PgLeaseError {
        PgLeaseError::Database(e)
    }
}

/// Error from [`PgIdGenerator::generate`] / [`PgIdGenerator::generate_async`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum PgGenerateError {
    /// The 31-bit timestamp field has overflowed for this epoch (permanent).
    EpochExhausted,
    /// The advisory lock is not confirmed held and the lease uses
    /// [`LeaseLossPolicy::Poison`]; retry after re-acquisition.
    MachineIdLeaseLost,
}

impl From<GenerateError> for PgGenerateError {
    fn from(e: GenerateError) -> PgGenerateError {
        match e {
            GenerateError::EpochExhausted => PgGenerateError::EpochExhausted,
        }
    }
}

impl fmt::Display for PgGenerateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PgGenerateError::EpochExhausted => {
                f.write_str("timestamp field exhausted for this epoch")
            }
            PgGenerateError::MachineIdLeaseLost => {
                f.write_str("machine-ID lease lost; waiting for re-acquisition")
            }
        }
    }
}

impl std::error::Error for PgGenerateError {}
