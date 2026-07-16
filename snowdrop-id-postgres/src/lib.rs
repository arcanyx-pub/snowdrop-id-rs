//! Postgres-backed machine-ID leasing for `snowdrop-id` generators.
//!
//! Machine IDs must be unique among concurrently active generators. This
//! crate leases them from a small Postgres table
//! (`public.snowdrop_machine_id_leases` by default): a worker claims the lowest
//! free machine ID, then a background task heartbeats to keep the lease alive.
//! Every operation is a single autocommit statement with no session state, so —
//! unlike session advisory locks — it works through a connection pool and under
//! any pgBouncer pooling mode, and survives a primary failover.
//!
//! Provisioning fits three workflows: `auto_provision(true)` creates and seeds
//! the table on boot (needs DDL rights); or run
//! [`PgMachineIdLease::schema_sql`] and [`PgMachineIdLease::seeding_sql`] in
//! your own migrations. See `docs/pg-machine-id-leasing.md` for the full design
//! rationale. The safety-critical points:
//!
//! - **Liveness is holder-declared.** Each lease row stores `reclaimable_after`,
//!   a deadline the holder pushes forward on every heartbeat. A claimer steals a
//!   row only once `NOW()` passes that deadline, so no claimer applies its own
//!   timing policy to another worker's lease.
//! - **Self-poison, not just fencing.** A fencing token ([`claimed_at`]) lets a
//!   heartbeat detect that a lease was stolen, but generation never touches the
//!   database, so detection alone is too slow. The generator therefore refuses
//!   to generate ([`PgGenerateError::MachineIdLeaseLost`]) once it cannot prove
//!   its lease is fresh — measured against both a monotonic and a wall clock, so
//!   a VM suspend cannot mask an expired lease.
//!
//! [`claimed_at`]: https://www.postgresql.org/docs/current/functions-datetime.html

#![warn(missing_docs)]
#![forbid(unsafe_code)]

use core::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU16, AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use sqlx::PgPool;

use snowdrop_id::{Epoch, Id, IdGenerator, MachineId, TryGenerateError};

/// Default schema for the lease table: `public` (the connection's default), so
/// a lease needs only create-table rights — not `CREATE` on the database for a
/// dedicated schema. Override with
/// [`schema_name`](PgMachineIdLeaseBuilder::schema_name).
pub const DEFAULT_SCHEMA: &str = "public";

/// Fixed, `snowdrop_`-prefixed lease table name (unqualified). The prefix keeps
/// it collision-safe in a shared schema like `public`.
const LEASES_TABLE: &str = "snowdrop_machine_id_leases";

/// The lease table's column definition, shared by every DDL flavor so the shape
/// is defined in one place.
const LEASES_COLUMNS: &str = "machine_id SMALLINT PRIMARY KEY, \
                              claimed_at TIMESTAMPTZ, \
                              reclaimable_after TIMESTAMPTZ";

// --- Timing constants (fixed in v0.2; see docs §6). ------------------------

/// How often the background task renews the lease.
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(10 * 60);
/// Reclaim TTL written into `reclaimable_after` on each heartbeat, in seconds.
const RECLAIM_TTL_SECS: i64 = 30 * 60;
/// Local lease age past which the generator self-poisons: `RECLAIM_TTL` minus a
/// margin covering worker/DB clock skew plus in-flight generation.
const SELF_POISON_AFTER: Duration = Duration::from_secs((RECLAIM_TTL_SECS as u64) - 5 * 60);
/// Short deadline written at claim time, so an ID claimed by a worker that
/// crashes before its first heartbeat frees quickly. In seconds.
const BOOTLOOP_GRACE_SECS: i64 = 60;
/// Self-poison horizon for the brief window between a fresh claim and the first
/// heartbeat; strictly inside `BOOTLOOP_GRACE_SECS`.
const BOOTLOOP_SELF_POISON: Duration = Duration::from_secs(45);
/// Delay before the first heartbeat after a fresh claim; strictly inside the
/// bootloop grace so a healthy worker extends its deadline in time.
const FIRST_HEARTBEAT_DELAY: Duration = Duration::from_secs(20);
/// Retry delay after a transient heartbeat error (deadlines are left untouched;
/// the age model still governs poisoning).
const HEARTBEAT_RETRY_BACKOFF: Duration = Duration::from_secs(30);
/// Retry delay while re-claiming after a lost lease.
const RECLAIM_RETRY_BACKOFF: Duration = Duration::from_secs(1);

