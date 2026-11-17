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
use crate::update::{CompareError, Outdated};

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
            Self::Python | Self::JavaScript => Ok(Vec::new()),
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
            Self::Python | Self::JavaScript => Ok(None),
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
    pub fn new(ecosystem: Ecosystem, manifest: PathBuf) -> Option<Self> {
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

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;
    use std::path::{Path, PathBuf};

    use strum::IntoEnumIterator;

    use super::{Ecosystem, Project};

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
    fn unsupported_ecosystems_have_no_dependencies() {
        assert!(
            Ecosystem::Python
                .dependencies("invalid toml")
                .unwrap()
                .is_empty()
        );
        assert!(
            Ecosystem::JavaScript
                .dependencies("invalid toml")
                .unwrap()
                .is_empty()
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
