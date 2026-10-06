#!/usr/bin/env bash
# M26 phase-01 measurement block. Creates only de-probe-* sandboxes and removes them.
set -u
HERE=$(cd "$(dirname "$0")" && pwd)
W=$(mktemp -d); trap 'sbx rm -f de-probe-a de-probe-b de-probe-c >/dev/null 2>&1; rm -rf "$W"' EXIT
echo "S0 version: $(sbx version 2>&1 | head -1)"
echo "S0 daemon: $(sbx daemon status 2>&1 | head -1)"
echo "S0 leftover_probes_before: $(sbx ls -q 2>/dev/null | grep -c '^de-probe-')"
echo "S0 sbx_daemons_before: $(pgrep -fc '^/usr/bin/sbx daemon start')"

# S1 lifecycle + errors
sbx create --name de-probe-a -q --memory 1g --cpus 2 shell >/dev/null 2>&1; echo "S1 create_exit: $?"
echo "S1 limits: nproc=$(sbx exec de-probe-a nproc) memMiB=$(sbx exec de-probe-a sh -c "awk '/MemTotal/{print int(\$2/1024)}' /proc/meminfo")"
sbx exec de-probe-nope true >/dev/null 2>"$W/err"; echo "S1 exec_missing: exit=$? stderr=$(head -1 "$W/err")"
sbx rm -f de-probe-nope >/dev/null 2>"$W/err"; echo "S1 rm_missing: exit=$? stderr=$(head -1 "$W/err")"
sbx create --name de-probe-a -q shell >/dev/null 2>"$W/err"; echo "S1 create_duplicate: exit=$? stderr=$(head -1 "$W/err")"
L=de-probe-$(printf 'x%.0s' $(seq 1 60)); sbx create --name "$L" -q shell >/dev/null 2>"$W/err"; echo "S1 name_69_chars: exit=$? stderr=$(head -1 "$W/err")"; sbx rm -f "$L" >/dev/null 2>&1
FOO_HOST_ONLY=leak sbx exec de-probe-a sh -c 'echo "S1 host_env_forwarded: ${FOO_HOST_ONLY:-no}"'
HOME="$W" sbx ls -q >/dev/null 2>"$W/err"; echo "S1 other_HOME: exit=$? stderr=$(head -1 "$W/err" | cut -c1-120)"
HOME="$W" XDG_STATE_HOME="$HOME/.local/state" sbx ls -q >/dev/null 2>"$W/err"; echo "S1 other_HOME_real_XDG_STATE_HOME: exit=$? stderr=$(head -1 "$W/err" | cut -c1-120)"

# S2 script staging with sbx cp
printf '#!/bin/sh\necho staged-ok "$@"\n' > "$W/s.sh"; chmod 700 "$W/s.sh"
sbx exec de-probe-a mkdir -p /de/scripts 2>/dev/null || sbx exec -u root de-probe-a sh -c 'mkdir -p /de/scripts && chown agent:agent /de/scripts'
sbx cp "$W/s.sh" de-probe-a:/de/scripts/s.sh >/dev/null 2>"$W/err"; echo "S2 cp_exit: $? $(head -1 "$W/err")"
echo "S2 staged_mode_owner: $(sbx exec de-probe-a stat -c '%a %U' /de/scripts/s.sh)"
echo "S2 staged_run: $(sbx exec de-probe-a /de/scripts/s.sh arg1)"

# S3 network policy mapping (Q1/Q2)
code() { sbx exec "$1" curl -sS -o /dev/null -w '%{http_code}' --max-time 8 "$2" 2>/dev/null; }
echo "S3 default example.org: $(code de-probe-a https://example.org)"
sbx policy allow network --sandbox de-probe-a example.org >/dev/null 2>&1; sleep 6
echo "S3 after_sandbox_allow example.org: $(code de-probe-a https://example.org)"
sbx create --name de-probe-b -q --deny-network api.github.com shell >/dev/null 2>&1
echo "S3 sandbox_deny api.github.com: $(code de-probe-b https://api.github.com)  github.com: $(code de-probe-b https://github.com)"
echo "S3 allow_did_not_leak_to_b example.org: $(code de-probe-b https://example.org)"
sbx create --name de-probe-c -q --deny-network '**' shell >/dev/null 2>&1
echo "S3 deny_all github.com: $(code de-probe-c https://github.com)"
sbx policy log de-probe-b --json --limit 5 > "$W/log.json" 2>/dev/null
python3 - "$W/log.json" <<'PY'
import json,sys
d=json.load(open(sys.argv[1]))
def keys(x):
    if isinstance(x,dict): return {k:keys(v) for k,v in x.items()}
    if isinstance(x,list): return [keys(x[0])] if x else []
    return type(x).__name__
print("S3 policy_log_json_shape:", json.dumps(keys(d), sort_keys=True))
PY
echo "S3 secrets_stored: $(sbx secret ls 2>&1 | head -1)"
echo "S3 sentinel_present_anyway: $(sbx exec de-probe-a sh -c 'echo ${ANTHROPIC_API_KEY:-unset}')"
sbx rm -f de-probe-b de-probe-c >/dev/null 2>&1
echo "S3 rules_after_rm: $(sbx policy ls 2>&1 | grep -c 'de-probe-b')"

# S4 PTY probe (removes de-probe-a itself in P6)
python3 "$HERE/pty_probe.py" de-probe-a

sleep 1; echo "S5 leftover_probes_after: $(sbx ls -q 2>/dev/null | grep -c '^de-probe-')"
echo "S5 sbx_daemons_after: $(pgrep -fc '^/usr/bin/sbx daemon start')"