// Compile-time proof of the timing invariants (docs §6.4).
const _: () = assert!(FIRST_HEARTBEAT_DELAY.as_secs() < BOOTLOOP_GRACE_SECS as u64);
const _: () = assert!(BOOTLOOP_SELF_POISON.as_secs() < BOOTLOOP_GRACE_SECS as u64);
const _: () = assert!(FIRST_HEARTBEAT_DELAY.as_secs() < BOOTLOOP_SELF_POISON.as_secs());
const _: () = assert!(HEARTBEAT_INTERVAL.as_secs() * 2 <= SELF_POISON_AFTER.as_secs());
const _: () = assert!(SELF_POISON_AFTER.as_secs() < RECLAIM_TTL_SECS as u64);
const _: () = assert!((RECLAIM_TTL_SECS as u64) - SELF_POISON_AFTER.as_secs() >= 60);

/// SQL expression deriving the fencing token from `claimed_at`: a millisecond,
/// timezone-independent `bigint` (docs §5). `claimed_at` is stable for a lease's
/// life and rotates only on a steal, so this identifies one claim of a row.
const FENCING: &str = "to_char(claimed_at AT TIME ZONE 'UTC', 'YYYYMMDDHH24MISSMS')::bigint";

// --- Clocks & lease freshness state. ---------------------------------------

fn unix_millis() -> u64 {
    // On a pre-1970 wall clock this returns 0, which makes the wall-clock
    // freshness check in `is_fresh` always pass and leaves only the monotonic
    // deadline in force. That is an absurd clock state (it breaks TLS, NTP, and
    // much else), and the monotonic clock still bounds lease age, so we accept
    // the degraded — but not unsafe — check rather than complicating the API.
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// State shared between a [`PgMachineIdLease`] and its background task.
struct LeaseShared {
    /// The currently leased machine ID (may change after a steal + re-claim).
    machine_id: AtomicU16,
    /// The current lease's fencing token, for heartbeat/release `WHERE` clauses.
    fencing: AtomicI64,
    /// Set when a lease is known lost (fencing mismatch) and re-acquisition is
    /// in progress. Time-based expiry is tracked separately by the deadlines.
    poisoned: AtomicBool,
    /// Monotonic self-poison deadline, as nanoseconds since `base`.
    mono_deadline_nanos: AtomicU64,
    /// Wall-clock self-poison deadline, as Unix milliseconds.
    wall_deadline_ms: AtomicU64,
    /// Fixed monotonic reference captured at construction.
    base: Instant,
}

impl LeaseShared {
    /// Records a fresh lease valid for `valid_for` from now, on both clocks.
    fn refresh_deadlines(&self, valid_for: Duration) {
        let mono = self.base.elapsed().saturating_add(valid_for);
        self.mono_deadline_nanos
            .store(mono.as_nanos() as u64, Ordering::Release);
        self.wall_deadline_ms.store(
            unix_millis() + valid_for.as_millis() as u64,
            Ordering::Release,
        );
    }

    /// `true` while the lease is provably fresh on both clocks and not poisoned.
    fn is_fresh(&self) -> bool {
        if self.poisoned.load(Ordering::Acquire) {
            return false;
        }
        let now_mono = self.base.elapsed().as_nanos() as u64;
        if now_mono >= self.mono_deadline_nanos.load(Ordering::Acquire) {
            return false;
        }
        if unix_millis() >= self.wall_deadline_ms.load(Ordering::Acquire) {
            return false;
        }
        true
    }
}

// --- Schema-name validation & SQL construction. ----------------------------

/// Validates a schema name. It is interpolated as a (quoted) SQL identifier —
/// schema names cannot be bound parameters — so it must be a single allowlisted
/// identifier: ASCII letters/digits/underscore, not starting with a digit, at
/// most 63 bytes. Quoting lets reserved words (e.g. `order`) work and makes
/// matching case-sensitive.
fn validate_schema_name(name: &str) -> Result<(), PgLeaseError> {
    let invalid = || PgLeaseError::InvalidSchemaName(name.to_string());
    if name.is_empty() || name.len() > 63 {
        return Err(invalid());
    }
    let mut chars = name.chars();
    let first = chars.next().expect("non-empty checked above");
    if !(first.is_ascii_alphabetic() || first == '_') {
        return Err(invalid());
    }
    if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Err(invalid());
    }
    Ok(())
}

/// The quoted, schema-qualified lease table reference, e.g.
/// `"public"."snowdrop_machine_id_leases"`. `schema` must already be validated.
fn qualified(schema: &str) -> String {
    format!("\"{schema}\".\"{LEASES_TABLE}\"")
}

