//! Integration tests for Postgres lease-table machine-ID assignment.
//!
//! Require a live Postgres: set `SNOWDROP_TEST_PG_URL` (e.g.
//! `postgres://postgres:postgres@localhost:5432/postgres`); tests skip silently
//! when it is unset. The table name is fixed, so each test isolates itself in a
//! distinct schema and drops it on entry.

use std::time::{Duration, Instant};

use snowdrop_id_postgres::{PgIdGenerator, PgLeaseError, PgMachineIdLease};
use sqlx::PgPool;

async fn pool() -> Option<PgPool> {
    match std::env::var("SNOWDROP_TEST_PG_URL") {
        Ok(url) => Some(PgPool::connect(&url).await.expect("connect failed")),
        Err(_) => {
            eprintln!("skipping: SNOWDROP_TEST_PG_URL not set");
            None
        }
    }
}

/// The quoted, qualified lease-table reference for a schema (mirrors the crate's
/// private helper) — for direct SQL in tests.
fn qualified(schema: &str) -> String {
    format!("\"{schema}\".\"snowdrop_machine_id_leases\"")
}

/// Cold-start a test's schema: drop it (and its table) if a prior run left it.
async fn drop_schema(pool: &PgPool, schema: &str) {
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "DROP SCHEMA IF EXISTS \"{schema}\" CASCADE"
    )))
    .execute(pool)
    .await
    .expect("drop schema failed");
}

#[tokio::test]
async fn leases_lowest_free_and_releases_on_drop() {
    let Some(pool) = pool().await else { return };
    let schema = "snowdrop_test_release";
    drop_schema(&pool, schema).await;

    let a = PgMachineIdLease::builder(pool.clone())
        .schema_name(schema)
        .unwrap()
        .auto_provision(true)
        .build()
        .await
        .unwrap();
    let b = PgMachineIdLease::builder(pool.clone())
        .schema_name(schema)
        .unwrap()
        .build()
        .await
        .unwrap();
    assert_eq!(a.machine_id().get(), 0);
    assert_eq!(b.machine_id().get(), 1);
    assert!(!a.is_poisoned());

    // Dropping best-effort releases the row; machine ID 0 becomes leasable
    // again. Poll to allow the detached release task to land.
    drop(a);
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let c = PgMachineIdLease::builder(pool.clone())
            .schema_name(schema)
            .unwrap()
            .build()
            .await
            .unwrap();
        if c.machine_id().get() == 0 {
            break;
        }
        drop(c);
        assert!(Instant::now() < deadline, "machine ID 0 never released");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[tokio::test]
async fn all_ids_held_errors() {
    let Some(pool) = pool().await else { return };
    let schema = "snowdrop_test_exhausted";
    drop_schema(&pool, schema).await;

    // Provision via the published DDL/seed, then mark every row held far into the
    // future without acquiring any lease.
    let ddl = PgMachineIdLease::schema_sql_with_schema(schema).unwrap();
    sqlx::raw_sql(sqlx::AssertSqlSafe(ddl))
        .execute(&pool)
        .await
        .unwrap();
    let seed = PgMachineIdLease::seeding_sql_with_schema(schema).unwrap();
    sqlx::raw_sql(sqlx::AssertSqlSafe(seed))
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "UPDATE {} SET claimed_at = NOW(), reclaimable_after = NOW() + INTERVAL '1 hour'",
        qualified(schema)
    )))
    .execute(&pool)
    .await
    .unwrap();

    match PgMachineIdLease::builder(pool.clone())
        .schema_name(schema)
        .unwrap()
        .build()
        .await
    {
        Err(PgLeaseError::NoMachineIdAvailable) => {}
        other => panic!("expected NoMachineIdAvailable, got {other:?}"),
    }
}

/// A table that doesn't exist (schema absent) with the default builder is a
/// clean database error, not a panic.
#[tokio::test]
async fn missing_table_without_provision_errors() {
    let Some(pool) = pool().await else { return };
    let schema = "snowdrop_test_absent";
    drop_schema(&pool, schema).await;

    let result = PgMachineIdLease::builder(pool)
        .schema_name(schema)
        .unwrap()
        .build() // auto_provision defaults to false
        .await;
    assert!(
        matches!(result, Err(PgLeaseError::Database(_))),
        "expected a database error for the missing table, got {result:?}"
    );
}

/// A provisioned-but-unseeded table reports `TableNotSeeded`, distinct from
/// `NoMachineIdAvailable`; seeding then makes it work.
#[tokio::test]
async fn unseeded_table_is_distinguished_from_exhausted() {
    let Some(pool) = pool().await else { return };
    let schema = "snowdrop_test_unseeded";
    drop_schema(&pool, schema).await;

    // Create the schema + table but do NOT seed it.
    let ddl = PgMachineIdLease::schema_sql_with_schema(schema).unwrap();
    sqlx::raw_sql(sqlx::AssertSqlSafe(ddl))
        .execute(&pool)
        .await
        .unwrap();

    match PgMachineIdLease::builder(pool.clone())
        .schema_name(schema)
        .unwrap()
        .build()
        .await
    {
        Err(PgLeaseError::TableNotSeeded) => {}
        other => panic!("expected TableNotSeeded, got {other:?}"),
    }

    // Seed it, and acquisition succeeds.
    let seed = PgMachineIdLease::seeding_sql_with_schema(schema).unwrap();
    sqlx::raw_sql(sqlx::AssertSqlSafe(seed))
        .execute(&pool)
        .await
        .unwrap();
    let lease = PgMachineIdLease::builder(pool)
        .schema_name(schema)
        .unwrap()
        .build()
        .await
        .unwrap();
    assert_eq!(lease.machine_id().get(), 0);
}

