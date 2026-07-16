# Design: Postgres machine-ID leasing (`PgIdGenerator`)

**Status:** Implemented in v0.2.0; relocated to its own crate in v0.3.0
**Crate:** `snowdrop-id-postgres`
**Scope:** `snowdrop-id-postgres/src/lib.rs`

This document explains how `PgIdGenerator` and `PgMachineIdLease` assign a
unique machine ID to each generator instance out of a shared Postgres
database, why the design is shaped the way it is, and which failure modes it
does and does not defend against.

---

## 1. Purpose

A Snowdrop generator stamps a 10-bit **machine ID** (`0..=1023`) into every
ID. The core uniqueness guarantee — no two generators ever produce the same
ID — holds only if **every concurrently active generator in an ID space has a
distinct machine ID** (SPEC §5.1). Assigning those IDs is out of scope for the
spec; this module is one batteries-included way to do it.

The design goal is a **no-footgun-by-default** module: a service with a
Postgres database it already talks to should get correct, unique machine IDs
by handing us a connection pool and nothing else — no static per-replica
configuration, no coordination service, no operational sharp edges that only
show up under a pooler or a failover.

The safety property everything below serves:

> **Invariant S.** At no wall-clock instant do two live generators in the same
> ID space hold the same machine ID.

Violating S can produce duplicate IDs, which is the one thing the module
exists to prevent.

---

## 2. Why not Postgres advisory locks

The v0.1 approach (and the obvious one) was `pg_try_advisory_lock`: a
dedicated session holds a session-level advisory lock whose key encodes the
machine ID; the lock *is* the lease, and the server releases it when the
session ends. We are replacing it because it is a portability footgun:

- **Session affinity breaks under connection poolers.** `pgBouncer` in
  `transaction` or `statement` pooling mode hands a different backend to each
  transaction. A session-level advisory lock lands on whatever backend served
  the `SELECT pg_try_advisory_lock(...)` call and is released the instant that
  backend returns to the pool. It only works in `session` pooling mode — the
  mode operators disable to get pooling's benefits.
- **The dedicated-connection requirement is itself the footgun.** Advisory
  locking forces the generator to own a persistent `PgConnection` (not a
  pool), because the lock and the connection are the same lifetime. That
  persistent connection is what breaks across poolers, failovers, and restarts.
- **Cluster topologies.** Advisory locks are node-local server state; they do
  not replicate. Any primary failover silently drops every lease.

A lease **table** inverts all of this: every operation is a single autocommit
statement with **no session state**, so it works under any pooling mode and
survives failover (the lease is just a row, replicated like all data). It also
lets the generator use the caller's ordinary `PgPool` instead of owning a
dedicated connection — the single biggest deployability win here.

The cost we take on: a table lease is **timeout-based**, so it can falsely
declare a live-but-slow worker dead. Section 6 is entirely about making that
safe.

---

## 3. The lease table

```sql
CREATE TABLE snowdrop.machine_id_leases (   -- name is configurable
    machine_id        SMALLINT PRIMARY KEY, -- 0..=1023, prepopulated
    claimed_at        TIMESTAMPTZ,          -- identity / fencing source
    reclaimable_after TIMESTAMPTZ           -- liveness: steal me once NOW() passes this
) WITH (fillfactor = 70);
```

By default the table lives in a dedicated **`snowdrop`** schema
(`snowdrop.machine_id_leases`), keeping it out of `public`; the
fully-qualified name is configurable.

Three columns, two responsibilities cleanly split:

- **`claimed_at`** — set once when a lease is claimed, **never touched by
  heartbeats**. Stable for the life of a lease, rotates only when the row is
  re-claimed by someone else. This is the fencing source (§5).
- **`reclaimable_after`** — the **holder-declared death deadline**. As long as
  the holder heartbeats, it keeps pushing this into the future. When the holder
  stops, `NOW()` eventually passes it and the row becomes claimable.

`fillfactor = 70` leaves free space in each page so heartbeat `UPDATE`s (which
touch only `reclaimable_after`, a non-indexed column) qualify for HOT updates
and never bloat the primary-key index. Marginal at 1024 rows, but free and
correct-minded.

### 3.1 The inversion: liveness is declared, not judged

The important decision. A naive lease table stores `last_heartbeat_at` and the
**claimer** decides who is dead:

```sql
WHERE last_heartbeat_at <= NOW() - INTERVAL '30 minutes'   -- constant in the claimer
```

That embeds a **cluster-wide constant** in every claimer. All workers must
agree on it, and *changing* it is a hazardous migration: during a rollout, a
worker using a shorter threshold can declare a slow-but-alive worker (still
inside its own longer self-poison window) dead, steal its ID, and break
invariant S.