/// Race-safe provisioning DDL for `auto_provision` — the concurrent first-boot
/// path. Each `CREATE` is its own `BEGIN … EXCEPTION` sub-block that swallows
/// both the already-committed error (`duplicate_schema` / `duplicate_table`) and
/// the `unique_violation` a simultaneous creator hits on the `pg_namespace` /
/// `pg_class` unique index, so N instances creating at once all succeed. Seeding
/// is separate ([`seeding_ddl`]).
fn provision_sql(schema: &str) -> String {
    let table = qualified(schema);
    // `public` always exists; creating it needs `CREATE`-on-database that the
    // role may lack, so skip it.
    let schema_block = if schema == DEFAULT_SCHEMA {
        String::new()
    } else {
        format!(
            "BEGIN CREATE SCHEMA \"{schema}\"; \
             EXCEPTION WHEN duplicate_schema OR unique_violation THEN NULL; END; "
        )
    };
    format!(
        "DO $$ \
         BEGIN \
             {schema_block}\
             BEGIN \
                 CREATE TABLE {table} ({LEASES_COLUMNS}) WITH (fillfactor = 70); \
             EXCEPTION \
                 WHEN duplicate_table OR unique_violation THEN NULL; \
             END; \
         END $$;"
    )
}

/// Idempotent DDL for a one-time migration: creates the schema (unless the
/// default `public`) and the table if absent. Not race-safe — intended to run
/// once during a migration/deploy, not concurrently from many instances (use
/// `auto_provision` for that). Additive schema changes in future versions append
/// `ALTER TABLE … ADD COLUMN IF NOT EXISTS`.
fn schema_ddl(schema: &str) -> String {
    let table = qualified(schema);
    let schema_stmt = if schema == DEFAULT_SCHEMA {
        String::new()
    } else {
        format!("CREATE SCHEMA IF NOT EXISTS \"{schema}\"; ")
    };
    format!(
        "{schema_stmt}\
         CREATE TABLE IF NOT EXISTS {table} ({LEASES_COLUMNS}) WITH (fillfactor = 70);"
    )
}

/// Idempotent seed: inserts the 1024 machine-ID rows. No-op on re-run and safe
/// under concurrency (`ON CONFLICT DO NOTHING`).
fn seeding_ddl(schema: &str) -> String {
    let table = qualified(schema);
    format!(
        "INSERT INTO {table} (machine_id) SELECT generate_series(0, 1023) \
         ON CONFLICT (machine_id) DO NOTHING;"
    )
}

/// The claim statement (`table` is the quoted, qualified reference). A CTE picks
/// the lowest free row with `FOR UPDATE SKIP LOCKED` and claims it; the outer
/// `SELECT` also reports whether the table has any seed rows, so a caller can
/// tell "nothing claimed because the table is empty" from "…because every ID is
/// leased". Always returns exactly one row.
fn claim_sql(table: &str) -> String {
    format!(
        "WITH candidate AS ( \
             SELECT machine_id FROM {table} \
             WHERE machine_id BETWEEN 0 AND 1023 \
               AND (reclaimable_after IS NULL OR reclaimable_after <= NOW()) \
             ORDER BY machine_id ASC LIMIT 1 FOR UPDATE SKIP LOCKED \
         ), \
         claimed AS ( \
             UPDATE {table} \
             SET claimed_at = NOW(), reclaimable_after = NOW() + $1 * INTERVAL '1 second' \
             WHERE machine_id = (SELECT machine_id FROM candidate) \
             RETURNING machine_id, {FENCING} AS fencing_token \
         ) \
         SELECT (SELECT machine_id FROM claimed) AS machine_id, \
                (SELECT fencing_token FROM claimed) AS fencing_token, \
                EXISTS (SELECT 1 FROM {table} WHERE machine_id BETWEEN 0 AND 1023) AS seeded"
    )
}

fn heartbeat_sql(table: &str) -> String {
    format!(
        "UPDATE {table} SET reclaimable_after = NOW() + $1 * INTERVAL '1 second' \
         WHERE machine_id = $2 AND {FENCING} = $3"
    )
}

fn release_sql(table: &str) -> String {
    format!(
        "UPDATE {table} SET reclaimable_after = NULL, claimed_at = NULL \
         WHERE machine_id = $1 AND {FENCING} = $2"
    )
}

/// Outcome of a claim attempt.
enum Claim {
    /// Claimed a machine ID, with its fencing token.
    Got(i16, i64),
    /// The table exists but has no machine-ID rows in `0..=1023`.
    NotSeeded,
    /// Every machine ID in range is currently leased.
    Exhausted,
}

