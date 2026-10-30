import re

R = 'src/cli/render_ratatui.rs'
D = 'src/cli/render_ratatui_draw.rs'
r = open(R).read()
d = open(D).read()

# ---- #55: one status bar ----
STATUS = re.compile(
    r'let uptime = fmt_uptime\(start_time\.elapsed\(\)\);\n'
    r'\s*let status_text = format!\(\n'
    r'\s*" session:\{\} · \{\} · up \{\} ",\n'
    r'\s*short_session\((&?)session_id\),\n'
    r'\s*model,\n'
    r'\s*uptime,\n'
    r'\s*\);\n'
    r'\s*let status_block = Block::default\(\)\.borders\(Borders::NONE\)\.style\(\n'
    r'\s*Style::default\(\)\n'
    r'\s*\.fg\(Color::DarkGray\)\n'
    r'\s*\.add_modifier\(Modifier::DIM\),\n'
    r'\s*\);\n'
    r'\s*let status_para =\s*Paragraph::new\(Line::from\(Span::raw\(status_text\)\)\)\.block\(status_block\);\n'
    r'\s*frame\.render_widget\(status_para, chunks\[1\]\);'
)
def status(m):
    owned = m.group(1) == '&'
    return ('render_status_bar(frame, chunks[1], &session_id, &model, start_time);' if owned
            else 'render_status_bar(frame, chunks[1], session_id, model, start_time);')
r, nr = STATUS.subn(status, r)
d, nd = STATUS.subn(status, d)
assert (nr, nd) == (3, 3), (nr, nd)

# ---- #56: one alert panel shell for the approval and credential dialogs ----
PANEL = re.compile(
    r'let panel = Block::default\(\)\n'
    r'\s*\.borders\(Borders::ALL\)\n'
    r'\s*\.border_type\(BorderType::Rounded\)\n'
    r'\s*\.border_style\(Style::default\(\)\.fg\(red\)\)\n'
    r'\s*\.title\(Span::styled\(\n'
    r'\s*format!\(" \{title_owned\} "\),\n'
    r'\s*Style::default\(\)\.fg\(yellow\)\.add_modifier\(Modifier::BOLD\),\n'
    r'\s*\)\);'
)
d, np_ = PANEL.subn('let panel = alert_panel(&title_owned, red, yellow);', d)
assert np_ == 2, np_

anchor = 'fn fmt_uptime(elapsed: std::time::Duration) -> String {'
assert r.count(anchor) == 1
i = r.index(anchor)
pre = r[:i].rstrip('\n').split('\n')
k = len(pre)
while k > 0 and pre[k - 1].startswith('///'):
    k -= 1
at = len('\n'.join(pre[:k])) + 1
r = r[:at] + '''/// The dim status bar under the live region:
/// ` session:<short id> · <model> · up <uptime> `.
fn render_status_bar(
    frame: &mut ratatui::Frame,
    area: Rect,
    session_id: &str,
    model: &str,
    start_time: std::time::Instant,
) {
    let status_text = format!(
        " session:{} · {} · up {} ",
        short_session(session_id),
        model,
        fmt_uptime(start_time.elapsed()),
    );
    let status_block = Block::default().borders(Borders::NONE).style(
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::DIM),
    );
    frame.render_widget(
        Paragraph::new(Line::from(Span::raw(status_text))).block(status_block),
        area,
    );
}

/// The panel an approval or credential dialog draws in: rounded, a red
/// border, a bold yellow title. The picker draws a plain border on purpose:
/// it asks for a choice, not an approval.
fn alert_panel(title: &str, red: Color, yellow: Color) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(red))
        .title(Span::styled(
            format!(" {title} "),
            Style::default().fg(yellow).add_modifier(Modifier::BOLD),
        ))
}

''' + r[at:]
open(R, 'w').write(r)
open(D, 'w').write(d)
print('one render_status_bar for six status bars; one alert_panel for two dialogs')

