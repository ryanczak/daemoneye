# Phase 10 — One `test_home()` fixture

**Status:** in-progress
**Milestone:** M32 — Findings fixes
**Drafted:** 2026-10-30
**Findings:** M31 `FINDINGS.md` #4 (#8, #49, #50, #62 and #63 are
rejected below)

## Goal

Eleven test modules each define the same temp-`HOME` helper under four
names: `test_home`, `shell_test_home`, `lifecycle_home` and
`temp_home`. Each one returns `(crate::TestHomeGuard, tempfile::TempDir)`
after taking the HOME lock and pointing `HOME` at a fresh temp directory;
most of them also run `Config::ensure_dirs()`. After this phase,
`src/lib.rs` has one `#[cfg(test)] pub(crate) fn test_home()` that does
all three. The eleven local definitions are gone, and every call site
(154 of them) calls `crate::test_home()`.

This is a pure tidy (M32 decision 2). No test is added, removed or
changed in what it checks. The suite stays at **exactly 2607**; a
different count means something else changed.

**Rejected:** #8 (`search_tests.rs`'s `COUNTER` for unique temp-dir
names), #49, #50, #62 and #63 (single-file duplicates of a message
builder, an archive fixture, a config literal and `RecordingWriter`).
Each repeats three to six lines inside one file. A shared helper for
each would add an indirection that none of them needs.

## Architecture references

- `src/lib.rs` § `test_home_guard` and `TestHomeGuard`: the lock and
  `HOME` restore the new fixture builds on.
- M32 README, decision 2.

## Spec

1. **Run the script below as written, once.** It does the whole change:
   - adds `crate::test_home()` to `src/lib.rs`;
   - deletes the eleven helpers and their doc comments;
   - points every call at `crate::test_home()`, including the three
     files that reached a helper through `use super::*`;
   - removes the imports that are then unused;
   - runs `cargo fmt --all` twice.

   It was dry-run on a scratch worktree of `9c9905f1`: clippy clean, hygiene
   passing, and the full suite at `passed=2607 failed=0`. Do not
   hand-edit anything it touches. Do not `sed -i` a source file. It is a
   Python heredoc inside a shell script, so copy it **without adding
   indentation**. It asserts its anchors and prints
   `one crate::test_home(); eleven local helpers removed`, then
   `unused imports removed`. If an assert fires, stop and report it in
   the Update Log.
2. **Log it at once.** Append an `### Update — <date> (cuts done)` entry
   quoting the script's output, with an edit tool.
3. **Findings** go in their own `### Findings — <date>` entry. Run
   exactly `git diff --stat`, write what it shows about the change's
   shape in one or two lines, or write `none`. Open no file.
4. **Run the End-to-end block** and paste its output into an
   `### Update — <date> (end-to-end verification)` entry, as a fenced
   block of the output itself, not a description of it.
5. **Commit** as `test:`.

### The script

