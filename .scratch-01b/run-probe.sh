#!/usr/bin/env bash
# Task 6 probe driver for phase-01b: start a real `daemoneye shell-host`
# under a SHORT HOME (mktemp -d /tmp/de.XXXX so the socket path does not
# exceed SUN_LEN), run the sudo scrub against real sudo, and grep the
# resulting cast file.
set -u

REPO=/home/matt/src/daemoneye
BIN=$REPO/target/debug/daemoneye
SCRATCH=$REPO/.scratch-01b
WORK=$(mktemp -d /tmp/de.XXXX)
export HOME="$WORK"
LOGDIR=/tmp/de-01b-logs
mkdir -p "$LOGDIR"

# Kill any leftover host from a previous run by executable name first.
for pid in $(ps -eo pid=,comm= | awk '$2=="daemoneye" {print $1}'); do
    kill -9 "$pid" 2>/dev/null || true
done
sleep 0.2

"$BIN" shell-host --id s0 --shell /bin/bash --rows 24 --cols 80 \
    > "$LOGDIR/host.txt" 2>&1 &
HOSTPID=$!
sleep 1.5

export SOCK_PATH="$WORK/.daemoneye/var/run/shells/s0.sock"
CAST_GLOB="$WORK/.daemoneye/var/log/shells/*.cast"
OUT=$LOGDIR/probe.json

python3 "$SCRATCH/probe.py" "$CAST_GLOB" "$OUT" > "$LOGDIR/probe-line.txt" 2> "$LOGDIR/probe-err.txt"
PYRC=$?

sleep 0.5
kill -9 "$HOSTPID" 2>/dev/null || true
wait "$HOSTPID" 2>/dev/null || true

echo "=== probe rc=$PYRC ==="
echo "=== probe result line ==="
cat "$LOGDIR/probe-line.txt"
echo "=== probe stderr ==="
cat "$LOGDIR/probe-err.txt"
echo "=== probe json ==="
cat "$OUT" 2>/dev/null
echo "=== host log (tail) ==="
tail -5 "$LOGDIR/host.txt"
echo "=== hostpid= $HOSTPID ==="
echo "=== home= $HOME ==="
