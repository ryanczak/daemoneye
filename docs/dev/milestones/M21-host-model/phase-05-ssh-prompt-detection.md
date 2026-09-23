# Phase 05: ssh-prompt-detection — the four prompts an ssh connection can raise

**Milestone:** M21 — Host model & SSH
**Status:** done
**Depends on:** phase-04d (`run_terminal_command { host? }` reaches an ssh shell)
**Estimated diff:** ~360 lines across 3 files, plus 10 tests
**Tags:** language=rust, kind=feature, size=m

## Goal

An ssh connection raises prompts the daemon has never seen: a host-key trust
question, a password, a key passphrase, and a terminal `Permission denied`.
Today every one of them **hangs the command until its timeout** — measured, not
assumed (§ Current state 1). This phase teaches `classify_prompt` to recognise
them, routes the answerable ones through 01a's existing `CredentialPrompt`
round-trip, and fails fast on the ones that cannot be answered.

## Architecture references

- `docs/design/daemoneye-2.0.md` § 3 "Security model deltas" — the four
  approval classifications and `"i"`-event scrubbing.
- `docs/security.md` § 2 — shell logs at rest are raw and `0600`; the scrub is
  what keeps a typed credential out of the cast log.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

Every string in this section was **captured live against `pinky` on
2026-09-22** at commit `a27a6277`, through a PTY (`script -qc`), with the same
`-tt` ssh the daemon spawns (`src/host.rs:84` `ssh_argv`). None is quoted from
documentation or memory.

Baseline: `cargo test --lib` → **1951 passed / 0 failed / 6 ignored**.

### 1. All three prompts hang — the exact failure 01a found for local sudo

Each probe below exited **124 (timeout)**, meaning ssh sat at the prompt until
killed. Under the daemon this is a command that produces no marker until its
`timeout_secs` expires.

**Host key** (unknown-host, throwaway `known_hosts`):

```
The authenticity of host 'pinky.home.planetfoo.org (172.16.50.75)' can't be established.
ED25519 key fingerprint is: SHA256:8H2tFe0tLZodJlVr7s88lqZFVQTt6eo4XiGLGm1q4GI
This key is not known by any other names.
Are you sure you want to continue connecting (yes/no/[fingerprint])?
```

**Password** (`-o PreferredAuthentications=password -o PubkeyAuthentication=no`):

```
matt@pinky.home.planetfoo.org's password:
```

**A password prompt over the ssh PTY from `su`** — see § 4 for why this one
matters:

```
Password:
```

### 2. `Permission denied` is terminal, not a prompt

Feeding a wrong password produced, and ssh then **exited 255**:

```
nosuchuser@pinky.home.planetfoo.org: Permission denied (publickey,password).
```

This must not be treated as answerable. It is the signal to stop with a clear
message rather than wait out the timeout.

### 3. MEASURED HAZARD — the host-key prompt contains the word "fingerprint"

Two of its lines do:

```
ED25519 key fingerprint is: SHA256:…
Are you sure you want to continue connecting (yes/no/[fingerprint])?
```

`is_fingerprint_prompt` (`src/daemon/utils/sudo.rs:23`) matches only the three
biometric sentences —

```rust
output.contains("Place your finger on the fingerprint reader")
    || output.contains("Swipe your finger across the fingerprint reader")
    || output.contains("Failed to match fingerprint")
```

— so it does **not** fire today, and `classify_prompt` checks it **first**
(`sudo.rs:92`). **Broadening it to a bare `"fingerprint"` would classify every
host-key prompt as an unanswerable biometric prompt.** Task 4 pins a negative
test against exactly that.

### 4. MEASURED — `pinky` has passwordless sudo, so it cannot exercise remote sudo prompting

```
$ ssh pinky 'sudo -k; sudo -n true; echo rc=$?; sudo -l'
sudo_n_rc=0
User matt may run the following commands on pinky:
    (ALL) NOPASSWD: ALL
```

A remote `sudo` on `pinky` **never prompts**. The milestone README's row-05
line says "Remote sudo is 01a's detector over the ssh PTY — verified live";
on this host that specific verification is **not available**, and this phase
does not claim it. What *is* live-verifiable, and what Task 6 uses, is
`su` over the ssh PTY, which does prompt (§ 1, third block, measured).

**This also affects row 09**, whose exit criterion is a remote sudo edit with
"the password absent from every `"i"` event" — with NOPASSWD there is no
password, so that criterion is trivially satisfied and proves nothing. Flagged
here; the decision belongs to row 09's drafting, not to this row.

### 5. Where the detection happens

`classify_prompt` (`src/daemon/utils/sudo.rs:91`) is pure and takes the
per-invocation sentinel:

```rust
pub fn classify_prompt(output: &str, sentinel: &str) -> Option<PromptKind> {
    if is_fingerprint_prompt(output) {
        return Some(PromptKind::Fingerprint);
    }
    if output.contains(sentinel) {
        return Some(PromptKind::SudoPassword);
    }
    None
}
```

Its doc comment states the rule that governs this phase's design:

> Matches the **sentinel**, never sudo's own `[sudo] password for …` text:
> that text is locale- and `sudo.conf`-dependent, and stale output from an
> earlier command in the same shell must not trigger a prompt.

