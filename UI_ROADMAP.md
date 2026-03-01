# DaemonEye Chat UI Improvement Roadmap

Improvements suggested after the initial Claude Code-inspired UI pass.
Ordered roughly by visual impact.

---

## 1. Markdown rendering ✅ (implemented)

The AI response arrives as markdown but previously rendered as raw text.
Now handles:

- `**bold**` / `*italic*` → ANSI bold / italic
- `` `inline code` `` → yellow (ANSI 33)
- `# / ## / ###` headings → bold + coloured (magenta / bright-cyan / blue)
- Fenced code blocks → dim border with language label, cyan code text
- `- / * / +` bullet lists → yellow `•` symbol
- `  - / * ` sub-bullets → dim `◦` symbol
- `1. 2. 3.` numbered lists → yellow numbers
- `> blockquote` → dim `│` prefix
- `---` horizontal rules → full-width dim `─` line
- Word-wrapping is ANSI-aware (`visual_len` strips escape sequences before
  measuring column width so bold / coloured words wrap correctly)

---

## 2. Syntax highlighting in code blocks ✅ (implemented)

On top of the fenced-code styling already in place, detect the language
from the fence marker (` ```bash `, ` ```python `, etc.) and apply basic
keyword coloring.  Even a two-color scheme (keywords vs strings) makes
code dramatically easier to scan.

**Approach**: maintain a small per-language keyword set; scan code lines
for matches and wrap them in ANSI colors before printing.  No external
crate needed for a first pass.

**Priority**: High — code blocks are extremely common in sysadmin responses.

---

## 3. Session / turn indicator ✅ (implemented)

`SessionInfo { message_count }` is silently consumed.  It could display
a subtle header before each response showing the current turn number and
total context depth, e.g.:

```
  ─ turn 3 · 7 messages ──────────────────────────
```

**Priority**: Medium — useful context, very low implementation cost.

---

## 4. Width-adaptive header ✅ (implemented)

The opening box is fixed at 48 chars, which looks small in a wide pane.
It could stretch to fill `terminal_width()` (already available) with a
centred title and a session ID or turn count on the right.

**Priority**: Medium — cosmetic but noticeable in wide splits.

---

## 5. AI response text color ✅ (implemented)

The streamed prose is plain terminal-default.  A subtle tint (e.g.
soft-white `\x1b[97m` or light-blue `\x1b[94m`) distinguishes AI output
from other terminal content at a glance.

**Priority**: Low-medium — personal taste; easy to add one line.

---

## 6. Structured tool-call output panel ✅ (implemented)

The current tool-call block is minimal.  Could look like a bordered panel:

```
 ╭─ terminal · visible to you ──────────────────╮
 │  $ journalctl -u nginx.service --no-pager    │
 ╰──────────────────────────────────────────────╯
   Approve? [y/N] ›
```

Background command results returned to the AI could be shown in a dimmed
result block rather than disappearing silently.

**Priority**: Medium — improves clarity of the tool-call flow.

---

## 7. Distinct coloring per message role ✅ (implemented)

Consistent palette across all message types:

| Element              | Suggested color          |
|----------------------|--------------------------|
| AI prose             | default / soft white     |
| System notifications | yellow / amber           |
| Errors               | red                      |
| Tool output excerpts | dim / muted              |
| Headings             | magenta / cyan / blue    |
| Code                 | cyan                     |
| Inline code          | yellow                   |

Most of this is covered by markdown rendering; the remaining gap is the
system-notification tokens sent by the daemon (sudo alerts, pane-switch
notices) — these could be given a distinct prefix symbol + colour.

**Priority**: Low — largely addressed by items 1 and 2.
