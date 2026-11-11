"""Compare the sleepy-update heap of the port and of the patched C++ build at one frame (lane 99).

Usage: python -I tools/heap_diff.py <port_err_file> <cpp_out_file> <frame> [-v]

Both files carry `ZR_L99_HEAP f=<F> i=<I> id=<ID> key=<KEY> mod=<CLASS> tmpl=<T>` lines: the port prints them on
stderr with `ZR_L99_HEAP=<frames>` (zr-sim `dump_sleepy_heap`), the patched C++ on stdout (GameLogic::update).
Prints the per-class counts, how many (id, class) pairs match, the keys that differ, the entries on one side only
(with -v), and the AI modules of the earliest key in heap order. `key` is `(wake << 2) | phase`.
"""
import re
import sys
from collections import Counter, defaultdict

LINE = re.compile(r"ZR_L99_HEAP f=(\d+) i=(\d+) id=(\d+) key=(\d+) mod=(\S+) tmpl=(\S+)")
AI_CLASSES = ("AIUpdateInterface", "DozerAIUpdate", "WorkerAIUpdate")


def load(path, frame):
    rows = []
    with open(path, encoding="utf-8", errors="replace") as fh:
        for line in fh:
            m = LINE.search(line)
            if m and int(m.group(1)) == frame:
                rows.append((int(m.group(2)), int(m.group(3)), int(m.group(4)), m.group(5), m.group(6)))
    rows.sort()
    return rows


def main():
    port_path, cpp_path, frame = sys.argv[1], sys.argv[2], int(sys.argv[3])
    verbose = "-v" in sys.argv
    port = load(port_path, frame)
    cpp = load(cpp_path, frame)
    print(f"f{frame}: port {len(port)} entries, C++ {len(cpp)} entries")
    for name, rows in (("port", port), ("C++", cpp)):
        c = Counter(r[3] for r in rows)
        print(f"  {name} classes:", ", ".join(f"{k}={v}" for k, v in sorted(c.items())))
    pmap = defaultdict(list)
    cmap = defaultdict(list)
    for r in port:
        pmap[(r[1], r[3])].append(r[2])
    for r in cpp:
        cmap[(r[1], r[3])].append(r[2])
    both = sorted(set(pmap) & set(cmap))
    only_p = sorted(set(pmap) - set(cmap))
    only_c = sorted(set(cmap) - set(pmap))
    same_key = [k for k in both if pmap[k] == cmap[k]]
    print(f"  (id, class) pairs: both {len(both)} (same key {len(same_key)}), port only {len(only_p)}, C++ only {len(only_c)}")
    for k in both:
        if pmap[k] != cmap[k]:
            print(f"    key differs id={k[0]} {k[1]}: port {pmap[k]} C++ {cmap[k]}")
    if verbose:
        for k in only_p:
            print(f"    port only id={k[0]} {k[1]} key {pmap[k]}")
        for k in only_c:
            print(f"    C++ only id={k[0]} {k[1]} key {cmap[k]}")
    for name, rows in (("port", port), ("C++", cpp)):
        ai = [r for r in rows if r[2] % 4 == 0 and r[3] in AI_CLASSES]
        k0 = min((r[2] for r in ai), default=None)
        p0 = [r for r in ai if r[2] == k0]
        print(f"  {name} AI heap order at the earliest AI key {k0} (heap index: id):",
              " ".join(f"{r[0]}:{r[1]}" for r in p0))


if __name__ == "__main__":
    main()
