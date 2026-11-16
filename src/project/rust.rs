// SPDX-FileCopyrightText: 2026 Mike Coats <i.am@mikecoats.com>
// SPDX-License-Identifier: GPL-3.0-or-later

use std::ffi::OsStr;

/// Cargo's build output directory. Every directory with this name is pruned,
/// including ones in non-Rust trees.
pub const SKIP_DIRS: &[&str] = &["target"];

/// Returns whether `file_name` is a Cargo manifest.
pub fn is_manifest(file_name: &OsStr) -> bool {
    file_name == "Cargo.toml"
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;

    use super::is_manifest;

    #[test]
    fn detects_cargo_manifest() {
        assert!(is_manifest(OsStr::new("Cargo.toml")));
    }

    #[test]
    fn rejects_other_manifest_names() {
        assert!(!is_manifest(OsStr::new("Cargo.lock")));
        assert!(!is_manifest(OsStr::new("cargo.toml")));
    }
}
