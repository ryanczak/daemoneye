import re

P = 'src/cli/commands/stream_prompts.rs'
A = 'src/cli/commands/stream_ask.rs'
p = open(P).read()
a = open(A).read()

# ---- #66: one approval-input state machine, the redraw a parameter ----
def span(src, start_sig, end_marker):
    i = src.index(start_sig)
    j = src.index(end_marker, i)
    return i, j

i, _ = span(p, 'async fn read_approval_input(\n', '\n}\n')
# back up over the doc comment above `read_approval_input`
head = p[:i].rstrip('\n').split('\n')
k = len(head)
while k > 0 and head[k - 1].startswith('///'):
    k -= 1
doc_start = len('\n'.join(head[:k])) + 1
end_sig = 'async fn prompt_with_session_approve('
j = p.index(end_sig)
jdoc = p[:j].rstrip('\n').split('\n')
m = len(jdoc)
while m > 0 and jdoc[m - 1].startswith('///'):
    m -= 1
next_doc = len('\n'.join(jdoc[:m])) + 1
old_block = p[doc_start:next_doc]
assert old_block.count('async fn read_approval_input(') == 1
assert old_block.count('async fn read_approval_input_panel(') == 1
old_doc = '\n'.join(head[k:])
new_block = old_doc + '''
async fn read_approval_input(
    renderer: &mut crate::cli::render_ratatui::RatatuiRendererStdout,
    stdin: &AsyncStdin,
    prompt_text: &str,
    status: &crate::cli::render::StatusBarState<'_>,
    deadline: tokio::time::Instant,
) -> Option<String> {
    read_approval_input_with(renderer, stdin, deadline, |r, line| {
        let _ = r.draw_prompt(prompt_text, line, status);
    })
    .await
}

/// `read_approval_input`, panel edition: identical key semantics, but every
/// redraw renders the themed approval panel instead of the plain prompt.
async fn read_approval_input_panel(
    renderer: &mut crate::cli::render_ratatui::RatatuiRendererStdout,
    stdin: &AsyncStdin,
    title: &str,
    session_label: &str,
    status: &crate::cli::render::StatusBarState<'_>,
    deadline: tokio::time::Instant,
) -> Option<String> {
    read_approval_input_with(renderer, stdin, deadline, |r, line| {
        let _ = r.draw_approval_panel(title, session_label, line, status);
    })
    .await
}

/// The approval input state machine both prompt shapes share. `redraw`
/// paints the current input; it runs once on entry and after every edit.
/// `y`, `n` or `a` as the first key answers at once; Enter on an empty
/// line, Ctrl+C and Escape answer with an empty string; anything else
/// starts a typed message, ended by Enter. `None` when the deadline passes.
async fn read_approval_input_with(
    renderer: &mut crate::cli::render_ratatui::RatatuiRendererStdout,
    stdin: &AsyncStdin,
    deadline: tokio::time::Instant,
    redraw: impl Fn(&mut crate::cli::render_ratatui::RatatuiRendererStdout, &crate::cli::input::InputLine),
) -> Option<String> {
    use crate::cli::input::InputLine;

    // Initial draw with empty input.
    let mut line = InputLine::new();
    redraw(renderer, &line);

    // Read the first byte to decide: single-key shortcut vs. full line edit.
    let first = read_byte_by(deadline, stdin.read_byte()).await?;
    let ch = first as char;
    match ch.to_ascii_lowercase() {
        'y' | 'n' | 'a' => {
            // Show the key the user pressed in the input box, then return.
            line.insert(ch);
            redraw(renderer, &line);
            Some(ch.to_string())
        }
        '\\r' | '\\n' => {
            // Empty input (Enter pressed immediately)
            Some(String::new())
        }
        _ => {
            // Start of a typed message — use the input editor.
            line.insert(ch);
            redraw(renderer, &line);

            loop {
                let b = read_byte_by(deadline, stdin.read_byte()).await?;
                match b {
                    b'\\r' | b'\\n' => return Some(line.as_str()),
                    b'\\x7f' | b'\\x08' => {
                        line.backspace();
                        redraw(renderer, &line);
                    }
                    b'\\x03' => {
                        // Ctrl+C — cancel, return empty
                        return Some(String::new());
                    }
                    b'\\x1b' => {
                        // Escape — cancel, return empty
                        return Some(String::new());
                    }
                    b => {
                        if b >= 0x20 {
                            line.insert(b as char);
                            redraw(renderer, &line);
                        }
                    }
                }
            }
        }
    }
}

'''
p = p[:doc_start] + new_block + p[next_doc:]

