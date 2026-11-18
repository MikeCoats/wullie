// SPDX-FileCopyrightText: 2026 Mike Coats <i.am@mikecoats.com>
// SPDX-License-Identifier: GPL-3.0-or-later

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use similar::TextDiff;

use crate::dependency::ParseError;
use crate::update::ProjectUpdates;

/// Errors while reading or parsing a manifest for diff generation.
#[derive(Debug, thiserror::Error)]
pub enum DiffError {
    /// A manifest could not be read.
    #[error("cannot read manifest {path}")]
    Read {
        /// The manifest path.
        path: PathBuf,
        /// The underlying I/O error.
        #[source]
        source: io::Error,
    },
    /// A manifest could not be parsed for rewriting.
    #[error("cannot parse manifest {path}")]
    Parse {
        /// The manifest path.
        path: PathBuf,
        /// The parsing error.
        #[source]
        source: ParseError,
    },
}

/// The combined unified diff and errors encountered while producing it.
#[derive(Debug, Default)]
pub struct DiffFindings {
    /// Unified diff text for every changed manifest.
    pub diff: String,
    /// Per-project failures; other diffs are still generated.
    pub errors: Vec<DiffError>,
}

/// Generates unified diffs for changed manifests, using paths relative to `root`.
pub fn manifest_diffs(root: &Path, projects: &[ProjectUpdates]) -> DiffFindings {
    let mut findings = DiffFindings::default();
    let mut projects: Vec<_> = projects.iter().collect();
    projects.sort_by(|left, right| left.project.manifest().cmp(right.project.manifest()));

    for project_updates in projects {
        let manifest = project_updates.project.manifest();
        let contents = match fs::read_to_string(manifest) {
            Ok(contents) => contents,
            Err(source) => {
                findings.errors.push(DiffError::Read {
                    path: manifest.to_path_buf(),
                    source,
                });
                continue;
            }
        };
        let pinned = match project_updates
            .project
            .ecosystem()
            .pin(&contents, &project_updates.updates)
        {
            Ok(pinned) => pinned,
            Err(source) => {
                findings.errors.push(DiffError::Parse {
                    path: manifest.to_path_buf(),
                    source,
                });
                continue;
            }
        };
        if contents == pinned {
            continue;
        }

        let name = relative(root, manifest).to_string_lossy();
        let diff = unified_diff(&name, &contents, &pinned);
        findings.diff.push_str(&diff);
    }

    findings
}

fn relative<'a>(root: &Path, path: &'a Path) -> &'a Path {
    path.strip_prefix(root).unwrap_or(path)
}

fn unified_diff(name: &str, old: &str, new: &str) -> String {
    let old_header = format!("a/{name}");
    let new_header = format!("b/{name}");
    TextDiff::from_lines(old, new)
        .unified_diff()
        .header(&old_header, &new_header)
        .to_string()
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use tempfile::TempDir;

    use crate::dependency::Dependency;
    use crate::project::{Ecosystem, Project};
    use crate::update::{ProjectUpdates, Update};

    use super::{DiffError, manifest_diffs, unified_diff};

    fn project(manifest: &Path) -> Project {
        Project::new(Ecosystem::Rust, manifest.to_path_buf()).unwrap()
    }

    fn updates(manifest: &Path, name: &str, requirement: &str, latest: &str) -> ProjectUpdates {
        ProjectUpdates {
            project: project(manifest),
            updates: vec![Update {
                dependency: Dependency::new(name, requirement),
                lowest: String::new(),
                latest: latest.to_owned(),
            }],
        }
    }

    #[test]
    fn unified_diff_uses_manifest_relative_headers() {
        let diff = unified_diff(
            "nested/Cargo.toml",
            "[dependencies]\nserde = \"1\"\n",
            "[dependencies]\nserde = \"=1.2.0\"\n",
        );

        assert!(diff.contains("--- a/nested/Cargo.toml\n"));
        assert!(diff.contains("+++ b/nested/Cargo.toml\n"));
        assert!(diff.contains("-serde = \"1\"\n"));
        assert!(diff.contains("+serde = \"=1.2.0\"\n"));
    }

    #[test]
    fn combines_changed_manifests_in_sorted_order_with_root_relative_paths() {
        let temp = TempDir::new().unwrap();
        let root_manifest = temp.path().join("Cargo.toml");
        let nested_manifest = temp.path().join("nested/Cargo.toml");
        fs::create_dir_all(nested_manifest.parent().unwrap()).unwrap();
        fs::write(&root_manifest, "[dependencies]\nroot_crate = \"1\"\n").unwrap();
        fs::write(&nested_manifest, "[dependencies]\nnested_crate = \"1\"\n").unwrap();
        let nested = updates(&nested_manifest, "nested_crate", "1", "1.2.0");
        let root = updates(&root_manifest, "root_crate", "1", "1.3.0");

        let findings = manifest_diffs(temp.path(), &[nested, root]);

        assert!(findings.errors.is_empty());
        let root_header = findings.diff.find("--- a/Cargo.toml\n").unwrap();
        let nested_header = findings.diff.find("--- a/nested/Cargo.toml\n").unwrap();
        assert!(root_header < nested_header);
        assert!(findings.diff.contains("+root_crate = \"=1.3.0\""));
        assert!(findings.diff.contains("+nested_crate = \"=1.2.0\""));
    }

    #[test]
    fn skips_manifests_unchanged_by_pinning() {
        let temp = TempDir::new().unwrap();
        let manifest = temp.path().join("Cargo.toml");
        fs::write(&manifest, "[dependencies]\nserde = \"=1.2.0\"\n").unwrap();
        let update = updates(&manifest, "serde", "=1.2.0", "1.2.0");

        let findings = manifest_diffs(temp.path(), &[update]);

        assert!(findings.diff.is_empty());
        assert!(findings.errors.is_empty());
    }

    #[test]
    fn reports_manifest_read_errors() {
        let temp = TempDir::new().unwrap();
        let manifest = temp.path().join("missing/Cargo.toml");
        let findings = manifest_diffs(temp.path(), &[updates(&manifest, "serde", "1", "1.2.0")]);

        assert!(matches!(
            findings.errors.as_slice(),
            [DiffError::Read { path, .. }] if path == &manifest
        ));
    }

    #[test]
    fn reports_manifest_parse_errors() {
        let temp = TempDir::new().unwrap();
        let manifest = temp.path().join("Cargo.toml");
        fs::write(&manifest, "[dependencies\nserde = \"1\"\n").unwrap();

        let findings = manifest_diffs(temp.path(), &[updates(&manifest, "serde", "1", "1.2.0")]);

        assert!(matches!(
            findings.errors.as_slice(),
            [DiffError::Parse { path, .. }] if path == &manifest
        ));
    }
}
