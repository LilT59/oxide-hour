#!/usr/bin/env python3
"""One-command compare of the port against a stored C++ reference (docs/HARNESS.md, lane 139).

Usage:
  python -I tools/zr_compare.py CODE [--seed N] [--frames N] [--every N] [--ref-dir DIR]
      [--cpp-snap FILE] [--cpp-rng FILE] [--cpp-coarse FILE] [--slot N] [--no-build]
      [--harness EXE] [--out DIR] [--skip TAG.FIELD[=VALUE] ...] [--no-skip] [--pos-tol X]
      [--hp-tol X] [--key id|tmpl] [--from-frame N] [--max-report N] [--max-minor N]
      [--timeline] [--tol-frames N] [--max-events N] [--timeout SECONDS]

Builds `zr-harness` (release) into `target/slot-N`, loads the matchup from `tools/matchups.json`,
streams a bits snapshot and the RNG trace, steps to `--frames`, then reports:

  (a) the first exact snapshot divergence, as `tools/snap_diff.py` reports it;
  (b) the first *meaningful* divergence per AGENTS.md ("near bit-exact" bar), with every earlier
      difference listed as minor (frame, field, size);
  (c) the first RNG mismatch with context on both sides and the C++ `at=` call sites;
  (d) the first divergent object's record for the ~10 frames before the divergence, side by side;
  (e) the port CRC at the end.

`--timeline` also runs the port at the coarse cadence to the last frame of `cpp_coarse.snap` (when
present) and compares the game-event timelines (`tools/timeline.py`).

Reference store: `<ref-dir>/` defaults to `zr-refs\\<CODE>_s<SEED>` next to the repo (in the repo's
parent directory) with `cpp_fine.snap`, `cpp_rng.txt`, optional `cpp_coarse.snap` and
`manifest.json`. `--cpp-snap` / `--cpp-rng` / `--cpp-coarse` override individual files.

Exit status: 0 identical (snapshot on the common frames and RNG), 1 a divergence, 2 an error.
"""
import argparse
import json
import math
import os
import re
import subprocess
import sys
import tempfile
import time
from pathlib import Path
from types import SimpleNamespace

HERE = Path(__file__).resolve().parent
REPO = HERE.parent
sys.path.insert(0, str(HERE))

import rng_diff  # noqa: E402  (after the sys.path line: `python -I` does not add the script dir)
import snap_diff  # noqa: E402

DEFAULT_REF_ROOT = REPO.parent / "zr-refs"
DEFAULT_SKIP = ("T.q=build", "T.q=ready")
# One pathfinding cell is 10 world units (AGENTS.md: "position difference of about a pathfinding cell
# (10 units) or more"). Positions are compared as a 3D distance.
POS_CELL = 10.0
MEANINGFUL_O = frozenset(("ai", "atk", "bld", "owner", "tmpl", "q"))
MEANINGFUL_P = frozenset(("money", "power"))
MEANINGFUL_FRAME = frozenset(("objs", "players"))
STRUCTURAL_TAGS = frozenset(("Q", "T", "S"))
O_FIELDS = ("id", "tmpl", "owner", "x", "y", "z", "hp", "ai", "cons", "q", "atk", "bld",
            "xb", "yb", "zb", "hpb", "consb")
FRAME_HEADER = re.compile(r"^SNAP f=(\d+) ")


class HarnessError(Exception):
    pass


# --------------------------------------------------------------------------------------------------
# matchup registry, build and harness


def load_registry(path=None):
    path = path or HERE / "matchups.json"
    with open(path, encoding="utf-8") as fh:
        return json.load(fh)


def matchup(registry, code):
    table = registry.get("matchups", {})
    if code not in table:
        known = ", ".join(sorted(table))
        raise SystemExit(f"error: unknown matchup {code!r}; tools/matchups.json has {known}")
    cfg = dict(table[code])
    n = len(cfg.get("factions", []))
    if n == 0:
        raise SystemExit(f"error: matchup {code} has no factions")
    cfg.setdefault("starts", list(range(n)))
    cfg.setdefault("teams", [0] * n)
    if len(cfg["starts"]) != n or len(cfg["teams"]) != n:
        raise SystemExit(f"error: matchup {code}: factions, starts and teams must have the same length")
    cfg["code"] = code
    return cfg


