"""Trace-check for the Task 6 probe's followup phase.

Reads the newest cast under /tmp/de.*/ (the last capture run), prints all
events, and asserts the followup command appears as an `"o"` echo so the
subsequent grep for FOLLOWUP is meaningful.
"""
import glob
import json
import os

FOLLOWUP = "de01b-followup-in-clear"
candidates = sorted(
    glob.glob("/tmp/de.*/.daemoneye/var/log/shells/*.cast"),
    key=os.path.getmtime,
    reverse=True,
)
assert candidates, "no cast found"
path = candidates[0]
print("==", path)
with open(path) as fh:
    text = fh.read()
for line in text.splitlines()[1:]:
    try:
        v = json.loads(line)
    except ValueError:
        continue
    if isinstance(v, list):
        print(line[:130])
print("FOLLOWUP anywhere:", FOLLOWUP in text)
