"""Static checks on the browser frontend in pynmapper/graphics/web.

No browser is involved. These tests make sure the files the page needs exist,
are served with a MIME type browsers accept for modules/stylesheets, and that
every relative import or reference points at a real file, so a typo in an
import path fails here instead of as a blank page.
"""

import json
import re
import unittest
import urllib.request

from pynmapper.graphics import MapServer, demo_map
from pynmapper.graphics.server import WEB_DIR

VENDOR = WEB_DIR / "vendor"
VENDOR_FILES = ("three.module.js", "three.core.js", "CSS2DRenderer.js", "LICENSE", "VERSION")
OWN_MODULES = ("app.js", "scene.js", "graph.js", "theme.js", "controls.js", "picking.js", "panel.js")

JS_MIME_TYPES = ("text/javascript", "application/javascript")


def get(url):
    with urllib.request.urlopen(url, timeout=5) as res:
        return res.status, res.headers["Content-Type"], res.read()


def import_map():
    html = (WEB_DIR / "index.html").read_text(encoding="utf-8")
    match = re.search(r'<script type="importmap">\s*(\{.*?\})\s*</script>', html, re.S)
    assert match, "index.html has no import map"
    return json.loads(match.group(1))["imports"]


class VendorTests(unittest.TestCase):
    def test_vendor_files_exist_and_are_not_empty(self):
        for name in VENDOR_FILES:
            with self.subTest(file=name):
                path = VENDOR / name
                self.assertTrue(path.is_file(), path)
                self.assertGreater(path.stat().st_size, 0, path)

    def test_version_file_matches_the_bundled_revision(self):
        core = (VENDOR / "three.core.js").read_text(encoding="utf-8", errors="replace")
        revision = re.search(r"const REVISION = '(\d+)'", core).group(1)
        version = (VENDOR / "VERSION").read_text(encoding="utf-8")
        self.assertIn(f"0.{revision}.", version)
        self.assertIn(f"r{revision}", version)

    def test_three_module_imports_its_core_from_the_vendor_dir(self):
        head = (VENDOR / "three.module.js").read_text(encoding="utf-8", errors="replace")[:4000]
        self.assertIn("from './three.core.js'", head)


class ReferenceTests(unittest.TestCase):
    def test_import_map_points_at_vendored_three(self):
        imports = import_map()
        self.assertEqual(imports["three"], "./vendor/three.module.js")
        self.assertTrue((WEB_DIR / "vendor/three.module.js").is_file())

    def test_index_references_exist(self):
        html = (WEB_DIR / "index.html").read_text(encoding="utf-8")
        refs = re.findall(r'(?:src|href)="(\./[^"]+)"', html)
        self.assertGreaterEqual(len(refs), 2)  # style.css + app.js at least
        for ref in refs:
            with self.subTest(ref=ref):
                self.assertTrue((WEB_DIR / ref[2:]).is_file(), ref)

    def test_module_imports_resolve(self):
        bare = set(import_map())
        for name in OWN_MODULES + ("vendor/CSS2DRenderer.js",):
            source = (WEB_DIR / name).read_text(encoding="utf-8")
            specifiers = re.findall(r"""^import[^;]*?from\s+["']([^"']+)["']""", source, re.M)
            for spec in specifiers:
                with self.subTest(file=name, spec=spec):
                    if spec.startswith("."):
                        target = (WEB_DIR / name).parent / spec
                        self.assertTrue(target.resolve().is_file(), f"{name}: {spec}")
                    else:
                        self.assertIn(spec, bare, f"{name}: bare specifier {spec!r} not in import map")

    def test_dom_ids_used_by_modules_exist_in_index(self):
        """Every getElementById / querySelector("#…") in our modules has a matching id= in index.html."""
        html = (WEB_DIR / "index.html").read_text(encoding="utf-8")
        declared = set(re.findall(r'\bid="([^"]+)"', html))
        for name in OWN_MODULES:
            source = (WEB_DIR / name).read_text(encoding="utf-8")
            used = re.findall(r"""getElementById\(["']([^"']+)["']\)""", source)
            used += re.findall(r"""querySelector\(["']#([\w-]+)["']\)""", source)
            for id_ in used:
                with self.subTest(file=name, id=id_):
                    self.assertIn(id_, declared, f"{name} looks up #{id_}, which index.html does not declare")


class ServedAssetTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.server = MapServer(demo_map(count=3), port=0).start()

    @classmethod
    def tearDownClass(cls):
        cls.server.stop()

    def test_stylesheet_and_modules_have_browser_friendly_types(self):
        base = self.server.url
        status, ctype, _ = get(base + "style.css")
        self.assertEqual(status, 200)
        self.assertTrue(ctype.startswith("text/css"), ctype)
        for name in OWN_MODULES + ("vendor/three.module.js", "vendor/three.core.js", "vendor/CSS2DRenderer.js"):
            with self.subTest(file=name):
                status, ctype, body = get(base + name)
                self.assertEqual(status, 200)
                self.assertTrue(ctype.startswith(JS_MIME_TYPES), ctype)
                self.assertGreater(len(body), 0)

    def test_index_loads_the_app_module(self):
        _, _, body = get(self.server.url)
        self.assertIn(b'<script type="module" src="./app.js">', body)
        self.assertIn(b'<script type="importmap">', body)


if __name__ == "__main__":
    unittest.main()
