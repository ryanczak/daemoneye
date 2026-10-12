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
        "get_shell_context" => dispatch::<GetShellContextArgs>(id, args, ts),
        "list_shells" => dispatch::<ListShellsArgs>(id, args, ts),
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
        "read_file" => dispatch::<ReadFileArgs>(id, args, ts),
        "read_shell" => dispatch::<ReadShellArgs>(id, args, ts),
        "find_in_shells" => dispatch::<FindInShellsArgs>(id, args, ts),
        "open_shell" => dispatch::<OpenShellArgs>(id, args, ts),
        "close_shell" => dispatch::<CloseShellArgs>(id, args, ts),
        "watch_shell" => dispatch::<WatchShellArgs>(id, args, ts),
        "send_input" => dispatch::<SendInputArgs>(id, args, ts),
        "list_shell_logs" => dispatch::<ListShellLogsArgs>(id, args, ts),
        "read_shell_log" => dispatch::<ReadShellLogArgs>(id, args, ts),
        "edit_file" => dispatch::<EditFileArgs>(id, args, ts),
        "write_runbook" => dispatch::<WriteRunbookArgs>(id, args, ts),
        "delete_runbook" => runbook_name_event(args, ts, |nm, t| AiEvent::DeleteRunbook {
            id: id.to_string(),
            name: nm,
            thought_signature: t,
        }),
        "read_runbook" => dispatch::<ReadRunbookArgs>(id, args, ts),
        "run_log_tail" => dispatch::<RunLogTailArgs>(id, args, ts),
        "run_log_search" => dispatch::<RunLogSearchArgs>(id, args, ts),
        "get_turn" => dispatch::<GetTurnArgs>(id, args, ts),
        "add_memory" => dispatch::<AddMemoryArgs>(id, args, ts),
        "update_memory" => dispatch::<UpdateMemoryArgs>(id, args, ts),
        "delete_memory" => dispatch::<DeleteMemoryArgs>(id, args, ts),
        "read_memory" => dispatch::<ReadMemoryArgs>(id, args, ts),
        "list_memories" => dispatch::<ListMemoriesArgs>(id, args, ts),
        "search_repository" => dispatch::<SearchRepositoryArgs>(id, args, ts),
        "recall_context" => dispatch::<RecallContextArgs>(id, args, ts),
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
    use super::super::defs::{APPROVAL_GATED_TOOLS, TOOLS};
    use super::super::schema::{
        Category, ParamTy, ToolDef, category_of, enum_values, get_tool_definition,
        render_anthropic, render_gemini, select_tools, tools_in_group,
    };
    use super::*;
    use serde_json::json;
    use std::collections::BTreeMap;

    #[test]
    fn tool_table_counts_are_42_28_14() {
        assert_eq!(TOOLS.len(), 42, "total tools");
        assert_eq!(
            TOOLS.iter().filter(|t| t.deferred_group.is_none()).count(),
            28,
            "core tools"
        );
        assert_eq!(
            TOOLS.iter().filter(|t| t.deferred_group.is_some()).count(),
            14,
            "deferred tools"
        );
    }

    #[test]
    fn e2e_removed_pane_read_tools_are_absent_from_every_render() {
        let names: Vec<&str> = TOOLS.iter().map(|t| t.name).collect();
        for gone in ["list_panes", "read_pane", "find_in_panes"] {
            assert!(!names.contains(&gone), "{gone} must be gone from TOOLS");
        }
        for kept in ["list_shells", "read_shell", "find_in_shells"] {
            assert!(
                names.contains(&kept),
                "{kept} is the replacement and must stay"
            );
        }
        let rendered = render_gemini(&TOOLS.iter().collect::<Vec<_>>());
        let blob = serde_json::to_string(&rendered).expect("render must serialize");
        for gone in ["list_panes", "read_pane", "find_in_panes"] {
            assert!(
                !blob.contains(gone),
                "{gone} must not survive in the render: {blob:.400}"
            );
        }
        println!(
            "PASTE MATCH >>>\ntools={} core={}\n<<<",
            TOOLS.len(),
            TOOLS.iter().filter(|t| t.deferred_group.is_none()).count()
        );
    }

    #[test]
    fn e2e_no_pane_identifiers_in_tool_descriptions() {
        // The milestone's exit criterion: no %-pane identifier anywhere in the
        // tool surface the model is shown.
        let mut offenders: Vec<String> = Vec::new();
        for t in TOOLS {
            if t.description.contains('%') {
                offenders.push(format!("{} (description)", t.name));
            }
            for p in t.params {
                if p.description.contains('%') {
                    offenders.push(format!("{}.{}", t.name, p.name));
                }
            }
        }
        assert!(
            offenders.is_empty(),
            "%-pane identifiers remain: {offenders:?}"
        );

        let run = TOOLS
            .iter()
            .find(|t| t.name == "run_terminal_command")
            .expect("run_terminal_command must exist");
        let names: Vec<&str> = run.params.iter().map(|p| p.name).collect();
        assert!(!names.contains(&"target_pane"), "retired: {names:?}");
        assert!(!names.contains(&"retry_in_pane"), "retired: {names:?}");
        for kept in ["command", "wait", "shell", "timeout_secs"] {
            assert!(names.contains(&kept), "{kept} must stay: {names:?}");
        }
        // Row 07b retired edit_file's pane parameter too — host? replaced it,
        // so target_pane is gone from the whole tool surface.
        let edit = TOOLS
            .iter()
            .find(|t| t.name == "edit_file")
            .expect("must exist");
        assert!(
            !edit.params.iter().any(|p| p.name == "target_pane"),
            "edit_file's target_pane is retired by M21 row 07b"
        );
        assert!(
            edit.params.iter().any(|p| p.name == "host"),
            "edit_file carries host in its place"
        );
        println!(
            "PASTE MATCH >>>\npane_ids={} run_params={}\n<<<",
            offenders.len(),
            run.params.len()
        );
    }

    #[test]
    fn e2e_removed_pane_watch_tools_are_absent_from_every_render() {
        let names: Vec<&str> = TOOLS.iter().map(|t| t.name).collect();
        for gone in ["watch_pane", "close_background_window"] {
            assert!(!names.contains(&gone), "{gone} must be gone from TOOLS");
        }
        for kept in ["watch_shell", "close_shell"] {
            assert!(names.contains(&kept), "{kept} must stay");
        }
        let rendered = render_gemini(&TOOLS.iter().collect::<Vec<_>>());
        let blob = serde_json::to_string(&rendered).expect("render must serialize");
        for gone in ["watch_pane", "close_background_window"] {
            assert!(
                !blob.contains(gone),
                "{gone} must not survive the render: {blob:.400}"
            );
        }
        println!(
            "PASTE MATCH >>>\ntools={} core={}\n<<<",
            TOOLS.len(),
            TOOLS.iter().filter(|t| t.deferred_group.is_none()).count()
        );
    }

    #[test]
    fn e2e_tmux_control_is_absent_and_the_surface_is_final() {
        let names: Vec<&str> = TOOLS.iter().map(|t| t.name).collect();
        assert!(
            !names.contains(&"tmux_control"),
            "tmux_control must be gone"
        );
        assert!(
            !APPROVAL_GATED_TOOLS.contains(&"tmux_control"),
            "and gone from the approval list"
        );
        for kept in ["run_terminal_command", "send_input", "get_shell_context"] {
            assert!(names.contains(&kept), "{kept} must stay");
        }
        let rendered = render_gemini(&TOOLS.iter().collect::<Vec<_>>());
        let blob = serde_json::to_string(&rendered).expect("render must serialize");
        assert!(
            !blob.contains("tmux_control"),
            "tmux_control must not survive the render: {blob:.400}"
        );
        println!(
            "PASTE MATCH >>>\ntools={} core={} gated={}\n<<<",
            TOOLS.len(),
            TOOLS.iter().filter(|t| t.deferred_group.is_none()).count(),
            APPROVAL_GATED_TOOLS.len()
        );
    }

    /// Phase-08b's end-to-end capture: the renamed tool's surface, written to
    /// stdout for the Update Log and asserted so the printer cannot drift.
    #[test]
    fn e2e_shell_context_tool_surface() {
        let names: Vec<&str> = TOOLS.iter().map(|t| t.name).collect();
        let def = TOOLS
            .iter()
            .find(|t| t.name == "get_shell_context")
            .expect("get_shell_context must exist in TOOLS");
        let scope_desc = def
            .params
            .iter()
            .find(|p| p.name == "scope")
            .map(|p| p.description)
            .unwrap_or_default();
        let scopes = ["shells", "session", "all"]
            .iter()
            .filter(|v| scope_desc.contains(&format!("\"{v}\"")))
            .copied()
            .collect::<Vec<_>>()
            .join("|");
        let retired_absent = !names.contains(&"get_terminal_context");
        assert_eq!(
            scopes, "shells|session|all",
            "scope description: {scope_desc}"
        );
        assert!(
            retired_absent,
            "get_terminal_context must be absent from TOOLS"
        );
        assert_eq!(TOOLS.len(), 42, "the rename must not change the tool count");
        let core = TOOLS.iter().filter(|t| t.deferred_group.is_none()).count();
        assert_eq!(core, 28, "the rename must not change the core count");
        let rendered = render_gemini(&TOOLS.iter().collect::<Vec<_>>());
        let blob = serde_json::to_string(&rendered).expect("render must serialize");
        assert!(
            !blob.contains("get_terminal_context"),
            "the retired name must be absent from the Gemini render: {blob:.400}"
        );
        assert!(
            blob.contains("get_shell_context"),
            "the new name must be present in the Gemini render: {blob:.400}"
        );
        println!(
            "PASTE MATCH >>>\ntool=get_shell_context scopes={scopes} retired_absent={retired_absent} tools={} core={core}\n<<<",
            TOOLS.len(),
        );
    }

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
        assert_eq!(props["wait"]["type"], "BOOLEAN");
        assert_eq!(props["shell"]["type"], "STRING");
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
                "read_file" => json!({"path": "/tmp/f"}),
                "edit_file" => json!({"path": "/tmp/f"}),
                "write_runbook" => json!({"name": "rb", "content": "# RB"}),
                "delete_runbook" => json!({"name": "rb"}),
                "read_runbook" => json!({"name": "rb"}),
                "list_runbooks" => json!({}),
                "run_log_tail" => json!({"run_id": "a1b2c3d4e5f60718", "limit": 5}),
                "run_log_search" => json!({"run_id": "a1b2c3d4e5f60718", "query": "q"}),
                "get_turn" => json!({"run_id": "a1b2c3d4e5f60718", "turn": 1}),
                "add_memory" => json!({"key": "k", "value": "v", "category": "knowledge"}),
                "update_memory" => json!({"key": "k", "category": "knowledge"}),
                "delete_memory" => json!({"key": "k", "category": "knowledge"}),
                "read_memory" => json!({"key": "k", "category": "knowledge"}),
                "list_memories" => json!({}),
                "search_repository" => json!({"query": "x", "kind": "all"}),
                "get_shell_context" => json!({"scope": "session"}),
                "list_shells" => json!({"scope": "mine"}),
                "read_shell" => {
                    json!({"shell": "s3", "lines": 10, "since_command": 0, "filter": "."})
                }
                "find_in_shells" => json!({"pattern": "error", "scope": "all"}),
                "spawn_ghost_shell" => json!({"runbook": "rb", "message": "investigate"}),
                "create_agent" => {
                    json!({"name": "analyst", "description": "test", "prompt": "You are a test."})
                }
                "read_agent" => json!({"name": "analyst"}),
                "list_agents" => json!({}),
                "delete_agent" => json!({"name": "analyst"}),
                "await_agent_result" => json!({"job_id": "ghost-abc-123", "agent_name": "analyst"}),
                "open_shell" => json!({}),
                "close_shell" => json!({"shell": "s3"}),
                "watch_shell" => json!({"shell": "s3", "timeout_secs": 5, "pattern": "x"}),
                "send_input" => json!({"shell": "s3", "text": "y", "enter": false}),
                "list_shell_logs" => json!({"since": 1_788_470_000u64, "limit": 10}),
                "read_shell_log" => {
                    json!({"log": "s3-1788470000", "command_index": 0, "lines": 10, "filter": "."})
                }
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

    /// The core/deferred partition is total and matches the dispatch arms: the
    /// fourteen deferred tools carry their group, a control core tool is None.
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
            ("run_log_tail", "runs"),
            ("run_log_search", "runs"),
            ("get_turn", "runs"),
            ("list_shell_logs", "history"),
            ("read_shell_log", "history"),
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

    #[test]
    fn every_tool_has_a_category() {
        assert_eq!(
            TOOLS.len(),
            42,
            "tool count should match the 28 core + 14 deferred claim"
        );
        for t in TOOLS {
            assert!(
                category_of(t.name).is_some(),
                "tool {:?} not resolvable via category_of",
                t.name
            );
        }
    }

    #[test]
    fn category_counts_match_the_assignment() {
        let mut counts: BTreeMap<Category, usize> = BTreeMap::new();
        for t in TOOLS {
            *counts.entry(t.category).or_insert(0) += 1;
        }
        assert_eq!(counts.get(&Category::Read), Some(&20), "Read count");
        assert_eq!(counts.get(&Category::Write), Some(&13), "Write count");
        assert_eq!(counts.get(&Category::Search), Some(&3), "Search count");
        assert_eq!(counts.get(&Category::Run), Some(&5), "Run count");
        assert_eq!(counts.get(&Category::Meta), Some(&1), "Meta count");
        assert_eq!(counts.values().sum::<usize>(), TOOLS.len());
    }

    #[test]
    fn approval_gated_tools_all_mutate_state() {
        for name in APPROVAL_GATED_TOOLS {
            let cat = category_of(name)
                .unwrap_or_else(|| panic!("approval-gated tool {name:?} not in TOOLS"));
            assert!(
                cat.mutates_state(),
                "approval-gated tool {name:?} has category {cat:?} that does not mutate state"
            );
        }
    }

    #[test]
    fn mutates_state_is_write_and_run_only() {
        assert!(Category::Write.mutates_state(), "Write mutates state");
        assert!(Category::Run.mutates_state(), "Run mutates state");
        assert!(!Category::Read.mutates_state(), "Read must not mutate");
        assert!(!Category::Search.mutates_state(), "Search must not mutate");
        assert!(!Category::Meta.mutates_state(), "Meta must not mutate");
    }

    #[test]
    fn category_of_resolves_every_tool_name_and_nothing_else() {
        assert_eq!(
            category_of("run_terminal_command"),
            Some(Category::Run),
            "run_terminal_command should be Run"
        );
        assert_eq!(
            category_of("no_such_tool"),
            None,
            "unknown tool name must not resolve to a category"
        );
    }

    /// `run_terminal_command`'s args land in the named fields of the struct
    /// variant, each value in the field it was given. A positional tuple
    /// transposed two adjacent `Option<String>`s silently; the named fields
    /// are what keeps row 04d's `host?` from landing in `shell`'s slot.
    #[test]
    fn run_terminal_command_args_map_to_named_event_fields() {
        let ev = dispatch_tool_event(
            "tc_1",
            "run_terminal_command",
            &json!({
                "command": "echo named-fields",
                "wait": false,
                "shell": "s9",
                "timeout_secs": 33
            }),
            Some("sig-1".to_string()),
        )
        .expect("run_terminal_command must dispatch");
        let AiEvent::ToolCall {
            id,
            cmd,
            wait,
            thought_signature,
            shell,
            host,
            timeout_secs,
        } = ev
        else {
            panic!("expected AiEvent::ToolCall, got {ev:?}")
        };
        assert_eq!(id, "tc_1", "the call id lands in `id`");
        assert_eq!(cmd, "echo named-fields", "the command lands in `cmd`");
        assert_eq!(wait, Some(false), "the wait value lands in `wait`");
        assert_eq!(
            thought_signature.as_deref(),
            Some("sig-1"),
            "the thought signature lands in `thought_signature`"
        );
        assert_eq!(shell.as_deref(), Some("s9"), "the shell lands in `shell`");
        assert_eq!(host, None, "the unnamed host lands as `None` in `host`");
        assert_eq!(
            timeout_secs,
            Some(33),
            "the timeout lands in `timeout_secs`"
        );
    }

    /// GLM's legacy `background` habit: the schema never names it, so the
    /// args struct must still keep it and invert it into `wait`. An explicit
    /// `wait` wins when both arrive.
    #[test]
    fn run_terminal_command_maps_legacy_background_to_wait() {
        let ev = dispatch_tool_event(
            "tc_1",
            "run_terminal_command",
            &json!({"command": "echo bg", "background": true}),
            None,
        )
        .expect("background: true must still dispatch");
        let AiEvent::ToolCall { wait, .. } = ev else {
            panic!("expected AiEvent::ToolCall, got {ev:?}")
        };
        assert_eq!(wait, Some(false), "background: true means don't wait");

        let ev = dispatch_tool_event(
            "tc_1",
            "run_terminal_command",
            &json!({"command": "echo bg", "background": false}),
            None,
        )
        .expect("background: false must still dispatch");
        let AiEvent::ToolCall { wait, .. } = ev else {
            panic!("expected AiEvent::ToolCall, got {ev:?}")
        };
        assert_eq!(wait, Some(true), "background: false means wait");

        let ev = dispatch_tool_event(
            "tc_1",
            "run_terminal_command",
            &json!({"command": "echo bg", "wait": true, "background": true}),
            None,
        )
        .expect("the wait/background pair must still dispatch");
        let AiEvent::ToolCall { wait, .. } = ev else {
            panic!("expected AiEvent::ToolCall, got {ev:?}")
        };
        assert_eq!(
            wait,
            Some(true),
            "an explicit wait wins over the legacy background"
        );

        let def = TOOLS
            .iter()
            .find(|t| t.name == "run_terminal_command")
            .unwrap_or_else(|| panic!("run_terminal_command is in TOOLS"));
        assert!(
            !def.params.iter().any(|p| p.name == "background"),
            "background must stay out of the tool schema — it is a legacy alias, \
             not a surface the model is told about"
        );
    }

    /// The root-edit flag on the tool surface: the schema carries it, and a
    /// dispatch carries it through to the event — present as `Some(true)`,
    /// absent as `None`, never a silent default.
    #[test]
    fn edit_file_takes_a_root_flag() {
        let def = TOOLS
            .iter()
            .find(|t| t.name == "edit_file")
            .unwrap_or_else(|| panic!("edit_file is in TOOLS"));
        let param = def
            .params
            .iter()
            .find(|p| p.name == "sudo")
            .unwrap_or_else(|| panic!("edit_file carries a root-edit param"));
        assert!(matches!(param.ty, ParamTy::Bool), "the param is a bool");
        assert!(!param.required, "the param is optional");

        let ev = dispatch_tool_event(
            "tc_1",
            "edit_file",
            &json!({"path": "/tmp/f", "host": "pinky", "sudo": true}),
            None,
        )
        .expect("the args dispatch");
        match ev {
            AiEvent::EditFile { sudo, host, .. } => {
                assert_eq!(sudo, Some(true), "the flag lands in the event");
                assert_eq!(host.as_deref(), Some("pinky"), "the host lands too");
            }
            other => panic!("expected AiEvent::EditFile, got {other:?}"),
        }

        let ev = dispatch_tool_event("tc_1", "edit_file", &json!({"path": "/tmp/f"}), None)
            .expect("the args dispatch");
        match ev {
            AiEvent::EditFile { sudo, .. } => {
                assert_eq!(sudo, None, "absent is None, never a silent default");
            }
            other => panic!("expected AiEvent::EditFile, got {other:?}"),
        }
    }
}
