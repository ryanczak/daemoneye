with open('/tmp/e2e-01b.txt') as fh:
    line = fh.read().strip()
expected = (
    'prompt_armed=true secret_in_cast=false '
    'marker_in_cast=true followup_in_clear=true'
)
print('actual  :', line)
print('expected:', expected)
print('match:', line == expected)
