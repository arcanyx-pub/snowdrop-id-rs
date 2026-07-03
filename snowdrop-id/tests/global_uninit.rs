//! The uninitialized-global panic, in its own test binary so no other test
//! can have initialized the process-wide state first.

#[test]
#[should_panic(expected = "snowdrop_id::global is not initialized")]
fn generate_before_init_panics() {
    let _ = snowdrop_id::global::generate();
}