Instead we store `reclaimable_after`, written by the **holder** using the
holder's *own* timing:

```sql
WHERE reclaimable_after IS NULL OR reclaimable_after <= NOW()   -- no constant
```

Consequences:

1. **Per-worker timing is safe.** Each lease carries its own expiry; a claimer
   never applies its own policy to another worker's lease.
2. **Timing changes roll out safely.** Worker X is stealable only after the
   instant X itself wrote, and X self-poisons just before that instant (§6).
   Mixed timings across the fleet are always safe.
3. **The boot-loop special case disappears** (§4.2): a short initial deadline
   pushed out by the first heartbeat covers it, so the claim predicate is a
   single clause with **zero policy constants**.

What we give up: the global upper bound on how long a dead worker's ID stays
locked is now whatever deadline the holder last wrote. In v0.2, with a fixed
`reclaim_ttl`, that is simply 30 min. The concern only becomes real once the
TTL is tunable (§10), where a builder-enforced `max_ttl` restores the bound;
either way the blast radius is a single self-inflicted ID, never a fleet
problem.

---

## 4. Operations

All timing values are bound as **integer seconds** (`NOW() + $n * INTERVAL '1
second'`), so the only values crossing the Rust boundary are `i16`
(machine_id) and `i64` (fencing token, seconds counts). No `sqlx/time`, no
interval type, no extra `sqlx` feature.

### 4.1 Bootstrap (opt-in auto-create)

Run at construction only when opted in via `auto_create(true)` (default: off,
§9). It creates the schema (when the table is schema-qualified) and the table.
Race-safe against concurrent creators: each `CREATE` runs in its own
`BEGIN … EXCEPTION` sub-block that swallows both the already-committed error
(`duplicate_schema` / `duplicate_table`) and the `unique_violation` a *simultaneous*
creator hits on the `pg_namespace` / `pg_class` unique index, so the loser
continues and the winner's `CREATE TABLE` + `INSERT` commit atomically (no
partial population). A first-boot fan-out of N instances is safe.

```sql
DO $$
BEGIN
    BEGIN
        CREATE SCHEMA snowdrop;                -- only when schema-qualified
    EXCEPTION WHEN duplicate_schema OR unique_violation THEN NULL;
    END;
    BEGIN
        CREATE TABLE snowdrop.machine_id_leases (
            machine_id        SMALLINT PRIMARY KEY,
            claimed_at        TIMESTAMPTZ,
            reclaimable_after TIMESTAMPTZ
        ) WITH (fillfactor = 70);
        INSERT INTO snowdrop.machine_id_leases (machine_id)
        SELECT generate_series(0, 1023);
    EXCEPTION WHEN duplicate_table OR unique_violation THEN NULL;
    END;
END $$;
```

Notes:

- **Auto-DDL is opt-in.** Creating the schema needs `CREATE` on the database and
  creating the table needs DDL rights — privileges many production roles lack —
  so auto-create defaults to **off**. Provision from
  `PgMachineIdLease::schema_sql()` (schema + table; `schema_sql_with_table(name)`
  for a custom name) in the caller's own migrations, or opt in with
  `auto_create(true)` where the connecting role may run DDL.
- **The table name is configurable** (this is also how independent ID spaces
  share one database: a different table is a different space). Because a table
  name **cannot be a bound parameter**, it is interpolated as an identifier and
  therefore **validated** in Rust against an allowlist (`[A-Za-z0-9_]`, bounded
  length, optional `schema.` qualifier). An unvalidated table name would be a
  SQL-injection vector.

### 4.2 Claim

Wrapped in an explicit `READ COMMITTED` transaction so the caller's database
default (which we do not own) cannot silently make it `SERIALIZABLE` and throw
spurious serialization failures. The isolation level is set **per transaction**
(`BEGIN ISOLATION LEVEL READ COMMITTED`), never `SET SESSION` — session-scoped
state is exactly what breaks under transaction pooling.

```sql
BEGIN ISOLATION LEVEL READ COMMITTED;

UPDATE snowdrop.machine_id_leases
SET claimed_at        = NOW(),
    reclaimable_after = NOW() + $1 * INTERVAL '1 second'   -- $1 = bootloop grace (~60s)
WHERE machine_id = (
    SELECT machine_id
    FROM snowdrop.machine_id_leases
    WHERE reclaimable_after IS NULL OR reclaimable_after <= NOW()
    ORDER BY machine_id ASC          -- lowest free ID → shortest base62 strings
    LIMIT 1
    FOR UPDATE SKIP LOCKED           -- concurrent claimers never pick the same row
)
RETURNING
    machine_id,
    to_char(claimed_at AT TIME ZONE 'UTC', 'YYYYMMDDHH24MISSMS')::bigint AS fencing_token;

COMMIT;
```

