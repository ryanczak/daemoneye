import json
import os
import socket
import time
from contextlib import suppress

SOCK = os.environ["SOCK_PATH"]
s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
s.connect(SOCK)
s.setblocking(False)


def send(o):
    s.sendall((json.dumps(o) + "\n").encode())


def drain(t=0.5):
    time.sleep(t)
    buf = b""
    with suppress(BlockingIOError, OSError):
        while True:
            n = s.recv(65536)
            if not n:
                break
            buf += n
    return buf


send({"type": "Subscribe"})
drain(0.5)
send({"type": "Input", "bytes": list(b"printf '[sudo] password for x: '\n")})
drain(0.8)
send({"type": "Input", "bytes": list(b"minimal-scrub-check\n")})
time.sleep(0.2)
drain(0.6)
