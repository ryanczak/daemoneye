set -u
BIN=/home/matt/src/daemoneye/target/debug/daemoneye
SCRATCH=/home/matt/src/daemoneye/.scratch-01b
export HOME="$(mktemp -d /tmp/de.XXXX)"
for pid in $(ps -eo pid=,comm= | awk '$2=="daemoneye" {print $1}'); do kill -9 "$pid" 2>/dev/null; done
sleep 0.2
"$BIN" shell-host --id s0 --shell /bin/bash --rows 24 --cols 80 > /tmp/de01b-host.txt 2>&1 &
HOSTPID=$!
sleep 1.5
export SOCK_PATH="$HOME/.daemoneye/var/run/shells/s0.sock"
python3 "$SCRATCH/probe.py" "$HOME"/.daemoneye/var/log/shells/*.cast /tmp/de01b-probe.json
PYRC=$?
sleep 0.5
kill -9 "$HOSTPID" 2>/dev/null
wait 2>/dev/null
echo "rc=$PYRC"
cat /tmp/de01b-probe.json
