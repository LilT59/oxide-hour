#!/usr/bin/env python3
"""Self-test for tools/timeline.py on synthetic snapshots (no game data, no harness).

Run: python -I tools/test_timeline.py
"""
import io
import subprocess
import sys
import tempfile
import unittest
from contextlib import redirect_stdout
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))

import timeline  # noqa: E402


def obj(f, oid, tmpl="GLAInfantryWorker", owner=2, cons=-1.0, atk=0, x=10.0, y=20.0):
    return (f"SNAP f={f} O id={oid} tmpl={tmpl} owner={owner} x={x:.4f} y={y:.4f} z=0.0000 "
            f"hp=100.000 ai=Idle cons={cons:.3f} q=0 atk={atk} bld=0")


def frame(f, objects=(), money=1000):
    return [
        f"SNAP f={f} FRAME crc=00000000 objs={len(objects)} players=1",
        f"SNAP f={f} P p=0 money={money} power=+ prod=10 cons=5 ai=1",
        f"SNAP f={f} P p=2 money={money} power=+ prod=10 cons=5 ai=1",
        *objects,
    ]


def base_port():
    start = frame(0, [obj(0, 1, "GLACommandCenter"), obj(0, 2)])
    start.append("SNAP f=0 Q obj=1 i=0 kind=unit tmpl=GLAInfantryWorker total=1 done=0 frames=5 door=-")
    return [
        start,
        frame(30, [obj(30, 1, "GLACommandCenter"), obj(30, 2, atk=99),
                   obj(30, 10, "GLAPowerPlant", cons=0.0)]),
        frame(60, [obj(60, 1, "GLACommandCenter"), obj(60, 2),
                   obj(60, 10, "GLAPowerPlant", cons=50.0)]),
        frame(90, [obj(90, 1, "GLACommandCenter"), obj(90, 2),
                   obj(90, 10, "GLAPowerPlant", cons=-1.0), obj(90, 20)]),
        frame(120, [obj(120, 1, "GLACommandCenter"), obj(120, 10, "GLAPowerPlant"),
                    obj(120, 20, atk=1)]),
        frame(150, [obj(150, 1, "GLACommandCenter"), obj(150, 10, "GLAPowerPlant"),
                    obj(150, 20, atk=1)], money=1300),
    ]