```bash
python3 - <<'PY'
import re
# The eleven per-file temp-HOME helpers, each `fn NAME() -> (crate::TestHomeGuard, tempfile::TempDir)`.
DEFS = [
    ('src/shell/session_tests.rs', 'test_home'),
    ('src/shell/launch.rs', 'test_home'),
    ('src/daemon/executor/shell_exec_tests.rs', 'test_home'),
    ('src/daemon/server/shells_tests.rs', 'shell_test_home'),
    ('src/daemon/server/sessions.rs', 'temp_home'),
    ('src/daemon/executor/bg_shell.rs', 'test_home'),
    ('src/daemon/executor/foreground.rs', 'test_home'),
    ('src/daemon/executor/ghost_remote.rs', 'test_home'),
    ('src/daemon/executor/ghost_sandbox.rs', 'test_home'),
    ('src/daemon/executor/shell_exec_connect_tests.rs', 'test_home'),
    ('src/daemon/executor/knowledge/shell_tests.rs', 'lifecycle_home'),
]
# Files that call a helper they reach through `use super::*`.
CALLERS = {
    'src/daemon/executor/shell_exec_hosted_tests.rs': 'test_home',
    'src/daemon/executor/knowledge/shell_lifecycle_tests.rs': 'lifecycle_home',
    'src/daemon/executor/knowledge/shell_observer_tests.rs': 'lifecycle_home',
}

def drop_def(src, name, path):
    lines = src.split('\n')
    sig = re.compile(r'^(\s*)(pub\(crate\) )?fn ' + name + r'\(\) -> \(crate::TestHomeGuard, tempfile::TempDir\) \{$')
    starts = [i for i, l in enumerate(lines) if sig.match(l)]
    assert len(starts) == 1, (path, name, starts)
    s = starts[0]
    indent = sig.match(lines[s]).group(1)
    e = next(i for i in range(s + 1, len(lines)) if lines[i] == indent + '}')
    d = s
    while d > 0 and lines[d - 1].strip().startswith('///'):
        d -= 1
    if d > 0 and lines[d - 1].strip() == '' and e + 1 < len(lines) and lines[e + 1].strip() == '':
        e += 1
    return '\n'.join(lines[:d] + lines[e + 1:])

def retarget(src, name):
    return re.sub(r'(?<![\w:])' + name + r'\(\)', 'crate::test_home()', src)

for path, name in DEFS:
    src = open(path).read()
    src = drop_def(src, name, path)
    src = retarget(src, name)
    open(path, 'w').write(src)
for path, name in CALLERS.items():
    src = open(path).read()
    assert re.search(r'(?<![\w:])' + name + r'\(\)', src), path
    open(path, 'w').write(retarget(src, name))

L = 'src/lib.rs'
lib = open(L).read()
anchor = '/// RAII guard returned by [`test_home_guard`].'
assert lib.count(anchor) == 1
lib = lib.replace(anchor, '''/// A fresh temporary `HOME` for one test. The guard holds
/// [`TEST_HOME_LOCK`] and restores `HOME` on drop; `HOME` points at the
/// returned directory, and the daemon's directory tree is created in it.
/// Hold both for the whole test body: dropping the directory first leaves
/// `HOME` pointing at a deleted path.
#[cfg(test)]
pub(crate) fn test_home() -> (TestHomeGuard, tempfile::TempDir) {
    let guard = test_home_guard();
    let dir = tempfile::tempdir().expect("tempdir");
    unsafe { std::env::set_var("HOME", dir.path()) };
    crate::config::Config::ensure_dirs().ok();
    (guard, dir)
}

''' + anchor)
open(L, 'w').write(lib)
print('one crate::test_home(); eleven local helpers removed')

# Imports the move leaves unused, as the compiler names them.
UNUSED = [
    ('src/daemon/executor/bg_shell.rs', '    use crate::config::Config;\n'),
    ('src/daemon/executor/ghost_remote.rs', '    use crate::test_home_guard;\n'),
    ('src/daemon/executor/shell_exec_connect_tests.rs', 'use crate::config::Config;\n'),
    ('src/daemon/executor/shell_exec_connect_tests.rs', 'use crate::test_home_guard;\n'),
    ('src/daemon/executor/shell_exec_tests.rs', 'use crate::config::Config;\n'),
    ('src/daemon/executor/shell_exec_tests.rs', 'use crate::test_home_guard;\n'),
    ('src/daemon/server/shells_tests.rs', 'use crate::config::Config;\n'),
    ('src/daemon/server/shells_tests.rs', 'use crate::test_home_guard;\n'),
    ('src/shell/launch.rs', '    use crate::test_home_guard;\n'),
    ('src/shell/session_tests.rs', 'use crate::config::Config;\n'),
]
for path, line in UNUSED:
    src = open(path).read()
    assert src.count(line) == 1, (path, line)
    open(path, 'w').write(src.replace(line, '', 1))
G = 'src/daemon/executor/ghost_sandbox.rs'
src = open(G).read()
old = 'spawn_in_process_host, test_home,'
assert src.count(old) == 1
open(G, 'w').write(src.replace(old, 'spawn_in_process_host,', 1))
print('unused imports removed')
PY
cargo fmt --all
cargo fmt --all
```