- **`FOR UPDATE SKIP LOCKED`** is the work-queue idiom: two workers booting
  together lock disjoint rows and never collide; no deadlocks.
- **`ORDER BY machine_id ASC`** is required, not incidental: without it
  `LIMIT 1` returns an arbitrary eligible row (heap order, which drifts after
  updates and vacuum). The spec makes "low machine IDs → shorter base62
  strings" a real property, so we hand out the lowest free ID deterministically.
- **A short initial `reclaimable_after`** (bootloop grace) is what reclaims
  crash-on-boot IDs quickly; the first heartbeat pushes it out to the full TTL.
- **Zero rows returned** ⇒ every ID in the range is currently held ⇒
  `PgLeaseError::NoMachineIdAvailable`.

### 4.3 Heartbeat

Pushes the deadline out to the full TTL, **fenced** on `claimed_at` so a zombie
whose lease was already stolen cannot resurrect it.

```sql
UPDATE snowdrop.machine_id_leases
SET reclaimable_after = NOW() + $1 * INTERVAL '1 second'   -- $1 = reclaim TTL (~1800s)
WHERE machine_id = $2
  AND to_char(claimed_at AT TIME ZONE 'UTC', 'YYYYMMDDHH24MISSMS')::bigint = $3;
```

**Rows affected = 0 ⇒ the lease was stolen** (someone re-claimed the row, so
`claimed_at` and thus the fencing token changed). The worker poisons and
re-claims a fresh ID. Rows affected = 1 ⇒ lease renewed; record a new local
expiry (§6).

### 4.4 Release (best-effort, on drop)

`Drop` cannot `await`, and blocking the runtime in `Drop` is worse than the
problem it solves, so release is **fire-and-forget**: spawn a detached task
that runs the fenced release and let it race the runtime shutdown. If it does
not land, the deadline reclaims the ID anyway — release is purely a latency
optimization for ID reuse, never a correctness requirement.

```sql
UPDATE snowdrop.machine_id_leases
SET reclaimable_after = NULL,
    claimed_at        = NULL          -- back to pristine → immediately claimable
WHERE machine_id = $1
  AND to_char(claimed_at AT TIME ZONE 'UTC', 'YYYYMMDDHH24MISSMS')::bigint = $2;
```

Fencing matters here too: a zombie must not release a row another worker now
holds.

---

## 5. The fencing token

A fencing token distinguishes one claim of a row from a later claim of the same
row. Ours is **`claimed_at`**, but transformed so it never crosses into Rust as
a date type. The token is **opaque to Rust**: the worker stores whatever the
claim returns and passes it back verbatim; only Postgres ever interprets it.

```sql
to_char(claimed_at AT TIME ZONE 'UTC', 'YYYYMMDDHH24MISSMS')::bigint
```

- Maps to Rust **`i64`** — no `sqlx/time` dependency, no extra column.
- `YYYYMMDDHH24MISSMS` is 17 digits (millisecond resolution), max ~`1.7e16`,
  comfortably inside `i64`. (`US`/microseconds would be 20 digits and overflow.)
- `AT TIME ZONE 'UTC'` makes it independent of the session `TimeZone` GUC —
  essential under a pool where sessions may differ; otherwise the same instant
  renders to different strings.
- Exact and Postgres-version-independent (a string of digits cast to `bigint`;
  no float).
- Millisecond resolution is ample: the minimum gap between two distinct claims
  of one row is the bootloop grace (~60 s) or the TTL (~30 min), and concurrent
  claimers get different rows via `SKIP LOCKED`. A worker's own heartbeats never
  touch `claimed_at`, so its token is stable for the lease's life.
- Fencing tokens are **never compared across workers** — a worker only ever
  checks its own token against its own row — so even a future change to this
  format cannot break a mixed-version fleet.

