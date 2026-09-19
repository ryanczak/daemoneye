#!/bin/sh
# Phase 01b Task 6 — drive a real `daemoneye shell-host` over its socket and
# check the cast log for the scrub. No real credential: the password string
# never reaches a real sudo, only a bash `read`.
#
# Protocol: one `Input` frame per write, sending `{"type":"Input","bytes":[..]}`.
# The wire format is pinned by `proto.rs`'s `proto_request_frames_match...`.
set -u

BINARY=./target/debug/daemoneye

HQ=$(mktemp -d /tmp/de.XXXX) || exit 1
export HOME="$HQ"
mkdir -p "$HQ/.daemoneye/var/run/shells" "$HQ/.daemoneye/var/log/shells"

"$BINARY" shell-host --id s0 --shell /bin/bash --rows 24 --cols 80 &
HOST_PID=$!

SOCK="$HQ/.daemoneye/var/run/shells/s0.sock"
sleep 1

python3 - "$SOCK" <<'PY' > /tmp/de-probe-out.txt
import json, socket, sys, time

def byte_frame(payload: bytes) -> str:
    return json.dumps({"type": "Input", "bytes": [b for b in payload]}) + "\n"

def send(payload: bytes):
    sock.sendall(byte_frame(payload).encode())

def send_subscribe():
    sock.sendall(b'{"type":"Subscribe"}\n')

def read_chunk(deadline: float) -> bytes:
    """Read one line; if it is a Chunk, return its bytes."""
    global buf
    while time.time() < deadline:
        if b"\n" not in buf:
            sock.settimeout(max(0.05, deadline - time.time()))
            try:
                got = sock.recv(65536)
            except socket.timeout:
                return b""
            if not got:
                return b""
            buf += got
            continue
        line, _, buf = buf.partition(b"\n")
        if not line:
            continue
        try:
            frame = json.loads(line.decode())
        except json.JSONDecodeError:
            continue
        if isinstance(frame, dict) and frame.get("type") == "Chunk":
            return bytes(frame.get("bytes", []))
        # Ok / Error / others — keep reading.
    return b""

buf = b""
sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
sock.settimeout(20)
sock.connect(sys.argv[1])

send_subscribe()

cmd = "echo '[de-sudo-prompt-0] Password:'; read -r pw\n"
send(cmd.encode())

prompt_armed = False
deadline = time.time() + 10
while time.time() < deadline:
    chunk = read_chunk(deadline)
    if not chunk:
        continue
    text = chunk.decode("utf-8", "replace")
    if "de-sudo-prompt-0" in text or "Password:" in text:
        prompt_armed = True
        break

print("prompt_armed=" + ("true" if prompt_armed else "false"))

# The "credential". Never a real one.
send(b"hunter2-secret-e2e\n")
time.sleep(0.5)

# The follow-up, which must appear in clear again.
send(b"echo followup-clear-marker\n")
time.sleep(2.0)
PY

CAST=$(ls /tmp/de.*/.daemoneye/var/log/shells/s0-*.cast 2>/dev/null | tail -1)
echo "CAST_FILE: $CAST"
[ -f "$CAST" ] || { echo "NO_CAST: no cast file was written"; }
echo "--- cast contents ---"
cat "$CAST" 2>/dev/null
echo "--- end cast ---"

printf 'secret_in_cast=%s marker_in_cast=%s followup_in_clear=%s\n' \
  "$(if grep -q 'hunter2-secret-e2e' "$CAST" 2>/dev/null; then echo true; else echo false; fi)" \
  "$(if grep -q '\[redacted\]' "$CAST" 2>/dev/null; then echo true; else echo false; fi)" \
  "$(if grep -q 'followup-clear-marker' "$CAST" 2>/dev/null; then echo true; else echo false; fi)"

kill "$HOST_PID" 2>/dev/null
wait "$HOST_PID" 2>/dev/null
# keep the temp HOME for inspection; removal is the caller's job.
echo KEEP_DIR=$HQ