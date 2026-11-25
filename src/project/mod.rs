// SPDX-FileCopyrightText: 2026 Mike Coats <i.am@mikecoats.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The Project Finder: finds projects on disk by their manifests, and holds
//! the per-ecosystem rules for those manifests.

mod javascript;
mod python;
mod rust;

use std::ffi::OsStr;
use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use strum::{Display, EnumIter, IntoEnumIterator};

use crate::dependency::{Dependency, ParseError};
use crate::registry::Release;
use crate::update::{CompareError, Outdated, Update};
use crate::walk::{self, WalkError};

/// The result of searching a directory tree for projects.
#[derive(Debug, Default)]
pub(crate) struct ProjectFindings {
    /// Projects found, sorted by manifest path then ecosystem.
    pub projects: Vec<Project>,
    /// Errors met during the walk; the search carries on past them.
    pub errors: Vec<WalkError>,
}

/// Searches `root` recursively for projects of every supported ecosystem.
pub(crate) fn find_projects(root: &Path) -> ProjectFindings {
    let skip_dirs: Vec<&'static str> = Ecosystem::iter()
        .flat_map(|ecosystem| ecosystem.skip_dirs().iter().copied())
        .collect();
    let mut projects = Vec::new();
    let mut errors = Vec::new();

    for entry in walk::files(root, &skip_dirs) {
        let file = match entry {
            Ok(file) => file,
            Err(error) => {
                errors.push(error);
                continue;
            }
        };
        let Some(file_name) = file.file_name() else {
            continue;
        };

        projects.extend(
            Ecosystem::iter()
                .filter(|ecosystem| ecosystem.is_manifest(file_name))
                .map(|ecosystem| Project::new(ecosystem, file.clone())),
        );
    }

    projects.sort();
    ProjectFindings { projects, errors }
}

/// A language ecosystem whose projects Wullie can find.
///
/// Each ecosystem's rules live in its own module; the methods here dispatch to
/// them, so adding a variant fails to compile until every stage handles it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Display, EnumIter)]
#[strum(serialize_all = "lowercase")]
pub(crate) enum Ecosystem {
    /// Cargo projects, with releases from crates.io.
    Rust,
    /// `pyproject.toml` projects, with releases from PyPI.
    Python,
    /// `package.json` projects, with releases from npm.
    JavaScript,
}

impl Ecosystem {
    /// Normalizes a package name according to this ecosystem's rules.
    pub(crate) fn normalize_name(self, name: &str) -> String {
        match self {
            Self::Rust => rust::normalize_name(name),
            Self::Python => python::normalize_name(name),
            Self::JavaScript => javascript::normalize_name(name),
        }
    }

    /// Returns whether a file with this name marks a project of this ecosystem.
    pub(crate) fn is_manifest(self, file_name: &OsStr) -> bool {
        match self {
            Self::Rust => rust::is_manifest(file_name),
            Self::Python => python::is_manifest(file_name),
            Self::JavaScript => javascript::is_manifest(file_name),
        }
    }

    /// Directory names that are never searched for projects of any ecosystem.
    pub(crate) fn skip_dirs(self) -> &'static [&'static str] {
        match self {
            Self::Rust => rust::SKIP_DIRS,
            Self::Python => python::SKIP_DIRS,
            Self::JavaScript => javascript::SKIP_DIRS,
        }
    }

    /// Parses dependencies from a manifest's contents.
    ///
    /// # Errors
    ///
    /// Fails if the manifest can't be parsed, or a dependency section or
    /// entry is malformed.
    pub(crate) fn dependencies(self, contents: &str) -> Result<Vec<Dependency>, ParseError> {
        match self {
            Self::Rust => rust::dependencies(contents),
            Self::Python => python::dependencies(contents),
            Self::JavaScript => javascript::dependencies(contents),
        }
    }

    /// Returns outdated-version details for a requirement and its releases.
    ///
    /// # Errors
    ///
    /// Fails if the requirement is invalid, or no non-yanked release matches it.
    pub(crate) fn outdated(
        self,
        requirement: &str,
        releases: &[Release],
    ) -> Result<Option<Outdated>, CompareError> {
        match self {
            Self::Rust => rust::outdated(requirement, releases),
            Self::Python => python::outdated(requirement, releases),
            Self::JavaScript => javascript::outdated(requirement, releases),
        }
    }

    /// Pins dependencies in a manifest to the selected release versions.
    ///
    /// # Errors
    ///
    /// Fails if the manifest can't be parsed, or a dependency section or
    /// entry is malformed.
    pub(crate) fn pin(self, contents: &str, updates: &[Update]) -> Result<String, ParseError> {
        match self {
            Self::Rust => rust::pin(contents, updates),
            Self::Python => python::pin(contents, updates),
            Self::JavaScript => javascript::pin(contents, updates),
        }
    }
}

