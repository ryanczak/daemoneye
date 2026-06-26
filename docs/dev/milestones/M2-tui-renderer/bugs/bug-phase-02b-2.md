# Bug 2 on phase-02b: credential prompt returns masking bullet characters, not the actual typed credential

**Severity:** blocker
**Status:** open
**Filed:** 2026-06-25

## What's wrong

`prompt_credential_ratatui` (`src/cli/commands/stream.rs:1548-1583`) uses a
single `InputLine` for both **display masking** and the **returned credential
value**. Line 1573 inserts the masking glyph `'•'` into the `InputLine` on
every printable keystroke:

```rust
// stream.rs:1572-1574
c if c >= 0x20 => {
    cred.insert('•'); // mask the character
    let _ = renderer.draw_prompt(prompt_text, &cred, status);
}
```

`cred.as_str()` is then returned at line 1582. Because `InputLine` stores
`'•'` for every typed character, the returned `String` is a run of bullet
characters (e.g., `•••••••` for a 7-char password), not the actual typed
credential. The daemon receives `credential: "•••••••"` in
`Request::CredentialResponse { id, credential }` (stream.rs:367) and injects
that into the background tmux window, causing authentication to fail.

The visual masking intent is correct (show `•` in the live region), but it
must not destroy the real value.

## What should happen

The function must track the **real characters** in a separate buffer and
return those, while showing the masked buffer in the live region. Two buffers:

- `cred_real: String` — accumulates the actual typed characters; returned at
  the end.
- `cred_display: InputLine` — accumulates `'•'` for display via
  `draw_prompt`; never returned.

Backspace removes one character from **both** buffers; Ctrl+C/Escape clears
both. Final return is `cred_real`, not `cred_display.as_str()`.

## How to fix

In `prompt_credential_ratatui` (`src/cli/commands/stream.rs`):

1. Replace `let mut cred = crate::cli::input::InputLine::new()` with two
   buffers:
   ```rust
   let mut cred_real = String::new();
   let mut cred_display = crate::cli::input::InputLine::new();
   ```
2. Replace all `cred.*` calls consistently:
   - On printable byte: `cred_real.push(c as char); cred_display.insert('•');`
   - On backspace: `cred_real.pop(); cred_display.backspace();`
   - On Ctrl+C/Escape: `cred_real.clear(); cred_display = InputLine::new();`
   - `draw_prompt` uses `&cred_display` (unchanged).
3. Return `cred_real` instead of `cred.as_str()`.

Also add a unit test that drives the credential path with injected bytes and
asserts the returned value equals the typed characters (not `•` characters).
This path had no test in the previous pass.

## Verification

- [ ] `prompt_credential_ratatui` called with bytes `[b'p', b'a', b's', b's', b'\r']`
      returns `"pass"`, not `"••••"`.
- [ ] `cargo build` zero warnings; `cargo clippy --all-targets --all-features -- -D warnings`
      passes; `cargo test` passes.
- [ ] Live E2E under tmux: invoke a script that requires a credential prompt;
      confirm the correct credential is injected and authentication succeeds.
      Quote `tmux capture-pane -p` output in the Update Log.
