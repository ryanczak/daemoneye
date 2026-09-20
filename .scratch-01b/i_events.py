import json
import sys
from contextlib import suppress

path = sys.argv[1]
with open(path) as fh:
    lines = fh.read().splitlines()[1:]
for line in lines:
    with suppress(ValueError, IndexError):
        v = json.loads(line)
        if isinstance(v, list) and len(v) == 3 and v[1] == "i":
            print(line)
