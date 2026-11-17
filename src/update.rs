// SPDX-FileCopyrightText: 2026 Mike Coats <i.am@mikecoats.com>
// SPDX-License-Identifier: GPL-3.0-or-later

use std::collections::HashMap;

use crate::dependency::{Dependency, ProjectDependencies};
use crate::project::{Ecosystem, Project};
use crate::registry::{Registry, RegistryError, Release};

/// The lowest matching and newest release when a requirement is out of date.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outdated {
    /// The lowest published non-yanked release matching the requirement.
    pub lowest: String,
    /// The newest published non-yanked release.
    pub latest: String,
}

/// A dependency found to be out of date.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Update {
    /// The dependency as declared in its manifest.
    pub dependency: Dependency,
    /// The lowest published non-yanked release matching its requirement.
    pub lowest: String,
    /// The newest published non-yanked release.
    pub latest: String,
}

/// Outdated dependencies belonging to one project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectUpdates {
    /// The project whose dependencies need updates.
    pub project: Project,
    /// Outdated dependencies in Dependency Finder order.
    pub updates: Vec<Update>,
}

/// Results from checking project dependencies against their registries.
#[derive(Debug, Default)]
pub struct UpdateFindings {
    /// Projects with outdated dependencies, in input order.
    pub projects: Vec<ProjectUpdates>,
    /// Lookup and comparison errors; other dependencies are still checked.
    pub errors: Vec<UpdateError>,
}

/// An error looking up or comparing a dependency's releases.
#[derive(Debug, thiserror::Error)]
pub enum UpdateError {
    /// A registry lookup failed for this package.
    #[error("cannot look up {name}")]
    Lookup {
        /// The dependency name that could not be looked up.
        name: String,
        /// The registry error.
        #[source]
        source: RegistryError,
    },
    /// A dependency requirement could not be compared with its releases.
    #[error("cannot check {name} {requirement}")]
    Compare {
        /// The dependency name that could not be checked.
        name: String,
        /// The requirement that could not be checked.
        requirement: String,
        /// The comparison error.
        #[source]
        source: CompareError,
    },
}

/// Finds outdated dependencies, fetching each ecosystem/package pair once.
pub fn find_updates(projects: &[ProjectDependencies], registry: &dyn Registry) -> UpdateFindings {
    let mut findings = UpdateFindings::default();
    let mut cache: HashMap<(Ecosystem, String), Option<Vec<Release>>> = HashMap::new();

    for project in projects {
        let ecosystem = project.project.ecosystem();
        let mut updates = Vec::new();

        for dependency in &project.dependencies {
            let key = (ecosystem, dependency.name.to_ascii_lowercase());
            if !cache.contains_key(&key) {
                match registry.releases(ecosystem, &dependency.name) {
                    Ok(releases) => {
                        cache.insert(key.clone(), Some(releases));
                    }
                    Err(source) => {
                        findings.errors.push(UpdateError::Lookup {
                            name: dependency.name.clone(),
                            source,
                        });
                        cache.insert(key.clone(), None);
                        continue;
                    }
                }
            }

            let Some(releases) = cache.get(&key).and_then(Option::as_deref) else {
                continue;
            };
            match ecosystem.outdated(&dependency.requirement, releases) {
                Ok(Some(outdated)) => updates.push(Update {
                    dependency: dependency.clone(),
                    lowest: outdated.lowest,
                    latest: outdated.latest,
                }),
                Ok(None) => {}
                Err(source) => findings.errors.push(UpdateError::Compare {
                    name: dependency.name.clone(),
                    requirement: dependency.requirement.clone(),
                    source,
                }),
            }
        }

        if !updates.is_empty() {
            findings.projects.push(ProjectUpdates {
                project: project.project.clone(),
                updates,
            });
        }
    }

    findings
}

/// An error comparing a requirement with published releases.
#[derive(Debug, thiserror::Error)]
#[error(transparent)]
pub struct CompareError(CompareErrorKind);

#[derive(Debug, thiserror::Error)]
enum CompareErrorKind {
    #[error("invalid version requirement")]
    InvalidRequirement(#[source] semver::Error),
    #[error("no non-yanked release matches the requirement")]
    NoMatchingRelease,
}

impl CompareError {
    pub(crate) fn invalid_requirement(source: semver::Error) -> Self {
        Self(CompareErrorKind::InvalidRequirement(source))
    }

