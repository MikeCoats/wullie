// SPDX-FileCopyrightText: 2026 Mike Coats <i.am@mikecoats.com>
// SPDX-License-Identifier: GPL-3.0-or-later

use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::PathBuf;

use crate::project::Project;

/// A dependency with a version requirement, regardless of optionality or ecosystem-specific category.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dependency {
    /// The dependency's package name, which may differ from its manifest key when renamed.
    pub name: String,
    /// The version requirement exactly as written in the manifest.
    pub requirement: String,
}

impl Dependency {
    /// Creates a dependency from its package name and manifest requirement.
    pub fn new(name: impl Into<String>, requirement: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            requirement: requirement.into(),
        }
    }
}

/// A project and the dependencies declared in its manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectDependencies {
    /// The project whose manifest declares these dependencies.
    pub project: Project,
    /// Dependencies with version requirements in the manifest.
    pub dependencies: Vec<Dependency>,
}

/// Results from parsing the manifests of discovered projects.
#[derive(Debug, Default)]
pub struct DependencyFindings {
    /// Projects with at least one dependency version requirement, in input order.
    pub projects: Vec<ProjectDependencies>,
    /// Manifest read or parse errors; other manifests are still processed.
    pub errors: Vec<ManifestError>,
}

/// An error reading or parsing a project manifest.
#[derive(Debug, thiserror::Error)]
pub enum ManifestError {
    /// A project manifest could not be read.
    #[error("cannot read {}", path.display())]
    Read {
        /// The manifest that could not be read.
        path: PathBuf,
        /// The underlying I/O error.
        #[source]
        source: io::Error,
    },
    /// A project manifest could not be parsed.
    #[error("cannot parse {}", path.display())]
    Parse {
        /// The manifest that could not be parsed.
        path: PathBuf,
        /// The underlying parse error.
        #[source]
        source: ParseError,
    },
}

/// Reads and parses each project's manifest, preserving the input order.
pub fn find_dependencies(projects: &[Project]) -> DependencyFindings {
    let mut findings = DependencyFindings::default();

    for project in projects {
        let path = project.manifest().to_path_buf();
        let contents = match fs::read_to_string(&path) {
            Ok(contents) => contents,
            Err(source) => {
                findings.errors.push(ManifestError::Read { path, source });
                continue;
            }
        };
        let dependencies = match project.ecosystem().dependencies(&contents) {
            Ok(dependencies) => dependencies,
            Err(source) => {
                findings.errors.push(ManifestError::Parse { path, source });
                continue;
            }
        };
        let dependencies = dedup(dependencies);

        if !dependencies.is_empty() {
            findings.projects.push(ProjectDependencies {
                project: project.clone(),
                dependencies,
            });
        }
    }

    findings
}

/// An error parsing a dependency manifest.
#[derive(Debug, thiserror::Error)]
#[error(transparent)]
pub struct ParseError(ParseErrorKind);

#[derive(Debug, thiserror::Error)]
enum ParseErrorKind {
    #[error(transparent)]
    Toml(toml_edit::TomlError),
    #[error(transparent)]
    Json(serde_json::Error),
    #[error("`{field}` must be {expected}")]
    InvalidField {
        field: String,
        expected: &'static str,
    },
    #[error("dependency `{0}` has no version, path, git, or workspace source")]
    MissingSource(String),
}

impl ParseError {
    pub(crate) fn toml(source: toml_edit::TomlError) -> Self {
        Self(ParseErrorKind::Toml(source))
    }

    pub(crate) fn json(source: serde_json::Error) -> Self {
        Self(ParseErrorKind::Json(source))
    }

    pub(crate) fn invalid_field(field: impl Into<String>, expected: &'static str) -> Self {
        Self(ParseErrorKind::InvalidField {
            field: field.into(),
            expected,
        })
    }

    pub(crate) fn missing_source(name: &str) -> Self {
        Self(ParseErrorKind::MissingSource(name.to_owned()))
    }
}

fn dedup(dependencies: Vec<Dependency>) -> Vec<Dependency> {
    let mut seen = HashSet::new();

    dependencies
        .into_iter()
        .filter(|dependency| seen.insert((dependency.name.clone(), dependency.requirement.clone())))
        .collect()
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use super::{Dependency, ParseError, dedup};

    fn dependency(name: &str, requirement: &str) -> Dependency {
        Dependency::new(name, requirement)
    }

    #[test]
    fn deduplicates_name_and_requirement_pairs_preserving_first_occurrence() {
        assert_eq!(
            dedup(vec![
                dependency("serde", "1"),
                dependency("other", "1"),
                dependency("serde", "1"),
                dependency("serde", "2"),
            ]),
            vec![
                dependency("serde", "1"),
                dependency("other", "1"),
                dependency("serde", "2"),
            ]
        );
    }

    #[test]
    fn parse_error_preserves_display_and_source() {
        let inner = "[broken".parse::<toml_edit::DocumentMut>().unwrap_err();
        let display = inner.to_string();
        let source = StdError::source(&inner).map(|error| error.to_string());
        let error = ParseError::toml(inner);

        assert_eq!(error.to_string(), display);
        assert_eq!(
            StdError::source(&error).map(|source| source.to_string()),
            source
        );
    }

    #[test]
    fn invalid_field_error_describes_the_expected_shape() {
        let error = ParseError::invalid_field("dependencies.example", "a string or table");

        assert_eq!(
            error.to_string(),
            "`dependencies.example` must be a string or table"
        );
    }
}
