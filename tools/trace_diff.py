#!/usr/bin/env python3
"""First-divergence diff of two per-object traces (ROADMAP "wave 17 tooling", docs/HARNESS.md).

Usage: python tools/trace_diff.py PORT CPP [--ids 243,209] [--fields NAME[,NAME...]] [--all-fields]
                                           [--tol X] [--window N] [--max-report N]

Both files hold `TRACE` lines, one per traced object per frame:

    TRACE f=<frame> id=<id> <name>=<value> <name>=<value> ...

The port writes them with `ZR_TRACE=<file>` (crates/zr-game/src/trace.rs, or the zr-harness `trace`
request); the patched C++ build writes the same format from GameLogicTrace.cpp. Lines that do not start
with `TRACE ` are ignored, so a C++ stdout log can be passed as it is.

Records are aligned by (frame, id). For every traced object the tool prints the earliest frame where
any compared field differs and the first differing field there, then the first difference over all
objects. `--window N` follows with a side-by-side window of N frames either side of the first
difference for that object, showing only the fields that differ in the window.

Comparison:
- Exact, except that two values that both parse as floats are compared numerically. A packed position
  (`x,y,z`, the path and goal fields) is compared component by component. `--tol` is the allowed
  absolute difference (default 0.0: the same printed value, so `1.0000` equals `1.0`).
- The documented gap fields are SKIPPED by default (docs/HARNESS.md, "Per-object trace", "Gaps"):
  `aiwake`, `aiphase`, `dtask` (C++ only), `dact` (port only), `dbuild` (matches only from the dozer's
  action dock on) and `victim` (the snapshot's `atk` approximation, not `m_currentVictimID`). They
  would otherwise make every default diff start with a false divergence. `--all-fields` compares
  everything; `--fields` compares exactly the named fields, gap fields included.
- `--fields` limits the comparison to the named fields (comma or space separated; e.g. `x,y,z,ai`).
  Otherwise every non-gap field of the record is compared.
- `--ids` limits the comparison to the named object ids. An object present on one side only is a
  difference at its first frame; a frame missing for one object is a difference at that frame.
- A final line without a newline (a killed run) is dropped with a warning.

Exit status: 0 identical, 1 a divergence, 2 unreadable or malformed trace, a trace with no records
(either side), or a bad option.
"""
import argparse
import re
import sys

HEADER = re.compile(r"^TRACE f=(\d+) id=(\d+)(?: (.*))?$")

# One-sided or partial fields, skipped unless `--all-fields` or `--fields` names them. See the module
# docstring and docs/HARNESS.md ("Gaps").
GAP_FIELDS = ("aiwake", "aiphase", "dtask", "dact", "dbuild", "victim")


class Malformed(Exception):
    pass


class Rec:
    __slots__ = ("frame", "id", "tmpl", "fields", "line")

    def __init__(self, frame, oid, fields, line):
        self.frame, self.id, self.fields, self.line = frame, oid, fields, line
        self.tmpl = fields.get("tmpl", "?")


def parse(no, line):
    m = HEADER.match(line)
    if not m:
        raise Malformed(f"line {no}: not a TRACE record: {line[:120]}")
    frame, oid, rest = int(m.group(1)), int(m.group(2)), m.group(3) or ""
    fields = {}
    for tok in rest.split():
        name, sep, value = tok.partition("=")
        if not sep or not name:
            raise Malformed(f"line {no}: token {tok!r} is not name=value")
        if name in fields:
            raise Malformed(f"line {no}: field {name!r} repeated")
        fields[name] = value
    return Rec(frame, oid, fields, line)


def load(path):
    """Records grouped by frame then id (file order kept)."""
    try:
        with open(path, encoding="utf-8", errors="replace") as fh:
            text = fh.read()
    except OSError as e:
        raise Malformed(f"{path}: {e}") from None
    lines = text.split("\n")
    last = lines.pop()  # "" when the file ends with a newline, else the unterminated last line
    frames = {}
    for no, line in enumerate(lines, 1):
        if not line.startswith("TRACE "):
            continue
        rec = parse(no, line)
        block = frames.setdefault(rec.frame, {})
        if rec.id in block:
            raise Malformed(f"{path}:{no}: id {rec.id} repeated in frame {rec.frame}")
        block[rec.id] = rec
    if last.startswith("TRACE "):
        print(f"warning: {path}: dropped a final line without a newline", file=sys.stderr)
    return frames