# ---- #27: one alt-screen entry for the tail, log and attach viewers ----
V = 'src/cli/viewer.rs'
v = open(V).read()
anchor = '/// Full-screen alternate-screen viewer loop.\n'
assert v.count(anchor) == 1
v = v.replace(anchor, '''/// The terminal a full-screen viewer draws into on the alternate screen.
pub(crate) type AltTerminal =
    ratatui::Terminal<ratatui::backend::CrosstermBackend<std::io::Stdout>>;

/// Enter the alternate screen for a full-screen viewer. The guard leaves it
/// when dropped: it leaves the alternate screen, shows the cursor and
/// re-pins `renderer`'s inline viewport, on every exit path including `?`.
/// The guard is built before the terminal, so a failed terminal still
/// leaves the alternate screen.
pub(crate) fn enter_alt_screen(
    renderer: &mut crate::cli::render_ratatui::RatatuiRendererStdout,
) -> std::io::Result<(AltScreenGuard<'_>, AltTerminal)> {
    use crossterm::cursor::Show;
    use crossterm::execute;
    use crossterm::terminal::{EnterAlternateScreen, LeaveAlternateScreen};

    execute!(std::io::stdout(), EnterAlternateScreen)?;
    let guard = AltScreenGuard::new(move || {
        let _ = execute!(std::io::stdout(), LeaveAlternateScreen);
        let _ = execute!(std::io::stdout(), Show);
        renderer.reanchor();
    });
    let terminal =
        ratatui::Terminal::new(ratatui::backend::CrosstermBackend::new(std::io::stdout()))?;
    Ok((guard, terminal))
}

/// `sigwinch` as a stream of resize events, for a viewer's event loop.
pub(crate) fn resize_events(
    sigwinch: &mut tokio::signal::unix::Signal,
) -> std::pin::Pin<Box<impl futures_util::Stream<Item = ()> + '_>> {
    Box::pin(futures_util::stream::unfold(sigwinch, |sig| async move {
        sig.recv().await.map(|()| ((), sig))
    }))
}

''' + anchor, 1)
open(V, 'w').write(v)

ENTER = re.compile(
    r'execute!\(std::io::stdout\(\), EnterAlternateScreen\)\?;\n'
    r'(\s*)let _guard = crate::cli::viewer::AltScreenGuard::new\(\|\| \{\n'
    r'\s*let _ = execute!\(std::io::stdout\(\), LeaveAlternateScreen\);\n'
    r'\s*let _ = execute!\(std::io::stdout\(\), Show\);\n'
    r'\s*renderer\.reanchor\(\);\n'
    r'\s*\}\);\n'
    r'\s*let mut terminal =\n'
    r'\s*ratatui::Terminal::new\(ratatui::backend::CrosstermBackend::new\(std::io::stdout\(\)\)\)\?;'
)
RESIZE = re.compile(
    r'let mut resizes = Box::pin\(futures_util::stream::unfold\(sigwinch, \|sig\| async move \{\n'
    r'\s*sig\.recv\(\)\.await\.map\(\|\(\)\| \(\(\), sig\)\)\n'
    r'\s*\}\)\);'
)
IMPORTS = ('    use crossterm::cursor::Show;\n'
           '    use crossterm::execute;\n'
           '    use crossterm::terminal::{EnterAlternateScreen, LeaveAlternateScreen};\n')
for path, n in [('src/cli/tail_view.rs', 2), ('src/cli/attach.rs', 1)]:
    s = open(path).read()
    s, ne = ENTER.subn(
        lambda m: 'let (_guard, mut terminal) = crate::cli::viewer::enter_alt_screen(renderer)?;', s)
    s, nz = RESIZE.subn('let mut resizes = crate::cli::viewer::resize_events(sigwinch);', s)
    assert (ne, nz) == (n, n), (path, ne, nz)
    assert s.count(IMPORTS) == n, (path, s.count(IMPORTS))
    s = s.replace(IMPORTS, '')
    open(path, 'w').write(s)
print('one enter_alt_screen and resize_events for the tail, log and attach viewers')

# ---- #30: pin the log arm above the note-carrying tail arm ----
T = 'src/cli/tail.rs'
t = open(T).read()
anchor = '    #[tokio::test]\n    async fn tail_loop_steps_a_log_with_n() {\n'
assert t.count(anchor) == 1
t = t.replace(anchor, '''    /// `render_tail`'s status match takes the log arm before the
    /// note-carrying tail arm. Stepping to a command sets a note, and the
    /// status line must still be the log's (M31 #30).
    #[test]
    fn render_tail_keeps_the_log_status_line_while_a_note_is_set() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let (events, commands) = session(3, 1);
        let mut m = TailModel::from_log("s3-17", 5, 20, &events, &commands, false);
        m.act(TailAction::NextCommand, 10);
        assert!(m.note.is_some(), "stepping to a command sets a note");
        let mut terminal = Terminal::new(TestBackend::new(140, 8)).unwrap();
        terminal.draw(|f| render_tail(f, &m)).unwrap();
        let bottom: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .skip(7 * 140)
            .take(140)
            .flat_map(|c| c.symbol().chars())
            .collect();
        assert!(bottom.starts_with("log s3-17"), "{bottom:?}");
        assert!(bottom.contains("n/p commands"), "{bottom:?}");
    }

''' + anchor, 1)
open(T, 'w').write(t)
print('render_tail_keeps_the_log_status_line_while_a_note_is_set added')
