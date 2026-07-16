//! Integration tests for Postgres lease-table machine-ID assignment.
//!
//! Require a live Postgres: set `SNOWDROP_TEST_PG_URL` (e.g.
//! `postgres://postgres:postgres@localhost:5432/postgres`); tests skip
//! silently when it is unset. Each test uses a distinct table so they are
//! isolated and can run concurrently against one database.

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

/// Fresh, empty table state for a test: drop any leftover from a prior run so
/// auto-create rebuilds it prepopulated.
async fn reset(pool: &PgPool, table: &str) {
    sqlx::query(sqlx::AssertSqlSafe(format!("DROP TABLE IF EXISTS {table}")))
        .execute(pool)
        .await
        .expect("drop failed");
}

#[tokio::test]
async fn leases_lowest_free_and_releases_on_drop() {
    let Some(pool) = pool().await else { return };
    let table = "snowdrop_test_lease_release";
    reset(&pool, table).await;

    let a = PgMachineIdLease::builder(pool.clone())
        .table_name(table)
        .unwrap()
        .build()
        .await
        .unwrap();
    let b = PgMachineIdLease::builder(pool.clone())
        .table_name(table)
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
            .table_name(table)
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
    let table = "snowdrop_test_lease_exhausted";
    reset(&pool, table).await;

    // Create the table via the published DDL, then mark every row held far
    // into the future without acquiring any lease.
    let ddl = PgMachineIdLease::schema_sql(table).unwrap();
    sqlx::query(sqlx::AssertSqlSafe(ddl))
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "UPDATE {table} SET claimed_at = NOW(), reclaimable_after = NOW() + INTERVAL '1 hour'"
    )))
    .execute(&pool)
    .await
    .unwrap();

    match PgMachineIdLease::builder(pool.clone())
        .table_name(table)
        .unwrap()
        .auto_create_table(false)
        .build()
        .await
    {
        Err(PgLeaseError::NoMachineIdAvailable) => {}
        other => panic!("expected NoMachineIdAvailable, got {other:?}"),
    }
}

#[tokio::test]
async fn generator_stamps_leased_machine_id() {
    let Some(pool) = pool().await else { return };
    let table = "snowdrop_test_lease_generate";
    reset(&pool, table).await;

    let generator = PgIdGenerator::builder(pool)
        .table_name(table)
        .unwrap()
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

/// A row whose fencing token was rotated out from under a live lease (as a
/// steal would do) must be detected on the next heartbeat, poisoning the lease
/// and driving a re-claim onto a fresh machine ID.
#[tokio::test]
async fn detects_stolen_lease_and_reacquires() {
    let Some(pool) = pool().await else { return };
    let table = "snowdrop_test_lease_stolen";
    reset(&pool, table).await;

    let lease = PgMachineIdLease::builder(pool.clone())
        .table_name(table)
        .unwrap()
        .build()
        .await
        .unwrap();
    assert_eq!(lease.machine_id().get(), 0);

    // Simulate a steal: rotate machine ID 0's claimed_at so the lease's stored
    // fencing token no longer matches, and occupy nothing else so re-claim can
    // only land on ID 0 again once we free it — instead leave 0 "held" by the
    // rotation and let re-claim pick the next free ID (1).
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "UPDATE {table} SET claimed_at = NOW() + INTERVAL '1 second', \
         reclaimable_after = NOW() + INTERVAL '1 hour' WHERE machine_id = 0"
    )))
    .execute(&pool)
    .await
    .unwrap();

    // The background task heartbeats ~20s after claim; wait for it to notice
    // the fencing mismatch and re-acquire a different ID.
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
