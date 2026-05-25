import re
with open('workspace_check_full.txt', 'r', encoding='utf-16', errors='replace') as f:
    lines = f.readlines()

results = []
i = 0
while i < len(lines):
    line = lines[i]
    if 'error[E0282]' in line:
        # Skip to find the --> line
        j = i + 1
        while j < len(lines) and '--> ' not in lines[j]:
            j += 1
        if j < len(lines):
            m = re.search(r'--> (.*?:\d+:\d+)', lines[j])
            if m:
                results.append(m.group(1))
    i += 1

with open('e0282_errors.txt', 'w', encoding='utf-8') as out:
    for r in results:
        out.write(r + '\n')
print(f"Wrote {len(results)} E0282 errors to e0282_errors.txt")
