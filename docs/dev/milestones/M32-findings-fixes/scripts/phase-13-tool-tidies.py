import re

def edit(path, pairs):
    s = open(path).read()
    for old, new, n in pairs:
        assert s.count(old) == n, (path, old[:60], s.count(old))
        s = s.replace(old, new)
    open(path, 'w').write(s)

# ---- #70: one regex builder for the shell tools' user patterns ----
K = 'src/daemon/executor/knowledge/shell.rs'
anchor = '#[path = "shell_read.rs"]\nmod readers;\n'
edit(K, [(anchor, '''/// Compile a pattern a shell tool was given. The 1 MiB compiled-size limit
/// is the one policy every pattern-taking shell tool shares; each caller
/// words its own refusal, naming its parameter, on an `Err`.
fn shell_tool_regex(pattern: &str) -> Result<regex::Regex, regex::Error> {
    regex::RegexBuilder::new(pattern).size_limit(1 << 20).build()
}

''' + anchor, 1)])
BUILD = re.compile(r'regex::RegexBuilder::new\((\w+)\)\s*\.size_limit\(1 << 20\)\s*\.build\(\)')
n_total = 0
for path, n in [('src/daemon/executor/knowledge/shell_read.rs', 2),
                ('src/daemon/executor/knowledge/shell_screen.rs', 2),
                ('src/daemon/executor/knowledge/shell_watch.rs', 2)]:
    s = open(path).read()
    s, k = BUILD.subn(lambda m: f'shell_tool_regex({m.group(1)})', s)
    assert k == n, (path, k)
    n_total += k
    open(path, 'w').write(s)
# watch_shell maps over an Option; the closure form reads `pattern.map(|p| shell_tool_regex(p))`.
W = 'src/daemon/executor/knowledge/shell_watch.rs'
edit(W, [('pattern.map(|p| shell_tool_regex(p))', 'pattern.map(shell_tool_regex)', 1)])
print(f'one shell_tool_regex for {n_total} pattern sites')

# ---- #72: send_input and close_shell use the shared unknown-shell refusal ----
DRIFT = re.compile(
    r'    let Some\(owner_key\) = owner_in_registry else \{\n'
    r'        let live = live_shell_ids\(reg\);\n'
    r'        return if live\.is_empty\(\) \{\n'
    r'.*?'
    r'        \};\n'
    r'    \};\n', re.S)
for path in ['src/daemon/executor/knowledge/shell_input.rs',
             'src/daemon/executor/knowledge/shell_life.rs']:
    s = open(path).read()
    s, k = DRIFT.subn('    let Some(owner_key) = owner_in_registry else {\n'
                      '        return unknown_shell_error(reg, shell);\n'
                      '    };\n', s)
    assert k == 1, (path, k)
    open(path, 'w').write(s)
# watch_shell's one-line alias of the same helper goes too.
s = open(W).read()
alias = re.compile(r'/// The `read_shell`-sentence shape `watch_shell` returns.*?\nfn watch_shell_unknown_error\(reg: &ShellRegistry, shell: &str\) -> String \{\n    unknown_shell_error\(reg, shell\)\n\}\n\n', re.S)
s, k = alias.subn('', s)
assert k == 1, k
assert s.count('watch_shell_unknown_error(') >= 1
s = s.replace('watch_shell_unknown_error(', 'unknown_shell_error(')
open(W, 'w').write(s)
print('one unknown_shell_error for send_input, close_shell and watch_shell')

# ---- #42 #43: window_text is linear, and its alias goes ----
T = 'src/cli/commands/top_tui_draw.rs'
edit(T, [
    ('''    let total = body.lines().count();
    visible_range(total, inner_h, top)
        .map(|i| body.lines().nth(i).unwrap_or_default().to_string())
        .collect::<Vec<_>>()
        .join("\\n")
''', '''    let range = visible_range(body.lines().count(), inner_h, top);
    body.lines()
        .skip(range.start)
        .take(range.len())
        .collect::<Vec<_>>()
        .join("\\n")
''', 1),
    ('''/// The Activity body's visible window while not tailing, from the same
/// range helper.
fn activity_window_text(body: &str, top: usize, inner_h: usize) -> String {
    window_text(body, top, inner_h)
}

''', '', 1),
    ('activity_window_text(', 'window_text(', 1),
])
print('window_text linear; activity_window_text gone')

# ---- #64: run_started_at calls started_at_of ----
edit('src/cli/commands/top_detail.rs', [
    ('fn started_at_of(records: &[crate::daemon::run::TurnLogRecord])',
     'pub(super) fn started_at_of(records: &[crate::daemon::run::TurnLogRecord])', 1)])
