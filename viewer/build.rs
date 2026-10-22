//! Lets the binary find the libpython it is linked against at run time.
//!
//! PyO3 links the binary against the libpython of the interpreter named by
//! `PYO3_PYTHON` (`.cargo/config.toml`). uv installs Python under
//! `~/.local/share/uv/python`, where the dynamic loader does not look, so this
//! records that directory in the binary itself (an "rpath").

fn main() {
    pyo3_build_config::add_libpython_rpath_link_args();
}