/// A project found on disk, identified by its manifest file.
///
/// Projects sort by manifest path, then ecosystem, which is the order Wullie
/// prints them in.
/// The field order matters: the derived `Ord` compares fields top to bottom.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Project {
    manifest: PathBuf,
    ecosystem: Ecosystem,
}

impl Project {
    /// Creates a project from its manifest path.
    pub(crate) fn new(ecosystem: Ecosystem, manifest: PathBuf) -> Self {
        Self {
            manifest,
            ecosystem,
        }
    }

    /// The ecosystem this project belongs to.
    pub(crate) fn ecosystem(&self) -> Ecosystem {
        self.ecosystem
    }

    /// The manifest file that identified this project.
    pub(crate) fn manifest(&self) -> &Path {
        &self.manifest
    }

    /// Reads the manifest, refusing one larger than [`MAX_MANIFEST_SIZE`].
    pub(crate) fn read_manifest(&self) -> io::Result<String> {
        read_limited(&self.manifest, MAX_MANIFEST_SIZE)
    }
}

/// The largest manifest Wullie reads, 16 MiB.
///
/// Real manifests are a few kilobytes, but the tree being searched may be
/// untrusted, and a huge file with a manifest's name would otherwise be read
/// whole into memory.
const MAX_MANIFEST_SIZE: u64 = 16 * 1024 * 1024;

/// Reads `path` as UTF-8, failing if it holds more than `limit` bytes.
///
/// Only `limit + 1` bytes are ever read, so the check holds even for a file
/// that grows while it is read.
fn read_limited(path: &Path, limit: u64) -> io::Result<String> {
    let mut bytes = Vec::new();
    File::open(path)?
        .take(limit.saturating_add(1))
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(io::Error::new(
            io::ErrorKind::FileTooLarge,
            format!("manifest is larger than {} MiB", limit / (1024 * 1024)),
        ));
    }
    String::from_utf8(bytes).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

/// Keeps the line endings and any byte-order mark of `original` in `edited`.
///
/// `toml_edit` uses `\n`, regardless of the source strings. Without this step,
/// a CRLF-rich manifest would report diffs on every line due to these line
/// ending changes. This ensures the diff is update focused and noise-free.
///
/// `toml_edit` also drops a leading byte-order mark, which would otherwise
/// show up as an unrelated change to the first line.
pub(crate) fn restore_layout(original: &str, edited: &str) -> String {
    let (bom, original) = split_bom(original);
    let (_, edited) = split_bom(edited);
    let mut restored = String::with_capacity(bom.len() + edited.len());
    restored.push_str(bom);

    let original_lines = original.split_inclusive('\n');
    let edited_lines = edited.split_inclusive('\n');
    if original_lines.clone().count() != edited_lines.clone().count() {
        restored.push_str(edited);
        return restored;
    }

    for (original_line, edited_line) in original_lines.zip(edited_lines) {
        restored.push_str(split_line_ending(edited_line).0);
        restored.push_str(split_line_ending(original_line).1);
    }
    restored
}

/// Splits off a leading UTF-8 byte-order mark, returning it (or `""`) and the
/// rest of `contents`.
///
/// Editors on Windows often add one. TOML allows it, and npm skips it in
/// `package.json`, so Wullie accepts and keeps it too.
pub(crate) fn split_bom(contents: &str) -> (&str, &str) {
    let rest = contents.strip_prefix('\u{feff}').unwrap_or(contents);
    contents.split_at(contents.len() - rest.len())
}

