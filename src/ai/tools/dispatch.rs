use super::args::*;
use crate::ai::types::AiEvent;
use serde_json::Value;

// ---------------------------------------------------------------------------
// Tool event dispatcher (shared by all three provider backends)
// ---------------------------------------------------------------------------

/// Dispatch arm helper — deserialises `args` into `T` and constructs the event.
fn dispatch<T: ToolArgs>(id: &str, args: Value, ts: Option<String>) -> Option<AiEvent> {
    T::from_value(args).map(|a| a.to_event(id, ts))
}

/// Given a tool call ID, name, and parsed arguments, produce the corresponding
/// [`AiEvent`].  Returns `None` for unrecognised tool names.
pub fn dispatch_tool_event(
    id: &str,
    name: &str,
    args: &Value,
    ts: Option<String>,
) -> Option<AiEvent> {
    let args = args.clone();
    match name {
        "run_terminal_command" => dispatch::<RunTerminalCommandArgs>(id, args, ts),
        "load_tools" => dispatch::<LoadToolsArgs>(id, args, ts),
        "schedule_command" => dispatch::<ScheduleCommandArgs>(id, args, ts),
        "list_schedules" => Some(AiEvent::ListSchedules {
            id: id.to_string(),
            thought_signature: ts,
        }),
        "list_scripts" => Some(AiEvent::ListScripts {
            id: id.to_string(),
            thought_signature: ts,
        }),
        "list_runbooks" => Some(AiEvent::ListRunbooks {
            id: id.to_string(),
            thought_signature: ts,
        }),
        "get_terminal_context" => Some(AiEvent::GetTerminalContext {
            id: id.to_string(),
            thought_signature: ts,
        }),
        "list_panes" => Some(AiEvent::ListPanes {
            id: id.to_string(),
            thought_signature: ts,
        }),
        "cancel_schedule" => {
            schedule_id_event(args, ts.clone(), |jid, t| AiEvent::CancelSchedule {
                id: id.to_string(),
                job_id: jid,
                thought_signature: t,
            })
        }
        "delete_schedule" => schedule_id_event(args, ts, |jid, t| AiEvent::DeleteSchedule {
            id: id.to_string(),
            job_id: jid,
            thought_signature: t,
        }),
        "write_script" => dispatch::<WriteScriptArgs>(id, args, ts),
        "read_script" => dispatch::<ReadScriptArgs>(id, args, ts),
        "delete_script" => dispatch::<DeleteScriptArgs>(id, args, ts),
        "watch_pane" => dispatch::<WatchPaneArgs>(id, args, ts),
        "read_file" => dispatch::<ReadFileArgs>(id, args, ts),
        "read_pane" => dispatch::<ReadPaneArgs>(id, args, ts),
        "edit_file" => dispatch::<EditFileArgs>(id, args, ts),
        "write_runbook" => dispatch::<WriteRunbookArgs>(id, args, ts),
        "delete_runbook" => runbook_name_event(args, ts, |nm, t| AiEvent::DeleteRunbook {
            id: id.to_string(),
            name: nm,
            thought_signature: t,
        }),
        "read_runbook" => dispatch::<ReadRunbookArgs>(id, args, ts),
        "add_memory" => dispatch::<AddMemoryArgs>(id, args, ts),
        "update_memory" => dispatch::<UpdateMemoryArgs>(id, args, ts),
        "delete_memory" => dispatch::<DeleteMemoryArgs>(id, args, ts),
        "read_memory" => dispatch::<ReadMemoryArgs>(id, args, ts),
        "list_memories" => dispatch::<ListMemoriesArgs>(id, args, ts),
        "search_repository" => dispatch::<SearchRepositoryArgs>(id, args, ts),
        "recall_context" => dispatch::<RecallContextArgs>(id, args, ts),
        "close_background_window" => dispatch::<CloseBgWindowArgs>(id, args, ts),
        "spawn_ghost_shell" => dispatch::<SpawnGhostArgs>(id, args, ts),
        "create_agent" => dispatch::<CreateAgentArgs>(id, args, ts),
        "read_agent" => dispatch::<ReadAgentArgs>(id, args, ts),
        "list_agents" => Some(AiEvent::ListAgents {
            id: id.to_string(),
            thought_signature: ts,
        }),
        "delete_agent" => runbook_name_event(args, ts, |nm, t| AiEvent::DeleteAgent {
            id: id.to_string(),
            name: nm,
            thought_signature: t,
        }),
        "await_agent_result" => dispatch::<AwaitAgentResultArgs>(id, args, ts),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::super::defs::TOOLS;
    use super::super::schema::{
        ToolDef, enum_values, get_tool_definition, render_anthropic, render_gemini, select_tools,
        tools_in_group,
    };
    use super::*;
    use serde_json::json;

    /// Every tool in TOOLS must appear in the Gemini render, in order.
    /// This is the regression test that would have caught every previous
    /// "tool missing from Gemini" bug.
    #[test]
    fn render_gemini_names_match_tools_slice() {
        let selection: Vec<&ToolDef> = TOOLS.iter().collect();
        let rendered = render_gemini(&selection);
        let arr = rendered
            .as_array()
            .expect("render_gemini must return an array");
        assert_eq!(
            arr.len(),
            TOOLS.len(),
            "rendered Gemini tool count ({}) != TOOLS slice length ({})",
            arr.len(),
            TOOLS.len()
        );
        for (i, (entry, def)) in arr.iter().zip(TOOLS.iter()).enumerate() {
            assert_eq!(
                entry["name"].as_str().unwrap(),
                def.name,
                "tool at index {} name mismatch",
                i
            );
        }
    }

    /// Parameter types must use Gemini's uppercase strings, not the lowercase
    /// variants used by Anthropic/OpenAI.
    #[test]
    fn render_gemini_types_are_uppercase() {
        let rendered = render_gemini(&TOOLS.iter().collect::<Vec<_>>());
        let arr = rendered.as_array().unwrap();
        let rtc = arr
            .iter()
            .find(|e| e["name"] == "run_terminal_command")
            .expect("run_terminal_command must be present");
        let props = &rtc["parameters"]["properties"];
        assert_eq!(props["command"]["type"], "STRING");
        assert_eq!(props["background"]["type"], "BOOLEAN");
        // target_pane is STRING too
        assert_eq!(props["target_pane"]["type"], "STRING");
    }

    /// Required fields must match the ParamDef required flags.
    #[test]
    fn render_gemini_required_fields_correct() {
        let rendered = render_gemini(&TOOLS.iter().collect::<Vec<_>>());
        let arr = rendered.as_array().unwrap();

        // run_terminal_command: only "command" is required
        let rtc = arr
            .iter()
            .find(|e| e["name"] == "run_terminal_command")
            .unwrap();
        let req = rtc["parameters"]["required"].as_array().unwrap();
        assert_eq!(req, &[serde_json::json!("command")]);

        // edit_file: only "path" is required; old_string/new_string/content/operation are optional
        let ef = arr.iter().find(|e| e["name"] == "edit_file").unwrap();
        let req_ef: Vec<&str> = ef["parameters"]["required"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert!(req_ef.contains(&"path"));
        assert!(
            !req_ef.contains(&"old_string"),
            "old_string should now be optional"
        );
        assert!(
            !req_ef.contains(&"new_string"),
            "new_string should now be optional"
        );
    }

    /// Tools with no params must not have a "required" key (would be an API error).
    #[test]
    fn render_gemini_no_required_for_empty_params() {
        let rendered = render_gemini(&TOOLS.iter().collect::<Vec<_>>());
        let arr = rendered.as_array().unwrap();
        let ls = arr.iter().find(|e| e["name"] == "list_schedules").unwrap();
        assert!(
            ls["parameters"].get("required").is_none(),
            "list_schedules must not have a 'required' key"
        );
    }

    /// Every tool in TOOLS must have a dispatch arm that accepts conformant args.
    /// This catches the case where a tool is added to TOOLS but forgotten in
    /// dispatch_tool_event, or where a required param name diverges between the
    /// ToolDef schema and the typed arg struct.
    #[test]
    fn dispatch_roundtrip_all_tools() {
        fn minimal_args(name: &str) -> Value {
            match name {
                "run_terminal_command" => json!({"command": "echo hi"}),
                "schedule_command" => json!({"name": "job"}),
                "list_schedules" => json!({}),
                "cancel_schedule" => json!({"id": "00000000-0000-0000-0000-000000000000"}),
                "delete_schedule" => json!({"id": "00000000-0000-0000-0000-000000000000"}),
                "write_script" => json!({"script_name": "s.sh", "content": "#!/bin/sh"}),
                "list_scripts" => json!({}),
                "read_script" => json!({"script_name": "s.sh"}),
                "delete_script" => json!({"script_name": "s.sh"}),
                "watch_pane" => json!({"pane_id": "%1"}),
                "read_file" => json!({"path": "/tmp/f"}),
                "read_pane" => json!({"pane_id": "%3"}),
                "edit_file" => json!({"path": "/tmp/f"}),
                "write_runbook" => json!({"name": "rb", "content": "# RB"}),
                "delete_runbook" => json!({"name": "rb"}),
                "read_runbook" => json!({"name": "rb"}),
                "list_runbooks" => json!({}),
                "add_memory" => json!({"key": "k", "value": "v", "category": "knowledge"}),
                "update_memory" => json!({"key": "k", "category": "knowledge"}),
                "delete_memory" => json!({"key": "k", "category": "knowledge"}),
                "read_memory" => json!({"key": "k", "category": "knowledge"}),
                "list_memories" => json!({}),
                "search_repository" => json!({"query": "x", "kind": "all"}),
                "get_terminal_context" => json!({}),
                "list_panes" => json!({}),
                "close_background_window" => json!({"pane_id": "%1"}),
                "spawn_ghost_shell" => json!({"runbook": "rb", "message": "investigate"}),
                "create_agent" => {
                    json!({"name": "analyst", "description": "test", "prompt": "You are a test."})
                }
                "read_agent" => json!({"name": "analyst"}),
                "list_agents" => json!({}),
                "delete_agent" => json!({"name": "analyst"}),
                "await_agent_result" => json!({"job_id": "ghost-abc-123", "agent_name": "analyst"}),
                "load_tools" => json!({"groups": ["agents"]}),
                _ => json!({}),
            }
        }

        for tool in TOOLS {
            let args = minimal_args(tool.name);
            let ev = dispatch_tool_event("tc_1", tool.name, &args, None);
            assert!(
                ev.is_some(),
                "dispatch_tool_event returned None for tool '{}'. \
                 Either the dispatch arm is missing or the arg struct's required fields \
                 don't match the ToolDef schema.",
                tool.name
            );
        }
    }

    /// The model must not be able to inject a namespace into a read tool.
    /// This is the direct lock: if any read tool ever gains a `namespace` or
    /// `namespaces` param, this test fails.
    #[test]
    fn read_tools_expose_no_namespace_param() {
        for tool in ["read_memory", "list_memories", "search_repository"] {
            let def = TOOLS
                .iter()
                .find(|t| t.name == tool)
                .unwrap_or_else(|| panic!("tool {tool} missing from TOOLS"));
            for p in def.params {
                assert!(
                    p.name != "namespace" && p.name != "namespaces",
                    "{tool} must not expose a namespace param (got '{}') — the namespace \
                     set is built server-side, never caller-supplied",
                    p.name
                );
            }
        }
    }

    #[test]
    fn enum_values_known_params() {
        assert_eq!(
            enum_values("operation"),
            Some(&["edit", "create", "delete", "copy"] as &[&str])
        );
        assert_eq!(
            enum_values("kind"),
            Some(&["runbooks", "scripts", "memory", "events", "all"] as &[&str])
        );
        assert_eq!(
            enum_values("category"),
            Some(&["session", "knowledge", "incident"] as &[&str])
        );
        assert!(enum_values("command").is_none());
        assert!(enum_values("nonexistent").is_none());
    }

    #[test]
    fn anthropic_render_emits_enums() {
        let rendered = render_anthropic(&TOOLS.iter().collect::<Vec<_>>());
        let arr = rendered.as_array().unwrap();

        let edit_file = arr.iter().find(|e| e["name"] == "edit_file").unwrap();
        let op = &edit_file["input_schema"]["properties"]["operation"];
        assert_eq!(
            op.get("enum"),
            Some(&json!(["edit", "create", "delete", "copy"])),
            "edit_file.operation must carry an enum"
        );

        let search = arr
            .iter()
            .find(|e| e["name"] == "search_repository")
            .unwrap();
        let kind = &search["input_schema"]["properties"]["kind"];
        assert_eq!(
            kind.get("enum"),
            Some(&json!(["runbooks", "scripts", "memory", "events", "all"])),
            "search_repository.kind must carry an enum"
        );

        let add_mem = arr.iter().find(|e| e["name"] == "add_memory").unwrap();
        let cat = &add_mem["input_schema"]["properties"]["category"];
        assert_eq!(
            cat.get("enum"),
            Some(&json!(["session", "knowledge", "incident"])),
            "add_memory.category must carry an enum"
        );

        let run_cmd = arr
            .iter()
            .find(|e| e["name"] == "run_terminal_command")
            .unwrap();
        let cmd = &run_cmd["input_schema"]["properties"]["command"];
        assert!(
            cmd.get("enum").is_none(),
            "run_terminal_command.command must NOT carry an enum"
        );
    }

    #[test]
    fn gemini_render_emits_enums() {
        let rendered = render_gemini(&TOOLS.iter().collect::<Vec<_>>());
        let arr = rendered.as_array().unwrap();

        let edit_file = arr.iter().find(|e| e["name"] == "edit_file").unwrap();
        let op = &edit_file["parameters"]["properties"]["operation"];
        assert_eq!(
            op.get("enum"),
            Some(&json!(["edit", "create", "delete", "copy"])),
            "edit_file.operation must carry an enum in Gemini render"
        );

        let search = arr
            .iter()
            .find(|e| e["name"] == "search_repository")
            .unwrap();
        let kind = &search["parameters"]["properties"]["kind"];
        assert_eq!(
            kind.get("enum"),
            Some(&json!(["runbooks", "scripts", "memory", "events", "all"])),
            "search_repository.kind must carry an enum in Gemini render"
        );

        let add_mem = arr.iter().find(|e| e["name"] == "add_memory").unwrap();
        let cat = &add_mem["parameters"]["properties"]["category"];
        assert_eq!(
            cat.get("enum"),
            Some(&json!(["session", "knowledge", "incident"])),
            "add_memory.category must carry an enum in Gemini render"
        );
    }

    #[test]
    fn create_agent_accepts_array_and_string_scripts() {
        // As a real JSON array
        let ev_array = dispatch_tool_event(
            "tc_1",
            "create_agent",
            &json!({
                "name": "analyst",
                "description": "test",
                "prompt": "You are a test.",
                "auto_approve_scripts": ["a.sh"],
            }),
            None,
        );
        if let Some(AiEvent::CreateAgent {
            auto_approve_scripts,
            ..
        }) = ev_array
        {
            assert_eq!(auto_approve_scripts, vec!["a.sh"]);
        } else {
            panic!("expected AiEvent::CreateAgent from array input");
        }

        // As a JSON-encoded string
        let ev_string = dispatch_tool_event(
            "tc_1",
            "create_agent",
            &json!({
                "name": "analyst",
                "description": "test",
                "prompt": "You are a test.",
                "auto_approve_scripts": "[\"a.sh\"]",
            }),
            None,
        );
        if let Some(AiEvent::CreateAgent {
            auto_approve_scripts,
            ..
        }) = ev_string
        {
            assert_eq!(auto_approve_scripts, vec!["a.sh"]);
        } else {
            panic!("expected AiEvent::CreateAgent from string input");
        }

        // Omitted — defaults to empty
        let ev_omit = dispatch_tool_event(
            "tc_1",
            "create_agent",
            &json!({
                "name": "analyst",
                "description": "test",
                "prompt": "You are a test.",
            }),
            None,
        );
        if let Some(AiEvent::CreateAgent {
            auto_approve_scripts,
            ..
        }) = ev_omit
        {
            assert!(auto_approve_scripts.is_empty());
        } else {
            panic!("expected AiEvent::CreateAgent from omitted input");
        }
    }

    /// Helper: collect the tool names emitted by an Anthropic-style render Value.
    fn rendered_names(v: &Value) -> Vec<String> {
        v.as_array()
            .expect("render must be an array")
            .iter()
            .map(|e| {
                e["name"]
                    .as_str()
                    .expect("name must be a string")
                    .to_string()
            })
            .collect()
    }

    /// The core/deferred partition is total and matches the Goal table: the nine
    /// deferred tools carry their group, a control core tool is None.
    #[test]
    fn deferred_group_split_is_total() {
        let expected: &[(&str, &str)] = &[
            ("create_agent", "agents"),
            ("read_agent", "agents"),
            ("list_agents", "agents"),
            ("delete_agent", "agents"),
            ("read_script", "scripts"),
            ("list_scripts", "scripts"),
            ("read_runbook", "runbooks"),
            ("list_runbooks", "runbooks"),
            ("delete_memory", "memory"),
        ];
        for (name, group) in expected {
            let t = TOOLS
                .iter()
                .find(|t| t.name == *name)
                .unwrap_or_else(|| panic!("deferred tool {name} must exist in TOOLS"));
            assert_eq!(
                t.deferred_group,
                Some(*group),
                "{name} must be deferred under group {group}"
            );
        }
        // Every other tool must be core (None).
        let deferred_names: Vec<&str> = expected.iter().map(|(n, _)| *n).collect();
        for t in TOOLS.iter() {
            if !deferred_names.contains(&t.name) {
                assert_eq!(
                    t.deferred_group, None,
                    "{} must be core (deferred_group: None)",
                    t.name
                );
            }
        }
        // Control: a representative hot tool is core.
        let rtc = TOOLS
            .iter()
            .find(|t| t.name == "run_terminal_command")
            .unwrap();
        assert_eq!(rtc.deferred_group, None);

        // Loading every deferred tool reproduces the full set (no tool is dropped
        // or duplicated by selection) — the render count/order invariants then hold.
        let all_deferred: Vec<String> = TOOLS
            .iter()
            .filter(|t| t.deferred_group.is_some())
            .map(|t| t.name.to_string())
            .collect();
        assert_eq!(select_tools(&all_deferred).len(), TOOLS.len());
    }

    /// The default render (empty loaded set) emits core only — no deferred tool,
    /// but the load_tools meta-tool is present.
    #[test]
    fn default_render_omits_deferred() {
        let names = rendered_names(&get_tool_definition(&[]));
        assert!(
            names.contains(&"run_terminal_command".to_string()),
            "core tool must be present"
        );
        assert!(
            names.contains(&"load_tools".to_string()),
            "load_tools meta-tool must be present"
        );
        for deferred in [
            "create_agent",
            "delete_memory",
            "read_script",
            "list_runbooks",
        ] {
            assert!(
                !names.contains(&deferred.to_string()),
                "deferred tool {deferred} must be omitted from the default render"
            );
        }
    }

    /// Loading a group's member names surfaces exactly those tools' schemas, while
    /// still excluding other deferred groups.
    #[test]
    fn load_then_render_includes_group() {
        let loaded: Vec<String> = vec!["read_runbook".to_string(), "list_runbooks".to_string()];
        let names = rendered_names(&get_tool_definition(&loaded));
        assert!(names.contains(&"read_runbook".to_string()));
        assert!(names.contains(&"list_runbooks".to_string()));
        // Other deferred groups stay hidden.
        assert!(!names.contains(&"create_agent".to_string()));
        assert!(!names.contains(&"delete_memory".to_string()));
    }

    /// The rendered load_tools description carries the generated catalog naming all
    /// four groups and at least one member of each.
    #[test]
    fn load_tools_catalog_lists_all_groups() {
        let rendered = get_tool_definition(&[]);
        let arr = rendered.as_array().unwrap();
        let load_tools = arr.iter().find(|e| e["name"] == "load_tools").unwrap();
        let desc = load_tools["description"].as_str().unwrap();
        for group in ["agents", "scripts", "runbooks", "memory"] {
            assert!(
                desc.contains(group),
                "load_tools catalog must name group {group}"
            );
        }
        for member in [
            "create_agent",
            "read_script",
            "read_runbook",
            "delete_memory",
        ] {
            assert!(
                desc.contains(member),
                "load_tools catalog must list member {member}"
            );
        }
    }

    /// load_tools dispatch accepts groups as a real array and as a JSON-encoded
    /// string; both yield the same AiEvent::LoadTools groups vector.
    #[test]
    fn load_tools_accepts_array_and_string_groups() {
        let ev_array =
            dispatch_tool_event("tc_1", "load_tools", &json!({"groups": ["agents"]}), None);
        if let Some(AiEvent::LoadTools { groups, .. }) = ev_array {
            assert_eq!(groups, vec!["agents".to_string()]);
        } else {
            panic!("expected AiEvent::LoadTools from array input");
        }

        let ev_string = dispatch_tool_event(
            "tc_1",
            "load_tools",
            &json!({"groups": "[\"agents\"]"}),
            None,
        );
        if let Some(AiEvent::LoadTools { groups, .. }) = ev_string {
            assert_eq!(groups, vec!["agents".to_string()]);
        } else {
            panic!("expected AiEvent::LoadTools from string input");
        }
    }

    /// The group→names resolver returns the four agent tools for "agents" and an
    /// empty vec for an unknown group.
    #[test]
    fn tools_in_group_resolves_members() {
        let agents = tools_in_group("agents");
        assert_eq!(
            agents,
            vec!["create_agent", "read_agent", "list_agents", "delete_agent"]
        );
        assert!(tools_in_group("nonexistent").is_empty());
    }

    /// End-to-end seam the conversation loop relies on: the names the executor
    /// persists (`tools_in_group(group)`) are exactly what surfaces when the
    /// loop reads `loaded_tools` back and renders. Guards against the loop
    /// dropping the loaded set on the floor (regression: `Vec::new()` passed to
    /// `chat` instead of the session's `loaded_tools`).
    #[test]
    fn loaded_group_names_render_their_schemas() {
        let loaded: Vec<String> = tools_in_group("agents")
            .iter()
            .map(|s| s.to_string())
            .collect();
        let names = rendered_names(&get_tool_definition(&loaded));
        for member in ["create_agent", "read_agent", "list_agents", "delete_agent"] {
            assert!(
                names.contains(&member.to_string()),
                "loaded agent tool {member} must render its schema"
            );
        }
        // A group that was not loaded stays hidden.
        assert!(!names.contains(&"delete_memory".to_string()));
    }
}
