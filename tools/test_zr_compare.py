#!/usr/bin/env python3
"""Self-test for tools/zr_compare.py on synthetic snapshots and RNG traces (no game, no harness).

Run: python -I tools/test_zr_compare.py
"""
import io
import os
import sys
import tempfile
import unittest
from contextlib import redirect_stdout
from pathlib import Path
from types import SimpleNamespace

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))

import rng_diff  # noqa: E402
import snap_diff  # noqa: E402
import zr_compare  # noqa: E402


def obj(f, oid, *, y=0.0, ai="Idle", owner=2, tmpl="AmericaTankCrusader", cons=-1.0, q=0,
        atk=0, bld=0, hp=100.0):
    return (f"SNAP f={f} O id={oid} tmpl={tmpl} owner={owner} x=10.0000 y={y:.4f} z=0.0000 "
            f"hp={hp:.3f} ai={ai} cons={cons:.3f} q={q} atk={atk} bld={bld}")


def frame(f, objects=(), money=1000, power="+"):
    return [
        f"SNAP f={f} FRAME crc=00000000 objs={len(objects)} players=1",
        f"SNAP f={f} P p=0 money={money} power={power} prod=10 cons=5 ai=1",
        *objects,
    ]


def write(path, blocks):
    with open(path, "w", encoding="utf-8") as fh:
        for block in blocks:
            fh.write("\n".join(block) + "\n")


def options(skip=()):
    return SimpleNamespace(pos_tol=1.0, hp_tol=0.5, only=None, key="id", skip=snap_diff.Skip(list(skip)))


