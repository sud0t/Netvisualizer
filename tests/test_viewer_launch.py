"""Tests for how python -m pynmapper shows the map: native viewer or browser."""

import contextlib
import io
import os
import stat
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

from pynmapper import __main__ as cli


def fake_server(url="http://127.0.0.1:8765/"):
    server = mock.Mock(spec=["url", "open_browser", "serve_forever", "stop"])
    server.url = url
    return server


class ParserTests(unittest.TestCase):
    def test_native_viewer_is_the_default(self):
        args = cli.build_parser().parse_args([])
        self.assertEqual(args.viewer, "native")
        self.assertFalse(args.no_open)

    def test_web_viewer_and_no_open(self):
        args = cli.build_parser().parse_args(["--viewer", "web", "--no-open"])
        self.assertEqual(args.viewer, "web")
        self.assertTrue(args.no_open)

    def test_unknown_viewer_is_rejected(self):
        with self.assertRaises(SystemExit), contextlib.redirect_stderr(io.StringIO()):
            cli.build_parser().parse_args(["--viewer", "vr"])

    def test_no_browser_is_now_no_open(self):
        with self.assertRaises(SystemExit), contextlib.redirect_stderr(io.StringIO()):
            cli.build_parser().parse_args(["--no-browser"])


@unittest.skipIf(os.name == "nt", "uses a shell script as the stand-in viewer")
class OpenViewerTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.dir = Path(self.tmp.name)

    def stand_in_viewer(self, exit_status=0):
        """An executable that records its arguments and RUST_LOG, then exits."""
        record = self.dir / "argv.txt"
        script = self.dir / "pynmapper-viewer"
        script.write_text(f'#!/bin/sh\necho "$@" > "{record}"\necho "$RUST_LOG" >> "{record}"\nexit {exit_status}\n')
        script.chmod(script.stat().st_mode | stat.S_IXUSR)
        return script, record

    def test_web_opens_the_browser(self):
        server = fake_server()
        self.assertIsNone(cli.open_viewer(server, "web"))
        server.open_browser.assert_called_once_with()

    def test_native_without_a_build_falls_back_to_the_browser_and_says_how_to_build(self):
        server = fake_server()
        err = io.StringIO()
        with mock.patch.object(cli, "VIEWER_BINARY", self.dir / "missing"), contextlib.redirect_stderr(err):
            self.assertIsNone(cli.open_viewer(server, "native"))
        server.open_browser.assert_called_once_with()
        self.assertIn("cargo build --release", err.getvalue())
        self.assertIn("opening the browser instead", err.getvalue())

    def test_native_runs_the_viewer_on_the_server_url_quietly(self):
        script, record = self.stand_in_viewer()
        server = fake_server("http://127.0.0.1:9999/")
        with mock.patch.object(cli, "VIEWER_BINARY", script), mock.patch.dict(os.environ, clear=False) as env:
            env.pop("RUST_LOG", None)
            process = cli.open_viewer(server, "native")
            self.assertIsInstance(process, subprocess.Popen)
            self.assertEqual(process.wait(timeout=10), 0)
        server.open_browser.assert_not_called()
        self.assertEqual(record.read_text().splitlines(), ["http://127.0.0.1:9999/", "warn"])

    def test_verbose_keeps_the_viewer_log(self):
        script, record = self.stand_in_viewer()
        with mock.patch.object(cli, "VIEWER_BINARY", script), mock.patch.dict(os.environ, clear=False) as env:
            env.pop("RUST_LOG", None)
            cli.open_viewer(fake_server(), "native", quiet=False).wait(timeout=10)
        self.assertEqual(record.read_text().splitlines()[1], "")

    def test_closing_the_viewer_stops_the_server(self):
        script, _ = self.stand_in_viewer(exit_status=0)
        server = fake_server()
        cli.wait(server, subprocess.Popen([str(script)]))
        server.stop.assert_called_once_with()
        server.open_browser.assert_not_called()
        server.serve_forever.assert_not_called()

    def test_a_failing_viewer_leaves_the_map_in_the_browser(self):
        script, _ = self.stand_in_viewer(exit_status=101)  # a Rust panic, e.g. no usable GPU
        server = fake_server()
        err = io.StringIO()
        with contextlib.redirect_stderr(err):
            cli.wait(server, subprocess.Popen([str(script)]))
        server.open_browser.assert_called_once_with()
        server.serve_forever.assert_called_once_with()
        self.assertIn("status 101", err.getvalue())

    def test_without_a_viewer_the_server_runs_until_ctrl_c(self):
        server = fake_server()
        cli.wait(server, None)
        server.serve_forever.assert_called_once_with()


class ServeTests(unittest.TestCase):
    def args(self, *argv):
        return cli.build_parser().parse_args(["--port", "0", *argv])

    def test_no_open_serves_without_opening_anything(self):
        with mock.patch.object(cli, "open_viewer") as open_viewer:
            server, viewer = cli.serve(cli.demo_map(count=2), self.args("--no-open"))
        try:
            self.assertTrue(server.running)
            self.assertIsNone(viewer)
            open_viewer.assert_not_called()
        finally:
            server.stop()

    def test_no_serve_starts_nothing(self):
        self.assertEqual(cli.serve(cli.demo_map(count=2), self.args("--no-serve")), (None, None))

    def test_the_chosen_viewer_is_opened_on_the_running_server(self):
        with mock.patch.object(cli, "open_viewer", return_value=None) as open_viewer, \
                contextlib.redirect_stdout(io.StringIO()):
            server, _ = cli.serve(cli.demo_map(count=2), self.args("--viewer", "web"))
        try:
            open_viewer.assert_called_once_with(server, "web", quiet=True)
        finally:
            server.stop()


if __name__ == "__main__":
    unittest.main(argv=[sys.argv[0]])
