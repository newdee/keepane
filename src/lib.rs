//! keepane: persistent terminal panes, each with its own inbox (a terminal multiplexer).

pub mod client;
pub mod command;
pub mod completion;
pub mod config;
pub mod dashboard;
pub mod feishu;
pub mod format;
pub mod git;
pub mod gitsync;
pub mod histlog;
pub mod ipc;
pub mod keys;
pub mod legacy;
pub mod link;
pub mod link_proto;
pub mod link_stream;
pub mod link_watch;
pub mod logger;
pub mod man;
pub mod mcp;
pub mod pager;
pub mod platform;
pub mod resurrect;
pub mod server;
pub mod setup;
pub mod share;
pub mod shellhook;
pub mod sysinfo;
pub mod takeover;
pub mod truecolor;
pub mod vtinput;
pub mod web;

// Where these lived before the platform split, so their paths stay.
#[cfg(windows)]
pub use platform::winsec;
pub use platform::{clipboard, console, notify, proccwd, shutdown, startup, update, wt};