def parse_skips(items):
    """`--skip` items: one or more TAG.FIELD[=VALUE], each optionally a whitespace-separated list.

    `snap_diff.py`'s examples quote the whole list (`--skip "T.q=build T.q=ready"`), while argparse's
    `nargs="+"` also accepts them as separate arguments; both mean the same here. Values never contain
    spaces (the snapshot escapes them as `%20`).
    """
    return [part for item in (items or []) for part in item.split()]


def harness_path(slot):
    target = (REPO / "target" / f"slot-{slot}").resolve()
    name = "zr-harness.exe" if os.name == "nt" else "zr-harness"
    return target / "release" / name, target


def build_harness(slot):
    exe, target = harness_path(slot)
    env = os.environ.copy()
    env["CARGO_TARGET_DIR"] = target.as_posix()
    print(f"building zr-harness (release) into {target.as_posix()} ...", flush=True)
    t0 = time.time()
    proc = subprocess.run(["cargo", "build", "--release", "-p", "zr-harness"], cwd=REPO, env=env)
    secs = time.time() - t0
    if proc.returncode != 0:
        raise HarnessError(f"cargo build failed (exit {proc.returncode}) after {secs:.1f}s")
    print(f"built in {secs:.1f}s: {exe}", flush=True)
    if not exe.exists():
        raise HarnessError(f"cargo reported success but {exe} does not exist")
    return exe


def load_request(cfg, seed, snapshot=None):
    players = []
    for i, faction in enumerate(cfg["factions"]):
        players.append({
            "faction": faction,
            "ai": True,
            "difficulty": cfg["difficulty"],
            "start": cfg["starts"][i],
            "team": cfg["teams"][i],
            "color": -1,
        })
    req = {"cmd": "load", "map": cfg["map"], "seed": seed, "players": players}
    if snapshot is not None:
        req["snapshot"] = snapshot
    return req


def run_harness(exe, requests, timeout, extra_env=None):
    """Drive one session: send every request, read every reply (the harness answers in order)."""
    env = os.environ.copy()
    env.pop("ZR_RNG_TRACE", None)
    if extra_env:
        env.update(extra_env)
    text = "".join(json.dumps(r) + "\n" for r in requests)
    t0 = time.time()
    try:
        proc = subprocess.run([str(exe)], input=text, capture_output=True, text=True,
                              encoding="utf-8", errors="replace", timeout=timeout, env=env)
    except subprocess.TimeoutExpired:
        raise HarnessError(f"harness did not finish within {timeout:.0f}s") from None
    secs = time.time() - t0
    replies = []
    for line in proc.stdout.splitlines():
        line = line.strip()
        if not line:
            continue
        try:
            replies.append(json.loads(line))
        except json.JSONDecodeError:
            raise HarnessError(f"harness wrote a non-JSON stdout line: {line[:200]!r}") from None
    if len(replies) != len(requests):
        tail = [l for l in (proc.stderr or "").strip().splitlines() if l][-12:]
        raise HarnessError(f"harness exited {proc.returncode} after {len(replies)}/{len(requests)} replies"
                           + (f"; stderr: {' | '.join(tail)}" if tail else ""))
    for req, rep in zip(requests, replies):
        if not rep.get("ok"):
            raise HarnessError(f"{req.get('cmd')}: {rep.get('error', 'no error text')}")
    return replies, secs


def run_fine(exe, cfg, seed, frames, every, snap_path, rng_path, timeout):
    requests = [load_request(cfg, seed, {"path": str(snap_path), "every": every, "bits": True})]
    if frames > 0:
        requests.append({"cmd": "step", "until": frames})
    requests += [{"cmd": "snapshot"}, {"cmd": "crc"}, {"cmd": "quit"}]
    replies, secs = run_harness(exe, requests, timeout, {"ZR_RNG_TRACE": str(rng_path)})
    crc = replies[-2]
    if crc.get("frame") != frames:
        print(f"warning: asked for frame {frames}, the harness reports frame {crc.get('frame')}",
              file=sys.stderr)
    return crc, secs


def last_snap_frame(path):
    last = None
    with open(path, encoding="utf-8", errors="replace") as fh:
        for line in fh:
            m = FRAME_HEADER.match(line)
            if m:
                last = int(m.group(1))
    if last is None:
        raise ValueError(f"no SNAP records in {path}")
    return last


