// SPDX-FileCopyrightText: 2026 Mike Coats <i.am@mikecoats.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The Dependency Finder: reads each project's manifest into a list of
//! declared dependencies and their version requirements.

use std::collections::HashSet;
use std::io;
use std::path::PathBuf;

use crate::project::Project;

/// A dependency with a version requirement, regardless of optionality or ecosystem-specific category.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Dependency {
    /// The dependency's package name, which may differ from its manifest key when renamed.
    pub name: String,
    /// The version requirement exactly as written in the manifest.
    pub requirement: String,
}

impl Dependency {
    /// Creates a dependency from its package name and manifest requirement.
    pub(crate) fn new(name: impl Into<String>, requirement: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            requirement: requirement.into(),
        }
    }
}

/// A project and the dependencies declared in its manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProjectDependencies {
    /// The project whose manifest declares these dependencies.
    pub project: Project,
    /// Dependencies with version requirements in the manifest.
    pub dependencies: Vec<Dependency>,
}

/// Results from parsing the manifests of discovered projects.
#[derive(Debug, Default)]
pub(crate) struct DependencyFindings {
    /// Projects with at least one dependency version requirement, in input order.
    pub projects: Vec<ProjectDependencies>,
    /// Manifest read or parse errors; other manifests are still processed.
    pub errors: Vec<ManifestError>,
}

