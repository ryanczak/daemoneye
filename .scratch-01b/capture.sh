#!/usr/bin/env bash
# Phase-01b Task 6 capture (bounce round): drive a REAL `daemoneye
# shell-host` (the binary `cargo build` produces) into REAL `sudo`'s
# password prompt, answer it with a throwaway string, then grep the
# WHOLE cast file. All four booleans are whole-file greps.
#
# Outputs the captured line to stdout (also left at /tmp/e2e-01b.txt), and
# diagnostics to /tmp/e2e-01b-err.txt.
set -u
BIN=/home/matt/src/daemoneye/target/debug/daemoneye
SCRATCH=/home/matt/src/daemoneye/.scratch-01b
for pid in $(ps -eo pid=,comm= | awk '$2=="daemoneye" {print $1}'); do kill -9 "$pid" 2>/dev/null; done
sleep 0.2
export HOME="$(mktemp -d /tmp/de.XXXX)"          # short HOME — SUN_LEN
"$BIN" shell-host --id s0 --shell /bin/bash --rows 24 --cols 80 \
    > /tmp/de-01b/host.txt 2>&1 &
HOSTPID=$!
sleep 1.5
export SOCK_PATH="$HOME/.daemoneye/var/run/shells/s0.sock"
python3 "$SCRATCH/probe.py" \
    "$HOME"/.daemoneye/var/log/shells/s0-*.cast \
    /tmp/e2e-01b.json > /tmp/e2e-01b.txt 2> /tmp/e2e-01b-err.txt
PYRC=$?
sleep 0.5
kill -9 "$HOSTPID" 2>/dev/null
wait 2>/dev/null
echo "pyrc=$PYRC" >> /tmp/e2e-01b-err.txt
cat /tmp/e2e-01b.txt
