"""Fix multi-line ArgSpec and SubcommandSpec missing fields - v2."""
import os
import re

def fix_structs(content):
    lines = content.split('\n')
    new_lines = []
    i = 0
    modified = False

    while i < len(lines):
        line = lines[i]
        
        # Detect ArgSpec or SubcommandSpec anywhere on the line (but not struct definitions)
        m = re.search(r'(?<!\bstruct\s)(ArgSpec|SubcommandSpec)\s*\{', line)
        
        if m and 'pub struct' not in line:
            struct_type = m.group(1)
            # Get prefix before the match and after
            prefix = line[:m.start()]
            suffix = line[m.end():]
            
            # Brace depth tracking - start with the opening brace
            brace_depth = 1
            # Count remaining braces on this line
            for ch in suffix:
                if ch == '{': brace_depth += 1
                elif ch == '}': brace_depth -= 1
            
            # If brace_depth == 0 after this line, it's a single-line struct (already handled)
            if brace_depth == 0:
                new_lines.append(line)
                i += 1
                continue
            
            # Multi-line: collect all lines
            struct_lines = [line]
            j = i + 1
            closing_idx = None
            
            while j < len(lines):
                struct_lines.append(lines[j])
                for ch in lines[j]:
                    if ch == '{':
                        brace_depth += 1
                    elif ch == '}':
                        brace_depth -= 1
                if brace_depth == 0:
                    closing_idx = len(struct_lines) - 1
                    j += 1
                    break
                j += 1
            
            if closing_idx is None:
                new_lines.extend(struct_lines)
                i = j
                continue
            
            full = ''.join(struct_lines)
            
            if struct_type == 'ArgSpec':
                needs_default = 'default_value' not in full
                needs_desc = 'description' not in full
                
                if needs_default or needs_desc:
                    modified = True
                    # Find indentation from the first line
                    first_line_content = struct_lines[0].lstrip()
                    indent = ' ' * (len(struct_lines[0]) - len(first_line_content))
                    inner_indent = indent + '        '
                    
                    for k, sl in enumerate(struct_lines):
                        if k == closing_idx:
                            if needs_default:
                                new_lines.append(f"{inner_indent}default_value: None,\n")
                            if needs_desc:
                                new_lines.append(f"{inner_indent}description: None,\n")
                        new_lines.append(sl)
                else:
                    new_lines.extend(struct_lines)
            
            elif struct_type == 'SubcommandSpec':
                if 'examples' not in full:
                    modified = True
                    first_line_content = struct_lines[0].lstrip()
                    indent = ' ' * (len(struct_lines[0]) - len(first_line_content))
                    inner_indent = indent + '        '
                    
                    for k, sl in enumerate(struct_lines):
                        if k == closing_idx:
                            new_lines.append(f"{inner_indent}examples: None,\n")
                        new_lines.append(sl)
                else:
                    new_lines.extend(struct_lines)
            
            i = j
        else:
            new_lines.append(line)
            i += 1

    return '\n'.join(new_lines), modified

target_dir = r'd:\studying\Codecargo\CarpAI\src\completion\bash'
for root, dirs, files in os.walk(target_dir):
    for f in files:
        if f.endswith('.rs'):
            path = os.path.join(root, f)
            with open(path, 'r', encoding='utf-8') as fp:
                content = fp.read()
            new_content, mod = fix_structs(content)
            if mod:
                with open(path, 'w', encoding='utf-8') as fp:
                    fp.write(new_content)
                print(f"Fixed: {path}")

print("Done!")