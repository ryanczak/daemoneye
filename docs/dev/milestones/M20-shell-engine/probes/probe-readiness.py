# F3: a bound+listening Unix socket that is never accept()ed still completes a
# client connect() AND accepts a write — so "connect succeeded" is not proof
# the host is serving. Readiness must be a full Status round-trip.
import socket, os, time, tempfile
d = tempfile.mkdtemp(); p = os.path.join(d, "t.sock")
srv = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
srv.bind(p); srv.listen(16)          # bound + listening, but never accept()
t0 = time.time()
c = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
c.connect(p)
c.sendall(b'{"type":"Status"}\n')
print(f"connect+send succeeded without accept(), {1000*(time.time()-t0):.2f}ms")
c.settimeout(0.3)
try:
    print("reply:", c.recv(100))
except socket.timeout:
    print("reply: TIMEOUT — connect alone is NOT proof the host is serving")
