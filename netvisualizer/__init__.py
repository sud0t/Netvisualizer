"""Python that the netvisualizer program embeds.

The program itself is Rust (``scanner/`` finds the hosts, ``viewer/`` draws
them). It runs this package inside its own process through PyO3
(``viewer/src/python.rs``), for the parts that stay in Python: the MAC vendor
lookup (Milestone 6) and the SSH/Telnet console (Milestone 11).

Before the first import, the program puts the repo root and ``.venv``'s
site-packages on ``sys.path``, so packages installed into ``.venv`` with pip
are importable here.
"""

from importlib.util import find_spec

# Third-party packages the later features use; missing ones only switch off
# the feature that needs them.
OPTIONAL_PACKAGES = ("pandas", "asyncssh")


def optional_packages() -> dict[str, bool]:
    """Which of OPTIONAL_PACKAGES can be imported, without importing them."""
    return {name: find_spec(name) is not None for name in OPTIONAL_PACKAGES}
