// SPDX-FileCopyrightText: 2026 Mike Coats <i.am@mikecoats.com>
// SPDX-License-Identifier: GPL-3.0-or-later

use std::ffi::OsStr;
use std::str::FromStr;

use pep440_rs::{Version, VersionSpecifiers};
use toml_edit::{DocumentMut, Item};

use crate::dependency::{Dependency, ParseError};
use crate::registry::Release;
use crate::update::{CompareError, Outdated};

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

/// Normalizes a package name according to PEP 503.
pub fn normalize_name(name: &str) -> String {
    let mut normalized = String::with_capacity(name.len());
    let mut separator = false;

    for character in name.chars() {
        if matches!(character, '-' | '_' | '.') {
            if !separator {
                normalized.push('-');
            }
            separator = true;
        } else {
            normalized.push(character.to_ascii_lowercase());
            separator = false;
        }
    }

    normalized
}

/// Compares a Python requirement with published releases.
pub fn outdated(requirement: &str, releases: &[Release]) -> Result<Option<Outdated>, CompareError> {
    let specifiers = if requirement.is_empty() {
        VersionSpecifiers::empty()
    } else {
        VersionSpecifiers::from_str(requirement)
            .map_err(CompareError::invalid_python_requirement)?
    };
    let includes_prereleases = specifiers
        .iter()
        .any(|specifier| specifier.any_prerelease());
    let versions: Vec<_> = releases
        .iter()
        .filter(|release| !release.yanked)
        .filter_map(|release| {
            Version::from_str(&release.version)
                .ok()
                .map(|version| (version, release.version.as_str()))
        })
        .filter(|(version, _)| includes_prereleases || !version.any_prerelease())
        .collect();
    let lowest = versions
        .iter()
        .filter(|(version, _)| specifiers.contains(version))
        .min_by(|(left, _), (right, _)| left.cmp(right))
        .ok_or_else(CompareError::no_matching_release)?;
    let latest = versions
        .iter()
        .max_by(|(left, _), (right, _)| left.cmp(right))
        .ok_or_else(CompareError::no_matching_release)?;

    if lowest.0 < latest.0 {
        Ok(Some(Outdated {
            lowest: lowest.1.to_owned(),
            latest: latest.1.to_owned(),
        }))
    } else {
        Ok(None)
    }
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
    use crate::registry::Release;
    use crate::update::Outdated;

    use super::{dependencies, is_manifest, normalize_name, outdated, parse_requirement};

    fn release(version: &str, yanked: bool) -> Release {
        Release {
            version: version.to_owned(),
            yanked,
        }
    }

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
    fn normalizes_pep_503_name_variants() {
        for name in ["Foo_Bar", "foo-bar", "foo.bar", "FOO--bar", "foo._bar"] {
            assert_eq!(normalize_name(name), "foo-bar");
        }
    }

    #[test]
    fn reports_lowest_matching_release_and_latest_for_a_range() {
        assert_eq!(
            outdated(
                ">=2.31",
                &[
                    release("2.30.0", false),
                    release("2.31.0", false),
                    release("2.32.3", false),
                ]
            )
            .unwrap(),
            Some(Outdated {
                lowest: "2.31.0".to_owned(),
                latest: "2.32.3".to_owned(),
            })
        );
    }

    #[test]
    fn an_empty_requirement_matches_every_stable_release() {
        assert_eq!(
            outdated(
                "",
                &[
                    release("0.9.0", true),
                    release("not-a-version", false),
                    release("1.0.dev1", false),
                    release("1.0rc1", false),
                    release("1.0", false),
                    release("1.1", false),
                ]
            )
            .unwrap(),
            Some(Outdated {
                lowest: "1.0".to_owned(),
                latest: "1.1".to_owned(),
            })
        );
    }

    #[test]
    fn does_not_report_an_exact_current_pin() {
        assert_eq!(
            outdated(
                "==2.32.3",
                &[release("2.30.0", false), release("2.32.3", false)]
            )
            .unwrap(),
            None
        );
    }

    #[test]
    fn applies_pep_440_compatible_release_rules() {
        assert_eq!(
            outdated(
                "~=2.31",
                &[
                    release("2.31.0", false),
                    release("2.32.0", false),
                    release("3.0.0", false),
                ]
            )
            .unwrap(),
            Some(Outdated {
                lowest: "2.31.0".to_owned(),
                latest: "3.0.0".to_owned(),
            })
        );
    }

    #[test]
    fn excludes_prereleases_and_dev_releases_unless_named() {
        assert_eq!(
            outdated(
                ">=2",
                &[
                    release("2.5", false),
                    release("3.0.dev1", false),
                    release("3.0rc1", false),
                    release("3.0", false),
                ]
            )
            .unwrap(),
            Some(Outdated {
                lowest: "2.5".to_owned(),
                latest: "3.0".to_owned(),
            })
        );
        assert_eq!(
            outdated(
                ">=3.0rc1",
                &[
                    release("3.0.dev1", false),
                    release("3.0rc1", false),
                    release("3.0", false),
                ]
            )
            .unwrap(),
            Some(Outdated {
                lowest: "3.0rc1".to_owned(),
                latest: "3.0".to_owned(),
            })
        );
        assert_eq!(
            outdated("==3.0rc1", &[release("3.0rc1", false)]).unwrap(),
            None
        );
    }

    #[test]
    fn orders_epochs_and_post_releases() {
        assert_eq!(
            outdated("", &[release("2.0", false), release("1!0.1", false)]).unwrap(),
            Some(Outdated {
                lowest: "2.0".to_owned(),
                latest: "1!0.1".to_owned(),
            })
        );
        assert_eq!(
            outdated("", &[release("1.0", false), release("1.0.post1", false)]).unwrap(),
            Some(Outdated {
                lowest: "1.0".to_owned(),
                latest: "1.0.post1".to_owned(),
            })
        );
    }

    #[test]
    fn rejects_invalid_requirements_and_missing_matches() {
        let invalid = outdated(">>2", &[release("2.0", false)]).unwrap_err();
        assert!(invalid.to_string().contains("invalid version requirement"));

        let no_match = outdated(">=2", &[release("1.9", false)]).unwrap_err();
        assert!(
            no_match
                .to_string()
                .contains("no non-yanked release matches")
        );
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
