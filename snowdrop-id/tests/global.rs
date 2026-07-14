//! Tests for the process-global generator. Kept in their own integration
//! test binary (= their own process) because the global is process-wide
//! state; the whole lifecycle runs in a single #[test] to control order.

use snowdrop_id::{Epoch, MachineId, global};

#[test]
fn global_lifecycle() {
    // Untouched process: nothing initialized yet.
    assert!(global::try_generator().is_none());

    let mid = MachineId::new(5).unwrap();
    global::init_with(mid, Epoch::DEFAULT).unwrap();

    // Second init fails and leaves the first configuration in place.
    assert_eq!(
        global::init(MachineId::new(9).unwrap()),
        Err(global::AlreadyInitialized)
    );
    assert_eq!(global::generator().machine_id(), mid);

    // Accessors agree on the same instance.
    assert!(std::ptr::eq(
        global::generator(),
        global::try_generator().unwrap()
    ));

    // Generation works and is monotonic.
    let a = global::generate().unwrap();
    let b = global::generate().unwrap();
    assert!(b > a);
    assert_eq!(a.machine_id(), mid);
}
