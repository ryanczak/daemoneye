# OSC 7501 — Program Status Protocol (reference digest)

Source: <https://www.superlogical.com/rex/docs/build/program-status> (spec,
revision 0.4 era; the 0.4 history entry reads "Remove terminfo"), announced in
<https://mitchellh.com/writing/program-status-osc7501>. Implementations named
there: libghostty (ghostty-org/ghostty#14560) and Rex; proof-of-concept
emitters for Terraform, Claude Code, Codex and Homebrew. Digested 2026-11-03
for the M36 program-status phase — the executor cannot fetch URLs, so the
phase doc pre-injects from here. Re-check the spec at drafting time; it is
young and invites feedback.

## Syntax

- Report: `OSC 7501 ; pairs ST`. Feature query: `OSC 7501 ; ? ST`.
- ST is `ESC \` (0x1B 0x5C) **or** BEL (0x07).
- ```
  pairs := pair (":" pair)*
  pair  := key "=" value
  key   := [a-z]+
  value := [A-Za-z0-9_.,+/=-]*
  ```
- Whitespace around keys and values is trimmed. A malformed pair is skipped
  and the rest still applies; unknown keys MUST be ignored; a repeated key's
  last value wins.
- `msg` and `title` are standard base64 of UTF-8 (padding optional). Decoded
  text MUST NOT contain control characters (U+0000–U+001F, U+007F,
  U+0080–U+009F), newline and tab included.
- The **whole report is discarded** if it breaks a limit, its base64 does not
  decode, or its decoded text holds a control character.

## States (`state`, required; an unknown value drops the report)

| State | Meaning | Lifetime |
|---|---|---|
| `idle` | at rest, awaiting the next instruction | until replaced or cleared |
| `working` | running; may carry `progress` | until replaced, cleared, process exit, or the next shell prompt |
| `done` | finished, result not yet seen | survives process exit and the shell prompt |
| `blocked` | cannot continue until the user acts | as `working` |
| `error` | failed and stopped | as `done` |
| `clear` | not a state: removes the addressed record and everything beneath it; with no `id`, every record on the terminal | — |

## Keys

- `id` — `/`-separated path, each segment `[A-Za-z0-9_.+-]{1,32}`; absent =
  the root record. A report whose `id` fails the grammar is ignored (so a bad
  id cannot overwrite the root).
- `kind` — `permission` | `question` | `auth`; meaningful only with
  `blocked`; unknown values are treated as absent.
- `progress` — integer `0`–`100`; meaningful with `working` or `blocked`;
  absent = indeterminate.
- `app` — `[A-Za-z0-9_.+-]{1,32}`, a stable program identifier; a record
  without one inherits its nearest ancestor's.
- `title` — base64 UTF-8, a short label. `msg` — base64 UTF-8, one line.
- No timestamps are defined.

## Records

- One record per id per terminal. Each report **replaces** its record
  whole — keys it omits are gone.
- `/` gives parent/child; a parent need not exist.
- Records belong to the terminal, not the screen: switching primary and
  alternate screens does nothing. `RIS` removes every record; `DECSTR` does
  not.
- No heartbeat; a terminal MUST NOT require re-sending to keep a record.
- On process exit or a new shell prompt (OSC 133 `A`) the terminal MUST drop
  `working` and `blocked` records and MAY drop `idle`; `done` and `error`
  survive both.

## Feature detection

- Query `OSC 7501 ; ? ST`; reply `OSC 7501 ; ? : states=... : kinds=... ST`.
  The terminal MUST reply with `?` and MAY add `states` / `kinds` (default
  lists: the five states; `permission`, `question`, `auth`).
- Programs SHOULD send `CSI c` right after the query: if the DA reply comes
  first, the protocol is unsupported. Detection is optional; terminals MUST
  ignore unknown OSCs.

## Limits (a report breaking one is discarded whole)

Whole sequence 4096 bytes (largest legal report < 3300) · key 16 bytes ·
`msg` 2732 encoded / 2048 decoded · `title` 256 encoded / 192 decoded ·
`app` 32 · `id` 128 total, 32 per segment, 8 levels · records per terminal
256 (MUST support ≥ 64; at the cap, the least recently updated goes).

## Security

- MUST refuse a report whose decoded `msg`/`title` holds a control character;
  MUST NOT treat either as markup; SHOULD disarm bidi overrides and other
  invisible formatting outside the grid.
- Terminals write back **only** the feature-detection reply — never ids,
  titles or messages.
- Displayed records SHOULD identify their source terminal; external effects
  (desktop notifications, sounds) SHOULD be rate-limited.

## Not addressed by the spec

Multiplexers, nested terminals, tmux and passthrough are not mentioned. For
DaemonEye that matters on the emit side: a chat client inside tmux needs the
report to reach the outer terminal (tmux passthrough, `allow-passthrough`),
which the spec leaves open — decide at drafting.
