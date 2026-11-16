// SPDX-FileCopyrightText: 2026 Mike Coats <i.am@mikecoats.com>
// SPDX-License-Identifier: GPL-3.0-or-later

pub mod project;
pub mod walk;

use std::collections::BTreeMap;
use std::path::Path;

use ignore::Error;
use strum::IntoEnumIterator;

use crate::project::{Ecosystem, Project};

/// The result of searching a directory tree for projects.
#[derive(Debug, Default)]
pub struct Findings {
    /// Projects found, sorted by directory then ecosystem, one per `(dir, ecosystem)`.
    pub projects: Vec<Project>,
    /// Errors met during the walk; the search carries on past them.
    pub errors: Vec<Error>,
}

/// Searches `root` recursively for projects of every supported ecosystem.
pub fn find_projects(root: &Path) -> Findings {
    let skip_dirs: Vec<&'static str> = Ecosystem::iter()
        .flat_map(|ecosystem| ecosystem.skip_dirs().iter().copied())
        .collect();
    let mut projects = BTreeMap::new();
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

        for ecosystem in Ecosystem::iter().filter(|ecosystem| ecosystem.is_manifest(file_name)) {
            if let Some(project) = Project::new(ecosystem, file.clone()) {
                projects
                    .entry((project.dir().to_path_buf(), ecosystem))
                    .or_insert(project);
            }
        }
    }

    Findings {
        projects: projects.into_values().collect(),
        errors,
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use tempfile::TempDir;

    use crate::find_projects;
    use crate::project::{Ecosystem, Project};

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
