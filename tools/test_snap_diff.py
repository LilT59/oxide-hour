#!/usr/bin/env python3
"""Self-test for tools/snap_diff.py: two small synthetic snapshots, no game data.

Run: python tools/test_snap_diff.py
"""
import os
import subprocess
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
TOOL = os.path.join(HERE, "snap_diff.py")


def block(f, hp7=400.0, x7=100.0, state7="Idle", members="7,9", with9=True):
    """One snapshot block at frame f, shaped like the port's writer output."""
    lines = [
        f"SNAP f={f} FRAME crc=00000001 objs={2 if with9 else 1} players=1",
        f"SNAP f={f} P p=0 money=1000 power=+ prod=10 cons=5 ai=1",
        f"SNAP f={f} O id=7 tmpl=AmericaTankCrusader owner=0 x={x7:.4f} y=20.0000 z=0.0000 hp={hp7:.3f} "
        f"ai={state7} cons=100.000 q=0 atk=0 bld=0",
    ]
    if with9:
        lines.append(f"SNAP f={f} O id=9 tmpl=GLATechnical owner=1 x=50.0000 y=50.0000 z=0.0000 hp=200.000 ai=Idle cons=100.000 q=0 atk=0 bld=0")
    lines.append(f"SNAP f={f} S id=1 proto=usa%20team members={members}")
    return lines


def snapshot(blocks):
    """A file: (frame, block lines) pairs, with a C++-style log line in front (ignored by the tool)."""
    out = ["Log: some printf output the tool must skip"]
    for _, lines in blocks:
        out.extend(lines)
    return "\n".join(out) + "\n"