def run_coarse(exe, cfg, seed, horizon, every, snap_path, timeout):
    requests = [load_request(cfg, seed, {"path": str(snap_path), "every": every, "bits": False})]
    if horizon > 0:
        requests.append({"cmd": "step", "until": horizon})
    requests += [{"cmd": "snapshot"}, {"cmd": "quit"}]
    replies, secs = run_harness(exe, requests, timeout)
    # The `snapshot` stop reply carries no frame; the step reply (or the load reply at horizon 0) does.
    frame = (replies[1] if horizon > 0 else replies[0]).get("frame")
    return frame, secs


# --------------------------------------------------------------------------------------------------
# snapshot comparison


def pos_distance(a, b):
    try:
        return math.dist([float(a[k]) for k in snap_diff.POS], [float(b[k]) for k in snap_diff.POS])
    except (KeyError, ValueError):
        return None


def diff_size(p, c, name, a, b):
    if name in snap_diff.POS and p is not None and c is not None:
        d = pos_distance(p.fields, c.fields)
        if d is not None:
            return f"distance {d:.4f}"
    if name in snap_diff.POS or name in snap_diff.HP_LIKE:
        try:
            return f"size {abs(float(a) - float(b)):.4f}"
        except (TypeError, ValueError):
            return "exact"
    return "exact"


def far_position_objects(diffs):
    """Keys of the O records in one frame whose x/y/z moved at least one pathfinding cell."""
    far = set()
    for p, c, name, a, b in diffs:
        if p is None or c is None or p.tag != "O" or name not in snap_diff.POS or p.key in far:
            continue
        d = pos_distance(p.fields, c.fields)
        if d is not None and d >= POS_CELL:
            far.add(p.key)
    return far


def meaningful_reason(diff, far):
    p, c, name, a, b = diff
    rec = p or c
    if rec is None:
        return None
    if p is None or c is None:
        return "record present on one side only (created/destroyed/captured)"
    if name == "(record)":
        return "record present on one side only"
    tag = rec.tag
    if tag in STRUCTURAL_TAGS:
        return f"{tag} record differs"
    if tag == "O":
        if name in MEANINGFUL_O:
            return f"O.{name} is a decision/state field"
        if name in snap_diff.POS and rec.key in far:
            d = pos_distance(p.fields, c.fields)
            return (f"position moved {d:.4f} units (>= one pathfinding cell, {POS_CELL:g})"
                    if d is not None else "position moved >= 10 units (one pathfinding cell)")
    if tag == "P" and name in MEANINGFUL_P:
        return f"P.{name} is a decision/state field"
    if tag == "FRAME" and name in MEANINGFUL_FRAME:
        return f"FRAME.{name}: the object/player list length differs"
    return None


def describe_record(rec):
    """`snap_diff.describe` plus the template for an object record (keys alone are terse)."""
    text = snap_diff.describe(rec)
    if rec.tag == "O" and "tmpl" in rec.fields:
        text += f" tmpl={rec.fields['tmpl']}"
    return text


def object_diff(diff):
    p, c, name, a, b = diff
    rec = p or c
    return rec is not None and rec.tag == "O"


def compare_snapshots(port, cpp, opts, from_frame, max_minor):
    max_common = min(max(port), max(cpp))
    if from_frame > max_common:
        raise ValueError(f"--from-frame {from_frame} is past the last commonly compared frame {max_common}")
    res = {
        "max_port": max(port),
        "max_cpp": max(cpp),
        "max_common": max_common,
        "from_frame": from_frame,
        "first_exact": None,
        "first_meaningful": None,
        "first_object": None,
        "minor": [],
        "minor_total": 0,
        "minor_frames": 0,
        "differing_frames": 0,
        "last_diff": None,
        "stopped_at": None,
    }
    frames = sorted(f for f in set(port) | set(cpp) if from_frame <= f <= max_common)
    for f in frames:
        if f not in port or f not in cpp:
            side = "port" if f not in port else "cpp"
            desc = f"frame {f} is missing from the {side} snapshot"
            res["first_exact"] = res["first_exact"] or (f, desc)
            res["differing_frames"] += 1
            res["last_diff"] = f
            res["first_meaningful"] = (f, [(None, None, "(frame)", desc, None, desc)])
            res["stopped_at"] = f
            break
        diffs = snap_diff.diff_frame(port[f], cpp[f], opts)
        if not diffs:
            continue
        res["differing_frames"] += 1
        res["last_diff"] = f
        if res["first_exact"] is None:
            res["first_exact"] = (f, diffs)
        if res["first_object"] is None:
            first = next((d for d in diffs if object_diff(d)), None)
            if first is not None:
                res["first_object"] = (f, first)
        far = far_position_objects(diffs)
        reasons = [(d, meaningful_reason(d, far)) for d in diffs]
        meaningful = []
        for diff, why in reasons:
            if why:
                p, c, name, a, b = diff
                meaningful.append((p, c, name, a, b, why))
        if meaningful:
            res["first_meaningful"] = (f, meaningful)
            res["stopped_at"] = f
            minors = [d for d, why in reasons if not why]
            for d in minors:
                res["minor_total"] += 1
                if len(res["minor"]) < max_minor:
                    res["minor"].append((f, d, None))
            if minors:
                res["minor_frames"] += 1
            break
        for d in diffs:
            res["minor_total"] += 1
            if len(res["minor"]) < max_minor:
                res["minor"].append((f, d, None))
        res["minor_frames"] += 1
    return res


