use core::fmt;

/// The reference instant that the timestamp field counts 1024 ms windows
/// from, expressed in Unix milliseconds.
///
/// All generators sharing an ID space must use the same epoch, and it must
/// not change after IDs have been issued. To interleave exactly with an
/// existing Snowflake deployment, use that deployment's epoch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Epoch(u64);

impl Epoch {
    /// The default epoch: 2025-01-01T00:00:00Z. Timestamps exhaust at
    /// 2094-09-07T15:47:35Z.
    pub const DEFAULT: Epoch = Epoch(1_735_689_600_000);

    /// Twitter's Snowflake epoch: 2010-11-04T01:42:54.657Z. Use for
    /// interchangeability with classic Snowflake deployments.
    pub const TWITTER: Epoch = Epoch(1_288_834_974_657);

    /// Creates an epoch from a Unix-milliseconds timestamp.
    pub const fn from_unix_ms(ms: u64) -> Epoch {
        Epoch(ms)
    }

    /// Returns the epoch as Unix milliseconds.
    pub const fn unix_ms(self) -> u64 {
        self.0
    }
}

impl Default for Epoch {
    fn default() -> Epoch {
        Epoch::DEFAULT
    }
}

impl fmt::Display for Epoch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}
