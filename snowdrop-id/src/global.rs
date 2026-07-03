//! A process-global [`Generator`], for retrofits where injecting one isn't
//! practical.
//!
//! Configure once at server boot, then generate from anywhere:
//!
//! ```no_run
//! use snowdrop_id::MachineId;
//!
//! // In main(), after loading config:
//! snowdrop_id::global::init(MachineId::new(3).unwrap())
//!     .expect("global generator initialized twice");
//!
//! // Buried deep in the code:
//! let id = snowdrop_id::global::generate().unwrap();
//! ```
//!
//! # Guidance
//!
//! Prefer passing a [`Generator`] (or storing it in your app state) where
//! you can: an explicit dependency is easier to test — in particular, the
//! global is always driven by the system clock, so a mock [`Clock`] cannot
//! be injected through it. This module exists for the common migration
//! path where an ID generator is buried deep in existing code.
//!
//! Rules that keep the global from becoming a liability:
//!
//! - Only application startup code (e.g. `main`) should call [`init`].
//!   Libraries must never initialize the global.
//! - There is deliberately no default machine ID: generating before
//!   [`init`] panics rather than silently using an ID that could collide
//!   with another server's.
//!
//! [`Clock`]: crate::Clock

use std::sync::OnceLock;

use crate::{Epoch, GenerateError, Generator, MachineId, SnowdropId};

static GLOBAL: OnceLock<Generator> = OnceLock::new();

/// Initializes the global generator with the default [`Epoch`].
///
/// First call wins; later calls fail with [`AlreadyInitialized`] and leave
/// the existing generator untouched.
pub fn init(machine_id: MachineId) -> Result<(), AlreadyInitialized> {
    init_with(machine_id, Epoch::DEFAULT)
}

/// Initializes the global generator with a custom [`Epoch`].
///
/// First call wins; later calls fail with [`AlreadyInitialized`] and leave
/// the existing generator untouched.
pub fn init_with(machine_id: MachineId, epoch: Epoch) -> Result<(), AlreadyInitialized> {
    GLOBAL
        .set(Generator::builder(machine_id).epoch(epoch).build())
        .map_err(|_| AlreadyInitialized)
}

/// Returns the global generator, or `None` if [`init`] has not been called.
pub fn try_generator() -> Option<&'static Generator> {
    GLOBAL.get()
}

/// Returns the global generator.
///
/// Use this for anything beyond plain [`generate`] — e.g.
/// `global::generator().generate_async().await` with the `tokio` feature,
/// or [`Generator::try_generate`].
///
/// # Panics
///
/// Panics if the global generator has not been initialized. This is
/// deliberate: silently defaulting the machine ID could collide with
/// another server. Call [`init`] at application startup.
pub fn generator() -> &'static Generator {
    GLOBAL.get().expect(
        "snowdrop_id::global is not initialized; \
         call snowdrop_id::global::init(machine_id) at application startup",
    )
}

/// Generates the next ID from the global generator.
///
/// Equivalent to [`generator()`](generator)`.generate()`.
///
/// # Panics
///
/// Panics if the global generator has not been initialized (see
/// [`generator`]).
pub fn generate() -> Result<SnowdropId, GenerateError> {
    generator().generate()
}

/// Generates the next ID from the global generator, awaiting instead of
/// blocking the thread on (rare) sequence exhaustion.
///
/// Equivalent to [`generator()`](generator)`.generate_async().await`.
///
/// # Panics
///
/// Panics if the global generator has not been initialized (see
/// [`generator`]).
#[cfg(feature = "tokio")]
pub async fn generate_async() -> Result<SnowdropId, GenerateError> {
    generator().generate_async().await
}

/// The global generator was already initialized by an earlier [`init`] or
/// [`init_with`] call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AlreadyInitialized;

impl core::fmt::Display for AlreadyInitialized {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("the global Snowdrop generator is already initialized")
    }
}

impl std::error::Error for AlreadyInitialized {}