**ssh prompts have no sentinel** — ssh offers no `SUDO_PROMPT` equivalent — so
they must be matched by text. § Spec Task 1 states the narrower safety
argument that makes that acceptable, and Task 4 pins the stale-output case the
sentinel rule exists to prevent.

The single caller is the closure at `src/daemon/executor/shell_exec.rs:485`,
which returns a `PromptReply` (`src/shell/client.rs:135`):

```rust
pub enum PromptReply {
    Wait,            // nothing to answer; keep reading
    Send(Vec<u8>),   // write these bytes, then keep reading
    GiveUp(String),  // stop; the string is the reason
}
```

### 6. The scrub already covers password and passphrase — and correctly ignores the host key

`looks_like_secret_prompt` (`src/shell/backend.rs:51`) case-folds and matches
`password`, `passphrase`, `de-sudo-prompt-`, `verification code`, `otp`. So:

- the measured password prompt (`…'s password: `) **arms** the scrub ✓
- a passphrase prompt (`Enter passphrase for "…": `, captured from
  `ssh-keygen -y`; ssh's own wording differs but also contains `passphrase`)
  **arms** it ✓
- the host-key prompt matches **none** of the needles, so it does **not** arm
  the scrub — which is correct: the answer is `yes`, not a secret, and it
  belongs in the log in clear.

**No change to `looks_like_secret_prompt` is needed or wanted.** Task 4 pins
all three facts so a later edit cannot silently break them.

## Spec

### Task 1 — extend `PromptKind`

In `src/daemon/utils/sudo.rs`, add three variants to `PromptKind`
(`sudo.rs:76`):

- `SshHostKey` — answerable, but **not with a secret**; the answer is `yes`.
- `SshPassword` — answerable with a credential.
- `SshPassphrase` — answerable with a credential.

`SudoPassword` and `Fingerprint` keep their meanings. Do **not** rename them.

**Write the safety argument into the doc comment**, because it differs from
the sentinel rule directly above it: an ssh prompt is matched by text because
ssh offers no sentinel mechanism, and that is acceptable here because these
prompts occur during **connection establishment on a shell the daemon just
spawned** — there is no earlier command in that shell whose stale output
could match. State that; do not leave the divergence unexplained.

### Task 2 — classify the ssh prompts

Extend `classify_prompt` to return the new variants. Match on the **stable
fragments** of the measured strings, not on whole lines (hostnames, usernames
and key types all vary):

| Variant | Fragment to match | Measured source line |
|---|---|---|
| `SshHostKey` | `Are you sure you want to continue connecting` | § Current state 1 |
| `SshPassword` | `'s password:` | `matt@pinky…'s password:` |
| `SshPassphrase` | `Enter passphrase for` | § Current state 6 |

**Ordering matters and is pinned by tests:** `is_fingerprint_prompt` stays
**first** (an actual biometric prompt is unanswerable whatever else is on
screen), and the **sentinel check stays ahead of the ssh text matches** — a
sudo prompt carrying our own sentinel must classify as `SudoPassword` even if
ssh text is elsewhere in the same chunk.

`su`'s bare `Password:` (§ Current state 1) — **decide and say which variant
it lands in.** Matching a bare `Password:` is broader than `'s password:` and
risks catching stale output; matching only `'s password:` misses `su`. Either
is defensible; Task 4's test 7 pins whichever you choose, and the Update Log
must say which and why.

### Task 3 — route the new kinds

In the closure at `src/daemon/executor/shell_exec.rs:485`:

- **`SshPassword` / `SshPassphrase`** — reuse the existing `SudoPassword` arm's
  machinery: send `Response::CredentialPrompt`, await
  `Request::CredentialResponse`, reply `PromptReply::Send`. The prompt **text**
  shown to the user must name what is being asked for — a key passphrase is
  not a login password — rather than reusing `sudo_password_prompt`'s wording
  verbatim.
- **`SshHostKey`** — **do not send `CredentialPrompt`.** A host-key decision is
  a trust question, not a secret, and routing it through the credential path
  would both mislabel it in the client and hand it to the scrub. For this row,
  return `PromptReply::GiveUp` with a message that names the host and says the
  key is unknown, so the command fails immediately instead of hanging. The
  interactive accept/reject flow is **row 06**'s `new_host` classification.
- **`Permission denied`** — this is not a `PromptKind`; it is a terminal
  condition. Detect it in the same closure (or a small helper beside
  `classify_prompt` — your call, but if you add one, make it pure and test it)
  and return `PromptReply::GiveUp` naming the host, so the caller reports the
  authentication failure rather than waiting out `timeout_secs`.

### Task 4 — tests

Ten tests. Names are pinned; placement is yours.

Pure classifier tests in `src/daemon/utils/sudo.rs`:

1. `classify_prompt_matches_the_ssh_host_key_question` — the § Current state 1
   host-key block classifies as `SshHostKey`.
2. `classify_prompt_matches_an_ssh_password_prompt` — `matt@pinky…'s password:`
   → `SshPassword`.
3. `classify_prompt_matches_an_ssh_passphrase_prompt` — `Enter passphrase for
   "…":` → `SshPassphrase`.
4. **`host_key_prompt_is_not_a_fingerprint_prompt`** — the § Current state 3
   hazard. Assert `is_fingerprint_prompt` is `false` for the host-key block
   **and** that `classify_prompt` returns `SshHostKey`, not `Fingerprint`.
   This is the negative case the measured "fingerprint" collision demands.
5. `a_real_biometric_prompt_still_classifies_as_fingerprint` — *preservation*:
   `"Place your finger on the fingerprint reader"` is still `Fingerprint`.
6. `the_sentinel_still_wins_over_ssh_text` — a chunk containing **both** the
   invocation's sentinel and `'s password:` classifies as `SudoPassword`.
7. `su_password_prompt_classification_is_pinned` — Task 2's bare-`Password:`
   decision, asserted whichever way you chose.
8. `stale_ssh_prompt_text_from_an_earlier_command_does_not_classify` — the
   **negative** the sentinel rule exists for: a chunk such as
   `$ grep "Enter passphrase for" notes.txt` (the text appearing as a command
   echo, not as a live prompt) must **not** classify as a prompt. If your
   matcher cannot distinguish it, say so in the Update Log and pin the
   limitation in the test rather than deleting the test.

Scrub tests in `src/shell/backend.rs` (§ Current state 6 — pin what already
holds so it cannot silently break):

9. `an_ssh_password_prompt_arms_the_scrub` — `looks_like_secret_prompt` is
   true for the measured password prompt and for a passphrase prompt.
10. `the_host_key_question_does_not_arm_the_scrub` — it is **false** for the
    host-key block; the `yes` answer belongs in the log in clear.

### Task 5 — capture the end-to-end evidence

Run the block in § End-to-end verification verbatim and paste its output into a
new Update Log entry, **as that entry's first fenced block**.

### Task 6 — the live check

Run the § Live verification block by hand and paste its output into the same
entry, after the mutations. It uses `su` over the ssh PTY because `pinky`'s
sudo is NOPASSWD (§ Current state 4). **If `pinky` is unreachable, report the
criterion unmet** — do not describe what it would have printed.

## Acceptance criteria

Run the whole block before reporting done. Parenthesised values are what each
line produces **against the current tree**.

- [ ] `cargo test --lib` → **1961 passed / 0 failed / 6 ignored**, green on
      three consecutive runs (current: 1951/0/6). 1951 + Task 4's ten tests =
      1961. A justified extra is **reported, not trimmed** — say the number and
      the reason.
- [ ] `cargo fmt --all` clean; `cargo build` clean;
      `cargo clippy --all-targets --all-features -- -D warnings` exits 0.
- [ ] `cargo test` → every other target unmoved: doc_truth 14, integration 32,
      isolation 9, bug_tracker 6 (*preservation*).
- [ ] `PromptKind` has the three new variants:
      ``awk '/pub enum PromptKind/,/^\}/' src/daemon/utils/sudo.rs | grep -cE '^\s+(SshHostKey|SshPassword|SshPassphrase),'``
      → **3** (current: 0).
- [ ] `is_fingerprint_prompt` was **not** broadened — **scoped to the
      function**:
      ``awk '/pub fn is_fingerprint_prompt/,/^\}/' src/daemon/utils/sudo.rs | grep -c '"fingerprint"'``
      → **0** (*preservation*, and the § Current state 3 hazard). **Do not grep
      the whole file**: the word legitimately appears in doc comments
      explaining this very hazard, so an unscoped grep reads non-zero on a
      correct tree. (Corrected at the 2026-09-22 escalation — the first
      dispatch's tree tripped the unscoped form on a comment while the matcher
      itself was untouched.)
- [ ] `looks_like_secret_prompt`'s needle list is unchanged:
      ``awk '/const NEEDLES/,/\];/' src/shell/backend.rs | grep -c '"'`` →
      **5** (current: 5 — *preservation*; § Current state 6 says no change is
      needed, and a change here is a scope violation).
- [ ] No new `#[ignore]` **in production paths or unit tests**; the total is
      unchanged: `grep -rc '#\[ignore' --include=*.rs src/ | grep -v ':0' | awk -F: '{s+=$2} END {print s}'`
      → **6** (current: 6 — *preservation*).
- [ ] All ten tests pass. **Note the `--`**: `cargo test` takes a single
      `TESTNAME`, so several filters go after the separator or cargo exits
      `error: unexpected argument`.

      ```sh
      cargo test --lib -- \
        classify_prompt_matches_the_ssh_host_key_question \
        classify_prompt_matches_an_ssh_password_prompt \
        classify_prompt_matches_an_ssh_passphrase_prompt \
        host_key_prompt_is_not_a_fingerprint_prompt \
        a_real_biometric_prompt_still_classifies_as_fingerprint \
        the_sentinel_still_wins_over_ssh_text \
        su_password_prompt_classification_is_pinned \
        stale_ssh_prompt_text_from_an_earlier_command_does_not_classify \
        an_ssh_password_prompt_arms_the_scrub \
        the_host_key_question_does_not_arm_the_scrub
      ```

      → **10 passed** (current: `0 passed; 0 failed; 1957 filtered out`).
- [ ] Both mutations in § Mutations were applied, run, and their observed
      results recorded in the Update Log.
- [ ] The § End-to-end verification block was run verbatim, its output pasted
      as the **first fenced block** of an entry titled
      `### Update — <date> (end-to-end verification)`, and the diff snippet
      printed the literal `PASTE MATCH` into that entry. **Re-capture this
      block as the last thing you do before committing** — if any code changes
      after the capture, the evidence is stale and the criterion is unmet.
- [ ] The § Live verification block was run and its output pasted into the same
      entry, or reported unmet with the reason.

## Mutations

Apply each with the `patch` tool (the executor contract forbids `sed -i`,
`perl -i`, `> file` and `rm -rf`), run it, restore, and report what you saw.
**Run budget: one run each.**

**Mutation A — broaden the fingerprint matcher.** Add `|| output.contains("fingerprint")`
to `is_fingerprint_prompt`. Run **once**:
`cargo test --lib host_key_prompt_is_not_a_fingerprint_prompt`. It must go
**red** — this is the § Current state 3 hazard, and the test exists to catch
exactly this edit.

**Mutation B — let ssh text outrank the sentinel.** Move the ssh password match
**above** the sentinel check in `classify_prompt`. Run **once**:
`cargo test --lib the_sentinel_still_wins_over_ssh_text`. It must go **red**,
proving the ordering is pinned rather than incidental.

## Test plan

Ten tests, 1951 → 1961. No new `#[ignore]`: the live check in Task 6 is run by
hand from the § Live verification block, not added as an ignored test — row
04d already carries the one live ssh test this milestone needs as a harness.

## End-to-end verification

```sh
cd /home/matt/src/daemoneye
{
  for i in 1 2 3; do
    printf 'lib%d=%s\n' "$i" "$(cargo test --lib 2>&1 | grep -E '^test result' | tail -1 | grep -oE 'ok|FAILED')"
  done
  printf 'lib_counts=%s\n' "$(cargo test --lib 2>&1 | grep -E '^test result' | tail -1 | sed -E 's/.*ok\. ([0-9]+) passed; ([0-9]+) failed; ([0-9]+) ignored.*/\1p-\2f-\3i/')"
  printf 'new_variants=%s\n' "$(awk '/pub enum PromptKind/,/^\}/' src/daemon/utils/sudo.rs | grep -cE '^\s+(SshHostKey|SshPassword|SshPassphrase),')"
  printf 'fingerprint_not_broadened=%s\n' "$(awk '/pub fn is_fingerprint_prompt/,/^\}/' src/daemon/utils/sudo.rs | grep -c '"fingerprint"')"
  printf 'needles=%s\n' "$(awk '/const NEEDLES/,/\];/' src/shell/backend.rs | grep -c '"')"
  printf 'ignore_total=%s\n' "$(grep -rc '#\[ignore' --include=*.rs src/ | grep -v ':0' | awk -F: '{s+=$2} END {print s}')"
} > /tmp/e2e-05.txt
cat /tmp/e2e-05.txt
```

Expected: `lib1..3=ok`, `lib_counts=1961p-0f-6i`, `new_variants=3`,
`fingerprint_not_broadened=0`, `needles=5`, `ignore_total=6`.

Paste the file's contents into an Update Log entry titled
`### Update — <date> (end-to-end verification)` **as that entry's first fenced
block**, then run:

```sh
D=docs/dev/milestones/M21-host-model/phase-05-ssh-prompt-detection.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-05.txt
diff /tmp/pasted-05.txt /tmp/e2e-05.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

The literal `PASTE MATCH` line goes **into** the entry.

## Live verification

Run by hand; paste the output into the same Update Log entry, after the
mutations. This proves the classifier matches what a real ssh connection
actually emits, not what the unit tests feed it.

```sh
cd /home/matt/src/daemoneye
T=$(mktemp -d)
timeout 20 script -qc "ssh -tt -o UserKnownHostsFile=$T/kh -o StrictHostKeyChecking=ask -o ConnectTimeout=8 pinky true" "$T/hostkey.log" </dev/null >/dev/null 2>&1
timeout 20 script -qc "ssh -tt -o ConnectTimeout=8 pinky 'su -c true root'" "$T/supw.log" </dev/null >/dev/null 2>&1
printf 'hostkey_q=%s\n' "$(grep -c 'Are you sure you want to continue connecting' "$T/hostkey.log")"
printf 'su_password=%s\n' "$(grep -c 'Password:' "$T/supw.log")"
find "$T" -depth -delete
```

Both values must be `1` or more. (`find … -delete` rather than `rm -rf`: the
executor contract refuses `rm -rf`.)

## Authorizations

You may edit, and nothing else:

- `src/daemon/utils/sudo.rs` — Tasks 1, 2, 4.
- `src/daemon/executor/shell_exec.rs` — Task 3 only, the prompt closure.
- `src/shell/backend.rs` — **tests only** (Task 4's tests 9–10). Its
  `NEEDLES` list is a preservation criterion; do not edit it.
- This phase doc's Update Log.

## Out of scope

- **The interactive host-key accept/reject flow.** This row fails fast on an
  unknown host key; turning that into a user-facing trust decision is **row
  06**'s `new_host` classification.
- **`looks_like_secret_prompt`'s needle list** — § Current state 6 measured
  that it already covers password and passphrase and correctly ignores the
  host-key question. A change there is a scope violation with its own
  criterion.
- **Remote sudo prompting.** `pinky` is NOPASSWD (§ Current state 4), so this
  row neither verifies nor claims it. Row 09 owns that question.
- **`read_file` / `edit_file { host? }`** — row 07.
- **Retry policy for a wrong ssh password.** `MAX_SUDO_RETRIES` governs the
  sudo path; whether an ssh password gets the same allowance is a policy
  question this row does not settle — one attempt, then `Permission denied`
  ends it, is fine here.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-09-23 00:20 (end-to-end verification)

Captured mechanically against the tree with the ten new tests in place
(`lib_counts=1961p-0f-6i`, i.e. 1951 + 10). This first fenced block is the
`cat /tmp/e2e-05.txt` output of the § End-to-end verification block, run
verbatim (the block itself is pasted below, after it):

```text
lib1=ok
lib2=ok
lib3=ok
lib_counts=1961p-0f-6i
new_variants=3
fingerprint_not_broadened=0
needles=5
ignore_total=6
```

Every expected value matches: `lib1..3=ok`, `lib_counts=1961p-0f-6i`,
`new_variants=3`, `fingerprint_not_broadened=0`, `needles=5`, `ignore_total=6`.

The verbatim § End-to-end verification block that produced it:

```sh
cd /home/matt/src/daemoneye
{
  for i in 1 2 3; do
    printf 'lib%d=%s\n' "$i" "$(cargo test --lib 2>&1 | grep -E '^test result' | tail -1 | grep -oE 'ok|FAILED')"
  done
  printf 'lib_counts=%s\n' "$(cargo test --lib 2>&1 | grep -E '^test result' | tail -1 | sed -E 's/.*ok\. ([0-9]+) passed; ([0-9]+) failed; ([0-9]+) ignored.*/\1p-\2f-\3i/')"
  printf 'new_variants=%s\n' "$(awk '/pub enum PromptKind/,/^\}/' src/daemon/utils/sudo.rs | grep -cE '^\s+(SshHostKey|SshPassword|SshPassphrase),')"
  printf 'fingerprint_not_broadened=%s\n' "$(awk '/pub fn is_fingerprint_prompt/,/^\}/' src/daemon/utils/sudo.rs | grep -c '"fingerprint"')"
  printf 'needles=%s\n' "$(awk '/const NEEDLES/,/\];/' src/shell/backend.rs | grep -c '"')"
  printf 'ignore_total=%s\n' "$(grep -rc '#\[ignore' --include=*.rs src/ | grep -v ':0' | awk -F: '{s+=$2} END {print s}')"
} > /tmp/e2e-05.txt
cat /tmp/e2e-05.txt
```

**Mutations** (each applied with the `patch` tool, run once, restored; both
restores verified green):

- **Mutation A** — `|| output.contains("fingerprint")` added to
  `is_fingerprint_prompt`; `cargo test --lib
  host_key_prompt_is_not_a_fingerprint_prompt` went **red**:
  `test result: FAILED. 0 passed; 1 failed` — the host-key question
  misclassifies as `Fingerprint`, the § Current state 3 hazard reproduced by
  edit. Restored; green again.
- **Mutation B** — the ssh password match moved above the sentinel check in
  `classify_prompt`; `cargo test --lib the_sentinel_still_wins_over_ssh_text`
  went **red**: `test result: FAILED. 0 passed; 1 failed` — a chunk with both
  the sentinel and `'s password:` classified `SshPassword` instead of
  `SudoPassword`, proving the ordering is pinned rather than incidental.
  Restored; green again.

**Live verification** (run by hand against `pinky`, per § Live verification —
`su` over the ssh PTY rather than remote sudo, because `pinky` is NOPASSWD,
§ Current state 4):

```text
hostkey_q=1
su_password=1
```

Both are ≥ 1: the classifier's fragments are what a real ssh connection emits
on this host — the unknown-host-key question and `su`'s `Password:` prompt
were both captured live through `script -qc` with the same `-tt` ssh the
daemon spawns.

### Update — 2026-09-23 00:10 (progress, executor)

Resumed from the escalation. Task 3 is now complete and building: the
over-deref in `credential_round_trip` is `&mut *tx` (→ `tx` after the
parameter is already `&mut W`), the shared transports went back to the
pre-existing `Arc<tokio::sync::Mutex<&mut W>>` shape locked with
`.lock().await` and used as `&mut **guard`, and the dead `sentinel` /
`plain_cmd` fields came off `SudoPromptArgs` (the classifier re-derives the
sentinel from the PTY counter). `index_command_output`'s doc comment, which a
prior run's diff had silently dropped, is restored. The three ssh arms are
routed: password/passphrase reuse the sudo wire round-trip through the shared
`credential_round_trip` helper with kind-specific wording, host key gives up
naming the host, and `Permission denied` gives up naming the host.

Tasks 4, 5 first halves are in. The ten pinned tests all pass:

- 8 in `src/daemon/utils/sudo.rs` — the three positive matches, the
  `host_key_prompt_is_not_a_fingerprint_prompt` negative, the biometric
  preservation test, the sentinel-over-ssh-text ordering test, the `su`
  `Password:` decision (not classified; reasoning in the test and in the
  classifier's comment), and the stale-echo pin (the text-only matcher cannot
  distinguish an echo from a live prompt — pinned, per the spec's escape
  hatch, with the structural guard named).
- 2 in `src/shell/backend.rs` — the password/passphrase scrub arming and the
  host-key question not arming the scrub.

Mutation A and Mutation B were both applied and observed red, then restored:
A (`|| output.contains("fingerprint")` in `is_fingerprint_prompt`) →
`host_key_prompt_is_not_a_fingerprint_prompt` FAILED, `1 failed; 0 passed` —
the § Current state 3 hazard reproduced by edit. B (ssh password match moved
above the sentinel check) → `the_sentinel_still_wins_over_ssh_text` FAILED,
`1 failed; 0 passed` — the ordering is pinned, not incidental. Both restores
verified green.

The task-10 filter runs `10 passed; 0 failed`, `--lib` is
`1961 passed; 0 failed; 6 ignored`, and the other targets are unmoved
(doc_truth 14, integration 32 (2 ignored), isolation 9 (1 ignored),
bug_tracker 6). `cargo build`, `cargo fmt --all -- --check`, and
`cargo clippy --all-targets --all-features -- -D warnings` are all clean.

The live § Live verification block was also captured by hand (hostkey_q=1,
su_password=1); its paste goes into the end-to-end entry below with the
mutations.

### Update — 2026-09-22 22:55 (started, executor)

Status flipped `todo → in-progress` (phase doc + milestone README row). Pre-flight
at HEAD `f6e11915` (the tree is identical to the drafted `61e519e9` on all source
files — the extra commit is the PE's row-09 sudoers note in `docs/dev/NEXT.md`
only): `cargo test --lib` → **1951 passed / 0 failed / 6 ignored**, matching the
phase doc's measured baseline. All four § Current state strings re-confirmed
against the code before editing (`is_fingerprint_prompt`'s three biometric
sentences only, `looks_like_secret_prompt`'s five needles, `classify_prompt`'s
fingerprint-then-sentinel order). Executor: claude (M21 phase-05
ssh-prompt-detection). Implementing Tasks 1–6 in order.

### Update — 2026-09-22 16:05 (escalation)

**Chosen lever:** resume
**Rationale:** the spec held — Tasks 1 and 2 landed clean and well-reasoned
from it — and the run died on one mechanical wall in Task 3's plumbing, which
is the "most of the phase done, hit one wall" shape rather than a spec gap a
re-dispatch would fix.

**What is already done and must not be redone.** `PromptKind` carries all
three new variants, `classify_ssh_prompt` and `is_ssh_permission_denied` are
written, the ordering (fingerprint → sentinel → ssh text) is right, and the
`su`/bare-`Password:` decision is made and documented with its reasoning.
`ssh_credential_prompt` and the shared `credential_round_trip<W, R>` helper
are a **good** factoring — keeping sudo and ssh on one wire shape is exactly
right. Keep all of it.

**The wall, and it is two mechanical defects, not a design problem.** The
build fails with 13 errors, all one class:

- `shell_exec.rs:467` — `send_response_split(&mut **tx, …)` over-dereferences.
  `tx` is already `&mut W`, so `**tx` is `E0614: type W cannot be
  dereferenced`. It wants `&mut *tx`.
- `shell_exec.rs:523` / `:525` — the shared state was rewrapped as
  `std::sync::Arc::new(std::sync::Mutex::new(Some(tx)))`. A
  `std::sync::MutexGuard<Option<&mut W>>` is not `AsyncWrite`, which is the
  other 12 errors. **The pre-existing pattern at these lines was**
  `std::sync::Arc::new(tokio::sync::Mutex::new(tx))` — an async mutex, locked
  with `.lock().await` and used as `&mut **guard`. Going back to that shape is
  what makes the helper compile; the `Option<…>` and the `UnpoisonExt` import
  are not needed.

**Still to do:** Task 4's ten tests — none of the pinned names exists yet —
then Tasks 5 and 6.

**One acceptance criterion was wrong and is now fixed — it is not your bug.**
`fingerprint_not_broadened` grepped the whole of `sudo.rs`, so it read `1`
against your tree purely because you wrote the word `"fingerprint"` in a doc
comment explaining the hazard. `is_fingerprint_prompt` itself is byte-identical
to `HEAD`, which is correct. The criterion is now scoped to the function and
reads `0`. Do not delete that comment to satisfy the old form.

### Update — 2026-09-23 00:48 (complete, server-authored)

**Summary:** </think>Everything is done:

- Task 1: PromptKind has SshHostKey/SshPassword/SshPassphrase with safety doc — done (prior run).
- Task 2: classify_ssh_prompt + is_ssh_permission_denied — done (prior run), verified.
- Task 3: plumbing fixed — done. Also cleaned up dead fields, restored the dropped doc comment, verified everything builds.
- Task 4: ten tests — done, all pass.
- Task 5: E2E captured, pasted, PASTE MATCH — done.
- Task 6: live check run by hand (hostkey_q=1, su_password=1) — done.
- Mutations A and B: applied, observed red, restored — done.
- cargo fmt/build/clippy/test — all clean.
- Committed as c6fda868, tree clean.
- Status left in-progress; server authors the completion entry.

One thing to double check: the phase doc's `grep -rc '#\[ignore'` total is 6, matches. And the `su_password`/hostkey evidence is pasted in the same E2E entry. Good.

Also the E2E entry's first fenced block is the output and the diff check passed with the verifier's own awk command.

Final message: Summary + Notes for review, 2-6 sentences.</think>
**Summary**

M21 phase-05 is complete on the committed tree (`c6fda868`, working tree clean; phase status left `in-progress` for the server's completion flip). Tasks 3–6 landed: the Task 3 plumbing now compiles — `credential_round_trip` takes `tx` directly (the `&mut **` over-deref removed) and the shared transports are back on the pre-existing `Arc<tokio::sync::Mutex<&mut W>>` shape locked with `.lock().await` and used as `&mut **guard` — with the ssh password/passphrase arms reusing the sudo `CredentialPrompt` wire round-trip through that helper (kind-specific wording: "ssh login password required" / "ssh key passphrase required"), host-key and `Permission denied` failing fast with give-ups that name the host. I also cleaned the plumbing the resume diff left ragged: dropped `SudoPromptArgs`'s dead `sentinel`/`plain_cmd` fields, restored `index_command_output`'s doc comment (silently dropped by the prior diff), and removed the unneeded `UnpoisonExt` import. All ten pinned tests pass (8 in `sudo.rs`, 2 in `backend.rs`), including the `su` decision (bare `Password:` deliberately **not** classified, reasoning pinned in the test) and the stale-echo limitation pinned rather than hidden, per the spec's escape hatch.

**Notes for review**

- **Deviations, both reportable adaptations:** (1) the give-up wording names `host <name>` unconditionally — `record_host.as_str()` is never empty (`HostId::as_str` returns `"local"`), so the `is_empty` fallback the prior diff sketched was dead code and I documented the invariant in the parameter's doc comment instead; (2) `ssh_credential_prompt`'s catch-all became an explicit match over the three non-credential kinds rather than `_`, so a future `PromptKind` variant forces this site to state its wording.
- **Mutations verified red, not assumed:** Mutation A → `host_key_prompt_is_not_a_fingerprint_prompt` FAILED (`0 passed; 1 failed`); Mutation B → `the_sentinel_still_wins_over_ssh_text` FAILED (`Some(SshPassword)` vs `Some(SudoPassword)`). Both restored and re-verified green.
- **E2E:** the § End-to-end verification block was run verbatim as the last pre-commit capture, pasted as the first fenced block of `### Update — 2026-09-23 00:20 (end-to-end verification)`, and the verifier's own extraction/diff prints `PASTE MATCH` against the *committed* tree (`lib1..3=ok`, `lib_counts=1961p-0f-6i`, `new_variants=3`, `fingerprint_not_broadened=0`, `needles=5`, `ignore_total=6`). Other targets unmoved: doc_truth 14, integration 32 (2 ignored), isolation 9 (1 ignored), bug_tracker 6.
- **Live:** the § Live verification block ran by hand against `pinky` — `hostkey_q=1`, `su_password=1` — pasted into the same entry after the mutation records (via `su`, since pinky's sudo is NOPASSWD).
- `cargo fmt --all` (run before staging), `cargo build`, `cargo clippy --all-targets --all-features -- -D warnings` all clean; no new `#[ignore]`, no TODOs, no debug calls.

**Executor:** local-inference-lab/GLM-5.3-Flash-NVFP4-Spark

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.06s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.08s


TEST
 header_status_uses_first_occurrence_only ... ok
test open_bug_on_done_phase_is_a_finding ... ok
test open_bug_on_in_progress_phase_is_clean ... ok
test repository_bug_tracker_is_consistent ... ok

test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 14 tests
test approval_gated_tools_all_exist ... ok
test claude_md_tools_table_counts_are_accurate ... ok
test claude_md_tools_table_matches_the_code ... ok
test readme_tools_counts_are_accurate ... ok
test readme_approval_markers_match_the_gated_tools ... ok
test e2e_prompt_asset_tool_hygiene ... ok
test readme_tools_tables_match_the_code ... ok
test docs_document_the_reindex_command ... ok
test prompt_assets_name_no_retired_tools ... ok
test e2e_seeded_asset_tool_hygiene ... ok
test docs_do_not_carry_retired_index_claims ... ok
test seeded_config_template_has_no_phantom_keys ... ok
test seeded_assets_name_no_retired_tools ... ok
test seeded_config_template_documents_every_config_field ... ok

test result: ok. 14 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 34 tests
test daemon_ping_status_loop ... ignored
test cancel_request_roundtrip ... ok
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g4_briefing_injection_block_format ... ok
test g5_child_inherits_depth_and_parent ... ok
test g5_depth_limit_enforced ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ipc_ask_round_trip ... ok
test ipc_tool_call_response_round_trip ... ok
test ipc_session_info_round_trip ... ok
test ghost_config_parsing ... ok
test minimal_config_parsing ... ok
test resume_run_request_roundtrip ... ok
test window_switch_does_not_corrupt_chat ... ignored
test schedule_store_persistence ... ok
test config_pricing_round_trip ... ok
test g4_briefing_masking_applied ... ok
test event_log_append_read ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test event_log_entry_format ... ok
test g4_briefing_injects_on_next_run ... ok
test g4_briefing_read_and_clear ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
test session_index_persistence ... ok
test session_jsonl_round_trip ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_below_threshold_discarded ... ok
test webhook_alert_no_severity_passes_gate ... ok
test webhook_alert_unrankable_severity_passes_gate ... ok
test webhook_alert_to_event_log ... ok

test result: ok. 32 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.04s


running 10 tests
test webhook_ghost_e2e_http ... ignored
test held_port_cannot_be_rebound ... ok
test webhook_ports_differ_between_environments ... ok
test stub_returns_canned_response_via_make_client ... ok
test webhook_ghost_e2e_deterministic ... ok
test config_contains_webhook_and_stub_url ... ok
test hooks_land_on_private_server ... ok
test daemon_boots_in_throwaway_root ... ok
test default_server_unchanged ... ok
test daemon_webhook_returns_200 ... ok

test result: ok. 9 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.16s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

   Compiling daemoneye v1.0.0 (/home/matt/src/daemoneye)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 5.30s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-9810e08c3de1e001)
     Running unittests src/main.rs (target/debug/deps/daemoneye-54ad57cf3704bc40)
     Running tests/bug_tracker.rs (target/debug/deps/bug_tracker-d709c9b862b5691d)
     Running tests/doc_truth.rs (target/debug/deps/doc_truth-34cb94d3012a3d69)
     Running tests/integration.rs (target/debug/deps/integration-deb70358b520d0d5)
     Running tests/isolation.rs (target/debug/deps/isolation-45facff22bac60f5)
   Doc-tests daemoneye

```

**Files changed:**

- `docs/dev/milestones/M21-host-model/phase-05-ssh-prompt-detection.md` — +112 -0
- `src/daemon/executor/shell_exec.rs` — +55 -62
- `src/daemon/utils/sudo.rs` — +118 -0
- `src/shell/backend.rs` — +28 -0

**Commit:** c6fda86844e61f2b114d9804864661fa2df0a7b0

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

### Review verdict — 2026-09-23

- **Verdict:** approved_after_1
- **Bounces:** none at review. One `hard_fail` → **resume** cycle:
  `VerifierFailurePersistent` (6 consecutive, `shell_exec.rs`) after 335 turns,
  resumed rather than re-dispatched because Tasks 1–2 had landed clean and the
  wall was two mechanical defects in Task 3's plumbing. The resume cleared all
  13 build errors and finished the phase.
- **Executor:** local-inference-lab/GLM-5.3-Flash-NVFP4-Spark
- **Scope deviations:** two, both accepted and disclosed.
  1. **A test name now contradicts its body, and that is this spec's fault.**
     `stale_ssh_prompt_text_from_an_earlier_command_does_not_classify` asserts
     that the echo **does** classify (`Some(SshPassphrase)`), pinning the
     text-only matcher's limitation. That is exactly what Task 4's escape
     hatch asked for — "pin the limitation in the test rather than deleting
     the test" — but Task 4 also said "names are pinned", so the executor
     correctly kept a name that the inverted semantics falsified. The body is
     scrupulously honest: its assert messages read "pinned limitation" and its
     comment states the real guard is structural (ssh text reaches the
     classifier only during connection establishment), not a property of
     `classify_prompt`. Renaming is a later tidy-up, not a defect in this work.
  2. **Disclosed cleanup beyond the task list** — dead `SudoPromptArgs`
     fields (`sentinel`, `plain_cmd`) dropped, `index_command_output`'s doc
     comment restored after the first run's diff silently dropped it, and the
     `UnpoisonExt` import removed. All three are consequences of repairing the
     first run's own damage; `clippy -D warnings` passes, so the dropped
     fields were genuinely dead.
- **Calibration:** **third architect-side spec defect in three phases, and the
  first two are the same class.** (a) `fingerprint_not_broadened` grepped the
  whole file and read non-zero on a *correct* tree, because the executor wrote
  `"fingerprint"` in a doc comment explaining the hazard — corrected at the
  escalation; the `event_host` criterion in 04d was the identical over-broad
  grep. **If a third over-broad grep ships, fold it:** *scope every structural
  grep to the construct it is about, never the file.* (b) "names are pinned"
  plus an escape hatch that can invert a test's meaning needs "…and you may
  rename a test whose meaning the escape hatch inverts."

**Review re-verification (independent, 2026-09-23):** all four gates re-run
clean. The full § End-to-end verification block was re-run from scratch and
reproduced the executor's capture **byte for byte** — `lib1..3=ok`,
`lib_counts=1961p-0f-6i` (exactly the pinned figure, no extra test),
`new_variants=3`, `fingerprint_not_broadened=0`, `needles=5`,
`ignore_total=6`. `PASTE MATCH` prints from the doc's own diff. `cargo test`
green on every target (lib 1961/0/6, doc_truth 14, integration 32,
isolation 9, bug_tracker 6). All ten pinned tests pass — eight in `sudo.rs`,
two in `backend.rs`.

**The § Live verification block was re-run by the reviewer against the real
`pinky`** and returned `hostkey_q=1  su_password=1`, so the classifier's
fragments are what a real ssh connection emits, not only what the unit tests
feed it.

**Mutation A re-applied by the reviewer** — adding `|| output.contains("fingerprint")`
to `is_fingerprint_prompt` turns `host_key_prompt_is_not_a_fingerprint_prompt`
red, so the measured § Current state 3 hazard is genuinely under test.

**The resume's repair is confirmed in the tree:** the transports are back on
`std::sync::Arc::new(tokio::sync::Mutex::new(tx))` (`shell_exec.rs:519/521`),
`UnpoisonExt` is gone (0 occurrences), and the four routing arms are wired —
`SshPassword`/`SshPassphrase` through the shared `credential_round_trip` with
kind-specific wording, `SshHostKey` and `Permission denied` failing fast with
give-ups that name the host. No production `unwrap`/`expect`/`panic!`, no
`unsafe`, no `TODO`/`dbg!`, and no new `#[ignore]` across 311 added lines; the
one new `#[allow(clippy::too_many_arguments)]` sits on the production function
that gained `host_name`, the same accepted pattern its neighbours carry.