    pub(crate) fn no_matching_release() -> Self {
        Self(CompareErrorKind::NoMatchingRelease)
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::path::PathBuf;

    use crate::dependency::{Dependency, ProjectDependencies};
    use crate::project::{Ecosystem, Project};
    use crate::registry::{Registry, RegistryError, Release};

    use super::{ProjectUpdates, Update, UpdateError, find_updates};

    #[derive(Default)]
    struct FakeRegistry {
        releases: HashMap<String, Vec<Release>>,
        calls: RefCell<HashMap<String, usize>>,
    }

    impl FakeRegistry {
        fn set_releases(&mut self, name: &str, versions: &[(&str, bool)]) {
            self.releases.insert(
                name.to_ascii_lowercase(),
                versions
                    .iter()
                    .map(|(version, yanked)| Release {
                        version: (*version).to_owned(),
                        yanked: *yanked,
                    })
                    .collect(),
            );
        }

        fn calls(&self, name: &str) -> usize {
            self.calls
                .borrow()
                .get(&name.to_ascii_lowercase())
                .copied()
                .unwrap_or_default()
        }
    }

    impl Registry for FakeRegistry {
        fn releases(
            &self,
            ecosystem: Ecosystem,
            name: &str,
        ) -> Result<Vec<Release>, RegistryError> {
            assert_eq!(ecosystem, Ecosystem::Rust);
            let name = name.to_ascii_lowercase();
            *self.calls.borrow_mut().entry(name.clone()).or_default() += 1;
            self.releases
                .get(&name)
                .cloned()
                .ok_or_else(RegistryError::not_found)
        }
    }

    fn project(path: &str) -> Project {
        Project::new(Ecosystem::Rust, PathBuf::from(path)).unwrap()
    }

    fn project_dependencies(path: &str, dependencies: &[(&str, &str)]) -> ProjectDependencies {
        ProjectDependencies {
            project: project(path),
            dependencies: dependencies
                .iter()
                .map(|(name, requirement)| Dependency {
                    name: (*name).to_owned(),
                    requirement: (*requirement).to_owned(),
                })
                .collect(),
        }
    }

    fn update(name: &str, requirement: &str, lowest: &str, latest: &str) -> Update {
        Update {
            dependency: Dependency {
                name: name.to_owned(),
                requirement: requirement.to_owned(),
            },
            lowest: lowest.to_owned(),
            latest: latest.to_owned(),
        }
    }

    #[test]
    fn fetches_a_crate_shared_by_projects_once_and_preserves_input_order() {
        let mut registry = FakeRegistry::default();
        registry.set_releases("shared", &[("1.0.0", false), ("1.2.0", false)]);
        let projects = vec![
            project_dependencies("/z/Cargo.toml", &[("Shared", "1")]),
            project_dependencies("/a/Cargo.toml", &[("shared", "=1.0.0")]),
        ];

        let findings = find_updates(&projects, &registry);

        assert!(findings.errors.is_empty());
        assert_eq!(registry.calls("shared"), 1);
        assert_eq!(
            findings.projects,
            vec![
                ProjectUpdates {
                    project: project("/z/Cargo.toml"),
                    updates: vec![update("Shared", "1", "1.0.0", "1.2.0")],
                },
                ProjectUpdates {
                    project: project("/a/Cargo.toml"),
                    updates: vec![update("shared", "=1.0.0", "1.0.0", "1.2.0")],
                },
            ]
        );
    }

    #[test]
    fn drops_current_dependencies_and_projects_without_updates() {
        let mut registry = FakeRegistry::default();
        registry.set_releases("current", &[("1.0.0", false), ("1.2.0", false)]);
        registry.set_releases("old", &[("1.0.0", false), ("1.2.0", false)]);
        let projects = vec![
            project_dependencies("/empty/Cargo.toml", &[("current", "=1.2.0")]),
            project_dependencies("/mixed/Cargo.toml", &[("current", "=1.2.0"), ("old", "1")]),
        ];

        let findings = find_updates(&projects, &registry);

        assert!(findings.errors.is_empty());
        assert_eq!(findings.projects.len(), 1);
        assert_eq!(findings.projects[0].project, project("/mixed/Cargo.toml"));
        assert_eq!(
            findings.projects[0].updates,
            vec![update("old", "1", "1.0.0", "1.2.0")]
        );
    }

    #[test]
    fn caches_lookup_failures_and_continues_with_other_crates() {
        let mut registry = FakeRegistry::default();
        registry.set_releases("valid", &[("1.0.0", false), ("1.2.0", false)]);
        let projects = vec![
            project_dependencies("/first/Cargo.toml", &[("missing", "1"), ("valid", "1")]),
            project_dependencies("/second/Cargo.toml", &[("missing", "2")]),
        ];

        let findings = find_updates(&projects, &registry);

        assert_eq!(registry.calls("missing"), 1);
        assert_eq!(findings.errors.len(), 1);
        assert!(matches!(
            &findings.errors[0],
            UpdateError::Lookup { name, .. } if name == "missing"
        ));
        assert_eq!(findings.projects.len(), 1);
        assert_eq!(
            findings.projects[0].updates,
            vec![update("valid", "1", "1.0.0", "1.2.0")]
        );
    }

    #[test]
    fn reports_comparison_errors_with_the_dependency_and_requirement() {
        let mut registry = FakeRegistry::default();
        registry.set_releases("broken", &[("1.0.0", false)]);
        let projects = vec![project_dependencies(
            "/project/Cargo.toml",
            &[("broken", "not-semver")],
        )];

        let findings = find_updates(&projects, &registry);

        assert!(findings.projects.is_empty());
        assert_eq!(findings.errors.len(), 1);
        match &findings.errors[0] {
            UpdateError::Compare {
                name,
                requirement,
                source,
            } => {
                assert_eq!(name, "broken");
                assert_eq!(requirement, "not-semver");
                assert_eq!(source.to_string(), "invalid version requirement");
            }
            error => panic!("expected a comparison error, got {error:?}"),
        }
    }
}
