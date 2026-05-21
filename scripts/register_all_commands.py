#!/usr/bin/env python3
"""
批量注册所有命令至CommandRegistry
目标：确保100+命令全部可用
"""

import re
from pathlib import Path

def extract_commands_from_extra():
    """从extra_commands.rs提取所有命令"""
    extra_file = Path("src/commands/extra_commands.rs")
    content = extra_file.read_text(encoding='utf-8')

    # 匹配 define_command!(CommandName, "description");
    pattern = r'define_command!\((\w+),\s*"([^"]+)"\)'
    matches = re.findall(pattern, content)

    commands = []
    for class_name, description in matches:
        # 转换为命令名称（去掉Command后缀）
        cmd_name = class_name.replace('Command', '').lower()
        commands.append({
            'class_name': class_name,
            'cmd_name': cmd_name,
            'description': description
        })

    return commands


def generate_registry_code(commands):
    """生成注册代码"""
    lines = []

    # 添加extra_commands模块引用
    lines.append("// Extra commands (bulk registered)")
    lines.append("use super::extra_commands::*;")
    lines.append("")

    # 为每个命令生成注册语句
    for cmd in commands:
        lines.append(f"        // {cmd['description']}")
        lines.append(f"        self.register({cmd['class_name']});")

    return '\n'.join(lines)


def update_mod_rs(commands):
    """更新mod.rs文件"""
    mod_file = Path("src/commands/mod.rs")
    content = mod_file.read_text(encoding='utf-8')

    # 找到register_all函数的位置
    register_pattern = r'(fn register_all\(&mut self\) \{.*?)(\n\s*\})'
    match = re.search(register_pattern, content, re.DOTALL)

    if not match:
        print("ERROR: Could not find register_all function")
        return

    existing_code = match.group(1)
    closing_brace = match.group(2)

    # 生成新的注册代码
    new_registrations = generate_registry_code(commands)

    # 组合新内容
    new_register_all = existing_code + "\n\n" + new_registrations + closing_brace

    # 替换
    new_content = content[:match.start()] + new_register_all + content[match.end():]

    mod_file.write_text(new_content, encoding='utf-8')
    print(f"Updated mod.rs with {len(commands)} additional command registrations")


def main():
    commands = extract_commands_from_extra()
    print(f"Found {len(commands)} commands in extra_commands.rs")

    for cmd in commands:
        print(f"  - {cmd['cmd_name']}: {cmd['description']}")

    update_mod_rs(commands)

    print(f"\nTotal estimated commands:")
    print(f"  Manually registered: ~15")
    print(f"  From extra_commands: {len(commands)}")
    print(f"  Total: ~{15 + len(commands)}")


if __name__ == "__main__":
    main()
