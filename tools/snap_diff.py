#!/usr/bin/env python3
"""First-divergence diff of two per-frame state snapshots (ROADMAP step 3.1, docs/HARNESS.md).

Usage: python tools/snap_diff.py PORT CPP [--pos-tol X] [--hp-tol X] [--fields NAME ...] [--max-report N]
                                         [--key id|tmpl] [--skip TAG.FIELD[=VALUE] ...] [--from-frame N]

Both files hold `SNAP` records: the port writes them with `ZR_SNAPSHOT=<file>` (crates/zr-game/src/snapshot.rs), the
patched C++ build with its read-only printfs (docs/HARNESS.md). Lines that do not start with `SNAP ` are ignored, so
a C++ log with other output can be passed as it is. Records are aligned by frame (the `FRAME` header of each block)
and by key: player `P p`, object `O id`, factory queue entry `Q obj i`, AI team queue entry `T p q i`, script team
`S id`. A final line without a newline (a killed run) is dropped with its frame block, with a warning.

Keys (`--key`):
- `id` (default): the object/team id, as above.
- `tmpl`: object ids differ between the builds until the id allocation matches, so an `O` record is keyed by its
  template and its per-frame creation ordinal instead: the template name and the 0-based count of earlier live objects
  with the same template in the frame (the writer lists objects in id order, which is creation order). `Q` entries are
  keyed by their factory's template key, and `S` records by their prototype and per-frame ordinal. This aligns two runs
  whose ids drift, at the cost of reporting a late-created object whose earlier same-template twin died as a different
  object. The records' own id fields (`O.id`, `Q.obj`, `S.id`) are not compared in this mode, since they are the thing
  that differs; id fields inside values (`members`, `atk`, `bld`, `recruit`, `work`'s factory) are still compared as
  ids: those differ while the allocation differs, and `--skip` can silence them until it is fixed. `T` records keep
  their player/queue/index key (no object ids).

Comparison:
- Exact: every field except the floats (orders, states, owners, templates, queue contents, counts, team members).
- Positions `x y z` within --pos-tol (default 1.0); health `hp` and construction `cons` within --hp-tol (default 0.5).
- `crc` is never compared (the two builds hash different state). The hex-bit fields (`xb yb zb hpb consb`) are compared
  exactly only when named in --fields. `--fields pos` means `x y z`.
- A record present on one side only, and a frame present on one side only, are differences.
- `--skip TAG.FIELD` ignores one field of one record tag on both sides; `--skip TAG.FIELD=VALUE` drops the records
  that have that value (a trailing `*` in VALUE matches a prefix, `O.tmpl=Car*`). Both are for known gaps the run cannot
  match yet (docs/HARNESS.md, "Fidelity gaps"): the output still prints how many records and fields were skipped, so
  nothing is hidden silently.
- `--from-frame N` compares frames N and later only, for localising the first divergence past known start-up gaps
  (the frame-0 block catches the C++ `startNewGame` transients and the port's map-load differences). Frames before N
  are dropped before comparing, so a frame missing on one side there is not a difference either; the header says which
  frames were compared.

Reports the first frame that differs, then the first differing record and field in it, then a summary.

Exit status: 0 identical over every frame, 1 divergence, 2 unreadable or malformed snapshot, a snapshot with no SNAP
records (either side), or a bad --skip/--key value.
"""
import argparse
import re
import sys

HEADER = re.compile(r"^SNAP f=(\d+) (\S+)(?: (.*))?$")
# Key fields of each record tag, in the order the writer emits them.
KEY_FIELDS = {"FRAME": (), "P": ("p",), "O": ("id",), "Q": ("obj", "i"), "T": ("p", "q", "i"), "S": ("id",)}
POS = ("x", "y", "z")
HP_LIKE = ("hp", "cons")
BITS = ("xb", "yb", "zb", "hpb", "consb")
NEVER = ("crc",)
# The raw ids, which `--key tmpl` replaces as the record key (the builds allocate them differently).
IDENTITY_FIELDS = {("O", "id"), ("Q", "obj"), ("S", "id")}


