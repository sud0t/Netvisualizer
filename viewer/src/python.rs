//! The embedded Python interpreter (`PyO3`), for the parts of Netvisualizer
//! that stay in Python: the MAC vendor lookup (Milestone 6) and the
//! SSH/Telnet console (Milestone 13). Their code is the `netvisualizer`
//! package in the repo's `python/` directory.
//!
//! The binary links the system's libpython (the one `.venv` was made from),
//! so the interpreter runs inside this process: no `python` subprocess, and
//! Rust values go in and out as Python objects. An embedded interpreter
//! knows nothing about the repo or its virtual environment, so the first use
//! puts the repo's `python/` directory (for `import netvisualizer`) and
//! `.venv`'s site-packages (for pandas, asyncssh, ...) on `sys.path`.
//!
//! Python errors come back as one line of text, such as
//! `ModuleNotFoundError: No module named 'pandas'`: a feature that needs
//! Python skips itself with that message instead of stopping the program.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use pyo3::prelude::*;
use pyo3::sync::PyOnceLock;

/// What the first use of Python set up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Interpreter {
    /// The interpreter's version, e.g. "3.14.8".
    pub version: String,
    /// The directory `import netvisualizer` finds the package in: `python/`
    /// in the repo.
    pub package_root: PathBuf,
    /// The virtual environment's site-packages, if there is one.
    pub site_packages: Option<PathBuf>,
}

/// Set up once per process; a failure is remembered too.
static INTERPRETER: PyOnceLock<Result<Interpreter, String>> = PyOnceLock::new();

/// Start Python if it is not running yet and set up `sys.path`; then say
/// what was found.
pub fn interpreter() -> Result<Interpreter, String> {
    Python::attach(|py| {
        INTERPRETER
            .get_or_init(py, || set_up(py).map_err(|err| err.to_string()))
            .clone()
    })
}

/// Run `f` with the `netvisualizer` package imported, starting Python the
/// first time.
pub fn with_package<R>(
    f: impl for<'py> FnOnce(&Bound<'py, PyModule>) -> PyResult<R>,
) -> Result<R, String> {
    interpreter()?;
    Python::attach(|py| {
        let package = py.import("netvisualizer")?;
        f(&package)
    })
    .map_err(|err| err.to_string())
}

/// One line for `-v`: the interpreter, where the package and `.venv` are,
/// and which of the packages later features need can be imported.
pub fn describe() -> String {
    let interpreter = match interpreter() {
        Ok(interpreter) => interpreter,
        Err(err) => return format!("Python: not available, features that need it are off ({err})"),
    };
    let venv = match &interpreter.site_packages {
        Some(dir) => dir.display().to_string(),
        None => "no .venv found".to_string(),
    };
    let packages = match with_package(|package| {
        package
            .call_method0("optional_packages")?
            .extract::<BTreeMap<String, bool>>()
    }) {
        Ok(found) => found
            .iter()
            .map(|(name, here)| format!("{name} {}", if *here { "yes" } else { "missing" }))
            .collect::<Vec<_>>()
            .join(", "),
        Err(err) => format!("the netvisualizer package does not load: {err}"),
    };
    format!(
        "Python {} with the package in {} and site-packages from {venv}; {packages}",
        interpreter.version,
        interpreter.package_root.display()
    )
}

fn set_up(py: Python<'_>) -> PyResult<Interpreter> {
    let package_root = repo_root().join("python");
    // As text: the import system skips sys.path entries that are not str.
    py.import("sys")?
        .getattr("path")?
        .call_method1("insert", (0, package_root.to_string_lossy()))?;

    let version = py.version_info();
    let site_packages = venvs().into_iter().find_map(|venv| {
        let dir = venv
            .join("lib")
            .join(format!("python{}.{}", version.major, version.minor))
            .join("site-packages");
        dir.is_dir().then_some(dir)
    });
    if let Some(dir) = &site_packages {
        // What `python` itself does for a venv: the directory, plus any
        // .pth files in it, goes on sys.path.
        py.import("site")?
            .call_method1("addsitedir", (dir.to_string_lossy(),))?;
    }
    Ok(Interpreter {
        version: format!("{}.{}.{}", version.major, version.minor, version.patch),
        package_root,
        site_packages,
    })
}

/// The repo this binary was built from (the parent of `viewer/`). The
/// program is run from its repo, so the path it was compiled in is where
/// the Python package is.
fn repo_root() -> PathBuf {
    let viewer = Path::new(env!("CARGO_MANIFEST_DIR"));
    viewer.parent().unwrap_or(viewer).to_path_buf()
}

/// The virtual environments to take packages from, in order of preference:
/// the activated one (`VIRTUAL_ENV`), then the repo's `.venv`. The first
/// with site-packages for the embedded Python wins: an activated venv of
/// some other project, or of another Python, has nothing for us.
fn venvs() -> Vec<PathBuf> {
    let active = std::env::var_os("VIRTUAL_ENV").map(PathBuf::from);
    active
        .into_iter()
        .chain(std::iter::once(repo_root().join(".venv")))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_package_is_found_and_answers() {
        let interpreter = interpreter().unwrap();
        assert!(interpreter.version.starts_with("3."), "{interpreter:?}");
        assert!(interpreter.package_root.join("netvisualizer").is_dir());

        let name = with_package(|package| package.getattr("__name__")?.extract::<String>());
        assert_eq!(name.as_deref(), Ok("netvisualizer"));

        let packages = with_package(|package| {
            package
                .call_method0("optional_packages")?
                .extract::<BTreeMap<String, bool>>()
        })
        .unwrap();
        assert!(packages.contains_key("pandas"), "{packages:?}");
    }

    #[test]
    fn python_errors_come_back_as_one_line() {
        let err = with_package(|package| package.call_method0("no_such_function").map(|_| ()))
            .unwrap_err();
        assert!(err.starts_with("AttributeError"), "{err}");
        assert!(!err.contains('\n'), "{err}");
    }

    #[test]
    fn describe_names_the_interpreter() {
        let line = describe();
        assert!(line.starts_with("Python 3."), "{line}");
    }
}
