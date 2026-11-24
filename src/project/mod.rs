// SPDX-FileCopyrightText: 2026 Mike Coats <i.am@mikecoats.com>
// SPDX-License-Identifier: GPL-3.0-or-later

mod javascript;
mod python;
mod rust;

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use strum::{Display, EnumIter};

use crate::dependency::{Dependency, ParseError};
use crate::registry::Release;
use crate::update::{CompareError, Outdated, Update};

/// A language ecosystem whose projects Wullie can find.
///
/// Each ecosystem's rules live in its own module; the methods here dispatch to
/// them, so adding a variant fails to compile until every stage handles it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Display, EnumIter)]
#[strum(serialize_all = "lowercase")]
pub enum Ecosystem {
    Rust,
    Python,
    JavaScript,
}

impl Ecosystem {
    /// Normalizes a package name according to this ecosystem's rules.
    pub fn normalize_name(self, name: &str) -> String {
        match self {
            Self::Rust => rust::normalize_name(name),
            Self::Python => python::normalize_name(name),
            Self::JavaScript => javascript::normalize_name(name),
        }
    }

    /// Returns whether a file with this name marks a project of this ecosystem.
    pub fn is_manifest(self, file_name: &OsStr) -> bool {
        match self {
            Self::Rust => rust::is_manifest(file_name),
            Self::Python => python::is_manifest(file_name),
            Self::JavaScript => javascript::is_manifest(file_name),
        }
    }

    /// Directory names that are never searched for projects of any ecosystem.
    pub fn skip_dirs(self) -> &'static [&'static str] {
        match self {
            Self::Rust => rust::SKIP_DIRS,
            Self::Python => python::SKIP_DIRS,
            Self::JavaScript => javascript::SKIP_DIRS,
        }
    }

    /// Parses dependencies from a manifest's contents.
    pub fn dependencies(self, contents: &str) -> Result<Vec<Dependency>, ParseError> {
        match self {
            Self::Rust => rust::dependencies(contents),
            Self::Python => python::dependencies(contents),
            Self::JavaScript => javascript::dependencies(contents),
        }
    }

    /// Returns outdated-version details for a requirement and its releases.
    pub fn outdated(
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
    pub fn pin(self, contents: &str, updates: &[Update]) -> Result<String, ParseError> {
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
pub struct Project {
    manifest: PathBuf,
    ecosystem: Ecosystem,
}

impl Project {
    /// Creates a project from its manifest path, or `None` if the path has no parent directory.
    pub(crate) fn new(ecosystem: Ecosystem, manifest: PathBuf) -> Option<Self> {
        manifest.parent()?;
        Some(Self {
            ecosystem,
            manifest,
        })
    }

    /// The ecosystem this project belongs to.
    pub fn ecosystem(&self) -> Ecosystem {
        self.ecosystem
    }

    /// The manifest file that identified this project.
    pub fn manifest(&self) -> &Path {
        &self.manifest
    }

    /// The project directory, which is the directory containing the manifest.
    pub fn dir(&self) -> &Path {
        self.manifest
            .parent()
            .expect("Project::new guarantees the manifest has a parent")
    }
}

/// Keeps line endings consistent between `edited` and `original`.
///
/// `toml_edit` uses `\n`, regardless of the source strings. Without this step,
/// a CRLF-rich manifest would report diffs on every line due to these line
/// ending changes. This ensures the diff is update focused and noise-free.
pub(crate) fn restore_line_endings(original: &str, edited: &str) -> String {
    let original_lines = original.split_inclusive('\n');
    let edited_lines = edited.split_inclusive('\n');
    if original_lines.clone().count() != edited_lines.clone().count() {
        return edited.to_owned();
    }

    let mut restored = String::with_capacity(edited.len());
    for (original_line, edited_line) in original_lines.zip(edited_lines) {
        restored.push_str(split_line_ending(edited_line).0);
        restored.push_str(split_line_ending(original_line).1);
    }
    restored
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
    use std::path::{Path, PathBuf};

    use strum::IntoEnumIterator;

    use crate::update::Outdated;

    use super::{Ecosystem, Project, restore_line_endings};

    #[test]
    fn restores_crlf_endings() {
        assert_eq!(
            restore_line_endings("a = 1\r\nb = 2\r\n", "a = 1\nb = 3\n"),
            "a = 1\r\nb = 3\r\n"
        );
    }

    #[test]
    fn restores_mixed_endings_line_by_line() {
        assert_eq!(
            restore_line_endings("a\r\nb\nc\r\n", "a\nB\nc\n"),
            "a\r\nB\nc\r\n"
        );
    }

    #[test]
    fn restores_a_missing_final_newline() {
        assert_eq!(restore_line_endings("a\nb", "a\nB\n"), "a\nB");
        assert_eq!(restore_line_endings("a\r\nb", "a\nB\n"), "a\r\nB");
    }

    #[test]
    fn keeps_crlf_already_present_in_the_edited_text() {
        assert_eq!(
            restore_line_endings("s = \"\"\"\r\nx\"\"\"\r\n", "s = \"\"\"\r\nx\"\"\"\n"),
            "s = \"\"\"\r\nx\"\"\"\r\n"
        );
    }

    #[test]
    fn leaves_lf_text_unchanged() {
        assert_eq!(restore_line_endings("a\nb\n", "a\nB\n"), "a\nB\n");
        assert_eq!(restore_line_endings("", ""), "");
    }

    #[test]
    fn returns_edited_text_when_line_counts_differ() {
        assert_eq!(restore_line_endings("a\r\nb\r\n", "a\n"), "a\n");
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

    #[test]
    fn project_dir_is_manifest_parent() {
        let project = Project::new(
            Ecosystem::Rust,
            PathBuf::from("/projects/example/Cargo.toml"),
        )
        .unwrap();

        assert_eq!(project.dir(), Path::new("/projects/example"));
        assert_eq!(
            project.manifest(),
            Path::new("/projects/example/Cargo.toml")
        );
        assert_eq!(project.ecosystem(), Ecosystem::Rust);
    }

    #[test]
    fn projects_sort_by_manifest_path() {
        let root_js =
            Project::new(Ecosystem::JavaScript, PathBuf::from("/a/package.json")).unwrap();
        let root_rust = Project::new(Ecosystem::Rust, PathBuf::from("/a/Cargo.toml")).unwrap();
        let member = Project::new(Ecosystem::Rust, PathBuf::from("/a/b/Cargo.toml")).unwrap();
        let mut projects = vec![member.clone(), root_js.clone(), root_rust.clone()];

        projects.sort();

        assert_eq!(projects, vec![root_rust, member, root_js]);
    }

    #[test]
    fn project_requires_a_parent_directory() {
        assert_eq!(Project::new(Ecosystem::Rust, PathBuf::from("/")), None);
    }
}
