# NEXT

**Active phase:** **M21 phase-06a — prompt-classification**, drafted
2026-09-23, status `todo`. Dispatch with `/rexymcp:dispatch phase-06a`.

**Row 06 split into 06a/06b/06c at drafting — it was four features in one
row:** the `classification` field and its rendering, the `new_host` gate with
its offer-to-persist, per-class-per-host session scope, and ghost `hosts:`
enforcement. M21 is now **seventeen rows**.

**06a is the panel half.** `Response::ToolCallPrompt` gains a `classification`
(`command` / `remote_shell` / `new_host` / `escape_hatch`), `ApprovalRequest`
carries the host, and the client renders the class **naming the host** — the
milestone's "approval panel names the host" exit criterion. It classifies and
renders; it changes no gating.

**Measured at drafting, and it is why the panel half ships first.**
`prompt_and_await_approval` runs **before** host resolution
(`executor/mod.rs:316-345`; `host_entry` is refused only inside the
`Ok(cmd_id)` arm that follows). So today:

1. the model calls `run_terminal_command { command: "uptime", host: "nope" }`
2. the panel shows `uptime` and nothing about `nope`
3. the user approves
4. *then* the run returns `Error: unknown host "nope"; configured hosts: pinky`

You approve a command whose destination you were never shown, and the refusal
lands after your decision. 06a fixes step 2 without touching the order;
**reordering is 06b's job** and the spec says so explicitly, twice.

**Pre-dispatch check run; all three structural criteria confirmed failing**
(`variants=0`→4, `prompt_field=0`→1, `req_host=0`→1), `lib_counts=1961p-0f-6i`
→1970, `ignore_total=6` preserved, and the nine named tests read
`0 passed; 1967 filtered out`.

**The over-broad-grep calibration was applied, and then verified rather than
assumed.** Every structural criterion in 06a is `awk`-scoped to its construct.
More importantly, each range was **checked to anchor on real text** — ranges
that match nothing also return 0, which is the vacuous guard that shipped
twice (04d `event_host`, 05 `fingerprint_not_broadened`). The
`ToolCallPrompt` and `ApprovalRequest` ranges were printed and confirmed
non-empty; the `Classification` range is legitimately empty today, so its
declaration is pinned verbatim in Task 1 and the grep was **simulated against
that exact text** and returns 4. Still two occurrences, not three — no
WORKFLOW fold yet.

**Next after 06a: 06b — new-host-gating**, then 06c (ghost `hosts:`), 07, 08,
08b, 09.

Seventeen rows in M21; live host `pinky` (sudo requires a password since
2026-09-22); `max_turns` 1000.
