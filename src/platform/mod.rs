//! What differs from one operating system to the next, one module per
//! platform with the same names inside (docs/design/platform.md). The rest
//! of keepane calls `crate::platform::…`; a function one platform lacks
//! fails that platform's build.

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::*;
#[cfg(unix)]
mod unix;
#[cfg(unix)]
pub use unix::*;
