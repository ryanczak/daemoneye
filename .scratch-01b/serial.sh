set -u
BIN=/home/matt/src/daemoneye/target/debug/daemoneye
for pid in $(ps -eo pid=,comm= | awk '$2=="daemoneye" {print $1}'); do kill -9 "$pid" 2>/dev/null; done
sleep 0.2
export HOME="$(mktemp -d /tmp/de.XXXX)"
"$BIN" shell-host --id s0 --shell /bin/bash --rows 24 --cols 80 > /tmp/de01b-host.txt 2>&1 &
HOSTPID=$!
sleep 1.5
export SOCK_PATH="$HOME/.daemoneye/var/run/shells/s0.sock"
python3 .scratch-01b/serial_probe.py > /tmp/de01b-serial.txt 2>&1
kill -9 "$HOSTPID" 2>/dev/null
wait 2>/dev/null
echo "=== i events ==="
for f in "$HOME"/.daemoneye/var/log/shells/*.cast; do
  python3 - "$f" <<'PYEOF'
import json, sys

path = sys.argv[1]
lines = open(path).read().splitlines()[1:]
for line in lines:
    try:
        v = json.loads(line)
    except Exception:
        continue
    if isinstance(v, list) and v[1] == "i":
        print(line)
PYEOF
done
echo "=== serial stdout ==="
cat /tmp/de01b-serial.txt