/// Runs one claim, pinning READ COMMITTED for this transaction so the caller's
/// database default cannot make the `SKIP LOCKED` claim throw serialization
/// failures.
async fn claim_once(pool: &PgPool, claim: &str) -> Result<Claim, sqlx::Error> {
    let mut tx = pool.begin().await?;
    // Transaction-scoped (not session-scoped): safe under transaction pooling.
    sqlx::raw_sql("SET TRANSACTION ISOLATION LEVEL READ COMMITTED")
        .execute(&mut *tx)
        .await?;
    // `table` inside `claim` is built from validated + quoted identifiers; the
    // rest is a static template, so asserting SQL-safety is sound.
    let (machine_id, fencing, seeded): (Option<i16>, Option<i64>, bool) =
        sqlx::query_as(sqlx::AssertSqlSafe(claim))
            .bind(BOOTLOOP_GRACE_SECS)
            .fetch_one(&mut *tx)
            .await?;
    tx.commit().await?;
    Ok(match (machine_id, fencing) {
        (Some(id), Some(token)) => Claim::Got(id, token),
        _ if !seeded => Claim::NotSeeded,
        _ => Claim::Exhausted,
    })
}

/// Renews the lease. Returns rows affected: `1` = renewed, `0` = stolen.
async fn heartbeat_once(
    pool: &PgPool,
    heartbeat: &str,
    machine_id: i16,
    fencing: i64,
) -> Result<u64, sqlx::Error> {
    let mut tx = pool.begin().await?;
    sqlx::raw_sql("SET TRANSACTION ISOLATION LEVEL READ COMMITTED")
        .execute(&mut *tx)
        .await?;
    let done = sqlx::query(sqlx::AssertSqlSafe(heartbeat))
        .bind(RECLAIM_TTL_SECS)
        .bind(machine_id)
        .bind(fencing)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(done.rows_affected())
}

/// The background lease task: heartbeats on a schedule, self-heals a lost lease
/// by re-claiming, and lets transient errors fall through to age-based poison.
async fn run_lease_task(pool: PgPool, table: String, shared: Arc<LeaseShared>) {
    let heartbeat = heartbeat_sql(&table);
    let claim = claim_sql(&table);
    let mut next = FIRST_HEARTBEAT_DELAY;
    loop {
        tokio::time::sleep(next).await;
        let machine_id = shared.machine_id.load(Ordering::Acquire) as i16;
        let fencing = shared.fencing.load(Ordering::Acquire);
        match heartbeat_once(&pool, &heartbeat, machine_id, fencing).await {
            Ok(1) => {
                shared.refresh_deadlines(SELF_POISON_AFTER);
                shared.poisoned.store(false, Ordering::Release);
                next = HEARTBEAT_INTERVAL;
            }
            Ok(_) => {
                // Zero rows: the lease was stolen (fencing token rotated).
                shared.poisoned.store(true, Ordering::Release);
                reacquire(&pool, &claim, &shared).await;
                // A fresh claim is only valid for the bootloop grace, so
                // heartbeat again promptly to extend it.
                next = FIRST_HEARTBEAT_DELAY;
            }
            Err(_) => {
                // Transient: leave deadlines alone (age-based poison still
                // governs) and retry sooner than the full interval.
                next = HEARTBEAT_RETRY_BACKOFF;
            }
        }
    }
}

/// Re-claims any free machine ID after a lost lease, retrying until one is free.
async fn reacquire(pool: &PgPool, claim: &str, shared: &LeaseShared) {
    loop {
        match claim_once(pool, claim).await {
            Ok(Claim::Got(machine_id, fencing)) => {
                shared
                    .machine_id
                    .store(machine_id as u16, Ordering::Release);
                shared.fencing.store(fencing, Ordering::Release);
                shared.refresh_deadlines(BOOTLOOP_SELF_POISON);
                shared.poisoned.store(false, Ordering::Release);
                return;
            }
            // Not seeded, exhausted, or a transient error: stay poisoned, retry.
            Ok(Claim::NotSeeded) | Ok(Claim::Exhausted) | Err(_) => {
                tokio::time::sleep(RECLAIM_RETRY_BACKOFF).await
            }
        }
    }
}

// --- PgMachineIdLease -------------------------------------------------------

/// A leased machine ID, kept alive by a background heartbeat task.
///
/// The lease is a guard: hold it for as long as any generator uses its machine
/// ID. Dropping it aborts the heartbeat task and best-effort releases the row.
///
/// After a lost lease the background task may re-acquire a *different* machine
/// ID; [`machine_id`](Self::machine_id) always returns the current one. Use
/// [`is_poisoned`](Self::is_poisoned) to tell whether the lease is currently
/// confirmed held; a generator built manually from a lease should treat a
/// poisoned lease as a signal to stop. [`PgIdGenerator`] does this for you.
pub struct PgMachineIdLease {
    shared: Arc<LeaseShared>,
    pool: PgPool,
    table: String,
    task: tokio::task::JoinHandle<()>,
}

