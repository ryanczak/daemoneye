//! Configuration: `~/.daemoneye/etc/config.toml` parsing, FHS path helpers,
//! and first-run asset seeding. Split across submodules in phase-09; the
//! public surface is re-exported here.

mod lifecycle;
mod load;
mod models;
pub mod path_audit;
mod runtime_tree;
mod sandbox;
mod seeds;
mod shells;
mod types;

pub use lifecycle::*;
pub use load::*;
pub use models::*;
pub use runtime_tree::*;
pub use sandbox::*;
pub use seeds::*;
pub use shells::*;
pub use types::*;

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