class Malformed(Exception):
    pass


class Rec:
    __slots__ = ("frame", "tag", "key", "fields", "line", "names")

    def __init__(self, frame, tag, key, fields, line):
        self.frame, self.tag, self.key, self.fields, self.line = frame, tag, key, fields, line
        self.names = KEY_FIELDS[tag]


def parse(no, line):
    m = HEADER.match(line)
    if not m:
        raise Malformed(f"line {no}: not a SNAP record: {line[:120]}")
    frame, tag, rest = int(m.group(1)), m.group(2), m.group(3) or ""
    if tag not in KEY_FIELDS:
        raise Malformed(f"line {no}: unknown record tag {tag!r}")
    fields = {}
    for tok in rest.split():
        name, sep, value = tok.partition("=")
        if not sep or not name:
            raise Malformed(f"line {no}: token {tok!r} is not name=value")
        if name in fields:
            raise Malformed(f"line {no}: field {name!r} repeated")
        fields[name] = value
    try:
        key = (tag,) + tuple(fields[k] for k in KEY_FIELDS[tag])
    except KeyError as e:
        raise Malformed(f"line {no}: {tag} record has no {e.args[0]}= key") from None
    return Rec(frame, tag, key, fields, line)


def load(path, key_mode="id"):
    """Records grouped by frame (file order kept).

    A final line without a newline is a write cut short by a killed run: it is dropped, and so is the whole frame
    block it belongs to (the block may be incomplete). A warning says so. A malformed line that ends with a newline is
    an error, as is a repeated key inside one frame. With `key_mode == "tmpl"` the keys are rewritten afterwards (see
    `retag_by_template`).
    """
    with open(path, encoding="utf-8", errors="replace") as fh:
        text = fh.read()
    lines = text.split("\n")
    last = lines.pop()  # "" when the file ends with a newline, else the unterminated last line
    frames = {}
    for no, line in enumerate(lines, 1):
        if not line.startswith("SNAP "):
            continue
        rec = parse(no, line)
        block = frames.setdefault(rec.frame, {})
        if rec.key in block:
            raise Malformed(f"{path}:{no}: {rec.line[:80]!r} repeats a key already in frame {rec.frame}")
        block[rec.key] = rec
    if last.startswith("SNAP "):
        m = re.match(r"SNAP f=(\d+)", last)
        note = ""
        if m:
            frames.pop(int(m.group(1)), None)
            note = f" and the incomplete frame {m.group(1)} block"
        print(f"warning: {path}: dropped a final line without a newline{note}", file=sys.stderr)
    if key_mode == "tmpl":
        retag_by_template(frames)
    return frames


def retag_by_template(frames):
    """Rewrite the keys of one snapshot to template + per-frame creation ordinal (see the module docstring).

    The writer lists a block's records in a fixed order (`FRAME`, `P`, `O`, `Q`, `T`, `S`), and objects in id order
    (= creation order), so a single ordered pass can number the objects of each template and then point that frame's
    `Q` records at their factory's new key.
    """
    for block in frames.values():
        objects = {}  # id -> new key, for the Q records that follow the O records
        object_ordinal = {}
        team_ordinal = {}
        retagged = {}
        for rec in block.values():
            if rec.tag == "O":
                tmpl = rec.fields.get("tmpl", "?")
                n = object_ordinal.get(tmpl, 0)
                object_ordinal[tmpl] = n + 1
                rec.key = ("O", tmpl, str(n))
                rec.names = ("tmpl", "n")
                objects[rec.fields.get("id")] = rec.key
            elif rec.tag == "Q":
                factory = objects.get(rec.fields.get("obj"))
                # No O record for the factory (a hand-made file, or the factory died this frame): keep the raw id.
                obj = f"{factory[1]}#{factory[2]}" if factory else f"id:{rec.fields.get('obj')}"
                rec.key = ("Q", obj, rec.fields.get("i", "?"))
                rec.names = ("obj", "i")
            elif rec.tag == "S":
                proto = rec.fields.get("proto", "?")
                n = team_ordinal.get(proto, 0)
                team_ordinal[proto] = n + 1
                rec.key = ("S", proto, str(n))
                rec.names = ("proto", "n")
            retagged[rec.key] = rec
        block.clear()
        block.update(retagged)


