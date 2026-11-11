r"""Align two object-id allocation dumps: the port's `ZR_ID_DUMP` against the patched C++ build's
`ZR_L64_ID` (see docs/HARNESS.md, "Object ids").

Usage: python tools/id_diff.py PORT CPP [--max N] [--context C]

Both files are scanned for lines containing `ZR_L64_ID id=<id> f=<frame> tmpl=<name> at=<path>`;
every other line (a C++ log, for example) is ignored. Records are compared in allocation order: two
allocations agree when the id and the template name agree (the frames are not compared: a production
delay changes a frame but not the id sequence). Prints the first allocation that differs with a window
of context, or `identical`.

Exit status as `rng_diff.py` / `snap_diff.py`: 0 identical, 1 a divergence, 2 unreadable or empty.
"""
import argparse
import re
import sys

LINE = re.compile(r"ZR_L64_ID id=(\d+) f=(\d+) tmpl=(\S+) at=(\S+)")


def load(path):
    out = []
    try:
        with open(path, encoding="utf-8", errors="replace") as fh:
            for raw in fh:
                m = LINE.search(raw)
                if m:
                    out.append((int(m.group(1)), int(m.group(2)), m.group(3), m.group(4)))
    except OSError as e:
        print(f"cannot read {path}: {e}", file=sys.stderr)
        return None
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("port")
    ap.add_argument("cpp")
    ap.add_argument("--max", type=int, default=0, help="compare ids up to this value (inclusive)")
    ap.add_argument("--context", type=int, default=6, help="allocations shown on each side of the first difference")
    a = ap.parse_args()
    port = load(a.port)
    cpp = load(a.cpp)
    if port is None or cpp is None:
        return 2
    if a.max:
        port = [r for r in port if r[0] <= a.max]
        cpp = [r for r in cpp if r[0] <= a.max]
    if not port or not cpp:
        print("no records on one side", file=sys.stderr)
        return 2
    first = None
    for i in range(min(len(port), len(cpp))):
        p, c = port[i], cpp[i]
        if p[0] != c[0] or p[2] != c[2]:
            first = i
            break
    if first is None and len(port) == len(cpp):
        print(f"identical: {len(port)} allocations")
        return 0
    if first is None:
        first = min(len(port), len(cpp))
        print(f"sequences agree on the first {first} allocations; then port has {len(port)} and cpp {len(cpp)}")
    else:
        p, c = port[first], cpp[first]
        print(f"first difference at sequence index {first}:")
        print(f"  port: id={p[0]} f={p[1]} tmpl={p[2]} at={p[3]}")
        print(f"  cpp : id={c[0]} f={c[1]} tmpl={c[2]} at={c[3]}")
    lo = max(0, first - a.context)
    hi = min(max(len(port), len(cpp)), first + a.context)
    print(f"\ncontext {lo}..{hi - 1}:")
    for i in range(lo, hi):
        p = f"{port[i][0]:>4} f={port[i][1]:<5} {port[i][2]:<42} {port[i][3]}" if i < len(port) else "-"
        c = f"{cpp[i][0]:>4} f={cpp[i][1]:<5} {cpp[i][2]:<42} {cpp[i][3]}" if i < len(cpp) else "-"
        mark = "  <<<" if i == first else ""
        print(f"  [{i:>3}] P {p}")
        print(f"        C {c}{mark}")
    return 1


if __name__ == "__main__":
    sys.exit(main())