# ---- #67: the dead `_path_approved` ----
dead = '    let _path_approved = approval.file_edits.contains(path);\n'
assert p.count(dead) == 1
p = p.replace(dead, '', 1)

# ---- #68b: the shared redirect / skipped tail ----
TAIL = re.compile(
    r'    \} else if let Some\(msg\) = user_msg \{\n'
    r'        let _ = renderer\.commit\("  ↩ redirecting agent with your message…\\n"\);\n'
    r'        (Ok\()?\(false, Some\(msg\)\)\)?\n'
    r'    \} else \{\n'
    r'        let _ = renderer\.commit\("  ✗ skipped\\n"\);\n'
    r'        (Ok\()?\(false, None\)\)?\n'
    r'    \}\n'
)
def tail(m):
    ok = m.group(1) is not None
    inner = '(false, commit_declined(renderer, user_msg))'
    return '    } else {\n        ' + ('Ok(' + inner + ')' if ok else inner) + '\n    }\n'
p, n = TAIL.subn(tail, p)
assert n == 2, n
anchor = '/// Display info text and prompt Y/N/A with typed-message support.\n'
assert p.count(anchor) == 1
p = p.replace(anchor, '''/// The tail of a declined approval: a redirect line when the user typed a
/// message, a skip line otherwise. Returns the message, which goes back to
/// the daemon with the denial.
fn commit_declined(
    renderer: &mut crate::cli::render_ratatui::RatatuiRendererStdout,
    user_msg: Option<String>,
) -> Option<String> {
    match user_msg {
        Some(msg) => {
            let _ = renderer.commit("  ↩ redirecting agent with your message…\\n");
            Some(msg)
        }
        None => {
            let _ = renderer.commit("  ✗ skipped\\n");
            None
        }
    }
}

''' + anchor, 1)
open(P, 'w').write(p)
print('one read_approval_input_with; _path_approved gone; commit_declined shared')

# ---- #68a: one prompt_status for the turn's nine status literals ----
LIT = re.compile(
    r'let sb = crate::cli::render::StatusBarState \{\n'
    r'(\s*)session_id: "",\n'
    r'\s*approval_hint: &approval\.hint\(\),\n'
    r'\s*model: ctx\.model,\n'
    r'\s*prompt_tokens: \*prompt_tokens,\n'
    r'\s*context_window,\n'
    r'\s*daemon_up: false,\n'
    r'\s*tools_total: 0,\n'
    r'\s*cost_usd: \*session_cost,\n'
    r'\s*has_untracked: \*session_has_untracked,\n'
    r'\s*\};'
)
def lit(m):
    ind = m.group(1)[:-4]
    return ('let hint = approval.hint();\n' + ind +
            'let sb = prompt_status(&hint, ctx.model, *prompt_tokens, context_window, '
            '*session_cost, *session_has_untracked);')
a, n = LIT.subn(lit, a)
assert n == 9, n
anchor = 'pub(in super::super) async fn ask_with_session_ratatui('
assert a.count(anchor) == 1, 'ask_with_session_ratatui signature'
i = a.index(anchor)
pre = a[:i].rstrip('\n').split('\n')
k = len(pre)
while k > 0 and (pre[k - 1].startswith('///') or pre[k - 1].startswith('#[')):
    k -= 1
at = len('\n'.join(pre[:k])) + 1
a = a[:at] + '''/// The status bar each prompt in a turn draws over: the session's model,
/// approval hint, token count and cost. The daemon is shown as down and no
/// session id is named while a prompt holds the live region.
fn prompt_status<'a>(
    hint: &'a str,
    model: &'a str,
    prompt_tokens: u32,
    context_window: u32,
    cost_usd: f64,
    has_untracked: bool,
) -> crate::cli::render::StatusBarState<'a> {
    crate::cli::render::StatusBarState {
        session_id: "",
        approval_hint: hint,
        model,
        prompt_tokens,
        context_window,
        daemon_up: false,
        tools_total: 0,
        cost_usd,
        has_untracked,
    }
}

''' + a[at:]
open(A, 'w').write(a)
print('nine status literals use prompt_status')
