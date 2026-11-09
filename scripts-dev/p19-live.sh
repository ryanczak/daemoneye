#!/usr/bin/env bash
# Phase-19 live check: a release daemon under a throwaway HOME, four probes,
# transcript captured to the file given as $1. Ends the daemon by its pid.
set -u
TRANSCRIPT="${1:-/tmp/p19live/p19-live.txt}"
BIN=/home/matt/src/daemoneye/target/release/daemoneye
command -v tmux >/dev/null || { echo "SKIP: tmux unavailable"; exit 3; }

: > "$TRANSCRIPT"
say() { printf '%s\n' "$*" | tee -a "$TRANSCRIPT" >&2; }

ROOT=$(mktemp -d /tmp/p19live/root.XXXXXX)
export HOME="$ROOT"
export TMUX_TMPDIR="$ROOT"
unset TMUX TMUX_PANE

say "== throwaway root: $ROOT"

say "== daemoneye setup"
"$BIN" setup >> "$TRANSCRIPT" 2>&1 || { say "setup FAILED"; cat "$TRANSCRIPT"; exit 1; }

ETC="$HOME/.daemoneye/etc"
mkdir -p "$ETC"
cat > "$ETC/config.toml" <<'TOML'
[models.default]
provider = "openai"
api_key  = "test-key-not-a-real-credential"
model    = "claude-sonnet-4-6"
base_url = "http://127.0.0.1:1"

[limits]
total_tool_calls_per_turn = 5

[sandbox]
enabled = false
TOML

say "== daemoneye daemon (starting)"
"$BIN" daemon --session p19-live >> "$TRANSCRIPT" 2>&1 &
DAEMON_PID=$!
say "== daemon pid recorded: $DAEMON_PID"
echo "$DAEMON_PID" > /tmp/p19live/daemon.pid

READY=no
for i in $(seq 1 90); do
  if "$BIN" ping >/dev/null 2>&1; then READY=yes; break; fi
  sleep 0.5
done
say "== ping ready: $READY (after $i polls)"
if [ "$READY" != yes ]; then
  say "== tail daemon.log:"
  tail -30 "$HOME/.daemoneye/var/log/daemon.log" 2>/dev/null | tee -a "$TRANSCRIPT"
  kill "$DAEMON_PID" 2>/dev/null
  exit 1
fi

emit() { say "== \$ $*"; "$@" >> "$TRANSCRIPT" 2>&1; say "   exit=$?"; }

say "== probe 1: changed [limits] key"
sed -i 's/total_tool_calls_per_turn = 5/total_tool_calls_per_turn = 9/' "$ETC/config.toml"
emit "$BIN" reload

say "== probe 2: changed [sandbox] enabled"
sed -i 's/^enabled = false/enabled = true/' "$ETC/config.toml"
emit "$BIN" reload

say "== probe 3: broken file"
printf '[limits\ntotal_tool_calls_per_turn = 9\n' > "$ETC/config.toml"
emit "$BIN" reload

say "== probe 4: status still answers"
emit "$BIN" status

say "== ending daemon pid $DAEMON_PID (recorded pid)"
kill "$DAEMON_PID" 2>/dev/null
wait "$DAEMON_PID" 2>/dev/null
say "== daemon exit status: $?"

say "== done; transcript at $TRANSCRIPT"