/// `auto_provision(true)` on a custom schema creates the schema and table and
/// seeds all 1024 rows.
#[tokio::test]
async fn auto_provision_creates_and_seeds() {
    let Some(pool) = pool().await else { return };
    let schema = "snowdrop_test_provision";
    drop_schema(&pool, schema).await;

    let lease = PgMachineIdLease::builder(pool.clone())
        .schema_name(schema)
        .unwrap()
        .auto_provision(true)
        .build()
        .await
        .unwrap();
    assert_eq!(lease.machine_id().get(), 0);

    let rows: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT count(*) FROM {}",
        qualified(schema)
    )))
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(rows, 1024);
}

/// Concurrent `auto_provision(true)` from a cold start must be race-safe: one
/// creator wins, the rest tolerate the race, all get distinct machine IDs.
#[tokio::test]
async fn concurrent_auto_provision_is_race_safe() {
    let Some(pool) = pool().await else { return };
    let schema = "snowdrop_test_race";
    drop_schema(&pool, schema).await;

    let mut handles = Vec::new();
    for _ in 0..12 {
        let pool = pool.clone();
        handles.push(tokio::spawn(async move {
            PgMachineIdLease::builder(pool)
                .schema_name("snowdrop_test_race")
                .unwrap()
                .auto_provision(true)
                .build()
                .await
        }));
    }

    let mut leases = Vec::new();
    for h in handles {
        leases.push(
            h.await
                .unwrap()
                .expect("concurrent auto-provision should not race-fail"),
        );
    }
    // Hold every lease alive while checking, so no ID is released and reused.
    let ids: std::collections::HashSet<u16> = leases.iter().map(|l| l.machine_id().get()).collect();
    assert_eq!(
        ids.len(),
        12,
        "concurrent leases must get distinct machine IDs"
    );
}

/// A reserved-word schema name works, because the identifier is quoted.
#[tokio::test]
async fn reserved_word_schema_name_works() {
    let Some(pool) = pool().await else { return };
    let schema = "order"; // SQL reserved word
    drop_schema(&pool, schema).await;

    let lease = PgMachineIdLease::builder(pool)
        .schema_name(schema)
        .unwrap()
        .auto_provision(true)
        .build()
        .await
        .unwrap();
    assert_eq!(lease.machine_id().get(), 0);
    drop(lease);
}

#[tokio::test]
async fn generator_stamps_leased_machine_id() {
    let Some(pool) = pool().await else { return };
    let schema = "snowdrop_test_generate";
    drop_schema(&pool, schema).await;

    let generator = PgIdGenerator::builder(pool)
        .schema_name(schema)
        .unwrap()
        .auto_provision(true)
        .build()
        .await
        .unwrap();
    let machine_id = generator.machine_id().get();
    assert!(!generator.is_poisoned());

    let a = generator.generate().unwrap();
    let b = generator.generate_async().await.unwrap();
    assert!(b > a, "IDs from one generator must be strictly increasing");
    assert_eq!(a.machine_id().get(), machine_id);
    assert_eq!(b.machine_id().get(), machine_id);
}

/// A row whose fencing token was rotated out from under a live lease (as a steal
/// would do) must be detected on the next heartbeat, poisoning the lease and
/// driving a re-claim onto a fresh machine ID.
#[tokio::test]
async fn detects_stolen_lease_and_reacquires() {
    let Some(pool) = pool().await else { return };
    let schema = "snowdrop_test_stolen";
    drop_schema(&pool, schema).await;

    let lease = PgMachineIdLease::builder(pool.clone())
        .schema_name(schema)
        .unwrap()
        .auto_provision(true)
        .build()
        .await
        .unwrap();
    assert_eq!(lease.machine_id().get(), 0);

    // Simulate a steal: rotate machine ID 0's claimed_at so the lease's stored
    // fencing token no longer matches, and hold the row so re-claim must land on
    // the next free ID (1).
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "UPDATE {} SET claimed_at = NOW() + INTERVAL '1 second', \
         reclaimable_after = NOW() + INTERVAL '1 hour' WHERE machine_id = 0",
        qualified(schema)
    )))
    .execute(&pool)
    .await
    .unwrap();

    // The background task heartbeats ~20s after claim; wait for it to notice the
    // fencing mismatch and re-acquire a different ID.
    let deadline = Instant::now() + Duration::from_secs(40);
    loop {
        if lease.machine_id().get() == 1 && !lease.is_poisoned() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "lease did not detect the steal and re-acquire (machine_id={}, poisoned={})",
            lease.machine_id().get(),
            lease.is_poisoned(),
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}