impl PgMachineIdLease {
    /// Acquires a lease from `pool` with defaults: the `snowdrop_machine_id_leases`
    /// table in the [`DEFAULT_SCHEMA`], which must already exist (auto-provisioning
    /// is off by default — see
    /// [`auto_provision`](PgMachineIdLeaseBuilder::auto_provision)).
    pub async fn acquire(pool: PgPool) -> Result<PgMachineIdLease, PgLeaseError> {
        PgMachineIdLease::builder(pool).build().await
    }

    /// Starts building a lease with a custom schema or provisioning behavior.
    pub fn builder(pool: PgPool) -> PgMachineIdLeaseBuilder {
        PgMachineIdLeaseBuilder {
            pool,
            schema: DEFAULT_SCHEMA.to_string(),
            auto_provision: false,
        }
    }

    /// The currently leased machine ID (may change after a lost lease).
    pub fn machine_id(&self) -> MachineId {
        MachineId::new(self.shared.machine_id.load(Ordering::Acquire))
            .expect("leased machine ID is always in range")
    }

    /// `true` while the lease is not provably held — poisoned by a detected
    /// steal, or aged past its self-poison horizon without a confirmed
    /// heartbeat. A generator should not issue IDs while this is `true`.
    pub fn is_poisoned(&self) -> bool {
        !self.shared.is_fresh()
    }

    /// Idempotent DDL that creates the lease schema (unless the default `public`)
    /// and table in [`DEFAULT_SCHEMA`], for callers who provision it through
    /// their own migrations rather than
    /// [`auto_provision`](PgMachineIdLeaseBuilder::auto_provision). Run it once —
    /// it is not concurrency-safe (`auto_provision` is) — and pair it with
    /// [`seeding_sql`](Self::seeding_sql). Use
    /// [`schema_sql_with_schema`](Self::schema_sql_with_schema) for a custom schema.
    pub fn schema_sql() -> String {
        schema_ddl(DEFAULT_SCHEMA)
    }

    /// Like [`schema_sql`](Self::schema_sql), but for a custom schema.
    pub fn schema_sql_with_schema(schema: &str) -> Result<String, PgLeaseError> {
        validate_schema_name(schema)?;
        Ok(schema_ddl(schema))
    }

    /// Idempotent DML that seeds the lease table's 1024 machine-ID rows in
    /// [`DEFAULT_SCHEMA`]. Run it after [`schema_sql`](Self::schema_sql); safe to
    /// re-run. Use [`seeding_sql_with_schema`](Self::seeding_sql_with_schema) for
    /// a custom schema.
    pub fn seeding_sql() -> String {
        seeding_ddl(DEFAULT_SCHEMA)
    }

    /// Like [`seeding_sql`](Self::seeding_sql), but for a custom schema.
    pub fn seeding_sql_with_schema(schema: &str) -> Result<String, PgLeaseError> {
        validate_schema_name(schema)?;
        Ok(seeding_ddl(schema))
    }
}

impl Drop for PgMachineIdLease {
    fn drop(&mut self) {
        self.task.abort();
        // Best-effort fenced release so the ID is reusable immediately; the
        // reclaim deadline is the real backstop if this never lands.
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            let pool = self.pool.clone();
            let sql = release_sql(&self.table);
            let machine_id = self.shared.machine_id.load(Ordering::Acquire) as i16;
            let fencing = self.shared.fencing.load(Ordering::Acquire);
            handle.spawn(async move {
                let _ = sqlx::query(sqlx::AssertSqlSafe(sql))
                    .bind(machine_id)
                    .bind(fencing)
                    .execute(&pool)
                    .await;
            });
        }
    }
}

impl fmt::Debug for PgMachineIdLease {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PgMachineIdLease")
            .field("machine_id", &self.machine_id())
            .field("table", &self.table)
            .field("poisoned", &self.is_poisoned())
            .finish_non_exhaustive()
    }
}

/// Builder for a [`PgMachineIdLease`].
#[derive(Debug, Clone)]
pub struct PgMachineIdLeaseBuilder {
    pool: PgPool,
    schema: String,
    auto_provision: bool,
}

impl PgMachineIdLeaseBuilder {
    /// Sets the schema holding the lease table (default [`DEFAULT_SCHEMA`],
    /// `public`). Independent ID spaces in one database use distinct schemas.
    /// The name is quoted, so reserved words work and matching is case-sensitive.
    ///
    /// Returns [`PgLeaseError::InvalidSchemaName`] unless `name` is a single
    /// valid SQL identifier.
    pub fn schema_name(mut self, name: &str) -> Result<PgMachineIdLeaseBuilder, PgLeaseError> {
        validate_schema_name(name)?;
        self.schema = name.to_string();
        Ok(self)
    }