def same(name, a, b, pos_tol, hp_tol):
    if a == b:
        return True
    if name in POS or name in HP_LIKE:
        try:
            fa, fb = float(a), float(b)
        except ValueError:
            return False
        return abs(fa - fb) <= (pos_tol if name in POS else hp_tol)
    return False


def selected(name, only):
    if name in NEVER:
        return False
    if only is None:
        return name not in BITS
    return name in only


def describe(rec):
    # rec.key holds the tag first, then one value per name.
    parts = [f"{name}={rec.key[i + 1]}" for i, name in enumerate(rec.names)]
    return " ".join([rec.tag] + parts)


class Skip:
    """`--skip` rules: ignored fields, and dropped records (`TAG.FIELD=VALUE`, VALUE may end in `*` for a prefix)."""

    def __init__(self, items):
        self.fields = set()
        self.records = []
        for item in items:
            left, sep, value = item.partition("=")
            tag, dot, field = left.partition(".")
            if not tag or not dot or not field:
                raise ValueError(f"--skip {item!r}: want TAG.FIELD or TAG.FIELD=VALUE")
            if tag not in KEY_FIELDS:
                raise ValueError(f"--skip {item!r}: unknown record tag {tag!r}")
            if sep:
                self.records.append((tag, field, value))
            else:
                self.fields.add((tag, field))

    def drops(self, rec):
        for tag, field, value in self.records:
            if rec.tag != tag:
                continue
            v = rec.fields.get(field)
            if v is None:
                continue
            if value.endswith("*") and v.startswith(value[:-1]):
                return True
            if v == value:
                return True
        return False

    def skips_field(self, tag, name):
        return (tag, name) in self.fields

    def describe(self):
        parts = []
        if self.fields:
            parts.append("fields " + ", ".join(f"{t}.{f}" for t, f in sorted(self.fields)))
        if self.records:
            parts.append("records " + ", ".join(f"{t}.{f}={v}" for t, f, v in self.records))
        return "; ".join(parts)


def diff_frame(port, cpp, opts):
    """Differences in one frame as (port record or None, cpp record or None, field, port value, cpp value)."""
    out = []
    for key, p in port.items():
        if opts.skip.drops(p):
            continue
        c = cpp.get(key)
        if c is not None and opts.skip.drops(c):
            continue
        if c is None:
            out.append((p, None, "(record)", "present", "absent"))
            continue
        names = list(p.fields) + [k for k in c.fields if k not in p.fields]
        for name in names:
            if not selected(name, opts.only) or opts.skip.skips_field(p.tag, name):
                continue
            if opts.key == "tmpl" and (p.tag, name) in IDENTITY_FIELDS:
                continue  # the raw id is the key that differs between the builds, not a game fact
            a, b = p.fields.get(name), c.fields.get(name)
            if a is None or b is None or not same(name, a, b, opts.pos_tol, opts.hp_tol):
                out.append((p, c, name, a, b))
    for key, c in cpp.items():
        if key not in port and not opts.skip.drops(c):
            out.append((None, c, "(record)", "absent", "present"))
    return out


def show(v):
    return "<absent>" if v is None else v


def expand(names):
    if names is None:
        return None
    only = set()
    for n in names:
        only.update(POS if n == "pos" else (n,))
    return only


