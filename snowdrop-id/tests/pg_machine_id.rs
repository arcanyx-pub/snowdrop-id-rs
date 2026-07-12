//! Integration tests for Postgres advisory-lock machine-ID leasing.
//!
//! Require a live Postgres: set `SNOWDROP_TEST_PG_URL` (e.g.
//! `postgres://postgres:postgres@localhost:5432/postgres`); tests skip
//! silently when it is unset. Each test uses a distinct lock namespace so
//! they can run concurrently against one database.

use std::time::{Duration, Instant};

use snowdrop_id::{
    Epoch, LeaseLossPolicy, PgIdGenerator, PgLeaseConfig, PgLeaseError, PgMachineIdLease,
};
use sqlx::Connection;
use sqlx::postgres::{PgConnectOptions, PgConnection};

fn pg_options() -> Option<PgConnectOptions> {
    match std::env::var("SNOWDROP_TEST_PG_URL") {
        Ok(url) => Some(url.parse().expect("invalid SNOWDROP_TEST_PG_URL")),
        Err(_) => {
            eprintln!("skipping: SNOWDROP_TEST_PG_URL not set");
            None
        }
    }
}

/// The pid of the session holding the 64-bit advisory lock `key`, if any.
/// (Postgres stores the key split as classid = high 32 bits, objid = low
/// 32 bits; bigint shifts are pure bit ops, so reassembly can't overflow.)
async fn lock_holder(admin: &mut PgConnection, key: i64) -> Option<i32> {
    sqlx::query_scalar(
        "SELECT pid FROM pg_locks \
         WHERE locktype = 'advisory' \
           AND ((classid::int8 << 32) | objid::int8) = $1::int8",
    )
    .bind(key)
    .fetch_optional(admin)
    .await
    .expect("pg_locks query failed")
}

#[tokio::test]
async fn leases_lowest_free_machine_ids_and_releases_on_drop() {
    let Some(options) = pg_options() else { return };
    let config = || PgLeaseConfig::new().namespace(-0x1000_0000_0001);

    let a = PgMachineIdLease::acquire_with(options.clone(), config())
        .await
        .unwrap();
    let b = PgMachineIdLease::acquire_with(options.clone(), config())
        .await
        .unwrap();
    assert_eq!(a.machine_id().get(), 0);
    assert_eq!(b.machine_id().get(), 1);
    assert!(!a.is_poisoned());

    // Dropping releases the lock server-side; ID 0 becomes leasable again
    // (allow the server a moment to clean up the terminated session).
    drop(a);
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match PgMachineIdLease::acquire_with(options.clone(), config()).await {
            Ok(c) if c.machine_id().get() == 0 => break,
            Ok(_) | Err(_) => {
                assert!(Instant::now() < deadline, "machine ID 0 never released");
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    }
}

#[tokio::test]
async fn range_exhaustion_errors() {
    let Some(options) = pg_options() else { return };
    let config = || {
        PgLeaseConfig::new()
            .namespace(-0x1000_0000_0002)
            .machine_ids(7..=8)
    };

    let _a = PgMachineIdLease::acquire_with(options.clone(), config())
        .await
        .unwrap();
    let _b = PgMachineIdLease::acquire_with(options.clone(), config())
        .await
        .unwrap();
    match PgMachineIdLease::acquire_with(options, config()).await {
        Err(PgLeaseError::NoMachineIdAvailable) => {}
        other => panic!("expected NoMachineIdAvailable, got {other:?}"),
    }
}

#[tokio::test]
async fn generator_stamps_leased_machine_id() {
    let Some(options) = pg_options() else { return };
    let config = PgLeaseConfig::new()
        .namespace(-0x1000_0000_0003)
        .machine_ids(3..=3);

    let generator = PgIdGenerator::acquire_with(options, config, Epoch::DEFAULT)
        .await
        .unwrap();
    assert_eq!(generator.machine_id().get(), 3);

    let a = generator.generate().unwrap();
    let b = generator.generate_async().await.unwrap();
    assert!(b > a);
    assert_eq!(a.machine_id().get(), 3);
    assert_eq!(b.machine_id().get(), 3);
}

/// Kills the lease's backend session and verifies the keepalive task
/// re-acquires the same machine ID on a fresh session.
#[tokio::test]
async fn reacquires_after_backend_termination() {
    let Some(options) = pg_options() else { return };
    let namespace: i64 = -0x1000_0000_0004;
    let config = PgLeaseConfig::new()
        .namespace(namespace)
        .machine_ids(0..=1)
        .keepalive_interval(Duration::from_millis(100))
        .policy(LeaseLossPolicy::Poison);

    let generator = PgIdGenerator::acquire_with(options.clone(), config, Epoch::DEFAULT)
        .await
        .unwrap();
    assert_eq!(generator.machine_id().get(), 0);
    generator.generate().unwrap();

    // Kill the lease's backend from a second connection.
    let key = namespace; // machine ID 0 => key == namespace
    let mut admin = PgConnection::connect_with(&options).await.unwrap();
    let old_pid = lock_holder(&mut admin, key)
        .await
        .expect("lease backend should hold the advisory lock");
    sqlx::query("SELECT pg_terminate_backend($1)")
        .bind(old_pid)
        .execute(&mut admin)
        .await
        .unwrap();

    // The keepalive task must re-acquire the same key on a new session.
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match lock_holder(&mut admin, key).await {
            Some(pid) if pid != old_pid => break,
            _ => {
                assert!(
                    Instant::now() < deadline,
                    "lock was not re-acquired in time"
                );
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
    }

    // Same machine ID, and generation recovers (the poisoned flag may lag
    // the server-side lock by one scheduler tick).
    assert_eq!(generator.machine_id().get(), 0);
    let deadline = Instant::now() + Duration::from_secs(5);
    let id = loop {
        match generator.generate() {
            Ok(id) => break id,
            Err(e) => {
                assert!(Instant::now() < deadline, "generate did not recover: {e}");
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
    };
    assert_eq!(id.machine_id().get(), 0);
}
