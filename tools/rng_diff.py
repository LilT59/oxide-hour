#!/usr/bin/env python3
"""First-divergence diff of two logic RNG traces (ROADMAP next step 1).

Usage: python tools/rng_diff.py PORT_TRACE CPP_TRACE [--context N]

Both traces come from the same game. The port writes them with `ZR_RNG_TRACE=<path>` (crates/zr-sim/src/random.rs),
the patched C++ build with the same variable (zh-cpp-build/src-patched, RandomValue.cpp). One line per logic draw:

    RNG f=<frame> n=<draw index> k=I lo=<int> hi=<int> r=<int> at=<file>:<line>
    RNG f=<frame> n=<draw index> k=R lo=<hex> hi=<hex> r=<hex> at=<file>:<line>

For k=R the hex fields are the f32 bit patterns. Records are aligned by sequence (the n-th draw of each side). A
draw one side lacks shifts everything after it, so the first record where (frame, kind, range, result) differ is the
first place the two streams disagree. Call sites are reported but not compared: the C++ and Rust files differ.

Exit status: 0 identical over every draw, 1 divergence (or one trace is longer), 2 unreadable trace.
"""
import argparse
import re
import struct
import sys

LINE = re.compile(r"^RNG f=(\d+) n=(\d+) k=([IR]) lo=(\S+) hi=(\S+) r=(\S+) at=(.*):(\d+)$")


def f32(bits):
    return struct.unpack("<f", struct.pack("<I", bits))[0]


def load(path):
    """Parse a trace into records. A last line cut short by a killed run is dropped with a warning."""
    recs = []
    with open(path, encoding="utf-8", errors="replace") as fh:
        lines = fh.read().splitlines()
    for no, line in enumerate(lines, 1):
        if not line.strip():
            continue
        m = LINE.match(line)
        if not m:
            if no == len(lines):
                print(f"warning: {path}:{no}: dropped a cut-off last line", file=sys.stderr)
                break
            print(f"{path}:{no}: not an RNG trace line: {line[:120]}", file=sys.stderr)
            sys.exit(2)
        f, n, k, lo, hi, r, at, ln = m.groups()
        if k == "I":
            lo, hi, r = int(lo), int(hi), int(r)
        else:
            lo, hi, r = (int(x, 16) for x in (lo, hi, r))
        recs.append({"f": int(f), "n": int(n), "k": k, "lo": lo, "hi": hi, "r": r, "at": f"{at}:{ln}"})
    for i, rec in enumerate(recs):
        if rec["n"] != i:
            print(f"warning: {path}: draw {i} carries n={rec['n']} (lines missing or reordered)", file=sys.stderr)
            break
    return recs


def key(rec):
    return (rec["f"], rec["k"], rec["lo"], rec["hi"], rec["r"])


def differing(a, b):
    names = []
    if a["f"] != b["f"]:
        names.append("frame")
    if a["k"] != b["k"]:
        names.append("kind")
    if (a["lo"], a["hi"]) != (b["lo"], b["hi"]):
        names.append("range")
    if a["r"] != b["r"]:
        names.append("result")
    return names


def short_site(at):
    """The last three path components of a call site, for display."""
    parts = re.split(r"[\\/]", at)
    return "/".join(parts[-3:])


def show(rec):
    if rec["k"] == "I":
        val = f"lo={rec['lo']} hi={rec['hi']} r={rec['r']}"
    else:
        val = f"lo={f32(rec['lo']):.9g} hi={f32(rec['hi']):.9g} r={f32(rec['r']):.9g}"
    return f"n={rec['n']:<8} f={rec['f']:<6} k={rec['k']} {val:<46} at={short_site(rec['at'])}"


def main():
    ap = argparse.ArgumentParser(description="First divergence between a port and a C++ logic RNG trace.")
    ap.add_argument("port", help="trace written by the port (ZR_RNG_TRACE)")
    ap.add_argument("cpp", help="trace written by the C++ build (ZR_RNG_TRACE)")
    ap.add_argument("--context", type=int, default=10, help="draws of context before and after (default 10)")
    a = ap.parse_args()
    p, c = load(a.port), load(a.cpp)
    for name, recs in (("port", p), ("cpp", c)):
        if recs:
            print(f"{name}: {len(recs)} draws, frames {recs[0]['f']}..{recs[-1]['f']}")
        else:
            print(f"{name}: no draws")
    common = min(len(p), len(c))
    first = next((i for i in range(common) if key(p[i]) != key(c[i])), None)
    if first is None:
        if len(p) == len(c):
            print(f"identical: all {len(p)} draws match (frame, kind, range, result)")
            return 0
        short, long_, extra = ("port", "cpp", len(c) - len(p)) if len(p) < len(c) else ("cpp", "port", len(p) - len(c))
        print(f"identical for the first {common} draws; the {short} trace ends there and the {long_} trace has "
              f"{extra} more (frame {(c if long_ == 'cpp' else p)[common]['f']} at draw {common})")
        return 1

    pa, ca = p[first], c[first]
    print(f"\nfirst mismatch at draw n={first}: frame {pa['f']} vs {ca['f']}, differs in: {', '.join(differing(pa, ca))}")
    print(f"  port  {show(pa)}\n        call site: {pa['at']}")
    print(f"  cpp   {show(ca)}\n        call site: {ca['at']}")
    lo = max(0, first - a.context)
    print(f"\ncontext (draws {lo}..{first + a.context}; '>' marks the first mismatch):")
    for name, recs, other in (("PORT", p, c), ("CPP", c, p)):
        print(f"-- {name}")
        for i in range(lo, min(len(recs), first + a.context + 1)):
            mark = ">" if i == first else (" " if i < len(other) and key(recs[i]) == key(other[i]) else "*")
            print(f"{mark} {show(recs[i])}")
    return 1


if __name__ == "__main__":
    sys.exit(main())