## Acceptance criteria

- [ ] The End-to-end block prints `local_defs=0 shared=1 calls=154`
      (today: `local_defs=11 shared=0`).
- [ ] `cargo test` reports **exactly** `passed=2607 failed=0`, the same
      count as before the phase.
- [ ] `tests/hygiene.rs` and `cargo test --test bug_tracker` pass. fmt,
      build and clippy are clean.
- [ ] `grep -c '^### Findings — '` on this doc prints `1`, and the
      end-to-end entry holds the fenced output.

## End-to-end verification

```sh
bash <<'E2E'
echo "local_defs=$(grep -rnE 'fn (test_home|shell_test_home|lifecycle_home|temp_home)\(\) -> \(crate::TestHomeGuard' src | wc -l) shared=$(grep -c 'pub(crate) fn test_home()' src/lib.rs) calls=$(grep -rn 'crate::test_home()' src | wc -l)"
cargo test --test hygiene 2>&1 | grep -E '^test result'
cargo test --test bug_tracker 2>&1 | grep -E '^test result'
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1
cargo fmt --all --check; echo "fmt_exit=$?"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
E2E
```

Expected: `local_defs=0 shared=1 calls=154`, hygiene `ok. 16 passed`,
bug tracker `ok. 6 passed`, clippy clean, `fmt_exit=0`,
`passed=2607 failed=0`. **Known flakes:**
`read_shell_screen_lines_tails_the_visible_rows` (M31 #65) and the
event-log tests (M32 #11). One of them appeared in the dry run. If one
fails, the suite stops early, for example at `passed=2517 failed=1`.
Run the block once more and paste both outputs.

## Authorizations

- Edit `src/lib.rs` and the fourteen test-bearing files the script
  names, through the script only.

## Out of scope

- The other temp-`HOME` shapes, such as `struct TestHome`,
  `with_test_home(|| …)` and `fn temp_home() -> TmpHome`. They return
  different types and are not copies of this helper.
- #8, #49, #50, #62, #63 (rejected above).

## Update Log

<!-- entries appended below this line -->
### Update — 2026-10-30 02:22 (started)

Executor session opened on phase-10-test-home. Status flipped todo →
in-progress here and in the M32 README row; no code touched yet. Plan: run the
spec's script once as written, log its output, then findings, E2E block,
commit.
### Update — 2026-10-30 02:22 (cuts done)

Task 1's script ran once, as written, unmodified and unindented, via
`python3 - <<'PY'` inside `bash`, followed by `cargo fmt --all` twice. Both
assertions passed and both summary lines printed:

```
one crate::test_home(); eleven local helpers removed
unused imports removed
```

`cargo fmt --all` ran twice after the script and exited 0 both times.
### Findings — 2026-10-30 02:22

`git diff --stat` shows 17 files, 189 insertions, 296 deletions: `src/lib.rs`
gains the 13-line `test_home()` while the fifteen def/caller files and the three
`use super::*` callers shrink, the largest being `session_tests.rs` (68 lines
touched) and `shells_tests.rs` (61) — a net −107 lines. No file outside the
eleven def sites, the three callers and `src/lib.rs` changed.
### Update — 2026-10-30 02:25 (end-to-end verification)

Ran the phase doc's E2E block verbatim once, unmodified, redirecting nothing —
pasting its captured output below as received. No known flake appeared, so
there was no second run.

```
local_defs=0 shared=1 calls=154
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.98s
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 12.51s
fmt_exit=0
passed=2607 failed=0
```

Every acceptance criterion reads green off that capture: the shape moved from
`local_defs=11 shared=0` to `local_defs=0 shared=1 calls=154`; hygiene is
`ok. 16 passed`; bug tracker is `ok. 6 passed`; clippy finished clean;
`fmt_exit=0`; and the full suite is exactly `passed=2607 failed=0`, the same
count as before the phase.
