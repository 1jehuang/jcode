import re
line = r'                ArgSpec { name: "commit".to_string(), arg_type: ArgType::DynamicChoice { generator: "git_commits".to_string(), cache_ttl_secs: 10 }, required: false },'
m = re.match(r'^(\s*)(.*?\b(ArgSpec|SubcommandSpec)\s*\{)(.*?)$', line)
if m:
    print('indent:', repr(m.group(1)))
    print('struct:', repr(m.group(2)))
    print('rest:', repr(m.group(4)))
    rest = m.group(4)
    bc = rest.count('{') - rest.count('}')
    print('brace_count:', bc)
    print('rstripped ends:', repr(rest.rstrip()[-10:]))
    print('is_single:', bc < 0 or (bc == 0 and rest.rstrip().endswith(('}', '},'))))
else:
    print("NO MATCH")