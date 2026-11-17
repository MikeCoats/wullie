// SPDX-FileCopyrightText: 2026 Mike Coats <i.am@mikecoats.com>
// SPDX-License-Identifier: GPL-3.0-or-later

use std::ffi::OsStr;

use toml_edit::{DocumentMut, Item};

use crate::dependency::{Dependency, ParseError};

/// Directories holding installed or cached Python code rather than projects.
/// Every directory with one of these names is pruned, including ones in
/// non-Python trees.
///
/// Virtualenvs are caught by `site-packages` whatever the venv is called;
/// hidden ones like `.venv` and `.tox` are already skipped by the walk.
pub const SKIP_DIRS: &[&str] = &["__pycache__", "__pypackages__", "site-packages"];

/// Returns whether `file_name` is a `pyproject.toml` manifest.
pub fn is_manifest(file_name: &OsStr) -> bool {
    file_name == "pyproject.toml"
}

/// Parses Python dependency requirements from PEP 621 and PEP 735 tables.
pub fn dependencies(contents: &str) -> Result<Vec<Dependency>, ParseError> {
    let document = contents.parse::<DocumentMut>().map_err(ParseError::toml)?;
    let project = document.get("project").and_then(Item::as_table_like);
    let mut dependencies = Vec::new();

    if let Some(array) = project
        .and_then(|project| project.get("dependencies"))
        .and_then(Item::as_array)
    {
        append_requirements(array, &mut dependencies);
    }

    if let Some(optional) = project
        .and_then(|project| project.get("optional-dependencies"))
        .and_then(Item::as_table_like)
    {
        for (_, group) in optional.iter() {
            if let Some(array) = group.as_array() {
                append_requirements(array, &mut dependencies);
            }
        }
    }

    if let Some(groups) = document
        .get("dependency-groups")
        .and_then(Item::as_table_like)
    {
        for (_, group) in groups.iter() {
            if let Some(array) = group.as_array() {
                append_requirements(array, &mut dependencies);
            }
        }
    }

    Ok(dependencies)
}

fn append_requirements(array: &toml_edit::Array, dependencies: &mut Vec<Dependency>) {
    dependencies.extend(
        array
            .iter()
            .filter_map(|value| value.as_str())
            .filter_map(parse_requirement),
    );
}

fn parse_requirement(requirement: &str) -> Option<Dependency> {
    let requirement = requirement.trim();
    let name_end = requirement
        .char_indices()
        .take_while(|(_, character)| {
            character.is_ascii_alphanumeric() || "-_.".contains(*character)
        })
        .map(|(index, character)| index + character.len_utf8())
        .last()?;
    let name = &requirement[..name_end];
    if !name.as_bytes()[0].is_ascii_alphanumeric() {
        return None;
    }

    let mut rest = requirement[name_end..].trim_start();
    if let Some(extras) = rest.strip_prefix('[') {
        let closing = extras.find(']')?;
        rest = extras[closing + 1..].trim_start();
    }
    if rest.starts_with('@') {
        return None;
    }

    let version = rest
        .split_once(';')
        .map_or(rest, |(version, _)| version)
        .trim();
    let version = version
        .strip_prefix('(')
        .and_then(|version| version.strip_suffix(')'))
        .unwrap_or(version)
        .trim();

    Some(Dependency {
        name: name.to_owned(),
        requirement: version.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;

    use crate::dependency::Dependency;

    use super::{dependencies, is_manifest, parse_requirement};

    #[test]
    fn detects_pyproject_manifest() {
        assert!(is_manifest(OsStr::new("pyproject.toml")));
    }

    #[test]
    fn rejects_legacy_and_other_names() {
        assert!(!is_manifest(OsStr::new("setup.py")));
        assert!(!is_manifest(OsStr::new("setup.cfg")));
        assert!(!is_manifest(OsStr::new("requirements.txt")));
        assert!(!is_manifest(OsStr::new("Pipfile")));
        assert!(!is_manifest(OsStr::new("PyProject.toml")));
    }

    #[test]
    fn collects_project_optional_and_dependency_groups_in_order() {
        let contents = r#"
            [dependency-groups]
            dev = ["ruff>=0.1", { include-group = "lint" }]

            [project]
            dependencies = ["requests>=2.31", "attrs (>=23.1)"]

            [project.optional-dependencies]
            test = ["pytest[asyncio]>=7; python_version >= '3.9'"]
        "#;

        assert_eq!(
            dependencies(contents).unwrap(),
            vec![
                Dependency {
                    name: "requests".to_owned(),
                    requirement: ">=2.31".to_owned(),
                },
                Dependency {
                    name: "attrs".to_owned(),
                    requirement: ">=23.1".to_owned(),
                },
                Dependency {
                    name: "pytest".to_owned(),
                    requirement: ">=7".to_owned(),
                },
                Dependency {
                    name: "ruff".to_owned(),
                    requirement: ">=0.1".to_owned(),
                },
            ]
        );
    }

    #[test]
    fn parses_pep_508_requirements_and_skips_direct_references() {
        assert_eq!(
            parse_requirement(" package_name[extra]>=1.2 ; python_version < '4' "),
            Some(Dependency {
                name: "package_name".to_owned(),
                requirement: ">=1.2".to_owned(),
            })
        );
        assert_eq!(
            parse_requirement("local-package @ https://example.invalid/pkg"),
            None
        );
    }

    #[test]
    fn bare_names_have_empty_requirements() {
        assert_eq!(
            parse_requirement("bare_name"),
            Some(Dependency {
                name: "bare_name".to_owned(),
                requirement: String::new(),
            })
        );
    }

    #[test]
    fn missing_project_table_has_no_dependencies() {
        assert!(
            dependencies("[tool.black]\nline-length = 88")
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn invalid_toml_is_reported() {
        assert!(dependencies("[project\ndependencies = [").is_err());
    }
}
