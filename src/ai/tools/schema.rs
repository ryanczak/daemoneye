use super::defs::TOOLS;
use serde_json::{Value, json};

// ---------------------------------------------------------------------------
// Unified tool schema
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
pub enum ParamTy {
    Str,
    Bool,
    Int,
}

impl ParamTy {
    fn as_str(self) -> &'static str {
        match self {
            ParamTy::Str => "string",
            ParamTy::Bool => "boolean",
            ParamTy::Int => "integer",
        }
    }

    fn as_gemini_str(self) -> &'static str {
        match self {
            ParamTy::Str => "STRING",
            ParamTy::Bool => "BOOLEAN",
            ParamTy::Int => "INTEGER",
        }
    }
}

pub struct ParamDef {
    pub name: &'static str,
    pub ty: ParamTy,
    pub description: &'static str,
    pub required: bool,
}

pub struct ToolDef {
    pub name: &'static str,
    pub description: &'static str,
    pub params: &'static [ParamDef],
    /// `None` = core (always rendered, prose-documented in sre.toml).
    /// `Some(group)` = deferred: omitted from the default render, loaded on demand
    /// via `load_tools`, and listed in that tool's generated catalog under `group`.
    pub deferred_group: Option<&'static str>,
}

// ---------------------------------------------------------------------------
// Provider renderers
// ---------------------------------------------------------------------------

/// Closed value set for a parameter, keyed by param name. Returned as a JSON
/// `enum` in every provider's schema so the model is constrained to valid values
/// instead of relying on the prose description. Param names are globally unique
/// across `TOOLS` (operation→edit_file, kind→search_repository) or share one set
/// (category→the five memory tools), so name-keying is unambiguous.
pub(super) fn enum_values(param_name: &str) -> Option<&'static [&'static str]> {
    match param_name {
        "operation" => Some(&["edit", "create", "delete", "copy"]),
        "kind" => Some(&["runbooks", "scripts", "memory", "events", "all"]),
        "category" => Some(&["session", "knowledge", "incident"]),
        _ => None,
    }
}

fn build_properties(params: &[ParamDef]) -> serde_json::Map<String, Value> {
    params
        .iter()
        .map(|p| {
            let mut schema = json!({
                "type": p.ty.as_str(),
                "description": p.description,
            });
            if let Some(values) = enum_values(p.name) {
                schema["enum"] = json!(values);
            }
            (p.name.to_string(), schema)
        })
        .collect()
}

fn build_gemini_properties(params: &[ParamDef]) -> serde_json::Map<String, Value> {
    params
        .iter()
        .map(|p| {
            let mut schema = json!({
                "type": p.ty.as_gemini_str(),
                "description": p.description,
            });
            if let Some(values) = enum_values(p.name) {
                schema["enum"] = json!(values);
            }
            (p.name.to_string(), schema)
        })
        .collect()
}

fn required_names(params: &[ParamDef]) -> Vec<&'static str> {
    params
        .iter()
        .filter(|p| p.required)
        .map(|p| p.name)
        .collect()
}

pub(super) fn render_anthropic(tools: &[&ToolDef]) -> Value {
    let catalog = deferred_catalog_text();
    Value::Array(
        tools
            .iter()
            .map(|t| {
                let props = build_properties(t.params);
                let req = required_names(t.params);
                let mut schema = json!({ "type": "object", "properties": props });
                if !req.is_empty() {
                    schema["required"] = json!(req);
                }
                let desc = if t.name == "load_tools" {
                    format!("{}\n\nAvailable groups:\n{}", t.description, catalog)
                } else {
                    t.description.to_string()
                };
                json!({ "name": t.name, "description": desc, "input_schema": schema })
            })
            .collect(),
    )
}

fn render_openai(tools: &[&ToolDef]) -> Value {
    let catalog = deferred_catalog_text();
    Value::Array(
        tools
            .iter()
            .map(|t| {
                let props = build_properties(t.params);
                let req = required_names(t.params);
                let mut params = json!({ "type": "object", "properties": props });
                if !req.is_empty() {
                    params["required"] = json!(req);
                }
                let desc = if t.name == "load_tools" {
                    format!("{}\n\nAvailable groups:\n{}", t.description, catalog)
                } else {
                    t.description.to_string()
                };
                json!({ "type": "function", "function": {
                    "name": t.name, "description": desc, "parameters": params
                }})
            })
            .collect(),
    )
}

pub fn render_gemini(tools: &[&ToolDef]) -> Value {
    let catalog = deferred_catalog_text();
    Value::Array(
        tools
            .iter()
            .map(|t| {
                let props = build_gemini_properties(t.params);
                let req = required_names(t.params);
                let mut params = json!({ "type": "OBJECT", "properties": props });
                if !req.is_empty() {
                    params["required"] = json!(req);
                }
                let desc = if t.name == "load_tools" {
                    format!("{}\n\nAvailable groups:\n{}", t.description, catalog)
                } else {
                    t.description.to_string()
                };
                json!({ "name": t.name, "description": desc, "parameters": params })
            })
            .collect(),
    )
}

/// Return the subset of `TOOLS` that should be rendered for this session.
/// Core tools (deferred_group: None) are always included.
/// Deferred tools are included only if their name appears in `loaded`.
pub fn select_tools(loaded: &[String]) -> Vec<&'static ToolDef> {
    TOOLS
        .iter()
        .filter(|t| t.deferred_group.is_none() || loaded.iter().any(|n| n == t.name))
        .collect()
}

/// Lines describing each deferred group and its members, e.g.
/// "  - agents: create_agent, read_agent, list_agents, delete_agent".
pub fn deferred_catalog_text() -> String {
    let mut groups: Vec<(&'static str, Vec<&'static str>)> = Vec::new();
    for t in TOOLS.iter() {
        if let Some(g) = t.deferred_group {
            if let Some(entry) = groups.iter_mut().find(|(grp, _)| *grp == g) {
                entry.1.push(t.name);
            } else {
                groups.push((g, vec![t.name]));
            }
        }
    }
    groups
        .iter()
        .map(|(g, names)| format!("  - {}: {}", g, names.join(", ")))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Resolve a deferred group name to its member tool names.
pub fn tools_in_group(group: &str) -> Vec<&'static str> {
    TOOLS
        .iter()
        .filter(|t| t.deferred_group == Some(group))
        .map(|t| t.name)
        .collect()
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

pub fn get_tool_definition(loaded: &[String]) -> Value {
    render_anthropic(&select_tools(loaded).into_iter().collect::<Vec<_>>())
}

pub fn get_openai_tool_definition(loaded: &[String]) -> Value {
    render_openai(&select_tools(loaded).into_iter().collect::<Vec<_>>())
}

pub fn get_gemini_tool_definition(loaded: &[String]) -> Value {
    render_gemini(&select_tools(loaded).into_iter().collect::<Vec<_>>())
}