def show_fields(rec):
    if rec is None:
        return "<absent>"
    names = [n for n in O_FIELDS if n in rec.fields]
    names += [n for n in rec.fields if n not in names]
    return " ".join(f"{n}={rec.fields[n]}" for n in names)


def print_snapshot_report(res, port, cpp, opts, args):
    print(f"snapshot: port {len(port)} frames ({min(port)}..{res['max_port']}), "
          f"cpp {len(cpp)} frames ({min(cpp)}..{res['max_cpp']})")
    print(f"  comparing frames {res['from_frame']}..{res['max_common']}")
    if res["max_port"] != res["max_cpp"]:
        print(f"  horizon: the runs end on different frames (port {res['max_port']}, cpp {res['max_cpp']}); "
              f"frames past {res['max_common']} are not compared")
    if opts.skip.fields or opts.skip.records:
        print(f"  skipping by --skip: {opts.skip.describe()}")

    first = res["first_exact"]
    if first is None:
        print("first exact divergence: none (every compared frame and field matches)")
    else:
        f, diffs = first
        print(f"\nfirst exact divergence: frame {f}")
        if isinstance(diffs, str):
            print(f"  {diffs}")
        else:
            for p, c, name, a, b in diffs[: args.max_report]:
                rec = p or c
                if name == "(record)":
                    print(f"  {snap_diff.describe(rec)}: {'only in port' if p else 'only in cpp'}")
                else:
                    print(f"  {snap_diff.describe(rec)}: field {name}: port {snap_diff.show(a)}, "
                          f"cpp {snap_diff.show(b)}")
            if len(diffs) > args.max_report:
                print(f"  ... {len(diffs) - args.max_report} more in this frame")
            p, c = diffs[0][0], diffs[0][1]
            if p is not None:
                print(f"  port  {p.line}")
            if c is not None:
                print(f"  cpp   {c.line}")

    meaningful = res["first_meaningful"]
    if meaningful is None:
        print("\nfirst meaningful divergence: none (every difference before the end is minor)")
        if res["stopped_at"] is None:
            print(f"  compared through frame {res['max_common']}; "
                  f"{res['differing_frames']} frames differ")
    else:
        f, items = meaningful
        same = first is not None and f == first[0]
        print(f"\nfirst meaningful divergence: frame {f}" + (" (same frame as the first exact divergence)" if same else ""))
        for item in items[: args.max_report]:
            p, c, name, a, b, why = item
            if name == "(frame)":
                print(f"  {why}")
                continue
            rec = p or c
            if name == "(record)":
                print(f"  {describe_record(rec)}: {'only in port' if p else 'only in cpp'}  [{why}]")
            else:
                print(f"  {describe_record(rec)}: field {name}: port {snap_diff.show(a)}, "
                      f"cpp {snap_diff.show(b)}  [{why}]")
        if len(items) > args.max_report:
            print(f"  ... {len(items) - args.max_report} more meaningful in this frame")

    if first is not None:
        print(f"\ndiffs before the first meaningful one (minor under the near bit-exact bar): "
              f"{res['minor_total']} in {res['minor_frames']} frames")
        for f, d, desc in res["minor"]:
            if d is None:
                print(f"  f{f} {desc}")
                continue
            p, c, name, a, b = d
            rec = p or c
            if name == "(record)":
                print(f"  f{f} {snap_diff.describe(rec)}: {'only in port' if p else 'only in cpp'} (exact)")
            else:
                print(f"  f{f} {snap_diff.describe(rec)} field {name}: {snap_diff.show(a)} -> "
                      f"{snap_diff.show(b)} ({diff_size(p, c, name, a, b)})")
        if res["minor_total"] > len(res["minor"]):
            print(f"  ... {res['minor_total'] - len(res['minor'])} more minor differences "
                  f"(--max-minor {args.max_minor})")

    print(f"\nframes that differ: {res['differing_frames']}"
          + (f" (first {first[0]}, last {res['last_diff']})" if first is not None else ""))
    if res["stopped_at"] is not None and meaningful is not None:
        print(f"  (stopped at the first meaningful divergence f{res['stopped_at']}; later frames not scanned)")

    # (d) context for the first divergent object.
    target = res["first_object"]
    if target is None:
        print("\nobject context: no object record differs")
        return
    f, diff = target
    rec = diff[0] or diff[1]
    key = rec.key
    lo = max(min(port), f - 10)
    print(f"\nobject context: {snap_diff.describe(rec)} (frames {lo}..{f})")
    for fr in range(lo, f + 1):
        mark = "*" if fr == f else " "
        print(f" {mark}f{fr} port: {show_fields(port.get(fr, {}).get(key))}")
        print(f" {mark}f{fr} cpp : {show_fields(cpp.get(fr, {}).get(key))}")


