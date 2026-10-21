"""Tests for how python -m pynmapper starts the viewer and waits for it."""

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
    server = mock.Mock(spec=["url", "serve_forever", "stop"])
    server.url = url
    return server


class ParserTests(unittest.TestCase):
    def test_the_viewer_opens_unless_no_open(self):
        self.assertFalse(cli.build_parser().parse_args([]).no_open)
        self.assertTrue(cli.build_parser().parse_args(["--no-open"]).no_open)

    def test_there_is_no_viewer_choice_any_more(self):
        with self.assertRaises(SystemExit), contextlib.redirect_stderr(io.StringIO()):
            cli.build_parser().parse_args(["--viewer", "web"])

    def test_no_browser_is_now_no_open(self):
        with self.assertRaises(SystemExit), contextlib.redirect_stderr(io.StringIO()):
            cli.build_parser().parse_args(["--no-browser"])


class MissingViewerTests(unittest.TestCase):
    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        patcher = mock.patch.object(cli, "VIEWER_BINARY", Path(tmp.name) / "missing")
        patcher.start()
        self.addCleanup(patcher.stop)

    def test_says_how_to_build_it(self):
        err = io.StringIO()
        with contextlib.redirect_stderr(err):
            self.assertFalse(cli.viewer_is_built())
        self.assertIn("cargo build --release", err.getvalue())
        self.assertIn("--no-open", err.getvalue())

    def test_main_stops_before_serving_anything(self):
        with mock.patch.object(cli, "serve") as serve, contextlib.redirect_stderr(io.StringIO()):
            self.assertEqual(cli.main(["--demo"]), 1)
        serve.assert_not_called()

    def test_no_open_and_no_serve_do_not_need_it(self):
        for flag in ("--no-open", "--no-serve"):
            with self.subTest(flag=flag), mock.patch.object(cli, "serve", return_value=(None, None)) as serve:
                self.assertEqual(cli.main(["--demo", flag]), 0)
                serve.assert_called_once()


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

    def test_runs_the_viewer_on_the_server_url_quietly(self):
        script, record = self.stand_in_viewer()
        server = fake_server("http://127.0.0.1:9999/")
        with mock.patch.object(cli, "VIEWER_BINARY", script), mock.patch.dict(os.environ, clear=False) as env:
            env.pop("RUST_LOG", None)
            process = cli.open_viewer(server)
            self.assertIsInstance(process, subprocess.Popen)
            self.assertEqual(process.wait(timeout=10), 0)
        self.assertEqual(record.read_text().splitlines(), ["http://127.0.0.1:9999/", "warn"])

    def test_verbose_keeps_the_viewer_log(self):
        script, record = self.stand_in_viewer()
        with mock.patch.object(cli, "VIEWER_BINARY", script), mock.patch.dict(os.environ, clear=False) as env:
            env.pop("RUST_LOG", None)
            cli.open_viewer(fake_server(), quiet=False).wait(timeout=10)
        self.assertEqual(record.read_text().splitlines()[1], "")

    def test_closing_the_viewer_stops_the_server(self):
        script, _ = self.stand_in_viewer(exit_status=0)
        server = fake_server()
        self.assertEqual(cli.wait(server, subprocess.Popen([str(script)])), 0)
        server.stop.assert_called_once_with()
        server.serve_forever.assert_not_called()

    def test_a_failing_viewer_stops_the_server_and_fails(self):
        script, _ = self.stand_in_viewer(exit_status=101)  # a Rust panic, e.g. no usable GPU
        server = fake_server()
        err = io.StringIO()
        with contextlib.redirect_stderr(err):
            self.assertEqual(cli.wait(server, subprocess.Popen([str(script)])), 1)
        server.stop.assert_called_once_with()
        server.serve_forever.assert_not_called()
        self.assertIn("status 101", err.getvalue())

    def test_without_a_viewer_the_server_runs_until_ctrl_c(self):
        server = fake_server()
        self.assertEqual(cli.wait(server, None), 0)
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

    def test_the_viewer_is_opened_on_the_running_server(self):
        with mock.patch.object(cli, "open_viewer", return_value=None) as open_viewer, \
                contextlib.redirect_stdout(io.StringIO()):
            server, _ = cli.serve(cli.demo_map(count=2), self.args())
        try:
            open_viewer.assert_called_once_with(server, quiet=True)
        finally:
            server.stop()


if __name__ == "__main__":
    unittest.main(argv=[sys.argv[0]])
