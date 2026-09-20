import json
import sys

path = sys.argv[1]
with open(path) as fh:
    data = json.load(fh)
print(json.dumps(data, indent=2))