class TimelineTest(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory(prefix="timeline_test_")
        self.dir = Path(self._tmp.name)

    def tearDown(self):
        self._tmp.cleanup()

    def write(self, name, blocks):
        path = self.dir / name
        with open(path, "w", encoding="utf-8") as fh:
            for block in blocks:
                fh.write("\n".join(block) + "\n")
        return path

    def run_tool(self, port, cpp, *args):
        port_path = self.write("port.snap", port)
        cpp_path = self.write("cpp.snap", cpp)
        out = io.StringIO()
        with redirect_stdout(out):
            diverged = timeline.compare_files(str(port_path), str(cpp_path), *args)
        return diverged, out.getvalue()

    def test_collect_finds_placed_completed_units_attacks_deaths_and_money(self):
        state = timeline.load_timeline(str(self.write("port.snap", base_port())))
        placed = state["events"]["structure_placed"]
        completed = state["events"]["structure_completed"]
        units = state["events"]["unit"]
        deaths = state["events"]["death"]
        self.assertEqual([(e.owner, e.template, e.ordinal, e.frame) for e in placed],
                         [(2, "GLAPowerPlant", 0, 30)])
        self.assertEqual([(e.template, e.ordinal, e.frame) for e in completed],
                         [("GLAPowerPlant", 0, 90)])
        self.assertEqual([(e.template, e.ordinal, e.frame) for e in units],
                         [("GLAInfantryWorker", 0, 90)])
        # id 2 is a starting unit: its unit event is not a production, but its death is tracked.
        self.assertEqual([(e.template, e.ordinal, e.frame) for e in deaths],
                         [("GLAInfantryWorker", 0, 90)])
        self.assertEqual(state["attacks"][2].frame, 30)
        self.assertEqual(state["last"], 150)

    def test_identical_timelines(self):
        diverged, out = self.run_tool(base_port(), base_port())
        self.assertFalse(diverged, out)
        self.assertIn("structure placed: 1 keys (1 matched), 0 shifted > 30 frames, 0 one-sided", out)
        self.assertIn("first attack order: 1 owners", out)
        self.assertIn("game end: port f=150, cpp f=150 (+0)", out)

    def test_shift_beyond_the_tolerance_is_flagged(self):
        cpp = base_port()
        # The unit appears at f90 on the port side; remove it there and add it at f150 on the C++ side.
        cpp[3] = frame(90, [obj(90, 1, "GLACommandCenter"), obj(90, 2),
                            obj(90, 10, "GLAPowerPlant", cons=-1.0)])
        cpp[4] = frame(120, [obj(120, 1, "GLACommandCenter"), obj(120, 10, "GLAPowerPlant")])
        cpp[5] = frame(150, [obj(150, 1, "GLACommandCenter"), obj(150, 10, "GLAPowerPlant"),
                             obj(150, 20)], money=1300)
        port = base_port()
        diverged, out = self.run_tool(port, cpp)
        self.assertTrue(diverged, out)
        self.assertIn("1 shifted > 30 frames", out)
        self.assertIn("unit produced", out)
        self.assertIn("(-60)", out)

    def test_shift_within_the_tolerance_is_not_flagged(self):
        cpp = base_port()
        # Move the structure placement one sample later: -30 frames, not > 30.
        cpp[1] = frame(30, [obj(30, 1, "GLACommandCenter"), obj(30, 2, atk=99)])
        cpp[2] = frame(60, [obj(60, 1, "GLACommandCenter"), obj(60, 2),
                            obj(60, 10, "GLAPowerPlant", cons=0.0)])
        port = base_port()
        diverged, out = self.run_tool(port, cpp)
        self.assertFalse(diverged, out)
        self.assertIn("structure placed: 1 keys (1 matched), 0 shifted", out)

    def test_one_sided_event_is_listed(self):
        cpp = base_port()
        cpp[3] = frame(90, [obj(90, 1, "GLACommandCenter"), obj(90, 2),
                            obj(90, 10, "GLAPowerPlant", cons=-1.0), obj(90, 20), obj(90, 21)])
        port = base_port()
        diverged, out = self.run_tool(port, cpp)
        self.assertTrue(diverged, out)
        self.assertIn("1 one-sided", out)
        self.assertIn("only  cpp   p2 GLAInfantryWorker #1", out)

    def test_first_attack_shift_is_flagged(self):
        cpp = base_port()
        cpp[1] = frame(30, [obj(30, 1, "GLACommandCenter"), obj(30, 2),
                            obj(30, 10, "GLAPowerPlant", cons=0.0)])
        cpp[4] = frame(120, [obj(120, 1, "GLACommandCenter"), obj(120, 10, "GLAPowerPlant"),
                             obj(120, 20, atk=1)])
        diverged, out = self.run_tool(base_port(), cpp)
        self.assertTrue(diverged, out)
        self.assertIn("first attack order", out)
        self.assertIn("shift p2", out)
        self.assertIn("(-90)", out)

    def test_money_at_minutes_and_game_end(self):
        port = [frame(0, [obj(0, 1, "GLACommandCenter")], money=1000),
                frame(1800, [obj(1800, 1, "GLACommandCenter")], money=1300)]
        cpp = [frame(0, [obj(0, 1, "GLACommandCenter")], money=1000),
               frame(1800, [obj(1800, 1, "GLACommandCenter")], money=1200)]
        diverged, out = self.run_tool(port, cpp)
        self.assertTrue(diverged, out)
        self.assertIn("minute 1 (f=1800)", out)
        self.assertIn("p2 1300 vs 1200 (+100)", out)
        self.assertIn("game end: port f=1800, cpp f=1800 (+0)", out)

    def test_death_shift_and_ordinals_with_two_units(self):
        port = [
            frame(0, [obj(0, 1, "GLACommandCenter"), obj(0, 2), obj(0, 3)]),
            frame(30, [obj(30, 1, "GLACommandCenter"), obj(30, 3)]),
            frame(90, [obj(90, 1, "GLACommandCenter"), obj(90, 3)]),
            frame(120, [obj(120, 1, "GLACommandCenter")]),
        ]
        cpp = [
            frame(0, [obj(0, 1, "GLACommandCenter"), obj(0, 2), obj(0, 3)]),
            frame(30, [obj(30, 1, "GLACommandCenter"), obj(30, 2), obj(30, 3)]),
            frame(90, [obj(90, 1, "GLACommandCenter"), obj(90, 2), obj(90, 3)]),
            frame(120, [obj(120, 1, "GLACommandCenter")]),
        ]
        diverged, out = self.run_tool(port, cpp)
        self.assertTrue(diverged, out)
        self.assertIn("GLAInfantryWorker #0", out)
        self.assertIn("GLAInfantryWorker #1", out)
        self.assertIn("(-90)", out)

    def test_debris_and_projectiles_are_not_units(self):
        blocks = base_port()
        blocks[1].append(obj(30, 30, "GenericDebris"))
        blocks[2].append(obj(60, 31, "ScorpionTankShell"))
        state = timeline.load_timeline(str(self.write("port.snap", blocks)))
        templates = {e.template for e in state["events"]["unit"]}
        self.assertEqual(templates, {"GLAInfantryWorker"})
        self.assertEqual(state["other_created"], 2)

    def test_unit_classification_requires_a_queue_record(self):
        blocks = base_port()
        blocks[0] = frame(0, [obj(0, 1, "GLACommandCenter"), obj(0, 2)])
        state = timeline.load_timeline(str(self.write("port.snap", blocks)))
        self.assertEqual(state["events"]["unit"], [])
        self.assertEqual(state["other_created"], 1)

    def test_different_cadences_compare_within_the_tolerance(self):
        # The C++ side sampled half as often: each event moves by 30 frames, which is not "> 30".
        start = frame(0, [obj(0, 1, "GLACommandCenter"), obj(0, 2)])
        start.append("SNAP f=0 Q obj=1 i=0 kind=unit tmpl=GLAInfantryWorker total=1 done=0 frames=5 door=-")
        cpp = [
            start,
            frame(60, [obj(60, 1, "GLACommandCenter"), obj(60, 2, atk=99),
                       obj(60, 10, "GLAPowerPlant", cons=50.0)]),
            frame(120, [obj(120, 1, "GLACommandCenter"), obj(120, 10, "GLAPowerPlant"),
                        obj(120, 20, atk=1)]),
        ]
        diverged, out = self.run_tool(base_port(), cpp)
        self.assertFalse(diverged, out)
        self.assertIn("timeline: port 6 frames", out)
        self.assertIn("cpp 3 frames", out)

    def test_cli_reads_the_files(self):
        port = self.write("port.snap", base_port())
        cpp = self.write("cpp.snap", base_port())
        proc = subprocess.run(
            [sys.executable, "-I", str(HERE / "timeline.py"), str(port), str(cpp)],
            capture_output=True, text=True, encoding="utf-8",
        )
        self.assertEqual(proc.returncode, 0, proc.stdout + proc.stderr)
        self.assertIn("timeline: port 6 frames", proc.stdout)
        self.assertIn("game end: port f=150, cpp f=150 (+0)", proc.stdout)


if __name__ == "__main__":
    unittest.main()
