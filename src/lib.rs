// SPDX-FileCopyrightText: 2026 Mike Coats <i.am@mikecoats.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Library functionality for finding projects and their dependency version requirements.

pub mod dependency;
pub mod project;
pub mod walk;
pub use dependency::find_dependencies;
pub use walk::WalkError;

use std::path::Path;

use strum::IntoEnumIterator;

use crate::project::{Ecosystem, Project};

/// The result of searching a directory tree for projects.
#[derive(Debug, Default)]
pub struct Findings {
    /// Projects found, sorted by manifest path then ecosystem.
    pub projects: Vec<Project>,
    /// Errors met during the walk; the search carries on past them.
    pub errors: Vec<WalkError>,
}

/// Searches `root` recursively for projects of every supported ecosystem.
pub fn find_projects(root: &Path) -> Findings {
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
                .filter_map(|ecosystem| Project::new(ecosystem, file.clone())),
        );
    }

    projects.sort();
    Findings { projects, errors }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use tempfile::TempDir;

    use crate::dependency::{Dependency, ManifestError, ProjectDependencies};
    use crate::project::{Ecosystem, Project};
    use crate::{find_dependencies, find_projects};

    fn create_manifest(path: &Path) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, "").unwrap();
    }

    fn rust_project(manifest: &Path) -> Project {
        Project::new(Ecosystem::Rust, manifest.to_path_buf()).unwrap()
    }

    fn python_project(manifest: &Path) -> Project {
        Project::new(Ecosystem::Python, manifest.to_path_buf()).unwrap()
    }

    fn javascript_project(manifest: &Path) -> Project {
        Project::new(Ecosystem::JavaScript, manifest.to_path_buf()).unwrap()
    }

    #[test]
    fn finds_a_single_rust_project() {
        let temp = TempDir::new().unwrap();
        let manifest = temp.path().join("Cargo.toml");
        create_manifest(&manifest);

        let findings = find_projects(temp.path());

        assert!(findings.errors.is_empty());
        assert_eq!(findings.projects, vec![rust_project(&manifest)]);
        assert_eq!(findings.projects[0].dir(), temp.path());
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
            vec![rust_project(&root_manifest), rust_project(&member_manifest)]
        );
        assert_eq!(findings.projects[1].dir(), temp.path().join("crates/a"));
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
        assert_eq!(findings.projects, vec![rust_project(&root_manifest)]);
    }

    #[test]
    fn finds_a_python_project_by_pyproject() {
        let temp = TempDir::new().unwrap();
        let manifest = temp.path().join("app/pyproject.toml");
        create_manifest(&manifest);

        let findings = find_projects(temp.path());

        assert!(findings.errors.is_empty());
        assert_eq!(findings.projects, vec![python_project(&manifest)]);
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
                javascript_project(&root_manifest),
                javascript_project(&member_manifest)
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
        assert_eq!(findings.projects, vec![javascript_project(&root_manifest)]);
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
                rust_project(&rust_manifest),
                javascript_project(&javascript_manifest),
                python_project(&python_manifest)
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
        assert!(findings.projects.is_empty());
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
            vec![rust_project(&cargo), python_project(&pyproject)]
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
        assert_eq!(findings.projects, vec![python_project(&root_manifest)]);
    }

    #[test]
    fn finds_nothing_in_an_empty_directory() {
        let temp = TempDir::new().unwrap();

        let findings = find_projects(temp.path());

        assert!(findings.errors.is_empty());
        assert!(findings.projects.is_empty());
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
        let projects = find_projects(temp.path()).projects;

        let findings = find_dependencies(&projects);

        assert!(findings.errors.is_empty());
        assert_eq!(
            findings.projects,
            vec![
                ProjectDependencies {
                    project: rust_project(&root_manifest),
                    dependencies: vec![Dependency {
                        name: "shared".to_owned(),
                        requirement: "1".to_owned(),
                    }],
                },
                ProjectDependencies {
                    project: rust_project(&member_manifest),
                    dependencies: vec![Dependency {
                        name: "member_dep".to_owned(),
                        requirement: "^2".to_owned(),
                    }],
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

        let projects = find_projects(temp.path()).projects;
        let findings = find_dependencies(&projects);

        assert!(findings.errors.is_empty());
        assert_eq!(
            findings.projects[0].dependencies,
            vec![
                Dependency {
                    name: "shared".to_owned(),
                    requirement: "1".to_owned(),
                },
                Dependency {
                    name: "shared".to_owned(),
                    requirement: "2".to_owned(),
                },
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
            rust_project(&invalid_manifest),
            rust_project(&valid_manifest),
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
                project: rust_project(&valid_manifest),
                dependencies: vec![Dependency {
                    name: "serde".to_owned(),
                    requirement: "1".to_owned(),
                }],
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
            rust_project(&unreadable_manifest),
            rust_project(&visible_manifest),
        ]);

        assert!(matches!(
            findings.errors.as_slice(),
            [ManifestError::Read { path, .. }] if path == &unreadable_manifest
        ));
        assert_eq!(
            findings.projects,
            vec![ProjectDependencies {
                project: rust_project(&visible_manifest),
                dependencies: vec![Dependency {
                    name: "visible_dep".to_owned(),
                    requirement: "2".to_owned(),
                }],
            }]
        );
    }

    #[test]
    fn python_and_javascript_projects_have_no_dependencies_or_errors() {
        let temp = TempDir::new().unwrap();
        let python_manifest = temp.path().join("pyproject.toml");
        let javascript_manifest = temp.path().join("package.json");
        fs::write(&python_manifest, "invalid toml").unwrap();
        fs::write(&javascript_manifest, "invalid json").unwrap();

        let findings = find_dependencies(&[
            python_project(&python_manifest),
            javascript_project(&javascript_manifest),
        ]);

        assert!(findings.projects.is_empty());
        assert!(findings.errors.is_empty());
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

        let findings = find_dependencies(&[rust_project(&manifest)]);

        assert!(findings.projects.is_empty());
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
            rust_project(&second_manifest),
            rust_project(&first_manifest),
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
        assert_eq!(findings.projects, vec![rust_project(&visible_manifest)]);
    }
}
