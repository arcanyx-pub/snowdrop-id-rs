use core::fmt;

/// A validated machine ID in `0..=1023`.
///
/// Every concurrently active generator sharing an ID space must have a
/// unique machine ID. Lower machine IDs produce shorter base62 strings:
/// IDs 0–25 keep the first ID of each window at 6 characters or fewer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MachineId(u16);

impl MachineId {
    /// The largest valid machine ID (1023).
    pub const MAX: MachineId = MachineId(1023);

    /// Creates a machine ID, returning `None` if `value > 1023`.
    pub const fn new(value: u16) -> Option<MachineId> {
        if value <= 1023 {
            Some(MachineId(value))
        } else {
            None
        }
    }

    /// Returns the machine ID as an integer.
    pub const fn get(self) -> u16 {
        self.0
    }
}

impl fmt::Display for MachineId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl From<MachineId> for u16 {
    fn from(mid: MachineId) -> u16 {
        mid.0
    }
}

impl TryFrom<u16> for MachineId {
    type Error = crate::InvalidId;

    fn try_from(value: u16) -> Result<MachineId, Self::Error> {
        MachineId::new(value).ok_or(crate::InvalidId::MachineIdOutOfRange)
    }
}
