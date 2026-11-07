# Phase 10 — secrets

**Milestone:** M36 — Chat UX and audit fixes
**Status:** todo
**Drafted:** 2026-11-06
**Depends on:** none
**Estimated diff:** ~200 lines
**Tags:** language=rust, kind=bug, size=s

## Goal

Fix three findings from `FINDINGS.md`. All three let a secret reach a place
it should not.

- **S4:** `GeminiClient::chat` (`src/ai/backends/gemini.rs`) puts the API key
  in the URL (`…?alt=sse&key=AIza…`). reqwest's `Error` Display appends the
  full URL, so a transport or stream error carries the key into
  `Response::Error`, the hard-fail briefing on disk, and the ghost and
  watchdog logs. There are two such sites in `src/ai/mod.rs`:
  - the `Request failed:` bail in `send_with_retry_inner`;
  - the `Ok(Some(Err(e))) => Some(Err(e.into()))` arm in
    `stream_next_with_timeout`.
- **S5:** `builtin_defs` (`src/ai/filter.rs`) has a generic secret pattern
  that matches only when `=` or `:` follows the keyword directly. These all
  reach the model unmasked:
  - `AWS_SECRET_ACCESS_KEY=…` and `aws_secret_access_key = …`;
  - JSON-quoted `"password": "…"` and `"api_key":"sk-…"`;
  - `Authorization: Bearer sk-ant-…`;
  - a bare `sk-…` or `sk-ant-…` key;
  - a `?key=` / `&key=` query parameter.
- **S10:** the three backends' "model sent invalid arguments for tool"
  warnings log the raw arguments unmasked.

## Behaviour

- **Gemini:** the key travels in the `x-goog-api-key` request header. The
  URL holds no `key=` parameter and no part of the key.
- **Transport errors:** an error built from a `reqwest::Error` never
  includes the request URL. Its message still names what went wrong (for
  example the connection failure).
- **Masking:** `mask_sensitive` replaces the secret value in each S5 form
  above. Every new pattern uses the replacement `<REDACTED>`, so
  `get_redaction_counts` keeps its key set unchanged.
- **Must NOT mask** (these stay byte-identical):
  - `disk-usage-report-generator-tool` and
    `task-scheduler-component-name` (hyphenated words that contain `sk-`);
  - `the token expired` (prose);
  - `AWS_REGION=us-east-1`;
  - `https://example.com/?keyword=rust`.
- **Invalid-args warnings** log the arguments after `mask_sensitive`.

## Constraints

- **Names are fixed. Use them; do not search the repo for them, because
  they do not exist yet.**
  - `gemini_stream_url(model: &str) -> String` in `gemini.rs`: the URL, with
    no key.
  - `gemini_stream_request(client: &reqwest::Client, model: &str, api_key: &str, body: &Value) -> reqwest::RequestBuilder`
    in `gemini.rs`: the POST with the header and the JSON body. `chat`'s
    `send_with_retry` closure calls it.
  - `scrub_transport_error(e: reqwest::Error) -> anyhow::Error` in
    `src/ai/mod.rs`: uses `reqwest::Error::without_url`. Both `mod.rs` sites
    above go through it.
  - `invalid_args_log_line(tool_name: &str, args: &Value) -> String` in
    `src/ai/tools/dispatch.rs`, next to `invalid_args_event`: returns the
    whole warning text with masked arguments. All three backends' warnings
    log it.
- **Never grep the repo root** (`.`). `.rexymcp/sessions/*.jsonl` lines are
  hundreds of KB and trip the runaway-output hard fail. Search `src/`,
  `tests/` or `docs/` only.
- **Tests are hermetic.**
  - The masking tests use the existing `mask` helper in `filter.rs`'s test
    module.
  - The header test builds the request with `.build()` and inspects it. It
    never sends the request.
  - The transport-error test sends to `http://127.0.0.1:1/?key=SECRETKEY123`
    using a client built with `.no_proxy()`. Nothing listens on port 1, so
    the request fails at once.
- **Allowlist:** `GeminiClient::chat` is on `tests/hygiene_allowlist.txt`.
  It must not grow. If it shrinks, lower its count to the value the
  hygiene gate reports.
- **Preservation:** the existing tests in `filter.rs` and the backends stay
  green without being edited.

## Spec

