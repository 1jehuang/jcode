"""Differential fuzz for the niri setup-hotkey insert point (see #719).

Run:  python3 scripts/fuzz/niri_insert_point_fuzz.py <seed> <count>

Emits niri-valid configs to ~/.jcode/scratch/fuzz719/valid_inputs.json. Feed
that to the committed corpus in
crates/jcode-setup-hints/src/linux_niri_fuzz_corpus.txt to widen coverage.

Differential fuzz: does jcode's insert-point scan agree with real niri?

Generates random-but-valid niri configs from KDL constructs that stress the
brace-depth scanner (block comments, line comments, strings, raw strings,
nested nodes), asks real `niri validate` whether the ORIGINAL is valid, then
asks it again after jcode splices its managed block in.

Any config that niri accepts before but rejects after is a bug in the scan.
This replaces "test the cases I thought of" with "let niri be the oracle".
"""
import json
import pathlib
import random
import subprocess
import sys

OPEN, CLOSE, NL, Q, HASH, BS = chr(123), chr(125), chr(10), chr(34), chr(35), chr(92)
BC_O, BC_C = '/' + '*', '*' + '/'
LC = '/' + '/'

WORK = pathlib.Path.home() / '.jcode' / 'scratch' / 'fuzz719'
WORK.mkdir(parents=True, exist_ok=True)


def noise(rng):
    """A construct that is legal KDL but may confuse a naive brace scanner."""
    choices = [
        lambda: BC_O + ' note ' + OPEN * rng.randint(1, 3) + ' ' + BC_C,
        lambda: BC_O + ' note ' + CLOSE * rng.randint(1, 3) + ' ' + BC_C,
        lambda: BC_O + ' balanced ' + OPEN + ' ' + CLOSE + ' ' + BC_C,
        lambda: LC + ' comment with ' + OPEN + OPEN,
        lambda: LC + ' comment with ' + CLOSE,
        lambda: '/-binds ' + OPEN + NL + '    Mod+T ' + OPEN + ' spawn ' + Q + 'x' + Q + '; ' + CLOSE + NL + CLOSE,
        lambda: BC_O + ' ' + BC_O + ' nested? ' + OPEN + ' ' + BC_C,
        lambda: BC_O + NL + ' multi ' + OPEN + NL + ' line ' + CLOSE + NL + BC_C,
        lambda: LC + ' trailing brace at EOL ' + OPEN,
        lambda: '',
    ]
    return rng.choice(choices)()


def nested_node(rng):
    inner = rng.choice(['next-window', 'previous-window'])
    return (
        'recent-windows ' + OPEN + NL
        + '    binds ' + OPEN + NL
        + '        Mod+Tab ' + OPEN + ' ' + inner + '; ' + CLOSE + NL
        + '    ' + CLOSE + NL
        + CLOSE
    )


def string_bind(rng):
    body = rng.choice([
        Q + 'echo ' + OPEN + ' ' + CLOSE + Q,
        Q + 'echo ' + BS + Q + ' ' + OPEN + Q,
        'r' + HASH + Q + 'echo ' + OPEN + ' p' + BS + Q + HASH,
    ])
    return '    Mod+Return ' + OPEN + ' spawn ' + Q + 'sh' + Q + ' ' + Q + '-c' + Q + ' ' + body + '; ' + CLOSE


def gen(rng):
    parts = []
    for _ in range(rng.randint(0, 3)):
        n = noise(rng)
        if n:
            parts.append(n)
    if rng.random() < 0.5:
        parts.append(nested_node(rng))
    for _ in range(rng.randint(0, 2)):
        n = noise(rng)
        if n:
            parts.append(n)
    if rng.random() < 0.85:
        binds = ['binds ' + OPEN]
        binds.append(string_bind(rng))
        binds.append(CLOSE)
        parts.append(NL.join(binds))
    return NL.join(parts) + NL


def niri_ok(path):
    r = subprocess.run(['niri', 'validate', '--config', str(path)],
                       capture_output=True, text=True)
    return r.returncode == 0, (r.stderr or r.stdout).strip().splitlines()[-1:]


def main():
    seed = int(sys.argv[1]) if len(sys.argv) > 1 else 0
    n = int(sys.argv[2]) if len(sys.argv) > 2 else 300
    rng = random.Random(seed)
    cases = []
    for i in range(n):
        cfg = gen(rng)
        p = WORK / f'in-{i}.kdl'
        p.write_text(cfg)
        ok, _ = niri_ok(p)
        if ok:
            cases.append(cfg)
        p.unlink()
    (WORK / 'valid_inputs.json').write_text(json.dumps(cases))
    print(f'generated {n}, kept {len(cases)} niri-valid configs')


if __name__ == '__main__':
    main()