**Rejected alternatives:** `extract(epoch …)::float8` (µs epochs sit at the
edge of a double's mantissa — not reliably exact); `extract(epoch …)::numeric`
(exact only on PG 14+, needs a documented minimum version); `xmin` (rewritten
to `FrozenTransactionId` by `VACUUM FREEZE`, silently changing a live lease's
token); a dedicated counter column (rejected to avoid a fourth column when
`claimed_at` already serves).

### 5.1 Why fencing alone is not enough

The fencing token only protects **writes to the lease table**. It does **not**
protect **ID generation**, which never touches the database. So on its own the
token buys only *detection latency of one heartbeat interval* — a worker whose
lease was stolen keeps generating under the stolen ID until its next heartbeat
fails. Closing that gap is self-poisoning (§6). Fencing **detects** a lost
lease; self-poison **prevents generation** while it is in doubt. Both are
required.

---

## 6. Timing and the safety invariant

### 6.1 Self-poison is the mechanism that preserves invariant S

A timeout lease can falsely declare a live-but-slow worker dead. The only
defense is that the worker itself **stops generating before anyone may steal
its ID**:

> On every successful claim/heartbeat, record a local expiry. `generate()`
> refuses (poisons) once the worker cannot prove its lease is still fresh.

Two clocks are in play, and separating them is what makes this correct:

- **The steal decision is DB-clock-only.** `reclaimable_after` and the `NOW()`
  it is compared against are both the DB server's clock, so *whether B may
  steal* has no cross-worker skew.
- **The self-poison decision is the worker's local clock**, measuring elapsed
  time since its last confirmed heartbeat. The worker cannot consult the DB on
  every `generate()`, so this is unavoidable — but it is the *only* place
  worker-vs-DB skew enters.

Safety reduces to one inequality:

```
self_poison_after  +  clock_skew_budget  +  generation_in_flight  <  reclaim_ttl
```

We guarantee it structurally by **deriving** the self-poison threshold rather
than exposing it: `self_poison_after = reclaim_ttl − poison_margin`.

### 6.2 The numbers

| Parameter               | Value   | Where it lives                   |
|-------------------------|---------|----------------------------------|
| `heartbeat_interval`    | 10 min  | worker-local (timer)             |
| `reclaim_ttl`           | 30 min  | written into `reclaimable_after` |
| `self_poison_after`     | 25 min  | worker-local (= ttl − margin)    |
| `poison_margin`         | 5 min   | worker-local                     |
| first-heartbeat delay   | 20 s    | worker-local (timer)             |
| bootloop grace          | 60 s    | written at claim                 |

**All of these are fixed constants in v0.2** — no timing knobs are exposed. The
timing surface is deliberately deferred until real-world use tells us what (if
anything) needs tuning (§10, Future work).

Heartbeat ≈ TTL/3 tolerates ~2 missed beats before the deadline, which absorbs
transient DB errors without any special-case retry logic — a blip is just an
unpushed deadline, and the age model already accounts for it.

### 6.3 Self-poison must survive VM suspend

`Instant` (CLOCK_MONOTONIC on Linux) **freezes during VM suspend**, while the
DB's wall clock keeps advancing. A VM suspended past its deadline would resume
believing its lease is fresh, generate, and collide with whoever stole it. So
the worker measures lease age against **both** clocks and poisons if **either**
exceeds the budget:

- monotonic (`Instant`) — catches wall-clock *backward* adjustments;
- wall (`SystemTime`) since last confirmed heartbeat — catches suspend and
  *forward* jumps.

Residual trusted assumption: the **DB server's clock** is the shared reference.
A large *forward* jump there could free live leases early, so the database node
needs ordinary NTP discipline. This is the same class of assumption as the
SPEC §5.3 restart hazard, and the reclaim TTL gives it a concrete bound: a
worker re-claims an ID only ≥ `reclaim_ttl` after the prior holder's last
heartbeat, so unless a clock rewinds more than `reclaim_ttl`, a fresh
generator's timestamps strictly exceed the previous holder's.

### 6.4 Invariants among the fixed timings

Because v0.2 ships these as constants, the relationships below are properties we
verify once with `const` assertions at compile time, not runtime validation:

1. `first_heartbeat_delay (20 s) < bootloop_grace (60 s)` — a healthy worker
   extends its deadline before the row is stealable.
2. `heartbeat_interval (10 min) ≤ self_poison_after / 2 (12.5 min)` — a single
   missed beat must not poison you.
3. `self_poison_after (25 min) < reclaim_ttl (30 min)` by `poison_margin`
   (5 min, comfortably above the ~60 s needed to cover skew + in-flight
   generation) — the core inequality of §6.1.

When timing becomes configurable (§10) these graduate into builder-time
validation, plus a `max_ttl` cap to bound how long a dead worker can squat an
ID (§3.1).

---

## 7. Failure analysis

| Scenario | Outcome |
|---|---|
| **GC / STW pause** past the deadline | Monotonic clock advances through the pause → worker self-poisons on resume; meanwhile its ID may have been stolen (correctly). No collision. |
| **VM suspend** past the deadline | Monotonic clock frozen, but wall-clock check (§6.3) trips → self-poison on resume. No collision. |
| **Network partition to DB** | Heartbeats fail; deadline not pushed; worker self-poisons at `self_poison_after`, before the row is stealable at `reclaim_ttl`. On reconnect it re-claims. |
| **Transient DB blip** (< heartbeat slack) | Deadline still in the future; worker keeps generating; next heartbeat repushes. No poison, no special-casing. |
| **Crash before first heartbeat** | `reclaimable_after` = claim + bootloop grace → ID freed in ~60 s. |
| **Primary failover** | Lease row replicated; heartbeats resume against the new primary through the pool. No dedicated connection to lose. |
| **Lease stolen while holder alive** | Holder's next heartbeat sees fencing mismatch (0 rows) → poisons and re-claims. Between steal and detection it is *not* generating, because it already self-poisoned at `self_poison_after` (which precedes the steal). |
| **DB clock forward jump** | Can free live leases early → possible collision. Out of scope; mitigated by NTP on the DB node (§6.3). |
| **All IDs held** | Claim returns 0 rows → `NoMachineIdAvailable`. |

---

## 8. Deliberately dropped

- **`reclaim_token`.** Its only real benefit (reclaiming an ID before the
  timeout, e.g. to bound a crash-looper to one ID) requires bypassing the
  freshness check — which is exactly the "steal from a live worker" footgun.
  The useful version *is* the dangerous version. Boot-loop exhaustion is already
  bounded by the fast first heartbeat + 60 s grace. Additive to re-add later if
  a real user needs it. (YAGNI.)
- **`LeaseLossPolicy` (Poison/Optimistic).** In the age-based model, `Optimistic`
  is precisely "generate past lease expiry" — the collision footgun. There is
  now one behavior: always self-poison at the derived threshold.
- **The dedicated `PgConnection`.** Replaced by the caller's `PgPool` (§2).
- **`machine_ids(range)` partitioning and the advisory-lock `namespace`.**
  Independent ID spaces are expressed by using a different **table name**; a
  single knob replaces both.

---

## 9. API sketch

```rust
use snowdrop_id_postgres::PgIdGenerator;
use snowdrop_id::Epoch;

let generator = PgIdGenerator::builder(pool)          // caller-provided PgPool
    .table_name("snowdrop.machine_id_leases")?        // validated identifier
    .auto_create(true)                                // opt-in; default is off
    .epoch(Epoch::DEFAULT)
    .build()                                           // claims an ID, spawns heartbeat task
    .await?;

let id = generator.generate()?;      // self-poisons if the lease is not provably fresh
generator.machine_id();              // current leased ID (may change after a steal + re-claim)
generator.is_poisoned();             // observability
// Drop → best-effort fenced release
```

`PgGenerateError`: `EpochExhausted` (permanent) | `MachineIdLeaseLost` (poisoned;
retry after re-claim).

---

## 10. Future work

v0.2 ships with **all timings fixed** and no `reclaim_token`. The intent is to
learn from real-world use before committing to a configuration surface — an
unused knob is just another footgun. Likely candidates, once there is evidence
for them:

- **Tunable timings.** Promote the §6.2 constants to railed builder methods
  (§6.4), with a `max_ttl` cap to bound how long a dead worker can squat an ID.
- **Scale-to-zero / intermittent-traffic databases.** A service with low,
  bursty traffic backed by a scale-to-zero Postgres may want a very long
  heartbeat interval — on the order of a day — so heartbeats do not keep the
  database perpetually awake and billed. That pulls several decisions with it:
  `reclaim_ttl` must grow to match (so a day-long heartbeat still tolerates a
  missed beat), which lengthens the worst-case reclaim latency, which is exactly
  the case where reclaiming a *specific* prior ID quickly becomes valuable
  again — see `footgun_reclaim_token` below.
- **`footgun_reclaim_token`.** A power-user opt-in to reclaim a specific machine
  ID by a caller-supplied token (e.g. a StatefulSet ordinal) *before* its
  deadline, trading the freshness guarantee for fast, stable reclaim. Dropped
  from v0.2 (§8) because its useful form bypasses freshness; worth revisiting
  for the long-heartbeat case above, where the deadline may be a day away and
  waiting it out is untenable. Would ship named to advertise the sharp edge, and
  documented as requiring a token that is unique among concurrently live workers.
- **Metrics hooks.** A callback exposing heartbeat success/failure and poison
  transitions, if `is_poisoned()` proves insufficient for operators.

Whatever we add here must preserve invariant S (§1) by default; configuration
should widen operational fit, not open a path to duplicate IDs without an
explicit, loudly-named opt-in.
