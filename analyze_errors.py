import re
from collections import Counter

with open('errors_v2.txt', 'r', encoding='utf-8') as f:
    content = f.read()

# Find all error locations
errors = re.findall(r'error\[E\d+\][^\n]*\n\s*-->\s*(.+?):(\d+):(\d+)', content)
files = [e[0] for e in errors]
c = Counter(files)
print(f'Total errors: {len(errors)}')
print()
for f, count in c.most_common():
    print(f'  {count:3d} errors  ->  {f}')

# Also show error codes
codes = re.findall(r'error\[(E\d+)\]', content)
cc = Counter(codes)
print()
print('Error code distribution:')
for code, count in cc.most_common():
    print(f'  {code}: {count}')
