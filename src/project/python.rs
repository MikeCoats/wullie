// SPDX-FileCopyrightText: 2026 Mike Coats <i.am@mikecoats.com>
// SPDX-License-Identifier: GPL-3.0-or-later

use std::ffi::OsStr;

/// Directories holding installed or cached Python code rather than projects.
/// Every directory with one of these names is pruned, including ones in
/// non-Python trees.
///
/// Virtualenvs are caught by `site-packages` whatever the venv is called;
/// hidden ones like `.venv` and `.tox` are already skipped by the walk.
pub const SKIP_DIRS: &[&str] = &["__pycache__", "__pypackages__", "site-packages"];

/// Returns whether `file_name` is a `pyproject.toml` manifest.
pub fn is_manifest(file_name: &OsStr) -> bool {
    file_name == "pyproject.toml"
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;

    use super::is_manifest;

    #[test]
    fn detects_pyproject_manifest() {
        assert!(is_manifest(OsStr::new("pyproject.toml")));
    }

    #[test]
    fn rejects_legacy_and_other_names() {
        assert!(!is_manifest(OsStr::new("setup.py")));
        assert!(!is_manifest(OsStr::new("setup.cfg")));
        assert!(!is_manifest(OsStr::new("requirements.txt")));
        assert!(!is_manifest(OsStr::new("Pipfile")));
        assert!(!is_manifest(OsStr::new("PyProject.toml")));
    }
}
