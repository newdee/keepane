//! Linux and macOS: pty panes (portable-pty), a Unix socket, termios, process
//! groups. What differs between the two is marked with `target_os` inside.

pub mod clipboard;
pub mod console;
pub mod ipc;
pub mod keys;
pub mod notify;
pub mod proccwd;
pub mod process;
pub mod random;
pub mod shell;
pub mod shutdown;
pub mod startup;
pub mod sysinfo;
pub mod update;
pub mod wt;