1. **Red tests first.** Write these against the current tree, run them and
   keep the failing output. All carry the `secrets_` prefix:
   - `secrets_gemini_key_in_header`: the built request's URL contains
     neither `key=` nor the key, and its `x-goog-api-key` header equals the
     key.
   - `secrets_transport_error_drops_url`: the scrubbed error's `{:#}` text
     does not contain `SECRETKEY123` or `127.0.0.1:1/`.
   - `secrets_mask_aws_secret_env`: both AWS forms lose
     `wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY`.
   - `secrets_mask_json_quoted`: `{"password": "hunter2"}` loses `hunter2`,
     and `{"api_key":"sk-abcdefghijklmnopqrstuv"}` loses its value.
   - `secrets_mask_bearer`: `Authorization: Bearer sk-ant-api03-abcdefghijklmnop`
     loses the token.
   - `secrets_mask_bare_sk_key`: `the key is sk-proj-ABCDEF1234567890abcdef`
     loses the key.
   - `secrets_mask_query_key`: `…:streamGenerateContent?alt=sse&key=AIzaSyEXAMPLE123`
     loses `AIzaSyEXAMPLE123`.
   - `secrets_mask_negatives`: each must-NOT-mask string above is
     unchanged.
   - `secrets_invalid_args_log_masked`: `invalid_args_log_line` for args
     `{"command": "export AWS_SECRET_ACCESS_KEY=wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY"}`
     names the tool and does not contain the secret.
2. **Implement**, one edit per turn, running `cargo build` after each, in
   this order:
   1. the masking patterns;
   2. `invalid_args_log_line` at the three warning sites;
   3. `scrub_transport_error` at both `mod.rs` sites;
   4. the two Gemini functions, used from `chat`.
3. **Run tests**: `cargo test` passes with `failed=0`.
4. **Red runs.** Add an `### Update — <date> (red runs)` entry with step 1's
   `test result:` lines and failure or compile messages, verbatim.
   - A test whose red run is only a compile error says so.
   - Label each pasted run with what the tree held at that moment.
   - Do not paste failures of your own wrong assertions as red runs.
5. **Findings.** Add a `### Findings — <date>` entry, two or three lines, or
   `none`.
6. **End-to-end.** Run § End-to-end verification once and paste its output,
   fenced, in an `### Update — <date> (end-to-end verification)` entry.
7. **Commit** as `fix: secrets stay out of URLs, errors and logs; masking covers AWS, JSON, Bearer and sk- keys (M36 phase-10)`.

## Acceptance criteria

- [ ] The E2E block's first line reads `secrets passed=P failed=0` with
      P ≥ 10: the nine new tests plus the existing
      `multiple_secrets_in_one_string`, which the filter also matches. Today
      it reads `passed=1`.
- [ ] The E2E block shows `url_key=0`: `gemini.rs` holds no `key={}` URL
      format. Today it is 1.
- [ ] The E2E block shows `without_url` at 1 or more. Today it is 0.
- [ ] The E2E block shows `raw_args_warn=0`: no backend formats `{args}`
      into an invalid-arguments warning. Today it is 3.
- [ ] The red-runs entry shows the new tests failing on the pre-phase code.
- [ ] Preservation: the existing `filter.rs` tests pass unedited.
- [ ] The E2E block shows `hygiene`, `doc_truth` and `bug_tracker` `ok`,
      `build_warnings=0`, clippy clean, `fmt_exit=0` and `failed=0`.

## End-to-end verification

```sh
bash <<'E2E'
cargo test secrets_ 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "secrets passed=" p, "failed=" f}'
grep -c 'key={}' src/ai/backends/gemini.rs | sed 's/^/url_key=/'
grep -rn 'without_url' src/ai | wc -l | sed 's/^/without_url=/'
grep -rn 'invalid arguments for tool' src/ai/backends | grep -c 'args' | sed 's/^/raw_args_warn=/'
touch src/main.rs && cargo build 2>&1 | grep -c '^warning' | sed 's/^/build_warnings=/'
for t in hygiene doc_truth bug_tracker; do cargo test --test $t 2>&1 | grep -E '^test result'; done
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1
cargo fmt --all --check; echo "fmt_exit=$?"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
E2E
```

## Authorizations

- Edit `src/ai/filter.rs`, `src/ai/mod.rs`, `src/ai/tools/dispatch.rs` and
  `src/ai/backends/{anthropic,openai,gemini}.rs`.
- Lower the `GeminiClient::chat` count in `tests/hygiene_allowlist.txt` if
  the hygiene gate asks you to.

## Out of scope

- The Anthropic and OpenAI keys, which already travel in headers.
- Masking anything other than the S5 forms listed.
- The "malformed JSON arguments" warnings: they log serde's error, not the
  arguments.

## Update Log

<!-- entries appended below this line -->
