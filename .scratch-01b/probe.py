#!/usr/bin/env python3
"""Task 6 probe (phase-01b): drive a real `daemoneye shell-host` into
REAL `sudo`'s password prompt, answer it with a throwaway string, then
verify the cast file.

Prints exactly one result line:
  prompt_armed=… secret_in_cast=… marker_in_cast=… followup_in_clear=…
where all four are greps over the WHOLE cast file.

The throwaway answer is always the constant PROMPT — never a real
credential.
"""
import fcntl
import glob
import json
import os
import socket
import sys
import time

PROMPT = "de01b-throwaway-secret-probe"
FOLLOWUP = "de01b-followup-in-clear"


def connect(sock_path: str, timeout: float = 15.0):
    t0 = time.time()
    while time.time() - t0 < timeout:
        try:
            s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
            s.connect(sock_path)
            return s
        except (FileNotFoundError, ConnectionRefusedError):
            time.sleep(0.1)
    raise RuntimeError(f"never became ready at {sock_path}")


def send_request(s, obj):
    s.sendall((json.dumps(obj) + "\n").encode())


def recv_frame(s, buf):
    """Return (frame_dict|None, leftover_buf)."""
    while b"\n" not in buf:
        try:
            n = s.recv(65536)
        except BlockingIOError:
            return None, buf
        if not n:
            return None, buf
        buf += n
    frame, buf = buf.split(b"\n", 1)
    return json.loads(frame), buf


def wait_for_chunk(sock, pred, timeout=15.0):
    """Consume Chunk frames until `pred(payload)` is true or `timeout`."""
    sock.setblocking(False)
    buf = b""
    t0 = time.time()
    while time.time() - t0 < timeout:
        frame, buf = recv_frame(sock, buf)
        if frame is not None:
            if frame.get("type") == "Chunk":
                text = bytes(frame["bytes"]).decode("utf-8", "replace")
                if pred(text):
                    return True
            continue
        time.sleep(0.02)
    return False


def drain_sock(sock, quiet: float = 0.3):
    """Consume everything already in flight, until the stream goes quiet."""
    sock.setblocking(True)
    sock.settimeout(quiet)
    try:
        while True:
            n = sock.recv(65536)
            if not n:
                break
    except TimeoutError:
        pass
    finally:
        sock.setblocking(False)
        sock.settimeout(None)


def read_cast(glob_pattern: str) -> str:
    candidates = sorted(glob.glob(glob_pattern), key=os.path.getmtime, reverse=True)
    assert candidates, f"no cast file matches {glob_pattern!r}"
    path = candidates[0]
    with open(path, "rb") as fh:
        fcntl.flock(fh, fcntl.LOCK_SH)
        text = fh.read().decode("utf-8", "replace")
        fcntl.flock(fh, fcntl.LOCK_UN)
    return text


def main():
    cast_glob = sys.argv[1]
    out_path = sys.argv[2]
    sock_path = os.environ["SOCK_PATH"]

    s = connect(sock_path)
    s.setblocking(False)

    # Subscribe before any write, so the pump can broadcast to us.
    r, buf = None, b""
    send_request(s, {"type": "Subscribe"})
    t0 = time.time()
    while r is None and time.time() - t0 < 5:
        r, buf = recv_frame(s, buf)
        if r is None:
            time.sleep(0.02)
    assert r and r.get("type") == "Ok", repr(r)

    # Warm the shell so real sudo starts against a settled prompt line.
    send_request(s, {"type": "Input", "bytes": list(b"echo priming\n")})
    wait_for_chunk(s, lambda ch: "priming" in ch, timeout=8.0)
    drain_sock(s)

    # The real sudo command. Real sudo disables terminal echo for its prompt.
    cmd = "sudo -k; sudo true\n"
    send_request(s, {"type": "Input", "bytes": list(cmd.encode())})
    prompt_seen = wait_for_chunk(
        s, lambda ch: "[sudo]" in ch and "password" in ch.lower(), timeout=15.0
    )

    # Answer with the throwaway string. This must reach the shell, must be
    # logged as `[redacted]`, and must not appear in the cast at all.
    answer = PROMPT + "\n"
    send_request(s, {"type": "Input", "bytes": list(answer.encode())})
    # Real sudo, on a refused password, re-prompts up to three times, and
    # every retry prompt re-arms the scrub (that is why the pump must arm on
    # the retry chunk, not just the first). Answer each retry the same way,
    # so by the time sudo's 3-attempt budget is exhausted every prompt's
    # answer has been consumed and the flag has been left clear by that
    # answer's trailing `\n`.
    for _ in range(3):
        retry = wait_for_chunk(
            s,
            lambda ch: "[sudo]" in ch and "password" in ch.lower(),
            timeout=20.0,
        )
        if not retry:
            break
        send_request(s, {"type": "Input", "bytes": list(answer.encode())})
    # Sudo has given up (`sudo: 3 incorrect password attempts` is on the
    # output stream, and that line itself carries the word "password", so
    # the scrub is armed by it). A bare input line consumes that arming —
    # it is logged `[redacted]` and harmless — and leaves the flag clear
    # for the followup.
    send_request(s, {"type": "Input", "bytes": list(b"true\n")})
    drain_sock(s, quiet=2.0)

    # The scrub is now disarmed, so an ordinary command is logged in clear.
    send_request(s, {"type": "Input", "bytes": list(f"echo {FOLLOWUP}\n".encode())})
    followup_seen = wait_for_chunk(s, lambda ch: FOLLOWUP in ch, timeout=8.0)
    drain_sock(s, quiet=1.0)

    cast = read_cast(cast_glob)
    i_events = [
        line
        for line in cast.splitlines()[1:]
        if line.startswith("[") and '"i"' in line
    ]
    i_prompt = any(PROMPT in line for line in i_events)
    # The key means what it says: True = the throwaway string IS somewhere in
    # the cast file. Every grep below is over the WHOLE file, never "i"-only.
    secret_in_cast_whole_file = PROMPT in cast
    marker_flag = "present" if "[redacted]" in cast else "absent"
    followup_clear = any(FOLLOWUP in line for line in i_events)

    out = {
        "prompt_armed": prompt_seen,
        "i_prompt_event": i_prompt,
        "secret_in_cast": secret_in_cast_whole_file,
        "marker_in_cast": marker_flag == "present",
        "followup_in_clear": followup_clear,
        "whole_file_secret_grep_count": cast.count(PROMPT),
        "followup_seen_on_stream": followup_seen,
        "i_events": i_events,
    }
    with open(out_path, "w") as fh:
        json.dump(out, fh, indent=2)
    line = (
        f"prompt_armed={str(out['prompt_armed']).lower()} "
        f"secret_in_cast={str(out['secret_in_cast']).lower()} "
        f"marker_in_cast={str(out['marker_in_cast']).lower()} "
        f"followup_in_clear={str(out['followup_in_clear']).lower()}"
    )
    print(line)
    print(
        f"whole_file_secret_grep_count={out['whole_file_secret_grep_count']}",
        file=sys.stderr,
    )
    print(f"i_prompt_event={i_prompt}", file=sys.stderr)
    for ev in i_events:
        print(f"i_event: {ev}", file=sys.stderr)


if __name__ == "__main__":
    main()