def same(a, b, tol):
    if a == b:
        return True
    try:
        return abs(float(a) - float(b)) <= tol
    except ValueError:
        pass
    pa, pb = a.split(","), b.split(",")
    if len(pa) > 1 and len(pa) == len(pb):
        try:
            return all(abs(float(x) - float(y)) <= tol for x, y in zip(pa, pb))
        except ValueError:
            return False
    return False


def name_list(text):
    if text is None:
        return None
    names = []
    for chunk in text.replace(",", " ").split():
        if chunk not in names:
            names.append(chunk)
    return names


def id_list(text):
    if text is None:
        return None
    ids = set()
    for chunk in text.replace(",", " ").split():
        try:
            ids.add(int(chunk))
        except ValueError:
            raise ValueError(f"--ids {text!r}: want comma-separated object ids") from None
    return ids


def rec_fields(rec, only, all_fields):
    if only is not None:
        return [n for n in only if n in rec.fields]
    names = [n for n in rec.fields if all_fields or n not in GAP_FIELDS]
    return names


def diff_records(p, c, only, all_fields, tol):
    """The differing fields of one (frame, id) pair, in field order: (name, port value, cpp value)."""
    names = rec_fields(p, only, all_fields)
    names += [n for n in rec_fields(c, only, all_fields) if n not in names]
    out = []
    for name in names:
        a, b = p.fields.get(name), c.fields.get(name)
        if a is None or b is None or not same(a, b, tol):
            out.append((name, a, b))
    return out


def first_per_object(port, cpp, only, all_fields, ids, tol):
    """For each id, (frame, differences) at its earliest differing frame; differences is a string for a
    one-sided record, else the list from diff_records."""
    found = {}
    for oid in sorted(set(oid for block in port.values() for oid in block) | set(oid for block in cpp.values() for oid in block)):
        if ids is not None and oid not in ids:
            continue
        frames = sorted(
            f
            for f in set(port) | set(cpp)
            if (oid in port.get(f, {}) or oid in cpp.get(f, {}))
        )
        for f in frames:
            p, c = port.get(f, {}).get(oid), cpp.get(f, {}).get(oid)
            if p is None:
                found[oid] = (f, [("(record)", "absent", c.fields.get("tmpl", "?"))])
                break
            if c is None:
                found[oid] = (f, [("(record)", p.fields.get("tmpl", "?"), "absent")])
                break
            diffs = diff_records(p, c, only, all_fields, tol)
            if diffs:
                found[oid] = (f, diffs, p, c)
                break
    return found


def show(v):
    return "<absent>" if v is None else v


def window_lines(port, cpp, oid, first, around, only, all_fields, tol):
    """The side-by-side window: frames `first-around .. first+around` for one object, one line per side
    per frame, limited to the fields that differ anywhere in the window."""
    lo, hi = first - around, first + around
    frames = sorted(f for f in set(port) | set(cpp) if lo <= f <= hi and (oid in port.get(f, {}) or oid in cpp.get(f, {})))
    if not frames:
        return []
    names = []
    for f in frames:
        p, c = port.get(f, {}).get(oid), cpp.get(f, {}).get(oid)
        if p is None or c is None:
            continue
        for name, _, _ in diff_records(p, c, only, all_fields, tol):
            if name not in names:
                names.append(name)
    if only is not None:
        names = [n for n in only if n in names] or [n for n in only]
    if not names:
        names = ["tmpl"]
    out = []
    for f in frames:
        p, c = port.get(f, {}).get(oid), cpp.get(f, {}).get(oid)
        for label, rec in (("port", p), ("cpp ", c)):
            if rec is None:
                out.append(f"  f={f} {label} <absent>")
                continue
            text = " ".join(f"{n}={rec.fields.get(n, '-')}" for n in names)
            out.append(f"  f={f} {label} {text}")
    return out