    /// Opts into creating the schema (unless `public`), table, and seed rows if
    /// absent (default `false`), race-safely across concurrent first boots.
    ///
    /// Off by default: creating a table needs DDL rights — and a non-`public`
    /// schema needs `CREATE` on the database — which many production roles lack.
    /// Prefer provisioning from [`schema_sql`](PgMachineIdLease::schema_sql) +
    /// [`seeding_sql`](PgMachineIdLease::seeding_sql) in your migrations; enable
    /// this only where the connecting role may run DDL.
    pub fn auto_provision(mut self, yes: bool) -> PgMachineIdLeaseBuilder {
        self.auto_provision = yes;
        self
    }

    /// Provisions (if enabled), claims the lowest free machine ID, and spawns the
    /// heartbeat task.
    pub async fn build(self) -> Result<PgMachineIdLease, PgLeaseError> {
        if self.auto_provision {
            sqlx::query(sqlx::AssertSqlSafe(provision_sql(&self.schema)))
                .execute(&self.pool)
                .await?;
            sqlx::query(sqlx::AssertSqlSafe(seeding_ddl(&self.schema)))
                .execute(&self.pool)
                .await?;
        }
        let table = qualified(&self.schema);
        let claim = claim_sql(&table);
        let (machine_id, fencing) = match claim_once(&self.pool, &claim).await? {
            Claim::Got(id, token) => (id, token),
            Claim::NotSeeded => return Err(PgLeaseError::TableNotSeeded),
            Claim::Exhausted => return Err(PgLeaseError::NoMachineIdAvailable),
        };

        let shared = Arc::new(LeaseShared {
            machine_id: AtomicU16::new(machine_id as u16),
            fencing: AtomicI64::new(fencing),
            poisoned: AtomicBool::new(false),
            mono_deadline_nanos: AtomicU64::new(0),
            wall_deadline_ms: AtomicU64::new(0),
            base: Instant::now(),
        });
        shared.refresh_deadlines(BOOTLOOP_SELF_POISON);

        let task = tokio::spawn(run_lease_task(
            self.pool.clone(),
            table.clone(),
            Arc::clone(&shared),
        ));
        Ok(PgMachineIdLease {
            shared,
            pool: self.pool,
            table,
            task,
        })
    }
}

// --- PgIdGenerator ----------------------------------------------------------

/// An [`IdGenerator`] whose machine ID is leased from Postgres, for clusters
/// with no static machine-ID assignment.
///
/// Owns a [`PgMachineIdLease`] and follows it: if the lease re-acquires a
/// different machine ID after a lost lease, the generator swaps to it
/// transparently, and it refuses to generate ([`PgGenerateError::MachineIdLeaseLost`])
/// whenever the lease is not confirmed held.
pub struct PgIdGenerator {
    lease: PgMachineIdLease,
    epoch: Epoch,
    inner: std::sync::RwLock<IdGenerator>,
}

impl PgIdGenerator {
    /// Acquires a lease from `pool` with defaults and the default [`Epoch`].
    pub async fn acquire(pool: PgPool) -> Result<PgIdGenerator, PgLeaseError> {
        PgIdGenerator::builder(pool).build().await
    }

    /// Starts building a generator with a custom schema, provisioning behavior,
    /// or epoch.
    pub fn builder(pool: PgPool) -> PgIdGeneratorBuilder {
        PgIdGeneratorBuilder {
            lease: PgMachineIdLease::builder(pool),
            epoch: Epoch::DEFAULT,
        }
    }

    /// Generates the next ID, blocking through the (rare) sequence-exhaustion
    /// wait. Fails with [`PgGenerateError::MachineIdLeaseLost`] while the lease
    /// is not confirmed held.
    pub fn generate(&self) -> Result<Id, PgGenerateError> {
        loop {
            if !self.lease.shared.is_fresh() {
                return Err(PgGenerateError::MachineIdLeaseLost);
            }
            match self.with_inner(|inner| inner.try_generate()) {
                Ok(id) => return Ok(id),
                Err(TryGenerateError::SequenceExhausted { retry_after }) => {
                    std::thread::sleep(retry_after);
                }
                // EpochExhausted — and any future terminal variant of the
                // `#[non_exhaustive]` `TryGenerateError` — is permanent.
                Err(_) => return Err(PgGenerateError::EpochExhausted),
            }
        }
    }

