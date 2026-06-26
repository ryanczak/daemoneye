//! Unified AI tool definitions, schema rendering, typed args, and dispatch.
//! Split across submodules in phase-07; the public surface is re-exported here.

mod args;
mod defs;
mod dispatch;
pub(crate) mod schema;

pub use defs::TOOLS;
pub use dispatch::dispatch_tool_event;
pub use schema::{
    ParamDef, ParamTy, ToolDef, deferred_catalog_text, get_gemini_tool_definition,
    get_openai_tool_definition, get_tool_definition, render_gemini, select_tools, tools_in_group,
};
