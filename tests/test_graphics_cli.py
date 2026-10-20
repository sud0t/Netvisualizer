"""Tests for the graphics command line (python -m pynmapper.graphics)."""

import contextlib
import io
import json
import tempfile
import unittest
from pathlib import Path

from pynmapper.graphics.__main__ import build_parser, main


def run(*argv):
    out, err = io.StringIO(), io.StringIO()
    with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
        code = main(list(argv))
    return code, out.getvalue(), err.getvalue()


class CliTests(unittest.TestCase):
    def test_demo_dump_prints_contract(self):
        code, out, _ = run("--demo", "--dump", "--count", "4")
        self.assertEqual(code, 0)
        doc = json.loads(out)
        self.assertEqual(len(doc["hosts"]), 6)
        self.assertEqual(doc["scan"]["type"], "demo")

    def test_save_then_json_round_trip(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "map.json"
            code, out, err = run("--demo", "--count", "3", "--save", str(path), "--dump")
            self.assertEqual(code, 0)
            self.assertIn("saved 5 hosts", err)
            code, out2, _ = run("--json", str(path), "--dump")
            self.assertEqual(code, 0)
            self.assertEqual(json.loads(out2), json.loads(out))

    def test_layout_option_recomputes_positions(self):
        code, out, _ = run("--demo", "--dump", "--layout", "star_ring")
        self.assertEqual(code, 0)
        self.assertTrue(all(h["position"] is not None for h in json.loads(out)["hosts"]))

    def test_missing_json_file_is_a_clean_error(self):
        code, _, err = run("--json", "/nonexistent/map.json", "--dump")
        self.assertEqual(code, 1)
        self.assertIn("could not load map", err)

    def test_source_is_required_and_exclusive(self):
        parser = build_parser()
        with self.assertRaises(SystemExit):
            with contextlib.redirect_stderr(io.StringIO()):
                parser.parse_args([])
        with self.assertRaises(SystemExit):
            with contextlib.redirect_stderr(io.StringIO()):
                parser.parse_args(["--demo", "--json", "x.json"])


if __name__ == "__main__":
    unittest.main()