# --------------------------------------------------------------------------------------------------
# RNG report


def draw_line(rec):
    if rec["k"] == "I":
        value = f"lo={rec['lo']} hi={rec['hi']} r={rec['r']}"
    else:
        value = (f"lo={rng_diff.f32(rec['lo']):.9g} hi={rng_diff.f32(rec['hi']):.9g} "
                 f"r={rng_diff.f32(rec['r']):.9g}")
    return f"n={rec['n']} f={rec['f']} k={rec['k']} {value} at={rec['at']}"


def print_rng_report(port_path, cpp_path, context):
    port = rng_diff.load(str(port_path))
    cpp = rng_diff.load(str(cpp_path))
    print(f"\nrng: port {len(port)} draws" + (f", frames {port[0]['f']}..{port[-1]['f']}" if port else ""))
    print(f"     cpp  {len(cpp)} draws" + (f", frames {cpp[0]['f']}..{cpp[-1]['f']}" if cpp else ""))
    common = min(len(port), len(cpp))
    first = next((i for i in range(common) if rng_diff.key(port[i]) != rng_diff.key(cpp[i])), None)
    if first is None:
        if len(port) == len(cpp):
            print(f"rng: identical, all {len(port)} draws match (frame, kind, range, result)")
            return False
        short, long_, extra = (("port", "cpp", len(cpp) - len(port)) if len(port) < len(cpp)
                               else ("cpp", "port", len(port) - len(cpp)))
        longer = cpp if long_ == "cpp" else port
        print(f"rng: identical for the first {common} draws; the {short} trace ends there and the "
              f"{long_} trace has {extra} more (first extra draw n={common} f={longer[common]['f']})")
        print(f"  {long_} {draw_line(longer[common])}")
        return True

    pa, ca = port[first], cpp[first]
    differing = rng_diff.differing(pa, ca)
    print(f"\nfirst RNG mismatch at draw n={first}: frame {pa['f']} vs {ca['f']}, "
          f"differs in: {', '.join(differing)}")
    print(f"  port {draw_line(pa)}")
    print(f"  cpp  {draw_line(ca)}")
    if differing == ["frame"] and rng_diff.key(pa)[2:] == rng_diff.key(ca)[2:]:
        print("  note: same kind/range/result, only the frame shifted: a timing difference, minor "
              "unless it changes a decision")
    else:
        print("  note: the draw itself differs (kind/range/result), which changes decisions: meaningful")
    lo = max(0, first - context)
    hi = min(max(len(port), len(cpp)), first + context + 1)
    print(f"\nrng context (draws {lo}..{hi - 1}; '>' marks the first mismatch, both sides):")
    for i in range(lo, hi):
        for side, recs in (("port", port), ("cpp ", cpp)):
            mark = ">" if i == first else " "
            if i < len(recs):
                print(f" {mark}{side} {draw_line(recs[i])}")
            else:
                print(f" {mark}{side} <no draw>")
    return True


# --------------------------------------------------------------------------------------------------
# reference metadata and timeline


def compact(value, limit=160):
    text = value if isinstance(value, str) else json.dumps(value, ensure_ascii=False)
    return text if len(text) <= limit else text[: limit - 3] + "..."


