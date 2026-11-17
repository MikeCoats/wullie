// SPDX-FileCopyrightText: 2026 Mike Coats <i.am@mikecoats.com>
// SPDX-License-Identifier: GPL-3.0-or-later

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
    /// A project manifest contains invalid TOML.
    #[error("cannot parse {}", path.display())]
    Parse {
        /// The manifest that could not be parsed.
        path: PathBuf,
        /// The underlying TOML parse error.
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

        if !dependencies.is_empty() {
            findings.projects.push(ProjectDependencies {
                project: project.clone(),
                dependencies,
            });
        }
    }

    findings
}

/// An error parsing a TOML manifest.
#[derive(Debug, thiserror::Error)]
#[error(transparent)]
pub struct ParseError(toml_edit::TomlError);

impl ParseError {
    pub(crate) fn new(source: toml_edit::TomlError) -> Self {
        Self(source)
    }
}
