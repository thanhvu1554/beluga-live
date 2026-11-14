//! Beluga Live PC server: bridges the iPhone tutor app and Claude Code.

pub mod brain;
pub mod config;
pub mod http;
pub mod memory;
pub mod mcp;
pub mod parser;
pub mod session;
pub mod store;
pub mod study;
pub mod voice;

pub use config::Config;
pub use http::{start, AppState};