def print_manifest(path, cfg, seed):
    if not path.exists():
        return
    try:
        with open(path, encoding="utf-8") as fh:
            data = json.load(fh)
    except (OSError, json.JSONDecodeError) as e:
        print(f"manifest: unreadable ({e}): {path}")
        return
    if not isinstance(data, dict):
        print(f"manifest: {compact(data)}")
        return
    keys = ("code", "seed", "map", "cpp_map", "difficulty", "factions", "horizon", "frames", "frame",
            "created", "command", "exe", "note")
    parts = [f"{k}={compact(data[k])}" for k in keys if k in data]
    print("manifest: " + (", ".join(parts) if parts else compact(data)))
    # Only code and seed are unambiguous across manifest schemas (lanes may store the C++ map path,
    # the matrix difficulty letter or faction letters for the map/difficulty/factions fields).
    for key, want in (("code", cfg["code"]), ("seed", seed)):
        if key in data and str(data[key]).lower() != str(want).lower():
            print(f"  warning: manifest {key}={data[key]!r} does not match this run's {want!r}")


def run_timeline_tool(port_coarse, cpp_coarse, tol_frames, max_events):
    import timeline as timeline_tool
    try:
        return timeline_tool.compare_files(str(port_coarse), str(cpp_coarse), tol_frames, max_events)
    except (OSError, ValueError, snap_diff.Malformed) as e:
        raise HarnessError(f"timeline: {e}") from None


# --------------------------------------------------------------------------------------------------
# main


def out_dir_for(args, code, seed):
    if args.out is not None:
        return Path(args.out).resolve()
    return (Path(tempfile.gettempdir()) / "zr_compare" / f"{code}_s{seed}").resolve()


