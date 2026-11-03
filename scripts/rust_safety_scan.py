"""Rust lexical masking and external-module classification for safety ratchets."""
from pathlib import Path
import re

# Lifetimes are deliberately not treated as character literals.
TOKEN = re.compile(r'//[^\n]*|/\*|(?:br|r)(?P<hashes>\#*)"|b?"|b?\'(?:\\.|[^\'\\\n])\'')


def code_only(text: str) -> str:
    output = list(text)
    pos = 0
    while match := TOKEN.search(text, pos):
        start, end = match.span()
        token = match.group()
        if token == '/*':
            depth = 1
            while depth and end < len(text):
                nested = re.search(r'/\*|\*/', text[end:])
                if nested is None:
                    end = len(text)
                    break
                end += nested.end()
                depth += 1 if nested.group() == '/*' else -1
        elif match.group('hashes') is not None:
            closing = '"' + match.group('hashes')
            found = text.find(closing, end)
            end = len(text) if found < 0 else found + len(closing)
        elif token in ('"', 'b"'):
            while end < len(text):
                if text[end] == '\\':
                    end += 2
                elif text[end] == '"':
                    end += 1
                    break
                else:
                    end += 1
        for index in range(start, min(end, len(text))):
            if text[index] != '\n':
                output[index] = ' '
        pos = end
    return ''.join(output)


MODULE = re.compile(r'^[ \t]*(?P<attrs>(?:#\s*\[[^\]]*\]\s*)*)(?:pub(?:\([^)]*\))?\s+)?mod\s+(?P<name>\w+)\s*;', re.M)
PATH = re.compile(r'#\s*\[\s*path\s*=\s*"([^"\n]+)"\s*\]')


def module_paths(path: Path, text: str) -> dict[str, set[Path]]:
    directory = path.parent if path.stem in ('lib', 'main', 'mod') else path.with_suffix('')
    result = {}
    for match in MODULE.finditer(code_only(text)):
        name = match['name']
        alias = PATH.search(text[match.start('attrs'):match.end('attrs')])
        candidates = (path.parent / alias[1],) if alias else (directory / f'{name}.rs', directory / name / 'mod.rs')
        result.setdefault(name, set()).update(candidate.resolve() for candidate in candidates if candidate.is_file())
    return result


def test_module_files(files: list[Path], production_lines) -> set[Path]:
    test_only = set()
    edges = {}
    for path in files:
        declarations = module_paths(path, path.read_text(encoding='utf-8'))
        prod_names = {match['name'] for match in MODULE.finditer('\n'.join(production_lines(path)))}
        edges[path.resolve()] = set()
        for name, children in declarations.items():
            if name in prod_names:
                edges[path.resolve()].update(children)
            else:
                test_only.update(children)
    # Include all descendants of test-only roots.
    pending = list(test_only)
    while pending:
        for child in edges.get(pending.pop(), set()) - test_only:
            test_only.add(child)
            pending.append(child)
    # Production references override test-only classification, including descendants.
    production = set().union(*(children for parent, children in edges.items() if parent not in test_only))
    pending = list(production)
    while pending:
        parent = pending.pop()
        test_only.discard(parent)
        for child in edges.get(parent, set()) - production:
            production.add(child)
            pending.append(child)
    return test_only

CFG_TEST = re.compile(r'#\s*\[\s*cfg\s*\(\s*(?:all\s*\(\s*)?test\s*[,)]')
ITEM = re.compile(r'(?:pub(?:\([^)]*\))?\s+)?(?:(?:async|unsafe|const)\s+)*(?:mod|fn)\b')


def production_lines(path: Path) -> list[str]:
    """Mask test items, including multiline signatures and external modules.

    This is deliberately conservative: only recognized test-only cfg forms and
    function/module items are omitted. Other cfg expressions remain counted.
    """
    text = code_only(path.read_text(encoding='utf-8', errors='ignore'))
    output = list(text)
    for match in CFG_TEST.finditer(text):
        end = text.find(']', match.end())
        if end < 0:
            continue
        cursor = end + 1
        # Attributes between cfg(test) and the item can span multiple lines.
        while True:
            whitespace = re.match(r'\s*', text[cursor:])
            cursor += whitespace.end()
            if text.startswith('#', cursor):
                attribute = re.match(r'#\s*\[[^\]]*\]', text[cursor:])
                if attribute:
                    cursor += attribute.end()
                    continue
            break
        item = ITEM.match(text, cursor)
        if not item:
            continue
        opening = re.search(r'[;{]', text[item.end():])
        if opening is None:
            continue
        end = item.end() + opening.end()
        if opening.group() == '{':
            depth = 1
            while end < len(text) and depth:
                depth += (text[end] == '{') - (text[end] == '}')
                end += 1
            if depth:
                continue  # Malformed input must not suppress later production.
        for index in range(match.start(), end):
            if text[index] != '\n':
                output[index] = ' '
    return ''.join(output).splitlines()
