import json
import os
import socket
import time

SOCK_PATH = os.environ["SOCK_PATH"]
s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
s.connect(SOCK_PATH)
s.setblocking(False)
buf = b""


def recv_frames():
    global buf
    out = []
    while True:
        try:
            n = s.recv(65536)
        except BlockingIOError:
            return out
        if not n:
            return out
        buf += n


def chunks_contain(frames, needle):
    return any(
        f.get("type") == "Chunk"
        and needle in bytes(f["bytes"]).lower()
        for f in frames
    )


def send(o):
    s.sendall((json.dumps(o) + "\n").encode())


send({"type": "Subscribe"})
time.sleep(0.4)
recv_frames()

send({"type": "Input", "bytes": list(b"sudo -k; sudo true\n")})
for _ in range(200):
    frames = recv_frames()
    if chunks_contain(frames, b"password"):
        break
    time.sleep(0.03)
prompt_seen = True
time.sleep(0.05)

# Stop reading: leave the socket open but only pull frames slowly so the
# broadcast buffer does not fill. The point is minimal disturbance to the
# editor's timing.
send({"type": "Input", "bytes": list(b"mirror-a\nmirror-b\n")})
time.sleep(0.3)
recv_frames()
send({"type": "Input", "bytes": list(b"echo done-probe\n")})
time.sleep(0.5)
recv_frames()
print("prompt_seen=true", flush=True)
for _ in recv_frames():
    pass
