"""Who made a network interface, from its MAC address.

The IEEE sells makers blocks of MAC addresses and publishes the list;
``assets/mac-vendors-export.csv`` (from maclookup.app) is that list, one row
per block: ``Mac Prefix``, ``Vendor Name``, ``Private``, ``Block Type``,
``Last Update``. Blocks come in three sizes, so a prefix is 6, 7 or 9 hex
digits (24, 28 or 36 bits)::

    00:00:0C        MA-L (and CID): 24 bits, 16 million addresses
    1C:87:76:D      MA-M: 28 bits
    00:50:C2:00:0   MA-S (and the older IAB): 36 bits

A finer block is carved out of a coarser one, so an address is matched
against the longest prefix first. A block whose owner paid to stay
``Private`` has an empty name and counts as unknown.

The program calls :func:`vendor_names` from Rust (``viewer/src/vendors.rs``)
after the ping sweep, with the MACs the kernel's ARP table gave it.
"""

from __future__ import annotations

from functools import cache
from pathlib import Path

import pandas as pd

CSV = Path(__file__).resolve().parent.parent / "assets" / "mac-vendors-export.csv"

# How many characters of a written address each block size covers, longest
# first, so a finer block wins over the coarser one it was carved out of.
PREFIX_LENGTHS = (len("00:50:C2:00:0"), len("1C:87:76:D"), len("00:00:0C"))


@cache
def vendors_by_prefix() -> pd.Series:
    """Vendor name by upper-case prefix, read from the CSV once per process.

    Private blocks have no name and are left out, so a miss and a Private
    block look the same to the caller: no vendor.
    """
    table = pd.read_csv(
        CSV, usecols=["Mac Prefix", "Vendor Name"], dtype=str, keep_default_na=False
    )
    table = table[table["Vendor Name"] != ""]
    return pd.Series(table["Vendor Name"].to_numpy(), index=table["Mac Prefix"].str.upper())


def vendor_names(macs: list[str]) -> list[str | None]:
    """The vendor for each MAC in ``macs``, or ``None``, in the same order.

    ``macs`` are written ``b8:27:eb:8e:cc:eb`` (any case, ``:`` or ``-``).
    """
    addresses = pd.Series(macs, dtype=str).str.upper().str.replace("-", ":", regex=False)
    names = pd.Series([None] * len(addresses), dtype=object)
    for length in PREFIX_LENGTHS:
        # `map` with a Series is a vectorised dictionary lookup: each address
        # cut to this prefix length, looked up, NaN where the prefix is not a
        # block. `where` keeps the names already found by a longer prefix.
        found = addresses.str[:length].map(vendors_by_prefix())
        names = names.where(names.notna(), found)
    return [None if pd.isna(name) else str(name) for name in names]
