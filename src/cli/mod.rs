// CLI Module

pub mod attach;
pub mod commands;
pub(crate) mod diff;
pub mod doctor;
pub mod input;
pub mod local_cmds;
pub mod markdown;
pub mod palette;
pub mod program_status_emit;
pub mod render;
pub mod render_ratatui;
pub mod status;
pub mod tail;
pub(crate) mod terminal_guard;
pub mod transcript;
pub mod viewer;

pub use commands::*;
pub use doctor::{DoctorInputs, run_doctor};
pub use local_cmds::*;
pub use status::*;

#[cfg(test)]
mod tests;