edit('src/cli/commands/top_load.rs', [('''    let records = read_turn_log(id);
    let ts = records.iter().find_map(|r| match &r.event {
        crate::daemon::run::TurnLogEvent::RunStarted { .. } => Some(r.timestamp.clone()),
        _ => None,
    })?;
    DateTime::parse_from_rfc3339(&ts)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
''', '''    super::started_at_of(&read_turn_log(id))
''', 1)])
print('run_started_at calls started_at_of')

# ---- #76: one run-id prologue for the run-log tools ----
R = 'src/daemon/executor/dispatch_runs.rs'
s = open(R).read()
assert s.count('crate::daemon::run::RunId::parse(run_id)') == 3
s = '''//! `execute_tool_call`'s arms for the run-log tools: `RunLogTail`, `RunLogSearch`, `GetTurn`.
//! A child module of `src/daemon/executor/mod.rs`.

use super::*;

pub(super) fn dispatch(call: &PendingCall) -> anyhow::Result<ToolCallOutcome> {
    use crate::daemon::run::{get_turn, run_log_search, run_log_tail};
    match call {
        PendingCall::RunLogTail { run_id, limit, .. } => {
            with_run_id(run_id, |id| run_log_tail(id, *limit))
        }
        PendingCall::RunLogSearch {
            run_id,
            query,
            limit,
            ..
        } => with_run_id(run_id, |id| run_log_search(id, query, *limit)),
        PendingCall::GetTurn { run_id, turn, .. } => with_run_id(run_id, |id| get_turn(id, *turn)),

        other => anyhow::bail!("{} is not one of the run-log tools", other.tool_name()),
    }
}

/// Parse the run id a run-log tool was given and run `read` on it. An id
/// that does not parse gets the run-log tools' shared rejection instead.
fn with_run_id(
    run_id: &str,
    read: impl FnOnce(&crate::daemon::run::RunId) -> String,
) -> anyhow::Result<ToolCallOutcome> {
    let result = match crate::daemon::run::RunId::parse(run_id) {
        Err(e) => crate::daemon::run::run_log_input_rejected(&e.to_string()),
        Ok(id) => read(&id),
    };
    Ok(ToolCallOutcome::Result(result))
}
'''
open(R, 'w').write(s)
print('one with_run_id for the three run-log tools')

# ---- #81: load_tools looks each group up once ----
edit('src/daemon/executor/dispatch_meta.rs', [
    ('''            let mut loaded: Vec<String> = Vec::new();
            let mut unknown: Vec<String> = Vec::new();
            for g in groups {
                let members = crate::ai::tools::tools_in_group(g);
                if members.is_empty() {
                    unknown.push(g.clone());
                } else {
                    for m in members {
                        loaded.push(m.to_string());
                    }
                }
            }
''', '''            let mut loaded: Vec<String> = Vec::new();
            let mut unknown: Vec<String> = Vec::new();
            let mut result_parts = Vec::new();
            for g in groups {
                let members = crate::ai::tools::tools_in_group(g);
                if members.is_empty() {
                    unknown.push(g.clone());
                } else {
                    result_parts.push(format!("Loaded group(s): {} → {}", g, members.join(", ")));
                    loaded.extend(members.iter().map(|m| m.to_string()));
                }
            }
''', 1),
    ('''            let mut result_parts = Vec::new();
            for g in groups {
                let members = crate::ai::tools::tools_in_group(g);
                if !members.is_empty() {
                    result_parts.push(format!("Loaded group(s): {} → {}", g, members.join(", ")));
                }
            }
''', '', 1),
])
print('load_tools looks each group up once')

# ---- #72, pinned: both refusals are the shared wording, byte for byte ----
edit('src/daemon/executor/knowledge/shell_observer_tests.rs', [(
    '''    assert!(out.contains("list_shells"));
    assert!(!sent.load(Ordering::SeqCst));
}
''', '''    assert!(out.contains("list_shells"));
    assert!(!sent.load(Ordering::SeqCst));
    assert_eq!(
        out,
        unknown_shell_error(&reg, "s99"),
        "send_input words the refusal as every shell tool does (M31 #72)"
    );
}
''', 1)])
edit('src/daemon/executor/knowledge/shell_lifecycle_tests.rs', [(
    '''    assert!(
        out.contains("list_shells"),
        "the model is told the way to get a fresh id: {out}"
    );
}
''', '''    assert!(
        out.contains("list_shells"),
        "the model is told the way to get a fresh id: {out}"
    );
    assert_eq!(
        out,
        unknown_shell_error(&reg, "s99"),
        "close_shell words the refusal as every shell tool does (M31 #72)"
    );
}
''', 1)])
print('the two unknown-id tests pin the shared wording')
