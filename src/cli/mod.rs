// CLI Module

pub mod commands;
pub(crate) mod diff;
pub mod input;
pub mod local_cmds;
pub mod markdown;
pub mod notify;
pub mod palette;
pub mod render;
pub mod render_ratatui;
pub mod status;
pub mod transcript;

pub use commands::*;
pub use local_cmds::*;
pub use notify::*;
pub use status::*;

#[cfg(test)]
mod tests;
