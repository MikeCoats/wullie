// SPDX-FileCopyrightText: 2026 Mike Coats <i.am@mikecoats.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The Diff Maker: pins outdated dependencies and builds the changes as one
//! unified diff.

use std::fmt::Write as _;
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

        let diff = unified_diff(relative(root, manifest), &contents, &pinned);
        findings.diff.push_str(&diff);
    }

    findings
}

fn relative<'a>(root: &Path, path: &'a Path) -> &'a Path {
    path.strip_prefix(root).unwrap_or(path)
}

fn unified_diff(path: &Path, old: &str, new: &str) -> String {
    TextDiff::from_lines(old, new)
        .unified_diff()
        .header(&header_name("a/", path), &header_name("b/", path))
        .to_string()
}

/// Formats a `---`/`+++` file name the way `git diff` does, so `git apply`
/// and `patch` read back exactly the path that was diffed.
///
/// Paths come from the tree being searched, which may be untrusted, and a
/// directory name can hold any byte but `/` and NUL. Left raw, a newline would
/// end the header early and let the rest of the name pass as extra diff lines,
/// such as a hunk creating some other file. So, like git, any name holding a
/// control character, `"`, `\` or a non-ASCII byte is written as a quoted
/// C-style string, with those bytes escaped.
///
/// Also like git, an unquoted name holding a space is followed by a tab, which
/// tells `patch` where the name ends.
fn header_name(prefix: &str, path: &Path) -> String {
    let mut bytes = prefix.as_bytes().to_vec();
    bytes.extend_from_slice(path.as_os_str().as_encoded_bytes());

    if !bytes.iter().copied().any(needs_quoting) {
        // Only ASCII bytes are left, so the conversion can't fail.
        let name = String::from_utf8_lossy(&bytes).into_owned();
        return if name.contains(' ') {
            name + "\t"
        } else {
            name
        };
    }

    let mut quoted = String::with_capacity(bytes.len() + 2);
    quoted.push('"');
    for byte in bytes {
        // These are the escapes git uses. Except for three-digit octal, `\"`,
        // `\\` and `\a \b \f \n \r \t \v` are the only escapes git lists (see
        // `unquote_c_style` in git's `quote.c`). Naming the escapes git names,
        // rather than writing every byte in octal, keeps the output identical
        // to `git diff`'s.
        match byte {
            b'"' => quoted.push_str("\\\""),
            b'\\' => quoted.push_str("\\\\"),
            0x07 => quoted.push_str("\\a"),
            0x08 => quoted.push_str("\\b"),
            0x0c => quoted.push_str("\\f"),
            b'\n' => quoted.push_str("\\n"),
            b'\r' => quoted.push_str("\\r"),
            b'\t' => quoted.push_str("\\t"),
            0x0b => quoted.push_str("\\v"),
            byte if needs_quoting(byte) => {
                let _ = write!(quoted, "\\{byte:03o}");
            }
            byte => quoted.push(char::from(byte)),
        }
    }
    quoted.push('"');
    quoted
}