    /// Generates the next ID, awaiting instead of blocking on (rare) sequence
    /// exhaustion.
    pub async fn generate_async(&self) -> Result<Id, PgGenerateError> {
        loop {
            if !self.lease.shared.is_fresh() {
                return Err(PgGenerateError::MachineIdLeaseLost);
            }
            match self.with_inner(|inner| inner.try_generate()) {
                Ok(id) => return Ok(id),
                Err(TryGenerateError::SequenceExhausted { retry_after }) => {
                    tokio::time::sleep(retry_after).await;
                }
                // EpochExhausted — and any future terminal variant of the
                // `#[non_exhaustive]` `TryGenerateError` — is permanent.
                Err(_) => return Err(PgGenerateError::EpochExhausted),
            }
        }
    }

    /// The currently leased machine ID (may change after a lost lease).
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

    /// `true` while the lease is not confirmed held; see
    /// [`PgMachineIdLease::is_poisoned`].
    pub fn is_poisoned(&self) -> bool {
        self.lease.is_poisoned()
    }

    /// Runs `f` on the inner generator, rebuilding it first if the lease's
    /// machine ID has changed. Never holds the lock across an await point.
    fn with_inner<R>(&self, f: impl FnOnce(&IdGenerator) -> R) -> R {
        let machine_id = self.lease.machine_id();
        {
            let inner = self.inner.read().expect("generator lock poisoned");
            if inner.machine_id() == machine_id {
                return f(&inner);
            }
        }
        let mut inner = self.inner.write().expect("generator lock poisoned");
        if inner.machine_id() != machine_id {
            *inner = IdGenerator::builder(machine_id).epoch(self.epoch).build();
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

/// Builder for a [`PgIdGenerator`].
#[derive(Debug, Clone)]
pub struct PgIdGeneratorBuilder {
    lease: PgMachineIdLeaseBuilder,
    epoch: Epoch,
}

impl PgIdGeneratorBuilder {
    /// Sets the schema holding the lease table (default [`DEFAULT_SCHEMA`]); see
    /// [`PgMachineIdLeaseBuilder::schema_name`].
    pub fn schema_name(mut self, name: &str) -> Result<PgIdGeneratorBuilder, PgLeaseError> {
        self.lease = self.lease.schema_name(name)?;
        Ok(self)
    }

    /// Opts into creating the schema, table, and seed rows if absent (default
    /// `false`); see [`PgMachineIdLeaseBuilder::auto_provision`].
    pub fn auto_provision(mut self, yes: bool) -> PgIdGeneratorBuilder {
        self.lease = self.lease.auto_provision(yes);
        self
    }

    /// Sets the epoch (default [`Epoch::DEFAULT`]).
    pub fn epoch(mut self, epoch: Epoch) -> PgIdGeneratorBuilder {
        self.epoch = epoch;
        self
    }

    /// Acquires the lease and builds the generator.
    pub async fn build(self) -> Result<PgIdGenerator, PgLeaseError> {
        let lease = self.lease.build().await?;
        let inner = IdGenerator::builder(lease.machine_id())
            .epoch(self.epoch)
            .build();
        Ok(PgIdGenerator {
            lease,
            epoch: self.epoch,
            inner: std::sync::RwLock::new(inner),
        })
    }
}

// --- Errors -----------------------------------------------------------------

/// Error acquiring a [`PgMachineIdLease`] or [`PgIdGenerator`].
#[derive(Debug)]
#[non_exhaustive]
pub enum PgLeaseError {
    /// The database connection or a lease query failed.
    Database(sqlx::Error),
    /// Every machine ID in the table is currently leased.
    NoMachineIdAvailable,
    /// The lease table exists but has no machine-ID rows. Seed it with
    /// [`seeding_sql`](PgMachineIdLease::seeding_sql), or enable
    /// [`auto_provision`](PgMachineIdLeaseBuilder::auto_provision).
    TableNotSeeded,
    /// The configured schema name is not a valid SQL identifier.
    InvalidSchemaName(String),
}

impl fmt::Display for PgLeaseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PgLeaseError::Database(e) => write!(f, "machine-ID lease query failed: {e}"),
            PgLeaseError::NoMachineIdAvailable => {
                f.write_str("no machine ID available: every ID in the table is leased")
            }
            PgLeaseError::TableNotSeeded => f.write_str(
                "lease table exists but is not seeded; run seeding_sql() or enable auto_provision",
            ),
            PgLeaseError::InvalidSchemaName(name) => {
                write!(f, "invalid schema name `{name}`: not a SQL identifier")
            }
        }
    }
}

