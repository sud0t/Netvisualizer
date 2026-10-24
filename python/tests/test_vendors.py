"""Tests for netvisualizer.vendors: MAC addresses to vendor names."""

import unittest

from netvisualizer import vendors


class VendorNames(unittest.TestCase):
    def test_the_longest_registered_prefix_wins(self):
        names = vendors.vendor_names(
            [
                "b8:27:eb:8e:cc:eb",  # 24-bit block (MA-L)
                "1c:87:76:d0:00:01",  # 28-bit block (MA-M) inside an IEEE-held 24-bit one
                "00:50:c2:00:01:02",  # 36-bit block (MA-S / IAB) inside 00:50:C2
                "00:50:c2:ff:01:02",  # another 36-bit block of the same 24-bit one
                "00:50:c2:04:d1:02",  # no 36-bit block there: falls back to the 24-bit owner
            ]
        )
        self.assertEqual(
            names,
            [
                "Raspberry Pi Foundation",
                "Qivivo",
                "T.L.S. Corp.",
                "GD Mission Systems",
                "IEEE Registration Authority",
            ],
        )

    def test_unknown_addresses_are_none(self):
        names = vendors.vendor_names(
            [
                "aa:b7:5d:f6:d4:16",  # randomised by a phone: never registered
                "1c:87:76:f0:00:01",  # 1C:87:76 is sold in 28-bit blocks, this one not yet
                "00:48:54:00:00:01",  # a Private block: registered, but no name given
            ]
        )
        self.assertEqual(names, [None, None, None])

    def test_case_and_separators_do_not_matter(self):
        names = vendors.vendor_names(["B8:27:EB:8E:CC:EB", "b8-27-eb-8e-cc-eb"])
        self.assertEqual(names, ["Raspberry Pi Foundation"] * 2)

    def test_an_empty_list_gives_an_empty_list(self):
        self.assertEqual(vendors.vendor_names([]), [])


if __name__ == "__main__":
    unittest.main()
