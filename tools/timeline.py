#!/usr/bin/env python3
"""Game-event timeline of two state snapshots (docs/HARNESS.md, lane 139).

Usage: python -I tools/timeline.py PORT.snap CPP.snap [--tol-frames N] [--max-events N]

Both files are `SNAP` snapshots (any cadence: every frame, every 30 frames, ...), as written by
`ZR_SNAPSHOT` / `zr-harness` on the port side and the patched C++ build on the other. Events are
extracted per side and matched by (owner, template, ordinal):

- structure placed: the first sampled frame its `O` record appears (objects present at the first
  sampled frame are the starting state, not placements);
- structure completed: the first sampled frame its `cons` reaches -1 (`CONSTRUCTION_COMPLETE`);
- unit produced: the first sampled frame a non-structure object appears after the first sampled frame
  whose template a factory is ever seen producing (`Q kind=unit`; in the engine the object is created
  when production finishes, and the exit door may move it later). Debris, projectiles and other objects
  that are never produced are not timeline events, only counted as "other objects";
- death: the last sampled frame the object appears, when it is gone before that side's last frame
  (with a coarse snapshot this is a lower bound, at most one cadence early); tracked for structures,
  units and objects present in the first frame (map props and starting units), not for debris;
- first attack order: for each owner, the first frame an object prints `atk` other than 0;
- money: each player's money at every 60 s mark (1800 logic frames at 30 Hz), from the latest sampled
  frame at or before the mark;
- game end: the last sampled frame.

A structure is an object that ever prints `cons` >= 0 while under construction. A coarse snapshot can
miss that window for an instant structure, which then counts as an "other object"; the C++ and port
snapshots use the same cadence, so the misclassification is the same on both sides.

Matched events print the frame difference and are marked `shift` when |port - cpp| > --tol-frames
(default 30: one second at 30 Hz). Events on one side only are listed. Exit status: 0 no shift past the
tolerance and no one-sided event, 1 a difference, 2 unreadable/malformed/no records.
"""
import argparse
import sys
from collections import defaultdict
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))

import snap_diff  # noqa: E402  (after the sys.path line: `python -I` does not add the script dir)

# 30 logic frames per second; a minute is 1800 frames.
MINUTE_FRAMES = 1800
EVENT_KINDS = (
    ("structure_placed", "structure placed"),
    ("structure_completed", "structure completed"),
    ("unit", "unit produced"),
    ("death", "death (last seen)"),
)


class Event:
    __slots__ = ("kind", "owner", "template", "ordinal", "frame", "detail")

    def __init__(self, kind, owner, template, ordinal, frame, detail=""):
        self.kind, self.owner, self.template, self.ordinal = kind, owner, template, ordinal
        self.frame, self.detail = frame, detail

    def key(self):
        return (self.owner, self.template, self.ordinal)


def parse_float(value):
    try:
        return float(value)
    except (TypeError, ValueError):
        return None


