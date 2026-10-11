// CLI Module

pub mod attach;
pub mod commands;
pub(crate) mod diff;
pub mod input;
pub mod local_cmds;
pub mod markdown;
pub mod palette;
pub mod render;
pub mod render_ratatui;
pub mod status;
pub mod tail;
pub mod transcript;
pub mod viewer;

pub use commands::*;
pub use local_cmds::*;
pub use status::*;

#[cfg(test)]
mod tests;
