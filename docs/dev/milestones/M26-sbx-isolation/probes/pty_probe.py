#!/usr/bin/env python3
"""M26 phase-01 PTY probe: drives `sbx exec -it <sandbox> bash` from a host PTY."""
import fcntl, os, pty, select, struct, subprocess, sys, termios, time

SB = sys.argv[1]

def sx(*cmd):
    r = subprocess.run(["sbx", "exec", SB, *cmd], capture_output=True, text=True)
    return r.stdout.strip()

def winsz(fd, rows, cols):
    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))

pid, fd = pty.fork()
if pid == 0:
    os.execvp("sbx", ["sbx", "exec", "-it", SB, "bash"])
winsz(fd, 24, 80)

def rd(t=1.5):
    out = b""; end = time.time() + t
    while time.time() < end:
        r, _, _ = select.select([fd], [], [], 0.1)
        if r:
            try: out += os.read(fd, 65536)
            except OSError: break
    return out.decode(errors="replace")

def run(line, t=1.5):
    os.write(fd, line.encode() + b"\n"); return rd(t)

def field(text, key, pat=r"\w+"):
    import re
    m = re.findall(key + "=(" + pat + ")", text)
    return m[-1] if m else "?"

rd(3)
# P1: default rc (no --norc): marker round trip + the in-VM shell pid
o = run("printf '\\037DE_BEG n1\\n'; (exit 5); printf '\\037DE_END n1 %s\\n' $?")
print("P1 marker_default_rc:", "ok" if "\x1fDE_BEG n1" in o and "\x1fDE_END n1 5" in o else "FAIL " + repr(o))
bash_pid = field(run("echo BASHPID=$$"), "BASHPID")
print("P1 in_vm_bash_pid_numeric:", bash_pid.isdigit())

# P2: resize propagates
a = field(run("echo SZ=$(stty size)"), "SZ", r"\d+ \d+")
winsz(fd, 40, 132); time.sleep(0.5)
b = field(run("echo SZ=$(stty size)"), "SZ", r"\d+ \d+")
print(f"P2 resize: {a} -> {b}")

# P3: foreground job discovery + pause of a loop typed at the prompt
idle = sx("ps", "-o", "tpgid=", "-p", bash_pid)
print("P3 idle_tpgid_is_shell:", idle.strip() == bash_pid)
os.write(fd, b"i=0; while true; do i=$((i+1)); echo $i > /tmp/tick; sleep 0.2; done\n"); time.sleep(1.5)
fg = sx("ps", "-o", "tpgid=", "-p", bash_pid).strip()
print("P3 running_tpgid_differs_from_shell:", fg.isdigit() and fg != bash_pid)
# (a) job only: the 10d defect shape — the loop lives in the shell, sleep is the job
sx("kill", "-STOP", "--", "-" + fg); t0 = sx("cat", "/tmp/tick"); time.sleep(2); t1 = sx("cat", "/tmp/tick")
print("P3a stop_job_only_freezes:", t0 == t1)
sx("kill", "-CONT", "--", "-" + fg); time.sleep(0.5)
# (b) shell first, then the job (pty.rs signal_foreground order)
fg = sx("ps", "-o", "tpgid=", "-p", bash_pid).strip()
sx("sh", "-c", f"kill -STOP {bash_pid}; kill -STOP -- -{fg}")
t0 = sx("cat", "/tmp/tick"); time.sleep(2); t1 = sx("cat", "/tmp/tick")
print("P3b stop_shell_then_job_freezes:", t0 == t1)
sx("sh", "-c", f"kill -CONT -- -{fg}; kill -CONT {bash_pid}")
time.sleep(1); t2 = sx("cat", "/tmp/tick")
print("P3b resumes_after_cont:", t2.isdigit() and t1.isdigit() and int(t2) > int(t1))

# P4: cancel — ^C through the PTY ends the loop and the prompt returns
os.write(fd, b"\x03"); rd(1)
t0 = sx("cat", "/tmp/tick"); time.sleep(1); t1 = sx("cat", "/tmp/tick")
print("P4 ctrl_c_ends_loop:", t0 == t1)
o = run("echo ALIVE=yes"); print("P4 shell_usable_after:", field(o, "ALIVE") == "yes")

# P5: in-VM TERM to the foreground group cancels a job the PTY cannot reach
os.write(fd, b"sleep 300\n"); time.sleep(1)
fg = sx("ps", "-o", "tpgid=", "-p", bash_pid).strip()
sx("kill", "-TERM", "--", "-" + fg); o = rd(1.5)
print("P5 in_vm_term_exit_status:", field(run("echo RC=$?"), "RC"))

# P6: sandbox removed underneath a live shell -> the host client exits
subprocess.run(["sbx", "rm", "-f", SB], capture_output=True)
end = time.time() + 20; status = None
while time.time() < end:
    p, st = os.waitpid(pid, os.WNOHANG)
    if p: status = st; break
    time.sleep(0.2)
print("P6 client_exits_on_rm:", status is not None,
      "exit=" + (str(os.waitstatus_to_exitcode(status)) if status is not None else "n/a"))