def collect(frames):
    """Extract the timeline state of one snapshot (see the module docstring)."""
    if not frames:
        raise ValueError("snapshot has no frames")
    order = sorted(frames)
    first_frame, last_frame = order[0], order[-1]
    objects = {}  # id -> {owner, template, first, last, cons: {frame: percent}}
    money = {}  # frame -> {player: cash}
    attacks = {}  # owner -> Event
    unit_templates = set()  # templates a factory is ever seen producing (`Q kind=unit`)
    for f in order:
        for rec in frames[f].values():
            if rec.tag == "P":
                try:
                    money.setdefault(f, {})[int(rec.fields["p"])] = int(rec.fields["money"])
                except (KeyError, ValueError):
                    pass
            elif rec.tag == "Q":
                if rec.fields.get("kind") == "unit":
                    unit_templates.add(rec.fields.get("tmpl", ""))
            elif rec.tag == "O":
                oid = rec.fields.get("id")
                if oid is None:
                    continue
                try:
                    owner = int(rec.fields.get("owner", "0"))
                except ValueError:
                    owner = 0
                tmpl = rec.fields.get("tmpl", "?")
                info = objects.get(oid)
                if info is None:
                    info = {"owner": owner, "template": tmpl, "first": f, "last": f, "cons": {}}
                    objects[oid] = info
                info["last"] = f
                info["cons"][f] = parse_float(rec.fields.get("cons"))
                atk = rec.fields.get("atk", "0")
                if owner not in attacks and atk not in ("0", "-", ""):
                    attacks[owner] = Event("attack", owner, tmpl, 0, f, f"atk={atk}")

    placed_ord = defaultdict(int)
    unit_ord = defaultdict(int)
    death_ord = defaultdict(int)
    events = {kind: [] for kind, _ in EVENT_KINDS}
    other_created = 0
    # First-appearance order (ties by id, the writers' order) so both sides number an owner's and
    # template's objects the same way.
    for oid, info in sorted(objects.items(), key=lambda kv: (kv[1]["first"], int(kv[0]))):
        structure = any(c is not None and c >= 0.0 for c in info["cons"].values())
        unit = not structure and info["template"] in unit_templates
        group = (info["owner"], info["template"])
        # Debris, projectiles and other transient records are not timeline events. Objects present at
        # the first frame stay death-tracked (map props, starting units and structures).
        tracked_death = structure or unit or info["first"] <= first_frame
        if tracked_death:
            death_ord[group] += 1
            if info["last"] < last_frame:
                events["death"].append(
                    Event("death", info["owner"], info["template"], death_ord[group] - 1, info["last"],
                          f"id={oid}")
                )
        if info["first"] <= first_frame:
            continue  # starting state, not a placement or a production
        if structure:
            placed_ord[group] += 1
            ordinal = placed_ord[group] - 1
            first_cons = info["cons"][info["first"]]
            events["structure_placed"].append(
                Event("structure_placed", info["owner"], info["template"], ordinal, info["first"],
                      f"id={oid} cons={first_cons if first_cons is not None else '?'}")
            )
            done = next((f for f in sorted(info["cons"]) if f >= info["first"] and info["cons"][f] == -1.0), None)
            if done is not None:
                events["structure_completed"].append(
                    Event("structure_completed", info["owner"], info["template"], ordinal, done, f"id={oid}")
                )
        elif unit:
            unit_ord[group] += 1
            events["unit"].append(
                Event("unit", info["owner"], info["template"], unit_ord[group] - 1, info["first"], f"id={oid}")
            )
        else:
            other_created += 1
    return {
        "frames": len(order),
        "first": first_frame,
        "last": last_frame,
        "objects": objects,
        "events": events,
        "attacks": attacks,
        "money": money,
        "unit_templates": unit_templates,
        "other_created": other_created,
    }


def load_timeline(path):
    return collect(snap_diff.load(path, "id"))


def _event_line(side, event, diff=None, flagged=False, only=False):
    mark = "shift" if flagged else ("only" if only else "ok")
    diff_text = "" if diff is None else f" ({diff:+d})"
    return (f"  {mark:<5} {side:<5} p{event.owner} {event.template} #{event.ordinal}: "
            f"f={event.frame}{diff_text} {event.detail}").rstrip()


