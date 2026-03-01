# Signal Gateway — Concept (Shelved)

> **Status**: Shelved. Captured here for future reference.
> The interactive prompt problem described in §4 must be solved in the main
> application first (see `PTY_EXECUTOR_PLAN.md`) before Signal integration is
> practical.

---

## 1. Motivation

Allow a DaemonEye user to interact with the AI agent from a mobile device via
Signal messenger — sending queries, receiving responses, and approving tool
calls — without needing SSH or a terminal emulator.

---

## 2. Architecture

```
Mobile Signal app
      │  (E2E encrypted)
      ▼
Signal servers
      │
      ▼
signal-cli  (runs on daemon host)
      │  (JSON output / stdin)
      ▼
Signal Gateway process  (new daemoneye subcommand or sidecar)
      │  (Unix Domain Socket: /tmp/daemoneye.sock)
      ▼
daemoneye daemon
```

The gateway is the only new component. The daemon's existing IPC protocol
(`Request`/`Response` over the Unix socket) is unchanged — the gateway is just
a new client that speaks it.

### 2.1 Signal transport

**signal-cli** (https://github.com/AsamK/signal-cli) is the recommended
transport layer. It is a community-maintained Java CLI that implements the
Signal protocol and can run in JSON-RPC daemon mode (`signal-cli -u +NUMBER
jsonRpc`), emitting received messages as JSON on stdout and accepting send
commands on stdin.

A dedicated phone number is required. Options:
- A spare SIM card
- A VoIP number (JMP.chat, Twilio, etc.)
- A linked device registration against the user's own number (the user's mobile
  number becomes both user and bot number; messages to self are used as the
  command channel)

The linked-device approach has the best UX: no second number, and the user's
existing Signal contacts do not need to know a new number.

### 2.2 Message flow

| Direction | Message |
|---|---|
| User → Gateway | Query text (e.g. "what's eating all the memory on web01?") |
| Gateway → Daemon | `Request::Ask { query, session_id, ... }` |
| Daemon → Gateway | Stream of `Response::Token` + final `Response::Ok` |
| Gateway → User | Aggregated full response sent as one Signal message |
| Daemon → Gateway | `Response::ToolCallPrompt { id, command, background }` |
| Gateway → User | Approval request message (e.g. "Approve `ps aux`? Reply Y / N / A") |
| User → Gateway | "Y", "N", or "A" |
| Gateway → Daemon | `Request::ToolCallResponse { id, approved }` |

Because Signal messages are asynchronous, the gateway must hold pending
tool-call approval state (keyed by `id`) and match incoming user replies to
the correct pending request.

---

## 3. Gateway Design

### 3.1 Session management

The gateway maintains one `session_id` per Signal sender. The session is
reused across messages from the same number so conversation history is
preserved. A `/clear` message text resets the session.

### 3.2 Response aggregation

`Response::Token` fragments are streamed from the daemon. The gateway
accumulates them into a buffer and sends a single Signal message when
`Response::Ok` or a final `Response::SessionInfo` is received. Markdown
formatting is stripped before sending (Signal renders plain text).

### 3.3 Access control

Only a configurable allowlist of Signal numbers may send commands. The gateway
rejects all messages from unlisted numbers with no reply (to avoid enumeration).
The allowlist lives in `config.toml` under a `[signal]` section.

---

## 4. The Interactive Prompt Problem

This is the primary blocker for Signal integration.

Background commands that require interactive input (passwords, host-key
confirmations, yes/no prompts) cannot be handled safely over Signal:

- **Sudo passwords**: Accepting a password via Signal is possible but
  operationally risky (message retention, notification on other devices, etc.).
  The recommendation is to **deny all sudo commands from the Signal gateway**.
  Foreground sudo (injected into the tmux pane) is not meaningful when the
  user is on their phone.

- **SSH host-key confirmations** (`Are you sure you want to continue connecting?`):
  The current pipe-based background executor does not have a PTY, so `ssh`
  will fail with `pseudo-terminal will not be allocated`. Even with a PTY, the
  gateway would need to relay the confirmation to Signal and wait for a reply —
  feasible but adds round-trip latency.

- **`su`, database CLI auth, GPG passphrases**: Same class of problem. The
  command hangs silently and eventually times out.

**Resolution**: The gateway should use the PTY-based background executor
(see `PTY_EXECUTOR_PLAN.md`). The PTY detects prompt patterns in the
subprocess output and emits structured `CredentialPrompt` / `ConfirmationPrompt`
IPC events. The gateway forwards these to the user via Signal and injects the
reply into the PTY. Sudo-specific commands can be individually denied via
gateway policy while other interactive flows remain supported.

Until `PTY_EXECUTOR_PLAN.md` is implemented, the gateway should refuse any
command the AI marks `background=true` that could plausibly prompt for input,
using a conservative pattern match on the command string.

---

## 5. Security Considerations

| Concern | Mitigation |
|---|---|
| Unauthorised access | Sender allowlist in config; gateway ignores all unlisted numbers |
| Credential exposure via Signal | Deny sudo from Signal gateway; for other credential prompts, warn user that credential will transit Signal infra |
| Signal protocol breakage | signal-cli is community-maintained; API can break on Signal app updates. Pin signal-cli version; test after Signal updates |
| Command injection via crafted Signal messages | Gateway passes query as a string to `Request::Ask`; the daemon's AI layer generates the command — the AI is not a shell. Tool calls still go through the approval gate |
| Replay / message reordering | Use Signal's built-in message sequencing; treat each message as independent (no implicit state outside of session_id) |

---

## 6. Implementation Phases (when resumed)

1. **Phase 1 — Read-only queries**: Gateway supports plain text queries and
   returns responses. No tool calls. Validates the signal-cli JSON-RPC
   integration and session management.

2. **Phase 2 — Tool call approval**: Add the approval request/response flow.
   Non-interactive background commands only. Deny interactive commands.

3. **Phase 3 — PTY integration**: Use the PTY-based executor for background
   commands. Support credential and confirmation prompts relayed via Signal,
   with a per-prompt timeout.

4. **Phase 4 — Autonomous mode**: When the daemon supports autonomous runbook
   execution, the gateway can trigger it with a `/runbook <name>` command and
   monitor progress.

---

## 7. Prerequisites

- `PTY_EXECUTOR_PLAN.md` implemented in the main daemon.
- signal-cli available on the daemon host.
- A registered Signal number (SIM, VoIP, or linked device).
- `[signal]` config section added to `config.toml`.
