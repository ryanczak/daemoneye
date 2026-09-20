set -u
BIN=/home/matt/src/daemoneye/target/debug/daemoneye
for pid in $(ps -eo pid=,comm= | awk '$2=="daemoneye" {print $1}'); do kill -9 "$pid" 2>/dev/null; done
sleep 0.2
export HOME="$(mktemp -d /tmp/de.XXXX)"
echo "home=$HOME"
"$BIN" shell-host --id s0 --shell /bin/bash --rows 24 --cols 80 > /tmp/de01b-host.txt 2>&1 &
HOSTPID=$!
sleep 1.5
SOCK="$HOME/.daemoneye/var/run/shells/s0.sock"
SOCK_PATH="$SOCK" python3 - <<'PYEOF' > /tmp/de01b-interrogate.txt 2>&1
import json, os, socket, sys, time
sock_path = os.environ["SOCK_PATH"]
s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
s.connect(sock_path)
def req(o):
    s.sendall((json.dumps(o) + "\n").encode())
def recv_raw(t=3.0):
    s.settimeout(t)
    buf = b""
    try:
        while True:
            n = s.recv(65536)
            if not n:
                break
            buf += n
    except TimeoutError:
        pass
    return buf
req({"type": "Subscribe"})
print("subscribe reply:", recv_raw(1.0))
req({"type": "Input", "bytes": list(b"stty -a | head -1\n")})
print("stty reply:", recv_raw(2.0).decode("utf-8", "replace")[:400])
req({"type": "Input", "bytes": list(b"sudo -k; sudo true\n")})
print("after sudo cmd:", recv_raw(4.0).decode("utf-8", "replace")[:400])
req({"type": "Input", "bytes": list(b"echo pwtest-probe\n")})
print("after answer:", recv_raw(3.0).decode("utf-8", "replace")[:400])
PYEOF
cat /tmp/de01b-interrogate.txt
kill -9 "$HOSTPID" 2>/dev/null
wait 2>/dev/null
for f in "$HOME"/.daemoneye/var/log/shells/*.cast; do echo "== $f =="; grep -c 'pwtest-probe' "$f"; done
