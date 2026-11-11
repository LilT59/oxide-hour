#!/usr/bin/env python3
"""Self-test for tools/trace_diff.py: small synthetic per-object traces, no game data.

Run: python tools/test_trace_diff.py
"""
import os
import subprocess
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
TOOL = os.path.join(HERE, "trace_diff.py")


def rec(f, oid, tmpl="AmericaTankCrusader", x="100.0000", y="20.0000", ai="Idle", gx="0.0000,0.0000,0.0000"):
    """One trace line, shaped like the writers' output."""
    return (
        f"TRACE f={f} id={oid} tmpl={tmpl} x={x} y={y} z=0.0000 ang=0.0000 vx=0.0000 vy=0.0000 vz=0.0000 "
        f"ai={ai} pcount=0 pfirst=- gx={gx}"
    )


def trace(frame_pairs):
    """A file from (frame, [object lines]) pairs, with a C++-style log line in front (ignored)."""
    out = ["Log: some printf output the tool must skip"]
    for _, lines in frame_pairs:
        out.extend(lines)
    return "\n".join(out) + "\n"


def two(f, x7="100.0000", y7="20.0000", ai7="Idle", with9=True, extra9=None, gx="0.0000,0.0000,0.0000"):
    lines = [rec(f, 7, x=x7, y=y7, ai=ai7, gx=gx)]
    if with9:
        lines.append(extra9 or rec(f, 9, tmpl="GLATechnical", x="50.0000", y="50.0000"))
    return (f, lines)


