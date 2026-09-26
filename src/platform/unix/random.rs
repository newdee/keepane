//! Random bytes from the system's generator.

use anyhow::{Context, Result};
use std::io::Read;

/// Fill `bytes` from `/dev/urandom`.
pub fn fill(bytes: &mut [u8]) -> Result<()> {
    std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(bytes)).context("/dev/urandom")
}