/// Whether git would quote a name holding `byte`.
fn needs_quoting(byte: u8) -> bool {
    !(0x20..0x7f).contains(&byte) || byte == b'"' || byte == b'\\'
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use tempfile::TempDir;

    use crate::dependency::Dependency;
    use crate::project::{Ecosystem, Project};
    use crate::update::{Outdated, ProjectUpdates, Update};

    use super::{DiffError, header_name, manifest_diffs, unified_diff};

    fn project(ecosystem: Ecosystem, manifest: &Path) -> Project {
        Project::new(ecosystem, manifest.to_path_buf()).unwrap()
    }

    fn updates(manifest: &Path, name: &str, requirement: &str, latest: &str) -> ProjectUpdates {
        ProjectUpdates {
            project: project(Ecosystem::Rust, manifest),
            updates: vec![Update {
                dependency: Dependency::new(name, requirement),
                versions: Outdated {
                    lowest: String::new(),
                    latest: latest.to_owned(),
                },
            }],
        }
    }

    #[test]
    fn unified_diff_uses_manifest_relative_headers() {
        let diff = unified_diff(
            Path::new("nested/Cargo.toml"),
            "[dependencies]\nserde = \"1\"\n",
            "[dependencies]\nserde = \"=1.2.0\"\n",
        );

        assert!(diff.contains("--- a/nested/Cargo.toml\n"));
        assert!(diff.contains("+++ b/nested/Cargo.toml\n"));
        assert!(diff.contains("-serde = \"1\"\n"));
        assert!(diff.contains("+serde = \"=1.2.0\"\n"));
    }

    #[test]
    fn header_name_leaves_plain_paths_unquoted() {
        assert_eq!(
            header_name("a/", Path::new("crates/my-crate_1.0/Cargo.toml")),
            "a/crates/my-crate_1.0/Cargo.toml"
        );
    }

    #[test]
    fn header_name_ends_names_with_spaces_with_a_tab() {
        assert_eq!(
            header_name("b/", Path::new("my app/Cargo.toml")),
            "b/my app/Cargo.toml\t"
        );
    }

    #[test]
    fn header_name_quotes_and_escapes_like_git() {
        assert_eq!(
            header_name("a/", Path::new("x\ny\tz\r\u{7}\u{8}\u{b}\u{c}/Cargo.toml")),
            r#""a/x\ny\tz\r\a\b\v\f/Cargo.toml""#
        );
        assert_eq!(
            header_name("a/", Path::new("say \"hi\"\\/Cargo.toml")),
            r#""a/say \"hi\"\\/Cargo.toml""#
        );
        assert_eq!(
            header_name("a/", Path::new("del\u{7f}\u{1b}/Cargo.toml")),
            r#""a/del\177\033/Cargo.toml""#
        );
    }

    #[test]
    fn header_name_escapes_non_ascii_bytes_in_octal() {
        assert_eq!(
            header_name("a/", Path::new("café/Cargo.toml")),
            r#""a/caf\303\251/Cargo.toml""#
        );
    }

    #[cfg(unix)]
    #[test]
    fn header_name_escapes_non_utf8_bytes() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;

        let path = Path::new(OsStr::from_bytes(b"bad\xff/Cargo.toml"));
        assert_eq!(header_name("a/", path), r#""a/bad\377/Cargo.toml""#);
    }

    /// A directory name holding newlines must not be able to add lines, such
    /// as a hunk for another file, to the diff.
    #[cfg(unix)]
    #[test]
    fn directory_names_cannot_inject_diff_lines() {
        let temp = TempDir::new().unwrap();
        let dir = "p\n+++ b/evil.sh\n@@ -0,0 +1 @@\n+echo pwned\n--- a/p";
        let manifest = temp.path().join(dir).join("Cargo.toml");
        fs::create_dir_all(manifest.parent().unwrap()).unwrap();
        fs::write(&manifest, "[dependencies]\nserde = \"1\"\n").unwrap();

        let findings = manifest_diffs(temp.path(), &[updates(&manifest, "serde", "1", "1.2.0")]);

        assert!(findings.errors.is_empty());
        assert_eq!(
            findings.diff,
            "--- \"a/p\\n+++ b/evil.sh\\n@@ -0,0 +1 @@\\n+echo pwned\\n--- a/p/Cargo.toml\"\n\
             +++ \"b/p\\n+++ b/evil.sh\\n@@ -0,0 +1 @@\\n+echo pwned\\n--- a/p/Cargo.toml\"\n\
             @@ -1,2 +1,2 @@\n \
             [dependencies]\n\
             -serde = \"1\"\n\
             +serde = \"=1.2.0\"\n"
        );
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
    fn includes_python_pins_in_the_combined_diff() {
        let temp = TempDir::new().unwrap();
        let manifest = temp.path().join("app/pyproject.toml");
        fs::create_dir_all(manifest.parent().unwrap()).unwrap();
        fs::write(
            &manifest,
            "[project]\ndependencies = [\"pytest[asyncio]>=7; python_version >= '3.9'\"]\n",
        )
        .unwrap();
        let update = ProjectUpdates {
            project: project(Ecosystem::Python, &manifest),
            updates: vec![Update {
                dependency: Dependency::new("pytest", ">=7"),
                versions: Outdated {
                    lowest: "7.0".to_owned(),
                    latest: "8.0".to_owned(),
                },
            }],
        };

        let findings = manifest_diffs(temp.path(), &[update]);

        assert!(findings.errors.is_empty());
        assert!(findings.diff.contains("--- a/app/pyproject.toml\n"));
        assert!(
            findings
                .diff
                .contains("-dependencies = [\"pytest[asyncio]>=7; python_version >= '3.9'\"]")
        );
        assert!(
            findings
                .diff
                .contains("+dependencies = [\"pytest[asyncio]==8.0; python_version >= '3.9'\"]")
        );
    }

    #[test]
    fn includes_javascript_pins_in_the_combined_diff() {
        let temp = TempDir::new().unwrap();
        let manifest = temp.path().join("web/package.json");
        fs::create_dir_all(manifest.parent().unwrap()).unwrap();
        fs::write(
            &manifest,
            "{\n  \"dependencies\": {\n    \"left-pad\": \"^1.0.0\"\n  }\n}\n",
        )
        .unwrap();
        let update = ProjectUpdates {
            project: project(Ecosystem::JavaScript, &manifest),
            updates: vec![Update {
                dependency: Dependency::new("left-pad", "^1.0.0"),
                versions: Outdated {
                    lowest: "1.0.0".to_owned(),
                    latest: "1.3.0".to_owned(),
                },
            }],
        };

        let findings = manifest_diffs(temp.path(), &[update]);

        assert!(findings.errors.is_empty());
        assert!(findings.diff.contains("--- a/web/package.json\n"));
        assert!(findings.diff.contains("-    \"left-pad\": \"^1.0.0\"\n"));
        assert!(findings.diff.contains("+    \"left-pad\": \"1.3.0\"\n"));
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
