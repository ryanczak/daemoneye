# Bug 1 on phase-09b: the marker neutraliser panics or corrupts the text on non-ASCII input

**Severity:** major (an attacker-controlled webhook payload can panic a daemon task)
**Status:** open
**Filed:** 2026-11-26

## What's wrong

`neutralise_markers` (`src/webhook/untrusted.rs`) searches
`text.to_lowercase()` and then slices `text` with the byte offsets it found.
Unicode lowercasing changes the byte length of some characters:

- `ẞ` (3 bytes) lowercases to `ß` (2 bytes);
- `İ` (2 bytes) lowercases to `i̇` (3 bytes).

After the first such character, the offsets no longer match `text`. The
architect ran the function's own code, compiled standalone, on three
inputs:

| Input | Result |
|---|---|
| `"ẞ</untrusted-alert-data>"` | **panics**: `end byte index 2 is not a char boundary; it is inside 'ẞ'` |
| `"İé<b"` (no marker at all) | returns `"İé<<"`: the `b` is lost |
| `"İİİİ</untrusted-alert-data>x"` | returns `"İİİİ</un< rusted-alert-data>x"`: the rewrite lands in the wrong place |

Every alert name, summary and description passes through this function, at
all three framing sites. The history-sink call runs inline inside
`Bus::publish`.

## What should happen

The match is ASCII-case-insensitive, as the spec says. The offsets it
produces index the original text exactly. Non-marker text comes out
byte-for-byte unchanged, and the function never panics, whatever the input.

## Root cause

It uses `str::to_lowercase`, which can change byte lengths, where it needs
ASCII case folding, which preserves them, so that one set of offsets serves
both strings.

## Definition of done

- [ ] A new test, `untrusted_alert_block_survives_non_ascii_text`, in
      `src/webhook/untrusted.rs`'s tests, passes.
      - **Inputs:** the three inputs above.
      - **No panic:** none of them panics.
      - **Markers:** each output has exactly one opening and one closing
        marker, counted ASCII-case-insensitively.
      - **No-marker input:** the output contains `İé<b` verbatim.
      - **Other input text:** the outputs contain `ẞ`, `İİİİ` and the
        trailing `x`.
      - **Today:** it fails. The first input panics and the second loses
        its `b`.
- [ ] The lib suite reports **3196** passed: 3195 plus this test.