def main():
    ap = argparse.ArgumentParser(description="Build, run and compare the port against a stored C++ reference.")
    ap.add_argument("code", help="matchup code (tools/matchups.json: AN1..AN7, AE1..AE3, AH1..AH7, TU1..TU8, FF1..FF2)")
    ap.add_argument("--seed", type=int, default=1)
    ap.add_argument("--frames", type=int, default=3000, help="fine run horizon in logic frames (default 3000)")
    ap.add_argument("--every", type=int, default=1, help="fine snapshot cadence (default 1; must match the reference)")
    ap.add_argument("--ref-dir", type=Path, help="reference store directory (default zr-refs/<CODE>_s<SEED> under the repo's parent)")
    ap.add_argument("--cpp-snap", type=Path, help="override <ref-dir>/cpp_fine.snap")
    ap.add_argument("--cpp-rng", type=Path, help="override <ref-dir>/cpp_rng.txt")
    ap.add_argument("--cpp-coarse", type=Path, help="override <ref-dir>/cpp_coarse.snap (used by --timeline)")
    ap.add_argument("--slot", type=int, default=1, help="build slot: CARGO_TARGET_DIR=target/slot-N (default 1)")
    ap.add_argument("--no-build", action="store_true", help="use the existing target/slot-N/release/zr-harness")
    ap.add_argument("--harness", type=Path, help="harness executable (skips the build)")
    ap.add_argument("--out", type=Path, help="directory for port_fine.snap, port_rng.txt, port_coarse.snap")
    ap.add_argument("--skip", nargs="+", default=None, metavar="TAG.FIELD[=VALUE]",
                    help=f"snap_diff skip rules (default: {' '.join(DEFAULT_SKIP)})")
    ap.add_argument("--no-skip", action="store_true", help="do not apply the default skip rules")
    ap.add_argument("--pos-tol", type=float, default=1.0, help="snapshot x/y/z tolerance (default 1.0)")
    ap.add_argument("--hp-tol", type=float, default=0.5, help="snapshot hp/cons tolerance (default 0.5)")
    ap.add_argument("--key", choices=("id", "tmpl"), default="id", help="snapshot record key (default id)")
    ap.add_argument("--from-frame", type=int, default=0, metavar="N", help="compare frames >= N only")
    ap.add_argument("--max-report", type=int, default=12, help="differing records printed per frame (default 12)")
    ap.add_argument("--max-minor", type=int, default=40, help="minor differences listed before the meaningful one (default 40)")
    ap.add_argument("--rng-context", type=int, default=5, help="RNG draws of context on each side (default 5)")
    ap.add_argument("--timeline", action="store_true", help="also compare the coarse game-event timeline")
    ap.add_argument("--tol-frames", type=int, default=30, help="timeline shift tolerance (default 30 frames)")
    ap.add_argument("--max-events", type=int, default=1000, help="timeline events printed per kind (default 1000)")
    ap.add_argument("--timeline-every", type=int, default=30, help="port coarse snapshot cadence (default 30)")
    ap.add_argument("--timeout", type=float, default=1800.0, help="one harness run's time limit in seconds (default 1800)")
    args = ap.parse_args()

    if args.skip is not None and args.no_skip:
        print("error: --skip and --no-skip are mutually exclusive", file=sys.stderr)
        return 2
    if args.frames < 0 or args.every <= 0 or args.timeline_every <= 0:
        print("error: --frames must be >= 0 and --every/--timeline-every must be positive", file=sys.stderr)
        return 2
    if args.slot < 1:
        print("error: --slot must be a positive build slot number", file=sys.stderr)
        return 2
    if args.pos_tol < 0 or args.hp_tol < 0 or args.from_frame < 0 or args.tol_frames < 0:
        print("error: tolerances and --from-frame must not be negative", file=sys.stderr)
        return 2
    if args.max_report < 1 or args.max_minor < 0 or args.rng_context < 0 or args.max_events < 0:
        print("error: --max-report must be >= 1; --max-minor/--rng-context/--max-events must not be negative",
              file=sys.stderr)
        return 2
    if args.timeout <= 0:
        print("error: --timeout must be positive", file=sys.stderr)
        return 2
    if args.pos_tol >= POS_CELL:
        print(f"warning: --pos-tol {args.pos_tol:g} is >= {POS_CELL:g} (one pathfinding cell): a position "
              f"drift the bar calls meaningful can be hidden by the tolerance", file=sys.stderr)
    skips = [] if args.no_skip else (parse_skips(args.skip) if args.skip is not None else list(DEFAULT_SKIP))

    try:
        registry = load_registry()
        cfg = matchup(registry, args.code)
    except (OSError, ValueError, SystemExit) as e:
        print(f"error: {e}" if not isinstance(e, SystemExit) else str(e), file=sys.stderr)
        return 2

    ref_dir = (Path(args.ref_dir) if args.ref_dir else DEFAULT_REF_ROOT / f"{args.code}_s{args.seed}").resolve()
    # A run past 3000 frames needs the long references (`cpp_fine_f9000.snap`, `cpp_rng_f9000.txt`) when the store has
    # them; the 3000-frame files would end the comparison at f3000 and report "no divergence".
    long_ref = args.frames > 3000 and (ref_dir / "cpp_fine_f9000.snap").exists()
    fine_name, rng_name = ("cpp_fine_f9000.snap", "cpp_rng_f9000.txt") if long_ref else ("cpp_fine.snap", "cpp_rng.txt")
    cpp_snap = (Path(args.cpp_snap) if args.cpp_snap else ref_dir / fine_name).resolve()
    cpp_rng = (Path(args.cpp_rng) if args.cpp_rng else ref_dir / rng_name).resolve()
    cpp_coarse = (Path(args.cpp_coarse) if args.cpp_coarse else ref_dir / "cpp_coarse.snap").resolve()
    manifest = ref_dir / "manifest.json"
    out_dir = out_dir_for(args, args.code, args.seed)

    print(f"== zr_compare {args.code} seed {args.seed} ==")
    slots = ", ".join(f"p{i + 2} {cfg['factions'][i]} start {cfg['starts'][i]} team {cfg['teams'][i]}"
                      for i in range(len(cfg["factions"])))
    print(f"matchup: {cfg['map']} (cpp {cfg.get('cpp_map', '?')}), {cfg['difficulty']}; {slots}; seed {args.seed}")
    print(f"reference: {ref_dir}")
    print_manifest(manifest, cfg, args.seed)

    missing = [str(p) for p in (cpp_snap, cpp_rng) if not p.exists()]
    if missing:
        print(f"error: missing C++ reference file(s): {', '.join(missing)}", file=sys.stderr)
        if args.code == "AN3" and args.seed == 1:
            print("hint: the AN3 seed 1 baseline lives at "
                  r"%TEMP%\zr-w15\115\cpp_fine_f3000.snap and cpp_rng_f3000.txt; "
                  "pass --cpp-snap/--cpp-rng", file=sys.stderr)
        print("      lane 140 is filling zr-refs/<CODE>_s<SEED> with cpp_fine.snap, cpp_rng.txt, "
              "cpp_coarse.snap and manifest.json", file=sys.stderr)
        return 2

    print(f"cpp fine: {cpp_snap}")
    print(f"cpp rng : {cpp_rng}")
    if args.timeline:
        print(f"cpp coarse: {cpp_coarse}")

    out_dir.mkdir(parents=True, exist_ok=True)
    port_snap = out_dir / "port_fine.snap"
    port_rng = out_dir / "port_rng.txt"
    port_coarse = out_dir / "port_coarse.snap"

    try:
        opts = SimpleNamespace(
            pos_tol=args.pos_tol,
            hp_tol=args.hp_tol,
            only=None,
            key=args.key,
            skip=snap_diff.Skip(skips),
        )
    except ValueError as e:
        print(f"error: {e}", file=sys.stderr)
        return 2

    try:
        if args.harness:
            exe = Path(args.harness).resolve()
            if not exe.exists():
                raise HarnessError(f"harness {exe} does not exist")
        elif args.no_build:
            exe, _ = harness_path(args.slot)
            if not exe.exists():
                raise HarnessError(f"{exe} does not exist; drop --no-build or build it first")
        else:
            exe = build_harness(args.slot)
    except HarnessError as e:
        print(f"error: {e}", file=sys.stderr)
        return 2

    print(f"\nrunning the port: {exe}")
    print(f"  snapshot every {args.every} (bits), rng trace, to frame {args.frames}")
    print(f"  output: {out_dir}")
    t0 = time.time()
    try:
        crc, secs = run_fine(exe, cfg, args.seed, args.frames, args.every, port_snap, port_rng, args.timeout)
    except (HarnessError, OSError) as e:
        print(f"error: {e}", file=sys.stderr)
        return 2
    print(f"  done in {secs:.1f}s: frame {crc.get('frame')}, crc {crc.get('crc')}, "
          f"{crc.get('objects')} objects")

    diverged = False
    try:
        t1 = time.time()
        port = snap_diff.load(str(port_snap), args.key)
        cpp = snap_diff.load(str(cpp_snap), args.key)
        print(f"\nloaded snapshots in {time.time() - t1:.1f}s")
        if not port:
            raise ValueError(f"no SNAP records in the port snapshot {port_snap}")
        if not cpp:
            raise ValueError(f"no SNAP records in the C++ snapshot {cpp_snap}")
        res = compare_snapshots(port, cpp, opts, args.from_frame, args.max_minor)
        print()
        print_snapshot_report(res, port, cpp, opts, args)
        diverged |= res["first_exact"] is not None
        del port, cpp
    except (OSError, ValueError, snap_diff.Malformed) as e:
        print(f"error: snapshot: {e}", file=sys.stderr)
        return 2

    try:
        diverged |= print_rng_report(port_rng, cpp_rng, args.rng_context)
    except SystemExit as e:  # rng_diff.load exits 2 on a malformed trace
        print(f"error: rng: malformed trace (exit {e.code})", file=sys.stderr)
        return 2
    except (OSError, ValueError) as e:
        print(f"error: rng: {e}", file=sys.stderr)
        return 2

    if args.timeline:
        if not cpp_coarse.exists():
            print(f"\ntimeline: no coarse reference at {cpp_coarse}; skipped "
                  f"(lane 140's store adds cpp_coarse.snap)")
        else:
            try:
                horizon = last_snap_frame(cpp_coarse)
                print(f"\ntimeline: running the port to frame {horizon} at every {args.timeline_every} "
                      f"(coarse, no bits) ...")
                frame, secs = run_coarse(exe, cfg, args.seed, horizon, args.timeline_every,
                                         port_coarse, args.timeout)
                print(f"  done in {secs:.1f}s: frame {frame}, {port_coarse}")
                if frame != horizon:
                    print(f"  warning: the port stopped at frame {frame}, the reference at {horizon}")
                diverged |= run_timeline_tool(port_coarse, cpp_coarse, args.tol_frames, args.max_events)
            except (HarnessError, OSError, ValueError, snap_diff.Malformed) as e:
                print(f"error: timeline: {e}", file=sys.stderr)
                return 2

    print(f"\nport CRC: frame {crc.get('frame')} crc {crc.get('crc')} (crc32 {crc.get('crc32')})")
    print(f"total: {time.time() - t0:.1f}s wall (run + compare)")
    print("result: " + ("DIVERGED" if diverged else "identical (snapshot on the common frames and rng)"))
    return 1 if diverged else 0


if __name__ == "__main__":
    sys.exit(main())