impl std::error::Error for PgLeaseError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            PgLeaseError::Database(e) => Some(e),
            PgLeaseError::NoMachineIdAvailable
            | PgLeaseError::TableNotSeeded
            | PgLeaseError::InvalidSchemaName(_) => None,
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
    /// The machine-ID lease is not confirmed held; retry after re-acquisition.
    MachineIdLeaseLost,
}

impl fmt::Display for PgGenerateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PgGenerateError::EpochExhausted => {
                f.write_str("timestamp field exhausted for this epoch")
            }
            PgGenerateError::MachineIdLeaseLost => {
                f.write_str("machine-ID lease not confirmed held; waiting for re-acquisition")
            }
        }
    }
}

impl std::error::Error for PgGenerateError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_valid_schema_names() {
        // `order` is a reserved word — valid because we quote the identifier.
        for name in ["public", "snowdrop", "s1", "_x", "order", "MySchema"] {
            assert!(validate_schema_name(name).is_ok(), "{name} should be valid");
        }
    }

    #[test]
    fn rejects_invalid_schema_names() {
        for name in [
            "",
            "1schema",         // starts with a digit
            "a.b",             // qualifier / dot no longer allowed
            "drop schema",     // space
            "schema;--",       // punctuation / injection attempt
            "reclaim\"; DROP", // embedded quote
            &"x".repeat(64),   // too long
        ] {
            assert!(
                matches!(
                    validate_schema_name(name),
                    Err(PgLeaseError::InvalidSchemaName(_))
                ),
                "{name:?} should be rejected"
            );
        }
    }

    #[test]
    fn qualified_name_is_quoted() {
        assert_eq!(
            qualified("public"),
            "\"public\".\"snowdrop_machine_id_leases\""
        );
        assert_eq!(
            qualified("MySchema"),
            "\"MySchema\".\"snowdrop_machine_id_leases\""
        );
    }

    #[test]
    fn claim_sql_is_a_cte_with_a_seeded_flag() {
        let claim = claim_sql(&qualified("public"));
        assert!(claim.contains("WITH candidate AS"));
        assert!(claim.contains("FOR UPDATE SKIP LOCKED"));
        assert!(claim.contains("ORDER BY machine_id ASC"));
        // Guards against out-of-range rows, so a claimed ID always fits `MachineId`.
        assert!(claim.contains("machine_id BETWEEN 0 AND 1023"));
        assert!(claim.contains("AS seeded"));
        assert!(claim.contains(FENCING));
        assert!(claim.contains("\"public\".\"snowdrop_machine_id_leases\""));

        let hb = heartbeat_sql(&qualified("public"));
        assert!(hb.contains(&format!("{FENCING} = $3")));
    }

    #[test]
    fn schema_ddl_creates_schema_only_when_not_public() {
        let public = schema_ddl("public");
        assert!(!public.contains("CREATE SCHEMA"));
        assert!(
            public.contains("CREATE TABLE IF NOT EXISTS \"public\".\"snowdrop_machine_id_leases\"")
        );

        let custom = schema_ddl("app");
        assert!(custom.contains("CREATE SCHEMA IF NOT EXISTS \"app\""));
        assert!(
            custom.contains("CREATE TABLE IF NOT EXISTS \"app\".\"snowdrop_machine_id_leases\"")
        );
    }

    #[test]
    fn provision_sql_is_race_safe() {
        let custom = provision_sql("app");
        assert!(custom.contains("DO $$"));
        assert!(custom.contains("CREATE SCHEMA \"app\""));
        assert!(custom.contains("WHEN duplicate_schema OR unique_violation THEN NULL"));
        assert!(custom.contains("WHEN duplicate_table OR unique_violation THEN NULL"));
        // No seeding in the provisioning DDL — that's a separate statement.
        assert!(!custom.contains("INSERT INTO"));

        // `public` needs no schema creation.
        assert!(!provision_sql("public").contains("CREATE SCHEMA"));
    }

    #[test]
    fn seeding_ddl_is_idempotent() {
        let seed = seeding_ddl("public");
        assert!(seed.contains("generate_series(0, 1023)"));
        assert!(seed.contains("ON CONFLICT (machine_id) DO NOTHING"));
    }

    #[test]
    fn public_sql_helpers_use_the_default_schema() {
        assert_eq!(PgMachineIdLease::schema_sql(), schema_ddl(DEFAULT_SCHEMA));
        assert_eq!(PgMachineIdLease::seeding_sql(), seeding_ddl(DEFAULT_SCHEMA));
        assert_eq!(
            PgMachineIdLease::schema_sql_with_schema("app").unwrap(),
            schema_ddl("app")
        );
        assert_eq!(
            PgMachineIdLease::seeding_sql_with_schema("app").unwrap(),
            seeding_ddl("app")
        );
    }
}
