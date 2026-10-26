//! Configuration: `~/.daemoneye/etc/config.toml` parsing, FHS path helpers,
//! and first-run asset seeding. Split across submodules in phase-09; the
//! public surface is re-exported here.

mod lifecycle;
mod load;
pub mod path_audit;
mod runtime_tree;
mod seeds;
mod types;

pub use lifecycle::*;
pub use load::*;
pub use runtime_tree::*;
pub use seeds::*;
pub use types::*;

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
