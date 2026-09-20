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


def drain_replies(frames):
    return [f for f in frames if f.get("type") != "Chunk"]


def send_and_wait(o, quiet=0.35):
    s.sendall((json.dumps(o) + "\n").encode())
    s.setblocking(False)
    deadline = time.time() + quiet
    replies = []
    while time.time() < deadline:
        time.sleep(0.03)
        replies.extend(drain_replies(recv_frames()))
    return replies


print("subscribe reply:", send_and_wait({"type": "Subscribe"}), flush=True)


def input_frame(text):
    return {"type": "Input", "bytes": list(text.encode())}


for label in ["as-1", "as-2", "as-3", "as-4"]:
    reply = send_and_wait(input_frame(f"echo {label}\n"))
    print(f"send {label}: replies={reply}", flush=True)
time.sleep(1.0)
for _ in recv_frames():
    pass
