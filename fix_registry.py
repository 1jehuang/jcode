"""Fix registry.rs v5: add ..Default::default() to struct literals."""
import re

FILE = r'd:\studying\Codecargo\CarpAI\src\completion\bash\registry.rs'
with open(FILE, 'r', encoding='utf-8-sig') as f:
    raw = f.read()

lines = raw.split('\n')
result = []
i = 0

while i < len(lines):
    line = lines[i]
    m = re.match(r'^(\s*)(.*?\b(ArgSpec|SubcommandSpec)\s*\{)(.*?)$', line)
    
    if m:
        indent_str = m.group(1)
        struct_part = m.group(2)
        rest = m.group(4)
        indent_len = len(indent_str)
        
        # Single-line struct: rest part contains the closing }
        brace_count = rest.count('{') - rest.count('}')
        is_single = (brace_count < 0) or (brace_count == 0 and rest.rstrip().endswith(('}', '},')))
        
        if is_single:
            if '..Default::default()' not in rest:
                stripped = rest.rstrip()
                suffix = rest[len(stripped):]
                if stripped.endswith('},'):
                    stripped = stripped[:-2] + ', ..Default::default() },'
                elif stripped.endswith('}'):
                    stripped = stripped[:-1] + ', ..Default::default() }'
                line = indent_str + struct_part + stripped + suffix
                result.append(line)
                i += 1
                continue
        else:
            # Multi-line: collect until closing } at SAME indent level
            struct_lines = [line]
            i += 1
            while i < len(lines):
                nxt = lines[i]
                struct_lines.append(nxt)
                i += 1
                # Check: line starts with same indent, then }[,]
                if nxt.startswith(indent_str) and re.match(r'\}[\s,]*$', nxt[indent_len:]):
                    break
            
            full = '\n'.join(struct_lines)
            if '..Default::default()' not in full:
                before = struct_lines[:-1]
                closing = struct_lines[-1]
                insert_line = indent_str + '    ..Default::default()'
                if before and not before[-1].rstrip().endswith(','):
                    new_lines = before[:-1] + [before[-1].rstrip() + ','] + [insert_line, closing]
                else:
                    new_lines = before + [insert_line, closing]
                result.extend(new_lines)
            else:
                result.extend(struct_lines)
            continue
    
    result.append(line)
    i += 1

output = '\n'.join(result)
with open(FILE, 'w', encoding='utf-8') as f:
    f.write(output)

print(f"Done. {len(lines)} -> {len(output.split(chr(10)))} lines")