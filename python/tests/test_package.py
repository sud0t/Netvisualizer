"""Tests for the netvisualizer package, the Python the program embeds."""

import unittest

import netvisualizer


class OptionalPackages(unittest.TestCase):
    def test_every_optional_package_is_reported(self):
        found = netvisualizer.optional_packages()
        self.assertEqual(set(found), set(netvisualizer.OPTIONAL_PACKAGES))
        self.assertTrue(all(isinstance(here, bool) for here in found.values()))


if __name__ == "__main__":
    unittest.main()
