//! Async generation from the process-global generator, in its own test binary
//! so the global state starts uninitialized regardless of test ordering (the
//! lifecycle test in `global.rs` asserts an untouched process, which only
//! holds when it owns the process).

#![cfg(feature = "tokio")]

use snowdrop_id::{MachineId, global};

#[tokio::test]
async fn global_generate_async() {
    let mid = MachineId::new(5).unwrap();
    global::init(mid).unwrap();

    let a = global::generate_async().await.unwrap();
    let b = global::generate_async().await.unwrap();
    assert!(b > a);
    assert_eq!(a.machine_id(), mid);
}