/// Splits a line into its content and its ending: `\r\n`, `\n`, or nothing.
fn split_line_ending(line: &str) -> (&str, &str) {
    let content = line
        .strip_suffix('\n')
        .map_or(line, |line| line.strip_suffix('\r').unwrap_or(line));
    line.split_at(content.len())
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;
    use std::fs;
    use std::path::{Path, PathBuf};

    use strum::IntoEnumIterator;
    use tempfile::TempDir;

    use crate::update::Outdated;

    use super::{Ecosystem, Project, find_projects, read_limited, restore_layout, split_bom};

    fn create_manifest(path: &Path) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, "").unwrap();
    }

    fn project(ecosystem: Ecosystem, manifest: &Path) -> Project {
        Project::new(ecosystem, manifest.to_path_buf())
    }

    #[test]
    fn restores_crlf_endings() {
        assert_eq!(
            restore_layout("a = 1\r\nb = 2\r\n", "a = 1\nb = 3\n"),
            "a = 1\r\nb = 3\r\n"
        );
    }

    #[test]
    fn restores_mixed_endings_line_by_line() {
        assert_eq!(
            restore_layout("a\r\nb\nc\r\n", "a\nB\nc\n"),
            "a\r\nB\nc\r\n"
        );
    }

    #[test]
    fn restores_a_missing_final_newline() {
        assert_eq!(restore_layout("a\nb", "a\nB\n"), "a\nB");
        assert_eq!(restore_layout("a\r\nb", "a\nB\n"), "a\r\nB");
    }

    #[test]
    fn keeps_crlf_already_present_in_the_edited_text() {
        assert_eq!(
            restore_layout("s = \"\"\"\r\nx\"\"\"\r\n", "s = \"\"\"\r\nx\"\"\"\n"),
            "s = \"\"\"\r\nx\"\"\"\r\n"
        );
    }

    #[test]
    fn leaves_lf_text_unchanged() {
        assert_eq!(restore_layout("a\nb\n", "a\nB\n"), "a\nB\n");
        assert_eq!(restore_layout("", ""), "");
    }

    #[test]
    fn returns_edited_text_when_line_counts_differ() {
        assert_eq!(restore_layout("a\r\nb\r\n", "a\n"), "a\n");
    }

    #[test]
    fn restores_a_byte_order_mark() {
        assert_eq!(
            restore_layout("\u{feff}a\r\nb\r\n", "a\nB\n"),
            "\u{feff}a\r\nB\r\n"
        );
        assert_eq!(restore_layout("\u{feff}a\n", "\u{feff}A\n"), "\u{feff}A\n");
        assert_eq!(restore_layout("a\n", "A\n"), "A\n");
    }

    #[test]
    fn reads_a_manifest_up_to_the_limit() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("Cargo.toml");
        fs::write(&path, "12345678").unwrap();

        assert_eq!(read_limited(&path, 8).unwrap(), "12345678");
    }

    #[test]
    fn refuses_a_manifest_over_the_limit() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("Cargo.toml");
        fs::write(&path, vec![b'a'; 3 * 1024 * 1024 + 1]).unwrap();

        let error = read_limited(&path, 3 * 1024 * 1024).unwrap_err();

        assert_eq!(error.kind(), std::io::ErrorKind::FileTooLarge);
        assert_eq!(error.to_string(), "manifest is larger than 3 MiB");
    }

    #[test]
    fn refuses_a_manifest_that_is_not_utf8() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("Cargo.toml");
        fs::write(&path, b"\xff\xfe").unwrap();

        let error = read_limited(&path, 8).unwrap_err();

        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    }

    #[test]
    fn splits_off_only_a_leading_byte_order_mark() {
        assert_eq!(split_bom("\u{feff}{}"), ("\u{feff}", "{}"));
        assert_eq!(split_bom("{}\u{feff}"), ("", "{}\u{feff}"));
        assert_eq!(split_bom(""), ("", ""));
    }

    #[test]
    fn ecosystem_displays_as_output_label() {
        assert_eq!(Ecosystem::Rust.to_string(), "rust");
        assert_eq!(Ecosystem::Python.to_string(), "python");
        assert_eq!(Ecosystem::JavaScript.to_string(), "javascript");
    }

    #[test]
    fn iterates_every_ecosystem() {
        assert_eq!(
            Ecosystem::iter().collect::<Vec<_>>(),
            vec![Ecosystem::Rust, Ecosystem::Python, Ecosystem::JavaScript]
        );
    }

    #[test]
    fn normalizes_names_per_ecosystem() {
        assert_eq!(Ecosystem::Rust.normalize_name("Foo_Bar"), "foo_bar");
        assert_eq!(Ecosystem::Python.normalize_name("Foo_Bar"), "foo-bar");
        assert_eq!(Ecosystem::Python.normalize_name("FOO--bar"), "foo-bar");
        assert_eq!(Ecosystem::JavaScript.normalize_name("Foo_Bar"), "Foo_Bar");
    }

    #[test]
    fn rust_dispatches_to_its_module() {
        assert!(Ecosystem::Rust.is_manifest(OsStr::new("Cargo.toml")));
        assert_eq!(Ecosystem::Rust.skip_dirs(), &["target"]);
    }

    #[test]
    fn python_dispatches_to_its_module() {
        assert!(Ecosystem::Python.is_manifest(OsStr::new("pyproject.toml")));
        assert!(!Ecosystem::Python.is_manifest(OsStr::new("Cargo.toml")));
        assert!(Ecosystem::Python.skip_dirs().contains(&"site-packages"));
    }

    #[test]
    fn javascript_dispatches_to_its_module() {
        assert!(Ecosystem::JavaScript.is_manifest(OsStr::new("package.json")));
        assert!(!Ecosystem::JavaScript.is_manifest(OsStr::new("package-lock.json")));
        assert_eq!(
            Ecosystem::JavaScript.skip_dirs(),
            &["node_modules", "bower_components"]
        );
    }

    #[test]
    fn python_dispatches_dependency_parsing_and_toml_errors() {
        assert_eq!(
            Ecosystem::Python
                .dependencies("[project]\ndependencies = ['requests>=2']")
                .unwrap(),
            vec![crate::dependency::Dependency::new("requests", ">=2")]
        );
        assert!(Ecosystem::Python.dependencies("invalid toml").is_err());
    }

    #[test]
    fn javascript_dispatches_dependency_parsing_and_json_errors() {
        assert_eq!(
            Ecosystem::JavaScript
                .dependencies(r#"{"dependencies":{"example":"^1"}}"#)
                .unwrap(),
            vec![crate::dependency::Dependency::new("example", "^1")]
        );
        assert!(Ecosystem::JavaScript.dependencies("invalid json").is_err());
    }

    #[test]
    fn javascript_dispatches_outdated_checks_to_its_module() {
        let releases = [
            crate::registry::Release {
                version: "1.0.0".to_owned(),
                yanked: false,
            },
            crate::registry::Release {
                version: "1.2.0".to_owned(),
                yanked: false,
            },
        ];

        assert_eq!(
            Ecosystem::JavaScript.outdated("1.0.0", &releases).unwrap(),
            Some(Outdated {
                lowest: "1.0.0".to_owned(),
                latest: "1.2.0".to_owned(),
            })
        );
    }

    #[test]
    fn pins_rust_python_and_javascript_manifests() {
        let contents = "[dependencies]\nexample = \"1\"\n";
        let updates = [crate::update::Update {
            dependency: crate::dependency::Dependency::new("example", "1"),
            versions: Outdated {
                lowest: "1.0.0".to_owned(),
                latest: "1.2.0".to_owned(),
            },
        }];

        assert_eq!(
            Ecosystem::Rust.pin(contents, &updates).unwrap(),
            "[dependencies]\nexample = \"=1.2.0\"\n"
        );
        let python_contents = "[project]\ndependencies = [\"example>=1\"]\n";
        let python_updates = [crate::update::Update {
            dependency: crate::dependency::Dependency::new("example", ">=1"),
            versions: Outdated {
                lowest: "1.0".to_owned(),
                latest: "1.2.0".to_owned(),
            },
        }];
        assert_eq!(
            Ecosystem::Python
                .pin(python_contents, &python_updates)
                .unwrap(),
            "[project]\ndependencies = [\"example==1.2.0\"]\n"
        );
        let javascript_contents = "{\"dependencies\": {\"example\": \"1\"}}\n";
        assert_eq!(
            Ecosystem::JavaScript
                .pin(javascript_contents, &updates)
                .unwrap(),
            "{\"dependencies\": {\"example\": \"1.2.0\"}}\n"
        );
    }

    #[cfg(unix)]
    #[test]
    fn skips_symbolic_links_to_manifests_and_directories() {
        use std::os::unix::fs::symlink;

        let temp = TempDir::new().unwrap();
        let outside = temp.path().join("outside");
        create_manifest(&outside.join("Cargo.toml"));
        let tree = temp.path().join("tree");
        fs::create_dir(&tree).unwrap();
        symlink(outside.join("Cargo.toml"), tree.join("Cargo.toml")).unwrap();
        symlink(&outside, tree.join("linked")).unwrap();

        let findings = find_projects(&tree);

        assert!(findings.errors.is_empty(), "{:?}", findings.errors);
        assert!(findings.projects.is_empty(), "{:?}", findings.projects);
    }

    #[test]
    fn project_keeps_its_manifest_path() {
        let project = Project::new(
            Ecosystem::Rust,
            PathBuf::from("/projects/example/Cargo.toml"),
        );

        assert_eq!(
            project.manifest(),
            Path::new("/projects/example/Cargo.toml")
        );
        assert_eq!(project.ecosystem(), Ecosystem::Rust);
    }

    #[test]
    fn projects_sort_by_manifest_path() {
        let root_js = Project::new(Ecosystem::JavaScript, PathBuf::from("/a/package.json"));
        let root_rust = Project::new(Ecosystem::Rust, PathBuf::from("/a/Cargo.toml"));
        let member = Project::new(Ecosystem::Rust, PathBuf::from("/a/b/Cargo.toml"));
        let mut projects = vec![member.clone(), root_js.clone(), root_rust.clone()];

        projects.sort();

        assert_eq!(projects, vec![root_rust, member, root_js]);
    }

    #[test]
    fn finds_a_single_rust_project() {
        let temp = TempDir::new().unwrap();
        let manifest = temp.path().join("Cargo.toml");
        create_manifest(&manifest);

        let findings = find_projects(temp.path());

        assert!(findings.errors.is_empty());
        assert_eq!(findings.projects, vec![project(Ecosystem::Rust, &manifest)]);
    }

    #[test]
    fn finds_workspace_root_and_member_in_path_order() {
        let temp = TempDir::new().unwrap();
        let root_manifest = temp.path().join("Cargo.toml");
        let member_manifest = temp.path().join("crates/a/Cargo.toml");
        create_manifest(&root_manifest);
        create_manifest(&member_manifest);

        let findings = find_projects(temp.path());

        assert!(findings.errors.is_empty());
        assert_eq!(
            findings.projects,
            vec![
                project(Ecosystem::Rust, &root_manifest),
                project(Ecosystem::Rust, &member_manifest)
            ]
        );
    }

    #[test]
    fn does_not_find_manifests_under_target() {
        let temp = TempDir::new().unwrap();
        let root_manifest = temp.path().join("Cargo.toml");
        let target_manifest = temp.path().join("target/generated/Cargo.toml");
        create_manifest(&root_manifest);
        create_manifest(&target_manifest);

        let findings = find_projects(temp.path());

        assert!(findings.errors.is_empty());
        assert_eq!(
            findings.projects,
            vec![project(Ecosystem::Rust, &root_manifest)]
        );
    }

    #[test]
    fn respects_gitignore_through_project_finding() {
        let temp = TempDir::new().unwrap();
        let visible_manifest = temp.path().join("Cargo.toml");
        create_manifest(&visible_manifest);
        fs::write(temp.path().join(".gitignore"), "ignored/\n").unwrap();
        create_manifest(&temp.path().join("ignored/Cargo.toml"));

        let findings = find_projects(temp.path());

        assert!(findings.errors.is_empty());
        assert_eq!(
            findings.projects,
            vec![project(Ecosystem::Rust, &visible_manifest)]
        );
    }

    #[test]
    fn finds_a_python_project_by_pyproject() {
        let temp = TempDir::new().unwrap();
        let manifest = temp.path().join("app/pyproject.toml");
        create_manifest(&manifest);

        let findings = find_projects(temp.path());

        assert!(findings.errors.is_empty());
        assert_eq!(
            findings.projects,
            vec![project(Ecosystem::Python, &manifest)]
        );
    }

    #[test]
    fn finds_javascript_workspace_root_and_member_in_path_order() {
        let temp = TempDir::new().unwrap();
        let root_manifest = temp.path().join("package.json");
        let member_manifest = temp.path().join("packages/app/package.json");
        create_manifest(&root_manifest);
        create_manifest(&member_manifest);

        let findings = find_projects(temp.path());

        assert!(findings.errors.is_empty());
        assert_eq!(
            findings.projects,
            vec![
                project(Ecosystem::JavaScript, &root_manifest),
                project(Ecosystem::JavaScript, &member_manifest)
            ]
        );
    }

    #[test]
    fn does_not_find_manifests_inside_javascript_dependency_directories() {
        let temp = TempDir::new().unwrap();
        let root_manifest = temp.path().join("package.json");
        create_manifest(&root_manifest);
        create_manifest(&temp.path().join("node_modules/dep/package.json"));
        create_manifest(&temp.path().join("bower_components/dep/package.json"));

        let findings = find_projects(temp.path());

        assert!(findings.errors.is_empty());
        assert_eq!(
            findings.projects,
            vec![project(Ecosystem::JavaScript, &root_manifest)]
        );
    }

    #[test]
    fn prunes_directories_globally_across_ecosystems() {
        let temp = TempDir::new().unwrap();
        let rust_manifest = temp.path().join("Cargo.toml");
        let python_manifest = temp.path().join("pyproject.toml");
        let javascript_manifest = temp.path().join("package.json");
        create_manifest(&rust_manifest);
        create_manifest(&python_manifest);
        create_manifest(&javascript_manifest);
        create_manifest(&temp.path().join("node_modules/dep/Cargo.toml"));
        create_manifest(&temp.path().join("target/generated/package.json"));
        create_manifest(&temp.path().join("site-packages/dep/package.json"));
        create_manifest(&temp.path().join("bower_components/dep/pyproject.toml"));

        let findings = find_projects(temp.path());

        assert!(findings.errors.is_empty());
        assert_eq!(
            findings.projects,
            vec![
                project(Ecosystem::Rust, &rust_manifest),
                project(Ecosystem::JavaScript, &javascript_manifest),
                project(Ecosystem::Python, &python_manifest)
            ]
        );
    }

    #[test]
    fn does_not_find_legacy_python_projects() {
        let temp = TempDir::new().unwrap();
        create_manifest(&temp.path().join("setuppy/setup.py"));
        create_manifest(&temp.path().join("setupcfg/setup.cfg"));
        create_manifest(&temp.path().join("requirements/requirements.txt"));

        let findings = find_projects(temp.path());

        assert!(findings.errors.is_empty());
        assert!(findings.projects.is_empty(), "{:?}", findings.projects);
    }

    #[test]
    fn finds_both_ecosystems_in_one_directory() {
        let temp = TempDir::new().unwrap();
        let cargo = temp.path().join("Cargo.toml");
        let pyproject = temp.path().join("pyproject.toml");
        create_manifest(&cargo);
        create_manifest(&pyproject);

        let findings = find_projects(temp.path());

        assert!(findings.errors.is_empty());
        assert_eq!(
            findings.projects,
            vec![
                project(Ecosystem::Rust, &cargo),
                project(Ecosystem::Python, &pyproject)
            ]
        );
    }

    #[test]
    fn does_not_find_manifests_inside_virtualenvs() {
        let temp = TempDir::new().unwrap();
        let root_manifest = temp.path().join("pyproject.toml");
        create_manifest(&root_manifest);
        create_manifest(
            &temp
                .path()
                .join("venv/lib/python3.13/site-packages/dep/pyproject.toml"),
        );
        create_manifest(&temp.path().join(".venv/lib/dep/pyproject.toml"));

        let findings = find_projects(temp.path());

        assert!(findings.errors.is_empty());
        assert_eq!(
            findings.projects,
            vec![project(Ecosystem::Python, &root_manifest)]
        );
    }

    #[test]
    fn finds_nothing_in_an_empty_directory() {
        let temp = TempDir::new().unwrap();

        let findings = find_projects(temp.path());

        assert!(findings.errors.is_empty());
        assert!(findings.projects.is_empty(), "{:?}", findings.projects);
    }

    #[cfg(unix)]
    #[test]
    fn reports_unreadable_directories_and_continues() {
        use std::os::unix::fs::PermissionsExt;

        /// Restores directory permissions on drop, so `TempDir` can clean up even if the test panics.
        struct RestorePermissions<'a>(&'a Path);

        impl Drop for RestorePermissions<'_> {
            fn drop(&mut self) {
                let _ = fs::set_permissions(self.0, fs::Permissions::from_mode(0o700));
            }
        }

        let temp = TempDir::new().unwrap();
        let unreadable = temp.path().join("unreadable");
        fs::create_dir_all(&unreadable).unwrap();
        let visible_manifest = temp.path().join("visible/Cargo.toml");
        create_manifest(&visible_manifest);
        fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o0)).unwrap();
        let _restore = RestorePermissions(&unreadable);

        // Running as root ignores permissions, so there is nothing to test.
        if fs::read_dir(&unreadable).is_ok() {
            return;
        }

        let findings = find_projects(temp.path());

        assert!(!findings.errors.is_empty());
        assert_eq!(
            findings.projects,
            vec![project(Ecosystem::Rust, &visible_manifest)]
        );
    }
}