def summarize(frames, ids):
    records = sum(len(b) for b in frames.values())
    objects = sorted(set(oid for b in frames.values() for oid in b))
    if ids is not None:
        objects = [o for o in objects if o in ids]
    span = f"frames {min(frames)}..{max(frames)}" if frames else "no frames"
    return f"{len(frames)} frames, {records} records, {span}, {len(objects)} ids"


def main():
    ap = argparse.ArgumentParser(description="First divergence between a port and a C++ per-object trace.")
    ap.add_argument("port", help="trace written by the port (ZR_TRACE / the harness `trace` request)")
    ap.add_argument("cpp", help="trace written by the C++ build (ZR_TRACE)")
    ap.add_argument("--ids", metavar="ID[,ID...]", help="compare only these object ids")
    ap.add_argument("--fields", metavar="NAME[,NAME...]", help="compare only these fields (default: every field except the documented gaps)")
    ap.add_argument("--all-fields", action="store_true", help="also compare the documented gap fields (aiwake, aiphase, dtask, dact, dbuild, victim)")
    ap.add_argument("--tol", type=float, default=0.0, help="absolute float tolerance (default 0.0: the same printed value)")
    ap.add_argument("--window", type=int, default=0, metavar="N", help="show N frames either side of the first difference (default 0)")
    ap.add_argument("--max-report", type=int, default=12, help="differing fields printed for the first frame (default 12)")
    opts = ap.parse_args()
    try:
        only = name_list(opts.fields)
        ids = id_list(opts.ids)
        if opts.tol < 0:
            raise ValueError("--tol must not be negative")
        if opts.window < 0:
            raise ValueError("--window must not be negative")
        port, cpp = load(opts.port), load(opts.cpp)
    except (Malformed, ValueError) as e:
        print(f"error: {e}", file=sys.stderr)
        return 2
    for name, frames in (("port", port), ("cpp", cpp)):
        if not frames:
            print(f"error: no TRACE records in the {name} trace", file=sys.stderr)
            return 2
    print(f"port: {summarize(port, ids)}")
    print(f"cpp:  {summarize(cpp, ids)}")
    if ids is not None:
        print(f"comparing ids {','.join(str(i) for i in sorted(ids))}")
    if only is not None:
        print(f"comparing fields {','.join(only)} (tolerance {opts.tol:g})")
    else:
        if not opts.all_fields:
            print(f"skipping gap fields {','.join(GAP_FIELDS)} (--all-fields compares them)")
        if opts.tol > 0:
            print(f"tolerance {opts.tol:g}")

    found = first_per_object(port, cpp, only, opts.all_fields, ids, opts.tol)
    if not found:
        print("identical: no traced object differs")
        return 0

    ordered = sorted(found.items(), key=lambda kv: (kv[1][0], kv[0]))
    first_id, (first_frame, first_diffs, *_rest) = ordered[0]
    tmpl = (port.get(first_frame, {}).get(first_id) or cpp.get(first_frame, {}).get(first_id)).tmpl
    print(f"\nfirst difference: frame {first_frame}, id {first_id} ({tmpl})")
    for name, a, b in first_diffs[: opts.max_report]:
        if name == "(record)":
            print(f"  only in {'cpp' if a == 'absent' else 'port'}")
        else:
            print(f"  field {name}: port {show(a)}, cpp {show(b)}")
    if len(first_diffs) > opts.max_report:
        print(f"  ... {len(first_diffs) - opts.max_report} more in this frame")

    print("\nper object (earliest differing frame and its first field):")
    for oid, entry in ordered:
        frame, diffs = entry[0], entry[1]
        rec = (port.get(frame, {}).get(oid) or cpp.get(frame, {}).get(oid))
        name, a, b = diffs[0]
        if name == "(record)":
            detail = f"only in {'cpp' if a == 'absent' else 'port'} ({rec.tmpl})"
        else:
            detail = f"field {name}: port {show(a)}, cpp {show(b)}"
        print(f"  id={oid} ({rec.tmpl}): frame {frame}, {detail}")

    if opts.window > 0:
        print(f"\nwindow around frame {first_frame}, id {first_id} ({tmpl}), differing fields:")
        for line in window_lines(port, cpp, first_id, first_frame, opts.window, only, opts.all_fields, opts.tol):
            print(line)

    return 1


if __name__ == "__main__":
    sys.exit(main())
