//! The authority behind Sendit's community servers and local bot matches.
mod bots;
mod clock;
mod config;
mod console;
mod dedicated;
mod discovery;
mod host;
mod local;
mod stats;

pub use dedicated::run_dedicated;
pub use local::LocalMatch;
