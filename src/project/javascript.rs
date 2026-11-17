// SPDX-FileCopyrightText: 2026 Mike Coats <i.am@mikecoats.com>
// SPDX-License-Identifier: GPL-3.0-or-later

use std::ffi::OsStr;

/// Directories holding installed dependencies rather than projects.
/// Every directory with one of these names is pruned, including ones in
/// non-JavaScript trees.
pub const SKIP_DIRS: &[&str] = &["node_modules", "bower_components"];

/// Returns whether `file_name` is a `package.json` manifest.
pub fn is_manifest(file_name: &OsStr) -> bool {
    file_name == "package.json"
}

/// Leaves JavaScript package names unchanged.
pub fn normalize_name(name: &str) -> String {
    name.to_owned()
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;

    use super::is_manifest;

    #[test]
    fn detects_package_json_manifest() {
        assert!(is_manifest(OsStr::new("package.json")));
    }

    #[test]
    fn rejects_other_manifest_names() {
        assert!(!is_manifest(OsStr::new("package-lock.json")));
        assert!(!is_manifest(OsStr::new("npm-shrinkwrap.json")));
        assert!(!is_manifest(OsStr::new("Package.json")));
    }
}