def main():
    ap = argparse.ArgumentParser(description="First divergence between a port and a C++ state snapshot.")
    ap.add_argument("port", help="snapshot written by the port (ZR_SNAPSHOT)")
    ap.add_argument("cpp", help="snapshot written by the C++ build (read-only printfs)")
    ap.add_argument("--pos-tol", type=float, default=1.0, help="tolerance for x y z (default 1.0)")
    ap.add_argument("--hp-tol", type=float, default=0.5, help="tolerance for hp and cons (default 0.5)")
    ap.add_argument("--fields", nargs="+", metavar="NAME", help="compare only these fields ('pos' is x y z)")
    ap.add_argument("--max-report", type=int, default=12, help="differing records printed for the first frame (default 12)")
    ap.add_argument(
        "--key",
        choices=("id", "tmpl"),
        default="id",
        help="record key: id (default) or template + per-template creation ordinal (object ids differ between builds)",
    )
    ap.add_argument(
        "--skip",
        nargs="+",
        default=[],
        metavar="TAG.FIELD[=VALUE]",
        help="ignore a field, or drop records whose field has VALUE (a trailing * matches a prefix), on both sides (for known gaps; counted in the output)",
    )
    ap.add_argument(
        "--from-frame",
        type=int,
        default=0,
        metavar="N",
        help="compare only frames >= N (default 0: every frame); for localising a divergence past known start-up gaps",
    )
    opts = ap.parse_args()
    opts.only = expand(opts.fields)
    try:
        opts.skip = Skip(opts.skip)
        port, cpp = load(opts.port, opts.key), load(opts.cpp, opts.key)
    except (OSError, Malformed, ValueError) as e:
        print(f"error: {e}", file=sys.stderr)
        return 2
    for name, snap in (("port", port), ("cpp", cpp)):
        if not snap:
            print(f"error: no SNAP records in the {name} snapshot", file=sys.stderr)
            return 2
    for name, snap in (("port", port), ("cpp", cpp)):
        n = sum(len(b) for b in snap.values())
        print(f"{name}: {len(snap)} frames, {n} records, frames {min(snap)}..{max(snap)}")
    if opts.from_frame > 0:
        # Frames before N are not compared at all (they may be missing on one side without that being a difference).
        for snap in (port, cpp):
            for f in [f for f in snap if f < opts.from_frame]:
                del snap[f]
        for name, snap in (("port", port), ("cpp", cpp)):
            if not snap:
                print(f"error: no SNAP records in the {name} snapshot at or after frame {opts.from_frame}", file=sys.stderr)
                return 2
        print(f"comparing frames {opts.from_frame}..{max(max(port), max(cpp))} (--from-frame {opts.from_frame})")
    if opts.skip.fields or opts.skip.records:
        counts = []
        for name, snap in (("port", port), ("cpp", cpp)):
            counts.append(f"{sum(1 for b in snap.values() for r in b.values() if opts.skip.drops(r))} {name}")
        print(f"skipping by --skip: {opts.skip.describe()} ({', '.join(counts)} records)")

    bad = []  # (frame, [differences] or a string for a frame missing on one side)
    for f in sorted(set(port) | set(cpp)):
        if f not in port:
            bad.append((f, f"frame {f} is missing from the port snapshot"))
        elif f not in cpp:
            bad.append((f, f"frame {f} is missing from the cpp snapshot"))
        else:
            ds = diff_frame(port[f], cpp[f], opts)
            if ds:
                bad.append((f, ds))
    if not bad:
        common = len(set(port) & set(cpp))
        print(f"identical: {common} frames, every record and compared field matches")
        return 0

    first_frame, first = bad[0]
    print(f"\nfirst divergence: frame {first_frame}")
    if isinstance(first, str):
        print(f"  {first}")
    else:
        for p, c, name, a, b in first[: opts.max_report]:
            rec = p or c
            where = describe(rec)
            if name == "(record)":
                print(f"  {where}: {'only in port' if p else 'only in cpp'}")
            else:
                print(f"  {where}: field {name}: port {show(a)}, cpp {show(b)}")
        if len(first) > opts.max_report:
            print(f"  ... {len(first) - opts.max_report} more in this frame")
        p, c = first[0][0], first[0][1]
        if p is not None:
            print(f"  port  {p.line}")
        if c is not None:
            print(f"  cpp   {c.line}")
    last_frame = bad[-1][0]
    print(f"\nframes that differ: {len(bad)} (first {first_frame}, last {last_frame})")
    return 1


if __name__ == "__main__":
    sys.exit(main())
