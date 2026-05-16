import re

path = r"d:\studying\Codecargo\CarpAI\src\completion\bash\nl_command.rs"

with open(path, "r", encoding="utf-8") as f:
    content = f.read()

# 1. Fix format( -> format!(  (as a function call, not macro)
content = re.sub(r"\bformat\(", "format!(", content)

# 2. Fix t() function signature: fixed-size arrays -> slices
old_t_sig = """fn t(
    id: &str,
    patterns: [&str; 4],
    commands: [(&str, &str, bool); 1],"""
new_t_sig = """fn t(
    id: &str,
    patterns: &[&str],
    commands: &[(&str, &str, bool)],"""
content = content.replace(old_t_sig, new_t_sig)

# 3. Fix t_multi() function signature: fixed-size arrays -> slices  
old_tm_sig = """fn t_multi(
    id: &str,
    patterns: [&str; 4],"""
new_tm_sig = """fn t_multi(
    id: &str,
    patterns: &[&str],"""
content = content.replace(old_tm_sig, new_tm_sig)

# 4. Fix call sites: add & before [ in t() and t_multi() call lines
#    t("id", ["p1","p2"], [c("cmd","desc",false)], Cat, Risk, None),
#    -> t("id", &["p1","p2"], &[c("cmd","desc",false)], Cat, Risk, None),
lines = content.split("\n")
new_lines = []
for line in lines:
    stripped = line.lstrip()
    if stripped.startswith("t(") or stripped.startswith("t_multi("):
        # Replace ", [" with ", &[" for the first two occurrences (patterns and commands arrays)
        line = re.sub(r", \[", ", &[", line, count=2)
    new_lines.append(line)
content = "\n".join(new_lines)

with open(path, "w", encoding="utf-8") as f:
    f.write(content)

print("Done! Fixed nl_command.rs")