class CompareTest(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory(prefix="zr_compare_test_")
        self.dir = Path(self._tmp.name)

    def tearDown(self):
        self._tmp.cleanup()

    def compare(self, port_blocks, cpp_blocks, skip=(), from_frame=0, pos_tol=1.0):
        port = self.dir / "port.snap"
        cpp = self.dir / "cpp.snap"
        write(port, port_blocks)
        write(cpp, cpp_blocks)
        opts = options(skip)
        opts.pos_tol = pos_tol
        res = zr_compare.compare_snapshots(
            snap_diff.load(str(port), "id"), snap_diff.load(str(cpp), "id"),
            opts, from_frame, max_minor=40,
        )
        return res, snap_diff.load(str(port), "id"), snap_diff.load(str(cpp), "id")

    # ---- meaningful vs minor classification -----------------------------------------------------

    def test_sub_cell_position_is_minor_and_a_later_state_change_is_meaningful(self):
        port = [frame(0, [obj(0, 7)]), frame(30, [obj(30, 7, y=1.5)]), frame(60, [obj(60, 7, ai="AttackObject")])]
        cpp = [frame(0, [obj(0, 7)]), frame(30, [obj(30, 7, y=0.0)]), frame(60, [obj(60, 7)])]
        res, _, _ = self.compare(port, cpp)
        self.assertEqual(res["first_exact"][0], 30)
        self.assertEqual(res["first_meaningful"][0], 60)
        self.assertEqual(res["first_meaningful"][1][0][2], "ai")
        self.assertEqual(res["minor_total"], 1)
        self.assertEqual(res["minor"][0][0], 30)
        self.assertEqual(res["minor"][0][1][2], "y")
        p, c, name, a, b = res["minor"][0][1]
        self.assertIn("distance", zr_compare.diff_size(p, c, name, a, b))

    def test_position_of_one_cell_is_meaningful(self):
        port = [frame(0, [obj(0, 7)]), frame(30, [obj(30, 7, y=12.0)])]
        cpp = [frame(0, [obj(0, 7)]), frame(30, [obj(30, 7, y=0.0)])]
        res, _, _ = self.compare(port, cpp)
        self.assertEqual(res["first_exact"][0], 30)
        self.assertEqual(res["first_meaningful"][0], 30)
        self.assertEqual(res["first_meaningful"][1][0][2], "y")
        self.assertIn("pathfinding cell", res["first_meaningful"][1][0][5])
        self.assertEqual(res["minor_total"], 0)

    def test_ai_money_power_and_object_presence_are_meaningful(self):
        base = [frame(0, [obj(0, 7)])]
        cases = [
            ("ai", [frame(0, [obj(0, 7)])], [frame(0, [obj(0, 7, ai="Guard")])]),
            ("money", [frame(0, [obj(0, 7)], money=1000)], [frame(0, [obj(0, 7)], money=900)]),
            ("power", [frame(0, [obj(0, 7)], power="+")], [frame(0, [obj(0, 7)], power="-")]),
            ("owner", [frame(0, [obj(0, 7, owner=2)])], [frame(0, [obj(0, 7, owner=3)])]),
            ("tmpl", [frame(0, [obj(0, 7, tmpl="A")])], [frame(0, [obj(0, 7, tmpl="B")])]),
            ("atk", [frame(0, [obj(0, 7, atk=0)])], [frame(0, [obj(0, 7, atk=9)])]),
            ("bld", [frame(0, [obj(0, 7, bld=0)])], [frame(0, [obj(0, 7, bld=4)])]),
            ("q", [frame(0, [obj(0, 7, q=0)])], [frame(0, [obj(0, 7, q=1)])]),
        ]
        for name, port, cpp in cases:
            with self.subTest(name=name):
                res, _, _ = self.compare(port, cpp)
                self.assertIsNotNone(res["first_meaningful"], name)
                self.assertEqual(res["first_meaningful"][1][0][2], name)

    def test_object_present_on_one_side_is_meaningful(self):
        port = [frame(0, [obj(0, 7)]), frame(30, [obj(30, 7), obj(30, 9)])]
        cpp = [frame(0, [obj(0, 7)]), frame(30, [obj(30, 7)])]
        res, _, _ = self.compare(port, cpp)
        item = next(item for item in res["first_meaningful"][1] if item[2] == "(record)")
        p, c, name, a, b, why = item
        self.assertIsNone(c)
        self.assertIn("one side only", why)

    def test_structural_records_are_meaningful_in_every_field(self):
        port = [frame(0, [obj(0, 7)]) + [
            "SNAP f=0 Q obj=7 i=0 kind=unit tmpl=AmericaTankCrusader total=1 done=0 frames=5 door=-",
            "SNAP f=0 T p=2 q=build i=0 key=teamskirmishamerica1 prio=0 started=0 reinf=0 recruit=0 sent=0 work=",
            "SNAP f=0 S id=1 proto=teamskirmishamerica1 members=7",
        ]]
        cpp = [frame(0, [obj(0, 7)]) + [
            "SNAP f=0 Q obj=7 i=0 kind=unit tmpl=AmericaTankCrusader total=1 done=0 frames=6 door=-",
            "SNAP f=0 T p=2 q=build i=0 key=teamskirmishamerica1 prio=1 started=0 reinf=0 recruit=0 sent=0 work=",
            "SNAP f=0 S id=1 proto=teamskirmishamerica1 members=",
        ]]
        res, _, _ = self.compare(port, cpp)
        # Q.frames, T.prio and S.members are all meaningful; the first is Q.frames.
        self.assertEqual(res["first_meaningful"][0], 0)
        self.assertTrue(all(item[5].endswith("record differs") for item in res["first_meaningful"][1]))
        self.assertEqual(len(res["first_meaningful"][1]), 3)

    def test_hp_only_is_minor(self):
        port = [frame(0, [obj(0, 7, hp=100.0)])]
        cpp = [frame(0, [obj(0, 7, hp=80.0)])]
        res, _, _ = self.compare(port, cpp)
        self.assertIsNotNone(res["first_exact"])
        self.assertIsNone(res["first_meaningful"])
        self.assertEqual(res["minor_total"], 1)

    def test_frame_missing_on_one_side_is_meaningful(self):
        port = [frame(0, [obj(0, 7)]), frame(30, [obj(30, 7)])]
        cpp = [frame(0, [obj(0, 7)]), frame(60, [obj(60, 7)])]
        res, _, _ = self.compare(port, cpp)
        self.assertEqual(res["first_exact"][0], 30)
        self.assertIn("missing from the cpp snapshot", res["first_exact"][1])
        self.assertIn("missing from the cpp snapshot", res["first_meaningful"][1][0][5])

    def test_common_horizon_skips_the_reference_tail(self):
        port = [frame(0, [obj(0, 7)]), frame(30, [obj(30, 7)])]
        cpp = [frame(0, [obj(0, 7)]), frame(30, [obj(30, 7)]), frame(60, [obj(60, 7)])]
        res, _, _ = self.compare(port, cpp)
        self.assertEqual(res["max_common"], 30)
        self.assertIsNone(res["first_exact"])

    def test_default_skips_silence_t_records(self):
        port = [frame(0, [obj(0, 7)]) + [
            "SNAP f=0 T p=2 q=build i=0 key=teamskirmishamerica1 prio=0 started=0 reinf=0 recruit=0 sent=0 work=",
        ]]
        cpp = [frame(0, [obj(0, 7)])]
        res, _, _ = self.compare(port, cpp, skip=("T.q=build",))
        self.assertIsNone(res["first_exact"])

    def test_skip_list_may_be_one_quoted_argument(self):
        items = zr_compare.parse_skips(["T.q=build T.q=ready", "O.tmpl=Car*"])
        self.assertEqual(items, ["T.q=build", "T.q=ready", "O.tmpl=Car*"])
        skip = snap_diff.Skip(items)
        rec = snap_diff.parse(1, "SNAP f=0 T p=2 q=ready i=0 key=k prio=0 started=0 reinf=0 recruit=0 sent=0 work=")
        self.assertTrue(skip.drops(rec))

    # ---- report ---------------------------------------------------------------------------------

    def test_report_lists_minor_diffs_then_the_meaningful_one_and_the_object_context(self):
        port, cpp = [], []
        for f in range(0, 11):
            port.append(frame(f, [obj(f, 7, y=0.0)]))
            cpp.append(frame(f, [obj(f, 7, y=0.0)]))
        port.append(frame(11, [obj(11, 7, y=1.0)]))
        cpp.append(frame(11, [obj(11, 7, y=0.0)]))
        port.append(frame(12, [obj(12, 7, y=1.5, ai="AttackObject")]))
        cpp.append(frame(12, [obj(12, 7, y=0.0)]))
        res, p, c = self.compare(port, cpp, pos_tol=0.1)
        args = SimpleNamespace(max_report=12, max_minor=40)
        out = io.StringIO()
        with redirect_stdout(out):
            zr_compare.print_snapshot_report(res, p, c, options(), args)
        text = out.getvalue()
        self.assertIn("first exact divergence: frame 11", text)
        self.assertIn("first meaningful divergence: frame 12", text)
        self.assertIn("minor under the near bit-exact bar", text)
        self.assertIn("O id=7 field y", text)
        self.assertIn("(distance", text)
        self.assertIn("object context: O id=7", text)
        self.assertIn("(frames 1..11)", text)
        self.assertIn("*f11 port:", text)
        self.assertIn("f1 port:", text)

    def test_report_is_clean_when_identical(self):
        blocks = [frame(f, [obj(f, 7)]) for f in range(0, 40, 10)]
        res, p, c = self.compare(blocks, blocks)
        out = io.StringIO()
        with redirect_stdout(out):
            zr_compare.print_snapshot_report(res, p, c, options(), SimpleNamespace(max_report=12, max_minor=40))
        self.assertIn("first exact divergence: none", out.getvalue())
        self.assertIn("first meaningful divergence: none", out.getvalue())


def rng_line(n, f, r, at="port/random.rs:1"):
    return f"RNG f={f} n={n} k=I lo=0 hi=60 r={r} at={at}"


class RngTest(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory(prefix="zr_compare_rng_")
        self.dir = Path(self._tmp.name)

    def tearDown(self):
        self._tmp.cleanup()

    def test_mismatch_prints_context_and_the_cpp_call_site(self):
        port = self.dir / "port.txt"
        cpp = self.dir / "cpp.txt"
        with open(port, "w", encoding="utf-8") as fh:
            for n in range(12):
                fh.write(rng_line(n, 100 + n, n) + "\n")
        with open(cpp, "w", encoding="utf-8") as fh:
            for n in range(12):
                at = r"C:\src\GeneralsMD\Code\GameEngine\Source\GameLogic\AI\AIGuard.cpp:723"
                fh.write(rng_line(n, 100 + n, n if n != 5 else 99, at) + "\n")
        out = io.StringIO()
        with redirect_stdout(out):
            diverged = zr_compare.print_rng_report(port, cpp, context=5)
        text = out.getvalue()
        self.assertTrue(diverged)
        self.assertIn("first RNG mismatch at draw n=5", text)
        self.assertIn("AIGuard.cpp:723", text)
        self.assertIn("rng context (draws 0..10", text)
        self.assertIn("port n=0 ", text)
        self.assertIn("cpp  n=0 ", text)
        self.assertIn("note: the draw itself differs", text)

    def test_frame_only_shift_is_reported_as_a_timing_difference(self):
        port = self.dir / "port.txt"
        cpp = self.dir / "cpp.txt"
        with open(port, "w", encoding="utf-8") as fh:
            for n in range(4):
                fh.write(rng_line(n, 200 + n, n) + "\n")
        with open(cpp, "w", encoding="utf-8") as fh:
            for n in range(4):
                fh.write(rng_line(n, 203 + n, n) + "\n")
        out = io.StringIO()
        with redirect_stdout(out):
            self.assertTrue(zr_compare.print_rng_report(port, cpp, context=2))
        self.assertIn("only the frame shifted", out.getvalue())

    def test_identical_and_longer_traces(self):
        port = self.dir / "port.txt"
        cpp = self.dir / "cpp.txt"
        with open(port, "w", encoding="utf-8") as fh:
            for n in range(4):
                fh.write(rng_line(n, n, n) + "\n")
        with open(cpp, "w", encoding="utf-8") as fh:
            for n in range(6):
                fh.write(rng_line(n, n, n) + "\n")
        out = io.StringIO()
        with redirect_stdout(out):
            self.assertTrue(zr_compare.print_rng_report(port, cpp, context=2))
            self.assertTrue(zr_compare.print_rng_report(cpp, port, context=2))
        text = out.getvalue()
        self.assertIn("identical for the first 4 draws", text)
        self.assertIn("first extra draw n=4", text)

    def test_two_runs_with_the_same_trace_are_identical(self):
        path = self.dir / "one.txt"
        with open(path, "w", encoding="utf-8") as fh:
            for n in range(3):
                fh.write(rng_line(n, n, n) + "\n")
        out = io.StringIO()
        with redirect_stdout(out):
            self.assertFalse(zr_compare.print_rng_report(path, path, context=1))
        self.assertIn("rng: identical, all 3 draws match", out.getvalue())
        self.assertEqual(len(rng_diff.load(str(path))), 3)


if __name__ == "__main__":
    unittest.main()