class TraceDiffTest(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory(prefix="trace_diff_test_")
        self.dir = self._tmp.name

    def tearDown(self):
        self._tmp.cleanup()

    def write(self, name, text):
        path = os.path.join(self.dir, name)
        with open(path, "w", encoding="utf-8") as fh:
            fh.write(text)
        return path

    def run_tool(self, port_text, cpp_text, *args):
        port = self.write("port.trace", port_text)
        cpp = self.write("cpp.trace", cpp_text)
        proc = subprocess.run(
            [sys.executable, TOOL, port, cpp, *args], capture_output=True, text=True, encoding="utf-8"
        )
        return proc.returncode, proc.stdout + proc.stderr

    def base(self):
        return trace([two(0), two(30), two(60), two(90)])

    def test_identical_files_exit_0(self):
        code, out = self.run_tool(self.base(), self.base())
        self.assertEqual(code, 0, out)
        self.assertIn("identical: no traced object differs", out)
        self.assertIn("port: 4 frames, 8 records, frames 0..90, 2 ids", out)

    def test_a_changed_field_reports_frame_id_and_field(self):
        cpp = trace([two(0), two(30), two(60, x7="103.0000"), two(90)])
        code, out = self.run_tool(self.base(), cpp)
        self.assertEqual(code, 1, out)
        self.assertIn("first difference: frame 60, id 7 (AmericaTankCrusader)", out)
        self.assertIn("field x: port 100.0000, cpp 103.0000", out)
        self.assertIn("id=7 (AmericaTankCrusader): frame 60, field x", out)
        self.assertNotIn("id=9", out.split("per object")[1])

    def test_float_compared_by_value_and_tolerance(self):
        # The same value printed differently is not a difference; a real delta needs --tol.
        same = trace([two(0, x7="100.0"), two(30), two(60), two(90)])
        self.assertEqual(self.run_tool(same, self.base())[0], 0)
        near = trace([two(0), two(30, x7="100.4000"), two(60), two(90)])
        self.assertEqual(self.run_tool(self.base(), near)[0], 1)
        self.assertEqual(self.run_tool(self.base(), near, "--tol", "0.5")[0], 0)
        self.assertEqual(self.run_tool(self.base(), near, "--tol", "0.1")[0], 1)

    def test_a_packed_position_compares_component_by_component(self):
        cpp = trace([two(0), two(30, gx="0.0000,1.0000,0.0000"), two(60), two(90)])
        code, out = self.run_tool(self.base(), cpp)
        self.assertEqual(code, 1, out)
        self.assertIn("field gx: port 0.0000,0.0000,0.0000, cpp 0.0000,1.0000,0.0000", out)
        self.assertEqual(self.run_tool(self.base(), cpp, "--tol", "2")[0], 0)

    def test_fields_selects_the_comparison(self):
        ai = trace([two(0), two(30, ai7="AttackObject"), two(60), two(90)])
        self.assertEqual(self.run_tool(self.base(), ai)[0], 1)
        self.assertEqual(self.run_tool(self.base(), ai, "--fields", "x,y,z")[0], 0)
        pos = trace([two(0), two(30, x7="101.0000"), two(60), two(90)])
        self.assertEqual(self.run_tool(self.base(), pos, "--fields", "ai")[0], 0)
        code, out = self.run_tool(self.base(), pos, "--fields", "x")
        self.assertEqual(code, 1, out)
        self.assertIn("comparing fields x (tolerance 0)", out)

    def test_ids_filters_objects(self):
        cpp = trace([two(0), two(30, extra9=rec(30, 9, tmpl="GLATechnical", x="999.0000", y="50.0000")), two(60), two(90)])
        self.assertEqual(self.run_tool(self.base(), cpp)[0], 1)
        self.assertEqual(self.run_tool(self.base(), cpp, "--ids", "7")[0], 0)
        self.assertEqual(self.run_tool(self.base(), cpp, "--ids", "9")[0], 1)

    def test_two_objects_are_listed_with_the_earliest_first(self):
        cpp = trace([two(0), two(30), two(60, x7="200.0000", extra9=rec(60, 9, tmpl="GLATechnical", x="50.0000", y="80.0000")), two(90)])
        code, out = self.run_tool(self.base(), cpp)
        self.assertEqual(code, 1, out)
        self.assertIn("first difference: frame 60, id 7", out)
        self.assertIn("id=7 (AmericaTankCrusader): frame 60, field x", out)
        self.assertIn("id=9 (GLATechnical): frame 60, field y", out)

    def test_a_record_missing_on_one_side_is_a_difference(self):
        cpp = trace([two(0), two(30, with9=False), two(60), two(90)])
        code, out = self.run_tool(self.base(), cpp)
        self.assertEqual(code, 1, out)
        self.assertIn("first difference: frame 30, id 9 (GLATechnical)", out)
        self.assertIn("only in port", out)

    def test_an_object_only_on_one_side_is_a_difference_at_its_first_frame(self):
        port = trace([(0, [rec(0, 7)]), (30, [rec(30, 7)])])
        cpp = trace([(0, [rec(0, 7)]), (30, [rec(30, 7), rec(30, 9, tmpl="GLATechnical")])])
        code, out = self.run_tool(port, cpp)
        self.assertEqual(code, 1, out)
        self.assertIn("frame 30, id 9", out)
        self.assertIn("only in cpp (GLATechnical)", out)

    def test_window_prints_only_the_differing_fields(self):
        port = trace([two(f) for f in range(57, 64)])
        cpp = trace([two(f, x7=("103.0000" if f == 60 else "100.0000")) for f in range(57, 64)])
        code, out = self.run_tool(port, cpp, "--window", "1")
        self.assertEqual(code, 1, out)
        self.assertIn("window around frame 60, id 7 (AmericaTankCrusader), differing fields:", out)
        self.assertIn("  f=59 port x=100.0000", out)
        self.assertIn("  f=59 cpp  x=100.0000", out)
        self.assertIn("  f=60 cpp  x=103.0000", out)
        self.assertNotIn("  f=62 ", out)  # the window is one frame each side

    def test_a_cut_off_final_line_is_dropped_with_a_warning(self):
        text = self.base()
        port = self.write("cut.trace", text + "TRACE f=120 id=7 tmpl=AmericaTankCrusader x=9")
        proc = subprocess.run([sys.executable, TOOL, port, port], capture_output=True, text=True, encoding="utf-8")
        self.assertEqual(proc.returncode, 0, proc.stdout + proc.stderr)
        self.assertIn("dropped a final line without a newline", proc.stderr)

    def test_malformed_and_repeated_and_empty_inputs_exit_2(self):
        bad = trace([(0, ["TRACE f=0 id=7 tmpl=X junk"])])
        self.assertEqual(self.run_tool(self.base(), bad)[0], 2)
        repeated = trace([(0, [rec(0, 7), rec(0, 7)])])
        code, out = self.run_tool(self.base(), repeated)
        self.assertEqual(code, 2, out)
        self.assertIn("repeated", out)
        self.assertEqual(self.run_tool(self.base(), "Log: nothing\n")[0], 2)
        code, out = self.run_tool(self.base(), self.base(), "--ids", "x")
        self.assertEqual(code, 2, out)
        self.assertIn("--ids", out)
        self.assertEqual(self.run_tool(self.base(), self.base(), "--tol", "-1")[0], 2)
        self.assertEqual(self.run_tool(self.base(), self.base(), "--window", "-1")[0], 2)

    def test_a_one_sided_window_falls_back_to_the_template(self):
        cpp = trace([two(0), two(30, with9=False), two(60), two(90)])
        code, out = self.run_tool(self.base(), cpp, "--window", "1")
        self.assertEqual(code, 1, out)
        self.assertIn("  f=30 cpp  <absent>", out)
        self.assertIn("  f=30 port tmpl=GLATechnical", out)

    def test_gap_fields_are_skipped_by_default(self):
        def gap_rec(f, aiwake="0", dtask="build", dbuild="SelectBuildDock", victim="0"):
            return (
                f"TRACE f={f} id=7 tmpl=AmericaDozer x=1.0000 y=2.0000 ai=Idle aiwake={aiwake} "
                f"aiphase=0 sub=- dtask={dtask} dbuild={dbuild} dact=- victim={victim}"
            )

        port = trace([(f, [gap_rec(f)]) for f in (0, 30)])
        cpp = trace([(f, [gap_rec(f, aiwake="31", dtask="invalid", dbuild="-", victim="7")]) for f in (0, 30)])
        code, out = self.run_tool(port, cpp)
        self.assertEqual(code, 0, out)
        self.assertIn("skipping gap fields aiwake,aiphase,dtask,dact,dbuild,victim", out)
        code, out = self.run_tool(port, cpp, "--all-fields")
        self.assertEqual(code, 1, out)
        self.assertIn("field aiwake: port 0, cpp 31", out)
        code, out = self.run_tool(port, cpp, "--fields", "dtask")
        self.assertEqual(code, 1, out)
        self.assertIn("field dtask: port build, cpp invalid", out)

    def test_a_real_difference_is_reported_before_the_gap_fields(self):
        def rec_with_gaps(f, x, aiwake):
            return f"TRACE f={f} id=7 tmpl=AmericaDozer x={x} y=2.0000 ai=Idle aiwake={aiwake} dtask=- dbuild=- dact=- victim=0"

        port = trace([(0, [rec_with_gaps(0, "1.0000", "0")]), (30, [rec_with_gaps(30, "1.0000", "0")])])
        cpp = trace([(0, [rec_with_gaps(0, "1.0000", "31")]), (30, [rec_with_gaps(30, "9.0000", "31")])])
        code, out = self.run_tool(port, cpp)
        self.assertEqual(code, 1, out)
        self.assertIn("first difference: frame 30, id 7", out)
        self.assertIn("field x: port 1.0000, cpp 9.0000", out)

    def test_unreadable_file_exits_2(self):
        proc = subprocess.run(
            [sys.executable, TOOL, os.path.join(self.dir, "nope.trace"), os.path.join(self.dir, "nope2.trace")],
            capture_output=True,
            text=True,
            encoding="utf-8",
        )
        self.assertEqual(proc.returncode, 2, proc.stdout + proc.stderr)


if __name__ == "__main__":
    unittest.main()