class SnapDiffTest(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory(prefix="snap_diff_test_")
        self.dir = self._tmp.name

    def tearDown(self):
        self._tmp.cleanup()

    def write(self, name, text):
        path = os.path.join(self.dir, name)
        with open(path, "w", encoding="utf-8") as fh:
            fh.write(text)
        return path

    def run_tool(self, port_text, cpp_text, *args):
        port = self.write("port.snap", port_text)
        cpp = self.write("cpp.snap", cpp_text)
        proc = subprocess.run(
            [sys.executable, TOOL, port, cpp, *args], capture_output=True, text=True, encoding="utf-8"
        )
        return proc.returncode, proc.stdout + proc.stderr

    def base(self):
        return snapshot([(0, block(0)), (30, block(30)), (60, block(60)), (90, block(90))])

    def test_identical_files_exit_0(self):
        code, out = self.run_tool(self.base(), self.base())
        self.assertEqual(code, 0, out)
        self.assertIn("identical: 4 frames", out)

    def test_hand_edited_hp_reports_the_frame_and_field(self):
        cpp = snapshot([(0, block(0)), (30, block(30)), (60, block(60, hp7=380.0)), (90, block(90))])
        code, out = self.run_tool(self.base(), cpp)
        self.assertEqual(code, 1, out)
        self.assertIn("first divergence: frame 60", out)
        self.assertIn("O id=7: field hp: port 400.000, cpp 380.000", out)
        self.assertIn("frames that differ: 1 (first 60, last 60)", out)

    def test_position_tolerance(self):
        near = snapshot([(0, block(0)), (30, block(30, x7=100.4)), (60, block(60)), (90, block(90))])
        far = snapshot([(0, block(0)), (30, block(30, x7=103.0)), (60, block(60)), (90, block(90))])
        self.assertEqual(self.run_tool(self.base(), near)[0], 0)
        code, out = self.run_tool(self.base(), far)
        self.assertEqual(code, 1, out)
        self.assertIn("first divergence: frame 30", out)
        self.assertIn("field x: port 100.0000, cpp 103.0000", out)
        self.assertEqual(self.run_tool(self.base(), far, "--pos-tol", "5")[0], 0)

    def test_decisions_are_exact_and_fields_can_be_selected(self):
        cpp = snapshot([(0, block(0)), (30, block(30, state7="AttackObject")), (60, block(60)), (90, block(90))])
        code, out = self.run_tool(self.base(), cpp)
        self.assertEqual(code, 1, out)
        self.assertIn("field ai: port Idle, cpp AttackObject", out)
        # Only positions and health are compared: the state change is then not a difference.
        self.assertEqual(self.run_tool(self.base(), cpp, "--fields", "pos", "hp")[0], 0)

    def test_script_team_members_change(self):
        cpp = snapshot([(0, block(0)), (30, block(30)), (60, block(60, members="7")), (90, block(90))])
        code, out = self.run_tool(self.base(), cpp)
        self.assertEqual(code, 1, out)
        self.assertIn("first divergence: frame 60", out)
        self.assertIn("S id=1: field members: port 7,9, cpp 7", out)

    def test_hp_within_tolerance_passes(self):
        cpp = snapshot([(0, block(0)), (30, block(30, hp7=400.4)), (60, block(60)), (90, block(90))])
        self.assertEqual(self.run_tool(self.base(), cpp)[0], 0)
        self.assertEqual(self.run_tool(self.base(), cpp, "--hp-tol", "0.1")[0], 1)

    def test_earliest_of_two_differing_frames_is_reported(self):
        cpp = snapshot([(0, block(0)), (30, block(30, hp7=380.0)), (60, block(60, hp7=370.0)), (90, block(90))])
        code, out = self.run_tool(self.base(), cpp)
        self.assertEqual(code, 1, out)
        self.assertIn("first divergence: frame 30", out)
        self.assertIn("frames that differ: 2 (first 30, last 60)", out)

    def test_max_report_limits_the_records_of_the_first_frame(self):
        cpp = snapshot([(0, block(0)), (30, block(30)), (60, block(60, hp7=380.0, state7="Dead")), (90, block(90))])
        code, out = self.run_tool(self.base(), cpp, "--max-report", "1")
        self.assertEqual(code, 1, out)
        self.assertIn("field hp", out)
        self.assertNotIn("field ai", out)
        self.assertIn("... 1 more in this frame", out)

    def test_crc_is_never_compared(self):
        cpp = self.base().replace("crc=00000001", "crc=0000abcd")
        self.assertEqual(self.run_tool(self.base(), cpp)[0], 0)
        self.assertEqual(self.run_tool(self.base(), cpp, "--fields", "crc", "hp")[0], 0)

    def test_truncated_final_line_drops_its_frame_block(self):
        full = self.base()
        cut = full[: full.rindex("SNAP f=90 S")]  # the last frame's team record is cut off before its newline
        cut += "SNAP f=90 S id=1 proto=usa%20te"
        code, out = self.run_tool(cut, cut)
        self.assertEqual(code, 0, out)  # both sides cut the same way: identical once frame 90 is dropped
        self.assertIn("identical: 3 frames", out)
        code, out = self.run_tool(self.base(), cut)
        self.assertEqual(code, 1, out)
        self.assertIn("frame 90 is missing from the cpp snapshot", out)
        self.assertIn("dropped a final line without a newline and the incomplete frame 90 block", out)

    def test_duplicate_key_in_a_frame_is_exit_2(self):
        dup = snapshot([(0, block(0) + [block(0)[2]]), (30, block(30)), (60, block(60)), (90, block(90))])
        code, out = self.run_tool(dup, self.base())
        self.assertEqual(code, 2, out)
        self.assertIn("repeats a key", out)

    def test_unreadable_file_is_exit_2(self):
        port = self.write("port.snap", self.base())
        proc = subprocess.run(
            [sys.executable, TOOL, port, os.path.join(self.dir, "missing.snap")],
            capture_output=True,
            text=True,
            encoding="utf-8",
        )
        self.assertEqual(proc.returncode, 2, proc.stdout + proc.stderr)

    def test_either_side_without_records_is_exit_2(self):
        code, out = self.run_tool("just a log\n", self.base())
        self.assertEqual(code, 2, out)
        self.assertIn("no SNAP records in the port snapshot", out)
        code, out = self.run_tool(self.base(), "just a log\n")
        self.assertEqual(code, 2, out)
        self.assertIn("no SNAP records in the cpp snapshot", out)

    def test_object_on_one_side_only(self):
        cpp = snapshot([(0, block(0)), (30, block(30)), (60, block(60, with9=False)), (90, block(90))])
        code, out = self.run_tool(self.base(), cpp)
        self.assertEqual(code, 1, out)
        self.assertIn("first divergence: frame 60", out)
        self.assertIn("O id=9: only in port", out)

    def test_frame_missing_on_one_side(self):
        cpp = snapshot([(0, block(0)), (30, block(30)), (60, block(60))])
        code, out = self.run_tool(self.base(), cpp)
        self.assertEqual(code, 1, out)
        self.assertIn("frame 90 is missing from the cpp snapshot", out)

    def test_malformed_record_is_exit_2(self):
        # Mid-file: a malformed last line is taken as a run that was killed, and is dropped with a warning.
        bad = snapshot([(0, block(0)), (30, ["SNAP f=30 BOGUS x=1"]), (60, block(60)), (90, block(90))])
        code, out = self.run_tool(self.base(), bad)
        self.assertEqual(code, 2, out)
        self.assertIn("unknown record tag", out)

    def test_no_records_is_exit_2(self):
        code, out = self.run_tool("nothing here\n", "nor here\n")
        self.assertEqual(code, 2, out)

    def test_tmpl_key_aligns_objects_queues_and_teams_with_other_ids(self):
        def side(factory_id, team_id):
            return snapshot([(0, [
                "SNAP f=0 FRAME crc=00000000 objs=1 players=1",
                "SNAP f=0 P p=0 money=1000 power=+ prod=10 cons=5 ai=1",
                f"SNAP f=0 O id={factory_id} tmpl=AmericaWarFactory owner=0 x=100.0000 y=20.0000 z=0.0000 "
                "hp=400.000 ai=Idle cons=-1.000 q=1 atk=0 bld=0",
                f"SNAP f=0 Q obj={factory_id} i=0 kind=unit tmpl=AmericaTankCrusader total=1 done=0 frames=5 door=-",
                f"SNAP f=0 S id={team_id} proto=usa%20team members=",
            ])])

        port, cpp = side(7, 1), side(11, 3)
        # The ids differ: id mode reports the pairs as objects only on one side.
        code, out = self.run_tool(port, cpp)
        self.assertEqual(code, 1, out)
        self.assertIn("O id=7: only in port", out)
        # Template + per-template creation order aligns them; the queues and teams follow their keys.
        code, out = self.run_tool(port, cpp, "--key", "tmpl")
        self.assertEqual(code, 0, out)
        self.assertIn("identical: 1 frames", out)

    def test_tmpl_key_still_reports_a_field_difference(self):
        def side(factory_id, hp):
            return snapshot([(0, [
                "SNAP f=0 FRAME crc=00000000 objs=1 players=1",
                f"SNAP f=0 O id={factory_id} tmpl=AmericaWarFactory owner=0 x=100.0000 y=20.0000 z=0.0000 "
                f"hp={hp:.3f} ai=Idle cons=-1.000 q=0 atk=0 bld=0",
            ])])

        code, out = self.run_tool(side(7, 400.0), side(11, 380.0), "--key", "tmpl")
        self.assertEqual(code, 1, out)
        self.assertIn("first divergence: frame 0", out)
        self.assertIn("O tmpl=AmericaWarFactory n=0: field hp: port 400.000, cpp 380.000", out)

    def test_tmpl_key_reports_an_extra_object_by_template_and_ordinal(self):
        extra = "SNAP f=0 O id=99 tmpl=StreetLamp owner=1 x=0.0000 y=0.0000 z=0.0000 hp=- ai=- cons=-1.000 q=0 atk=0 bld=0"
        cpp = snapshot([(0, block(0) + [extra]), (30, block(30)), (60, block(60)), (90, block(90))])
        code, out = self.run_tool(self.base(), cpp, "--key", "tmpl")
        self.assertEqual(code, 1, out)
        self.assertIn("first divergence: frame 0", out)
        self.assertIn("O tmpl=StreetLamp n=0: only in cpp", out)

    def test_skip_ignores_known_gaps_and_says_what_it_skipped(self):
        extra = "SNAP f=0 O id=99 tmpl=StreetLamp owner=1 x=0.0000 y=0.0000 z=0.0000 hp=- ai=- cons=-1.000 q=0 atk=0 bld=0"
        cpp = snapshot([(0, block(0) + [extra]), (30, block(30)), (60, block(60)), (90, block(90))])
        cpp = cpp.replace("money=1000", "money=2000")
        code, out = self.run_tool(self.base(), cpp)
        self.assertEqual(code, 1, out)
        code, out = self.run_tool(self.base(), cpp, "--skip", "P.money", "O.tmpl=StreetLamp")
        self.assertEqual(code, 0, out)
        self.assertIn("skipping by --skip: fields P.money; records O.tmpl=StreetLamp", out)
        self.assertIn("0 port, 1 cpp records", out)

    def test_skip_value_prefix(self):
        extra = "SNAP f=0 O id=99 tmpl=CarLuxury02 owner=1 x=0.0000 y=0.0000 z=0.0000 hp=- ai=- cons=-1.000 q=0 atk=0 bld=0"
        cpp = snapshot([(0, block(0) + [extra]), (30, block(30)), (60, block(60)), (90, block(90))])
        code, out = self.run_tool(self.base(), cpp, "--skip", "O.tmpl=Car*")
        self.assertEqual(code, 0, out)
        self.assertIn("records O.tmpl=Car*", out)

    def test_bad_skip_is_exit_2(self):
        code, out = self.run_tool(self.base(), self.base(), "--skip", "bogus")
        self.assertEqual(code, 2, out)
        self.assertIn("want TAG.FIELD", out)
        code, out = self.run_tool(self.base(), self.base(), "--skip", "Z.x=1")
        self.assertEqual(code, 2, out)
        self.assertIn("unknown record tag", out)

    def test_from_frame_ignores_earlier_frames(self):
        cpp = snapshot([(0, block(0, hp7=380.0)), (30, block(30)), (60, block(60)), (90, block(90))])
        code, out = self.run_tool(self.base(), cpp)
        self.assertEqual(code, 1, out)
        self.assertIn("first divergence: frame 0", out)
        code, out = self.run_tool(self.base(), cpp, "--from-frame", "30")
        self.assertEqual(code, 0, out)
        self.assertIn("comparing frames 30..90", out)

    def test_from_frame_hides_a_missing_earlier_frame_only(self):
        cpp = snapshot([(30, block(30)), (60, block(60)), (90, block(90))])
        code, out = self.run_tool(self.base(), cpp)
        self.assertEqual(code, 1, out)
        self.assertIn("frame 0 is missing from the cpp snapshot", out)
        code, out = self.run_tool(self.base(), cpp, "--from-frame", "30")
        self.assertEqual(code, 0, out)
        # Past the last frame there is nothing to compare: that is an error, not an "identical" result.
        code, out = self.run_tool(self.base(), self.base(), "--from-frame", "1000")
        self.assertEqual(code, 2, out)
        self.assertIn("at or after frame 1000", out)


if __name__ == "__main__":
    unittest.main()