/// An error reading or parsing a project manifest.
#[derive(Debug, thiserror::Error)]
pub(crate) enum ManifestError {
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
pub(crate) fn find_dependencies(projects: &[Project]) -> DependencyFindings {
    let mut findings = DependencyFindings::default();

    for project in projects {
        let path = project.manifest().to_path_buf();
        let contents = match project.read_manifest() {
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
pub(crate) struct ParseError(ParseErrorKind);

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
    use std::fs;
    use std::path::Path;

    use tempfile::TempDir;

    use crate::project::{Ecosystem, Project};

    use super::{
        Dependency, ManifestError, ParseError, ProjectDependencies, dedup, find_dependencies,
    };

    fn project(ecosystem: Ecosystem, manifest: &Path) -> Project {
        Project::new(ecosystem, manifest.to_path_buf())
    }

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
        let source = StdError::source(&inner).map(ToString::to_string);
        let error = ParseError::toml(inner);

        assert_eq!(error.to_string(), display);
        assert_eq!(StdError::source(&error).map(ToString::to_string), source);
    }

    #[test]
    fn invalid_field_error_describes_the_expected_shape() {
        let error = ParseError::invalid_field("dependencies.example", "a string or table");

        assert_eq!(
            error.to_string(),
            "`dependencies.example` must be a string or table"
        );
    }

    #[test]
    fn finds_dependencies_for_workspace_root_and_member_in_path_order() {
        let temp = TempDir::new().unwrap();
        let root_manifest = temp.path().join("Cargo.toml");
        let member_manifest = temp.path().join("crates/member/Cargo.toml");
        fs::write(
            &root_manifest,
            "[workspace]\nmembers = [\"crates/member\"]\n\
             [workspace.dependencies]\nshared = \"1\"\n",
        )
        .unwrap();
        fs::create_dir_all(member_manifest.parent().unwrap()).unwrap();
        fs::write(
            &member_manifest,
            "[package]\nname = \"member\"\nversion = \"0.1.0\"\n\
             [dependencies]\nmember_dep = \"^2\"\nshared = { workspace = true }\n",
        )
        .unwrap();
        let projects = vec![
            project(Ecosystem::Rust, &root_manifest),
            project(Ecosystem::Rust, &member_manifest),
        ];

        let findings = find_dependencies(&projects);

        assert!(findings.errors.is_empty());
        assert_eq!(
            findings.projects,
            vec![
                ProjectDependencies {
                    project: project(Ecosystem::Rust, &root_manifest),
                    dependencies: vec![Dependency::new("shared", "1")],
                },
                ProjectDependencies {
                    project: project(Ecosystem::Rust, &member_manifest),
                    dependencies: vec![Dependency::new("member_dep", "^2")],
                },
            ]
        );
    }

    #[test]
    fn find_dependencies_deduplicates_name_and_requirement_pairs() {
        let temp = TempDir::new().unwrap();
        let manifest = temp.path().join("Cargo.toml");
        fs::write(
            &manifest,
            "[dependencies]\nshared = \"1\"\n\
             [dev-dependencies]\nshared = \"1\"\n\
             shared_v2 = { package = \"shared\", version = \"2\" }\n",
        )
        .unwrap();

        let findings = find_dependencies(&[project(Ecosystem::Rust, &manifest)]);

        assert!(findings.errors.is_empty());
        assert_eq!(
            findings.projects[0].dependencies,
            vec![
                Dependency::new("shared", "1"),
                Dependency::new("shared", "2"),
            ]
        );
    }

    #[test]
    fn reports_invalid_manifest_and_continues_to_valid_project() {
        let temp = TempDir::new().unwrap();
        let invalid_manifest = temp.path().join("invalid/Cargo.toml");
        let valid_manifest = temp.path().join("valid/Cargo.toml");
        fs::create_dir_all(invalid_manifest.parent().unwrap()).unwrap();
        fs::write(&invalid_manifest, "[dependencies\nserde = \"1\"").unwrap();
        fs::create_dir_all(valid_manifest.parent().unwrap()).unwrap();
        fs::write(&valid_manifest, "[dependencies]\nserde = \"1\"\n").unwrap();
        let projects = vec![
            project(Ecosystem::Rust, &invalid_manifest),
            project(Ecosystem::Rust, &valid_manifest),
        ];

        let findings = find_dependencies(&projects);

        assert_eq!(findings.errors.len(), 1);
        assert!(matches!(
            &findings.errors[0],
            ManifestError::Parse { path, .. } if path == &invalid_manifest
        ));
        assert_eq!(
            findings.projects,
            vec![ProjectDependencies {
                project: project(Ecosystem::Rust, &valid_manifest),
                dependencies: vec![Dependency::new("serde", "1")],
            }]
        );
    }

    #[cfg(unix)]
    #[test]
    fn reports_unreadable_manifest_and_continues() {
        use std::os::unix::fs::PermissionsExt;

        /// Restores manifest permissions on drop, so the temporary directory remains removable.
        struct RestorePermissions<'a>(&'a Path);

        impl Drop for RestorePermissions<'_> {
            fn drop(&mut self) {
                let _ = fs::set_permissions(self.0, fs::Permissions::from_mode(0o600));
            }
        }

        let temp = TempDir::new().unwrap();
        let unreadable_manifest = temp.path().join("unreadable/Cargo.toml");
        let visible_manifest = temp.path().join("visible/Cargo.toml");
        fs::create_dir_all(unreadable_manifest.parent().unwrap()).unwrap();
        fs::write(
            &unreadable_manifest,
            "[dependencies]\nprivate_dep = \"1\"\n",
        )
        .unwrap();
        fs::create_dir_all(visible_manifest.parent().unwrap()).unwrap();
        fs::write(&visible_manifest, "[dependencies]\nvisible_dep = \"2\"\n").unwrap();
        fs::set_permissions(&unreadable_manifest, fs::Permissions::from_mode(0o0)).unwrap();
        let _restore = RestorePermissions(&unreadable_manifest);

        if fs::read_to_string(&unreadable_manifest).is_ok() {
            return;
        }

        let findings = find_dependencies(&[
            project(Ecosystem::Rust, &unreadable_manifest),
            project(Ecosystem::Rust, &visible_manifest),
        ]);

        assert!(matches!(
            findings.errors.as_slice(),
            [ManifestError::Read { path, .. }] if path == &unreadable_manifest
        ));
        assert_eq!(
            findings.projects,
            vec![ProjectDependencies {
                project: project(Ecosystem::Rust, &visible_manifest),
                dependencies: vec![Dependency::new("visible_dep", "2")],
            }]
        );
    }

