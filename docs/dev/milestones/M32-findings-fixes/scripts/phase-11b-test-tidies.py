import re

T = 'src/cli/render_ratatui_tests.rs'
L = 'src/cli/render_ratatui_layout_tests.rs'
t = open(T).read()
l = open(L).read()

# ---- #54: one parameterised renderer builder ----
BUILD = re.compile(
    r'let backend = TestBackend::new\((\d+), (\d+)\);\n'
    r'\s*let terminal = Terminal::with_options\(\n'
    r'\s*backend,\n'
    r'\s*ratatui::TerminalOptions \{\n'
    r'\s*viewport: ratatui::Viewport::Inline\((\w+)\),\n'
    r'\s*\},\n'
    r'\s*\)\n'
    r'\s*\.unwrap\(\);\n'
    r'\s*let mut renderer = RatatuiRenderer \{\n'
    r'\s*terminal,\n'
    r'\s*start_time: std::time::Instant::now\(\),\n'
    r'\s*palette: (?:crate::cli::palette::)?Palette::for_depth\(\s*(?:crate::cli::palette::)?ColorDepth::(\w+),?\s*\),\n'
    r'\s*origin_row: 0,\n'
    r'\s*inserted_rows: 0,\n'
    r'\s*rows_scrolled: 0,\n'
    r'\s*history: std::collections::VecDeque::new\(\),\n'
    r'\s*last_width: 0,\n'
    r'\s*last_height: 0,\n'
    r'\s*\};'
)

def builder(m):
    cols, rows, viewport, depth = m.groups()
    return (f'let mut renderer = test_renderer({cols}, {rows}, {viewport}, '
            f'crate::cli::palette::ColorDepth::{depth});')

t, nt = BUILD.subn(builder, t)
l, nl = BUILD.subn(builder, l)
assert (nt, nl) == (5, 12), (nt, nl)

old_make = t[t.index('pub(super) fn make_test_renderer() -> RatatuiRenderer<TestBackend> {'):]
old_make = old_make[:old_make.index('\n}\n') + 3]
assert 'RatatuiRenderer {' in old_make
t = t.replace(old_make, '''pub(super) fn make_test_renderer() -> RatatuiRenderer<TestBackend> {
    test_renderer(60, 10, VIEWPORT_ROWS, crate::cli::palette::ColorDepth::Truecolor)
}

/// A renderer over a `cols`×`rows` test backend with an inline viewport of
/// `viewport` rows, drawing in `depth` colours. Every other field starts as
/// a fresh renderer's does.
pub(super) fn test_renderer(
    cols: u16,
    rows: u16,
    viewport: u16,
    depth: crate::cli::palette::ColorDepth,
) -> RatatuiRenderer<TestBackend> {
    let terminal = Terminal::with_options(
        TestBackend::new(cols, rows),
        ratatui::TerminalOptions {
            viewport: ratatui::Viewport::Inline(viewport),
        },
    )
    .unwrap();
    RatatuiRenderer {
        terminal,
        start_time: std::time::Instant::now(),
        palette: crate::cli::palette::Palette::for_depth(depth),
        origin_row: 0,
        inserted_rows: 0,
        rows_scrolled: 0,
        history: std::collections::VecDeque::new(),
        last_width: 0,
        last_height: 0,
    }
}
''', 1)

# ---- #53: the two status fixtures, shared ----
def grab_fn(src, sig):
    i = src.index(sig)
    j = src.index('\n}\n', i) + 3
    d = i
    lines = src[:i].split('\n')
    k = len(lines) - 1
    while k > 0 and lines[k - 1].startswith('///'):
        k -= 1
    d = len('\n'.join(lines[:k])) + (1 if k else 0)
    return src[d:j]

ds = grab_fn(l, 'fn default_status() -> StatusBarState<\'static> {')
ap = grab_fn(l, 'fn approval_test_status() -> StatusBarState<\'static> {')
l = l.replace(ds, '', 1).replace(ap, '', 1)
t = t.rstrip('\n') + '\n\n' + (ds.replace('fn default_status', 'pub(super) fn default_status')
                               + '\n' + ap.replace('fn approval_test_status', 'pub(super) fn approval_test_status')).rstrip('\n') + '\n'

LIT = re.compile(r'let status = StatusBarState \{\n(.*?)\n\s*\};', re.S)
def fields(body):
    return ' '.join(body.split())
DEFAULT = ('session_id: "abcdef12-3456", approval_hint: "cmds: auto", model: "test-model", '
           'prompt_tokens: 0, context_window: 200_000, daemon_up: true, tools_total: 0, '
           'cost_usd: 0.0, has_untracked: false,')
APPROVAL = ('session_id: "test-session", approval_hint: "", model: "test-model", '
            'prompt_tokens: 0, context_window: 200_000, daemon_up: false, tools_total: 0, '
            'cost_usd: 0.0, has_untracked: false,')
counts = {'default': 0, 'approval': 0, 'kept': 0}
def status(m):
    f = fields(m.group(1))
    if f == DEFAULT:
        counts['default'] += 1
        return 'let status = default_status();'
    if f == APPROVAL:
        counts['approval'] += 1
        return 'let status = approval_test_status();'
    counts['kept'] += 1
    return m.group(0)
t = LIT.sub(status, t)
l = LIT.sub(status, l)
assert counts == {'default': 10, 'approval': 5, 'kept': 1}, counts

# The two Xterm256 tests imported the palette types for the inline build.
local = '    use crate::cli::palette::{ColorDepth, Palette};\n'
assert t.count(local) == 2, t.count(local)
t = t.replace(local, '')
head = 'use super::tests::make_test_renderer;\n'
assert l.count(head) == 1
l = l.replace(head, 'use super::tests::{approval_test_status, default_status, make_test_renderer, test_renderer};\n', 1)
open(T, 'w').write(t)
open(L, 'w').write(l)
print('one test_renderer builder; 17 inline renderers and 15 status literals replaced')

# ---- #51: the two bare asserts that are not self-describing ----
I = 'src/memory/index/incremental_tests.rs'
s = open(I).read()
for old, new in [
    ('    assert_eq!(mem_before, 1);\n',
     '    assert_eq!(mem_before, 1, "the seeded memory is indexed before the reconcile probe");\n'),
    ('    assert_eq!(filtered[0].1, "zephyr-incident");\n',
     '    assert_eq!(\n        filtered[0].1, "zephyr-incident",\n        "the one hit is the incident memory"\n    );\n'),
]:
    assert s.count(old) == 1, old
    s = s.replace(old, new, 1)
open(I, 'w').write(s)
print('two assert_eq! messages added')
