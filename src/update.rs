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
            let key = (ecosystem, ecosystem.normalize_name(&dependency.name));
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
    #[error("invalid version requirement")]
    InvalidPythonRequirement(#[source] pep440_rs::VersionSpecifiersParseError),
    #[error("invalid version requirement")]
    InvalidJavaScriptRequirement(#[source] nodejs_semver::SemverError),
    #[error("no non-yanked release matches the requirement")]
    NoMatchingRelease,
}

impl CompareError {
    pub(crate) fn invalid_requirement(source: semver::Error) -> Self {
        Self(CompareErrorKind::InvalidRequirement(source))
    }

    pub(crate) fn invalid_python_requirement(
        source: pep440_rs::VersionSpecifiersParseError,
    ) -> Self {
        Self(CompareErrorKind::InvalidPythonRequirement(source))
    }

    pub(crate) fn invalid_javascript_requirement(source: nodejs_semver::SemverError) -> Self {
        Self(CompareErrorKind::InvalidJavaScriptRequirement(source))
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
        releases: HashMap<(Ecosystem, String), Vec<Release>>,
        calls: RefCell<HashMap<(Ecosystem, String), usize>>,
    }

    impl FakeRegistry {
        fn set_releases(&mut self, ecosystem: Ecosystem, name: &str, versions: &[(&str, bool)]) {
            self.releases.insert(
                (ecosystem, ecosystem.normalize_name(name)),
                versions
                    .iter()
                    .map(|(version, yanked)| Release {
                        version: (*version).to_owned(),
                        yanked: *yanked,
                    })
                    .collect(),
            );
        }

        fn calls(&self, ecosystem: Ecosystem, name: &str) -> usize {
            self.calls
                .borrow()
                .get(&(ecosystem, ecosystem.normalize_name(name)))
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
            let name = ecosystem.normalize_name(name);
            *self
                .calls
                .borrow_mut()
                .entry((ecosystem, name.clone()))
                .or_default() += 1;
            self.releases
                .get(&(ecosystem, name))
                .cloned()
                .ok_or_else(RegistryError::not_found)
        }
    }

    fn project(path: &str) -> Project {
        project_for(Ecosystem::Rust, path)
    }

    fn project_for(ecosystem: Ecosystem, path: &str) -> Project {
        Project::new(ecosystem, PathBuf::from(path)).unwrap()
    }

    fn project_dependencies(path: &str, dependencies: &[(&str, &str)]) -> ProjectDependencies {
        project_dependencies_for(Ecosystem::Rust, path, dependencies)
    }

    fn project_dependencies_for(
        ecosystem: Ecosystem,
        path: &str,
        dependencies: &[(&str, &str)],
    ) -> ProjectDependencies {
        ProjectDependencies {
            project: project_for(ecosystem, path),
            dependencies: dependencies
                .iter()
                .map(|(name, requirement)| Dependency::new(*name, *requirement))
                .collect(),
        }
    }

    fn update(name: &str, requirement: &str, lowest: &str, latest: &str) -> Update {
        Update {
            dependency: Dependency::new(name, requirement),
            lowest: lowest.to_owned(),
            latest: latest.to_owned(),
        }
    }

    #[test]
    fn fetches_a_crate_shared_by_projects_once_and_preserves_input_order() {
        let mut registry = FakeRegistry::default();
        registry.set_releases(
            Ecosystem::Rust,
            "shared",
            &[("1.0.0", false), ("1.2.0", false)],
        );
        let projects = vec![
            project_dependencies("/z/Cargo.toml", &[("Shared", "1")]),
            project_dependencies("/a/Cargo.toml", &[("shared", "=1.0.0")]),
        ];

        let findings = find_updates(&projects, &registry);

        assert!(findings.errors.is_empty());
        assert_eq!(registry.calls(Ecosystem::Rust, "shared"), 1);
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
        registry.set_releases(
            Ecosystem::Rust,
            "current",
            &[("1.0.0", false), ("1.2.0", false)],
        );
        registry.set_releases(
            Ecosystem::Rust,
            "old",
            &[("1.0.0", false), ("1.2.0", false)],
        );
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
    fn javascript_requirements_follow_npm_range_rules() {
        let mut registry = FakeRegistry::default();
        registry.set_releases(
            Ecosystem::JavaScript,
            "lodash",
            &[("4.17.0", false), ("4.17.21", false)],
        );
        let projects = vec![project_dependencies_for(
            Ecosystem::JavaScript,
            "/javascript/package.json",
            &[("lodash", "4.17.0"), ("lodash", "4.17.21")],
        )];

        let findings = find_updates(&projects, &registry);

        assert!(findings.errors.is_empty());
        assert_eq!(registry.calls(Ecosystem::JavaScript, "lodash"), 1);
        assert_eq!(
            findings.projects,
            vec![ProjectUpdates {
                project: project_for(Ecosystem::JavaScript, "/javascript/package.json"),
                updates: vec![update("lodash", "4.17.0", "4.17.0", "4.17.21")],
            }]
        );
    }

    #[test]
    fn python_name_variants_share_one_registry_lookup() {
        let mut registry = FakeRegistry::default();
        registry.set_releases(
            Ecosystem::Python,
            "foo-bar",
            &[("1.0", false), ("1.2", false)],
        );
        let projects = vec![
            project_dependencies_for(
                Ecosystem::Python,
                "/first/pyproject.toml",
                &[("Foo_Bar", ">=1.0")],
            ),
            project_dependencies_for(
                Ecosystem::Python,
                "/second/pyproject.toml",
                &[("foo.bar", "")],
            ),
        ];

        let findings = find_updates(&projects, &registry);

        assert!(findings.errors.is_empty());
        assert_eq!(registry.calls(Ecosystem::Python, "FOO--bar"), 1);
        assert_eq!(
            findings.projects,
            vec![
                ProjectUpdates {
                    project: project_for(Ecosystem::Python, "/first/pyproject.toml"),
                    updates: vec![update("Foo_Bar", ">=1.0", "1.0", "1.2")],
                },
                ProjectUpdates {
                    project: project_for(Ecosystem::Python, "/second/pyproject.toml"),
                    updates: vec![update("foo.bar", "", "1.0", "1.2")],
                },
            ]
        );
    }

    #[test]
    fn same_package_name_is_looked_up_separately_per_ecosystem() {
        let mut registry = FakeRegistry::default();
        registry.set_releases(
            Ecosystem::Rust,
            "shared",
            &[("1.0.0", false), ("1.2.0", false)],
        );
        registry.set_releases(
            Ecosystem::Python,
            "shared",
            &[("1.0", false), ("1.2", false)],
        );
        registry.set_releases(
            Ecosystem::JavaScript,
            "shared",
            &[("1.0.0", false), ("1.2.0", false)],
        );
        let projects = vec![
            project_dependencies("/rust/Cargo.toml", &[("shared", "*")]),
            project_dependencies_for(
                Ecosystem::Python,
                "/python/pyproject.toml",
                &[("shared", "")],
            ),
            project_dependencies_for(
                Ecosystem::JavaScript,
                "/javascript/package.json",
                &[("shared", "^1")],
            ),
        ];

        let findings = find_updates(&projects, &registry);

        assert!(findings.errors.is_empty());
        assert_eq!(registry.calls(Ecosystem::Rust, "shared"), 1);
        assert_eq!(registry.calls(Ecosystem::Python, "shared"), 1);
        assert_eq!(registry.calls(Ecosystem::JavaScript, "shared"), 1);
        assert_eq!(findings.projects.len(), 3);
    }

    #[test]
    fn caches_lookup_failures_and_continues_with_other_crates() {
        let mut registry = FakeRegistry::default();
        registry.set_releases(
            Ecosystem::Rust,
            "valid",
            &[("1.0.0", false), ("1.2.0", false)],
        );
        let projects = vec![
            project_dependencies("/first/Cargo.toml", &[("missing", "1"), ("valid", "1")]),
            project_dependencies("/second/Cargo.toml", &[("missing", "2")]),
        ];

        let findings = find_updates(&projects, &registry);

        assert_eq!(registry.calls(Ecosystem::Rust, "missing"), 1);
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
        registry.set_releases(Ecosystem::Rust, "broken", &[("1.0.0", false)]);
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