    #[test]
    fn parses_python_and_javascript_dependencies_and_reports_invalid_manifests() {
        let temp = TempDir::new().unwrap();
        let python_manifest = temp.path().join("valid/pyproject.toml");
        let invalid_python_manifest = temp.path().join("invalid/pyproject.toml");
        let javascript_manifest = temp.path().join("valid/package.json");
        let invalid_javascript_manifest = temp.path().join("invalid/package.json");
        fs::create_dir_all(python_manifest.parent().unwrap()).unwrap();
        fs::write(
            &python_manifest,
            "[project]\ndependencies = [\"requests>=2.31\"]\n",
        )
        .unwrap();
        fs::create_dir_all(invalid_python_manifest.parent().unwrap()).unwrap();
        fs::write(&invalid_python_manifest, "[project\ndependencies = [").unwrap();
        fs::write(&javascript_manifest, r#"{"dependencies":{"left-pad":"^1","renamed":"npm:real-package@2"},"devDependencies":{"left-pad":"^1","test-package":"latest"}}"#).unwrap();
        fs::write(&invalid_javascript_manifest, "invalid json").unwrap();

        let findings = find_dependencies(&[
            project(Ecosystem::Python, &invalid_python_manifest),
            project(Ecosystem::JavaScript, &invalid_javascript_manifest),
            project(Ecosystem::JavaScript, &javascript_manifest),
            project(Ecosystem::Python, &python_manifest),
        ]);

        assert!(matches!(
            findings.errors.as_slice(),
            [
                ManifestError::Parse { path: python_path, .. },
                ManifestError::Parse { path: javascript_path, .. }
            ] if python_path == &invalid_python_manifest && javascript_path == &invalid_javascript_manifest
        ));
        assert_eq!(
            findings.projects,
            vec![
                ProjectDependencies {
                    project: project(Ecosystem::JavaScript, &javascript_manifest),
                    dependencies: vec![
                        Dependency::new("left-pad", "^1"),
                        Dependency::new("real-package", "2"),
                        Dependency::new("test-package", "latest"),
                    ],
                },
                ProjectDependencies {
                    project: project(Ecosystem::Python, &python_manifest),
                    dependencies: vec![Dependency::new("requests", ">=2.31")],
                },
            ]
        );
    }

    #[test]
    fn drops_projects_without_versioned_dependencies() {
        let temp = TempDir::new().unwrap();
        let manifest = temp.path().join("Cargo.toml");
        fs::write(
            &manifest,
            "[package]\nname = \"empty\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();

        let findings = find_dependencies(&[project(Ecosystem::Rust, &manifest)]);

        assert!(findings.projects.is_empty(), "{:?}", findings.projects);
        assert!(findings.errors.is_empty());
    }

    #[test]
    fn keeps_the_input_order_of_projects_with_dependencies() {
        let temp = TempDir::new().unwrap();
        let first_manifest = temp.path().join("first/Cargo.toml");
        let second_manifest = temp.path().join("second/Cargo.toml");
        fs::create_dir_all(first_manifest.parent().unwrap()).unwrap();
        fs::write(&first_manifest, "[dependencies]\nfirst_dep = \"1\"\n").unwrap();
        fs::create_dir_all(second_manifest.parent().unwrap()).unwrap();
        fs::write(&second_manifest, "[dependencies]\nsecond_dep = \"2\"\n").unwrap();

        let findings = find_dependencies(&[
            project(Ecosystem::Rust, &second_manifest),
            project(Ecosystem::Rust, &first_manifest),
        ]);

        assert!(findings.errors.is_empty());
        assert_eq!(
            findings
                .projects
                .iter()
                .map(|project| project.project.manifest().to_path_buf())
                .collect::<Vec<_>>(),
            vec![second_manifest, first_manifest]
        );
    }
}
