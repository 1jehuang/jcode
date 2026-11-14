"""Strip invalid `pub(crate)` qualifiers from enum variant fields.

Rust forbids visibility qualifiers on enum variants and their fields. A prior
scripted transform added them, breaking the crate. This removes `pub(crate)`
only inside enum bodies, leaving struct fields and the declarations intact.
"""

import re
import sys

PATH = "crates/jcode-provider-forgecode-runtime/src/parser.rs"


def find_enum_bodies(source: str):
    """Yield (start, end) char offsets of each enum body."""
    for match in re.finditer(r"\benum\s+\w+", source):
        open_brace = source.find("{", match.end())
        if open_brace == -1:
            continue
        depth = 0
        index = open_brace
        while index < len(source):
            char = source[index]
            if char == "{":
                depth += 1
            elif char == "}":
                depth -= 1
                if depth == 0:
                    yield (open_brace + 1, index)
                    break
            index += 1


def main() -> int:
    source = open(PATH).read()
    bodies = list(find_enum_bodies(source))
    print(f"enum bodies found: {len(bodies)}")

    pieces = []
    last = 0
    for start, end in sorted(bodies):
        pieces.append(source[last:start])
        pieces.append(source[start:end].replace("pub(crate) ", ""))
        last = end
    pieces.append(source[last:])

    open(PATH, "w").write("".join(pieces))
    print("stripped pub(crate) from enum bodies")
    return 0


if __name__ == "__main__":
    sys.exit(main())