def compare_events(port, cpp, tol, max_events):
    diverged = False
    print()
    for kind, label in EVENT_KINDS:
        amap = {e.key(): e for e in port["events"][kind]}
        bmap = {e.key(): e for e in cpp["events"][kind]}
        keys = sorted(
            set(amap) | set(bmap),
            key=lambda k: (
                min(amap[k].frame if k in amap else 1 << 30, bmap[k].frame if k in bmap else 1 << 30),
                k,
            ),
        )
        shifted = one_sided = shown = 0
        lines = []
        for key in keys:
            ea, eb = amap.get(key), bmap.get(key)
            if ea is not None and eb is not None:
                diff = ea.frame - eb.frame
                flagged = abs(diff) > tol
                shifted += int(flagged)
                diverged |= flagged
                if shown < max_events:
                    lines.append(_event_line("port", ea, diff, flagged))
                    lines.append(_event_line("cpp", eb, diff, flagged))
                shown += 1
            else:
                one_sided += 1
                diverged = True
                if shown < max_events:
                    event = ea or eb
                    lines.append(_event_line("port" if ea else "cpp", event, only=True))
                shown += 1
        print(f"{label}: {len(keys)} keys ({len(keys) - one_sided} matched), {shifted} shifted > {tol} frames, "
              f"{one_sided} one-sided")
        for line in lines:
            print(line)
        if shown > max_events:
            print(f"  ... {shown - max_events} more events")

    # First attack orders, one per owner.
    owners = sorted(set(port["attacks"]) | set(cpp["attacks"]))
    print(f"first attack order: {len(owners)} owners")
    for owner in owners:
        a, b = port["attacks"].get(owner), cpp["attacks"].get(owner)
        if a is not None and b is not None:
            diff = a.frame - b.frame
            same = (a.template, a.detail) == (b.template, b.detail)
            flagged = abs(diff) > tol or not same
            diverged |= flagged
            mark = "shift" if flagged else "ok"
            note = "" if same else f"; port {a.template} {a.detail}, cpp {b.template} {b.detail}"
            print(f"  {mark:<5} p{owner}: port f={a.frame}, cpp f={b.frame} ({diff:+d}){note}")
        else:
            diverged = True
            event = a or b
            print(f"  only   p{owner}: {'port' if a else 'cpp'} f={event.frame} {event.template} {event.detail}")

    # Money at each minute.
    port_money, cpp_money = port["money"], cpp["money"]
    ends = max(port["last"], cpp["last"])
    marks = list(range(MINUTE_FRAMES, ends + 1, MINUTE_FRAMES))
    differing = 0
    money_lines = []
    for mark in marks:
        pf = max((f for f in port_money if f <= mark), default=None)
        cf = max((f for f in cpp_money if f <= mark), default=None)
        players = sorted(set(port_money.get(pf, {})) | set(cpp_money.get(cf, {})))
        parts = []
        for p in players:
            pv = port_money.get(pf, {}).get(p)
            cv = cpp_money.get(cf, {}).get(p)
            if pv != cv:
                delta = f"{(pv - cv):+d}" if pv is not None and cv is not None else "?"
                parts.append(f"p{p} {pv if pv is not None else '<absent>'} vs "
                             f"{cv if cv is not None else '<absent>'} ({delta})")
        if parts:
            differing += 1
            money_lines.append(f"  minute {mark // MINUTE_FRAMES} (f={mark}): " + "; ".join(parts))
    if differing:
        diverged = True
        print(f"money: {len(marks)} minute marks, {differing} with differences")
        for line in money_lines:
            print(line)
    else:
        print(f"money: matches at {len(marks)} minute marks")

    diff = port["last"] - cpp["last"]
    flagged = abs(diff) > tol
    diverged |= flagged
    print(f"game end: port f={port['last']}, cpp f={cpp['last']} ({diff:+d})" + ("  [shift]" if flagged else ""))
    return diverged


def compare_files(port_path, cpp_path, tol_frames=30, max_events=1000):
    """Print the timeline comparison; return True when something differs."""
    port = load_timeline(port_path)
    cpp = load_timeline(cpp_path)
    print(f"timeline: port {port['frames']} frames ({port['first']}..{port['last']}), "
          f"cpp {cpp['frames']} frames ({cpp['first']}..{cpp['last']})")
    if port["other_created"] or cpp["other_created"]:
        print(f"  other objects created (debris/projectiles/props, not timeline events): "
              f"port {port['other_created']}, cpp {cpp['other_created']}")
    return compare_events(port, cpp, tol_frames, max_events)


def main():
    ap = argparse.ArgumentParser(description="Compare the game-event timelines of two state snapshots.")
    ap.add_argument("port", help="port snapshot (any cadence)")
    ap.add_argument("cpp", help="C++ snapshot (any cadence)")
    ap.add_argument("--tol-frames", type=int, default=30,
                    help="flag a matched event whose frames differ by more (default 30)")
    ap.add_argument("--max-events", type=int, default=1000, help="matched events printed per kind (default 1000)")
    a = ap.parse_args()
    if a.tol_frames < 0 or a.max_events < 0:
        print("error: --tol-frames and --max-events must not be negative", file=sys.stderr)
        return 2
    try:
        diverged = compare_files(a.port, a.cpp, a.tol_frames, a.max_events)
    except (OSError, ValueError, snap_diff.Malformed) as e:
        print(f"error: {e}", file=sys.stderr)
        return 2
    return 1 if diverged else 0


if __name__ == "__main__":
    sys.exit(main())
