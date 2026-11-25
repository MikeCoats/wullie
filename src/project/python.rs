// SPDX-FileCopyrightText: 2026 Mike Coats <i.am@mikecoats.com>
// SPDX-License-Identifier: GPL-3.0-or-later

use std::ffi::OsStr;
use std::str::FromStr;

use pep440_rs::{Version, VersionSpecifiers};
use toml_edit::{DocumentMut, Item};

use super::restore_layout;
use crate::dependency::{Dependency, ParseError};
use crate::registry::Release;
use crate::update::{self, CompareError, Outdated, Update};

/// Directories holding installed or cached Python code rather than projects.
/// Every directory with one of these names is pruned, including ones in
/// non-Python trees.
///
/// Virtualenvs are caught by `site-packages` whatever the venv is called;
/// hidden ones like `.venv` and `.tox` are already skipped by the walk.
pub(super) const SKIP_DIRS: &[&str] = &["__pycache__", "__pypackages__", "site-packages"];

/// Returns whether `file_name` is a `pyproject.toml` manifest.
pub(super) fn is_manifest(file_name: &OsStr) -> bool {
    file_name == "pyproject.toml"
}

/// Normalizes a package name according to PEP 503.
pub(super) fn normalize_name(name: &str) -> String {
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
pub(super) fn outdated(
    requirement: &str,
    releases: &[Release],
) -> Result<Option<Outdated>, CompareError> {
    let specifiers = if requirement.is_empty() {
        VersionSpecifiers::empty()
    } else {
        VersionSpecifiers::from_str(requirement)
            .map_err(CompareError::invalid_python_requirement)?
    };
    let includes_prereleases = specifiers
        .iter()
        .any(pep440_rs::VersionSpecifier::any_prerelease);
    let mut versions = update::published(releases, |version| Version::from_str(version).ok());
    // Pre-releases the requirement doesn't ask for are dropped before
    // matching, so they can't be the lowest matching release either.
    versions.retain(|(version, _)| includes_prereleases || !version.any_prerelease());
    update::outdated(&versions, |version| specifiers.contains(version), |_| true)
}

/// Parses Python dependency requirements from PEP 621 and PEP 735 tables.
pub(super) fn dependencies(contents: &str) -> Result<Vec<Dependency>, ParseError> {
    let document = contents.parse::<DocumentMut>().map_err(ParseError::toml)?;
    let mut dependencies = Vec::new();

    let project = document
        .get("project")
        .map(|item| {
            item.as_table_like()
                .ok_or_else(|| ParseError::invalid_field("project", "a table"))
        })
        .transpose()?;

    if let Some(item) = project.and_then(|project| project.get("dependencies")) {
        let array = item
            .as_array()
            .ok_or_else(|| ParseError::invalid_field("project.dependencies", "an array"))?;
        append_requirements(array, "project.dependencies", false, &mut dependencies)?;
    }

    if let Some(item) = project.and_then(|project| project.get("optional-dependencies")) {
        let optional = item
            .as_table_like()
            .ok_or_else(|| ParseError::invalid_field("project.optional-dependencies", "a table"))?;
        for (name, group) in optional.iter() {
            let label = format!("project.optional-dependencies.{name}");
            let array = group
                .as_array()
                .ok_or_else(|| ParseError::invalid_field(&label, "an array"))?;
            append_requirements(array, &label, false, &mut dependencies)?;
        }
    }

    if let Some(item) = document.get("dependency-groups") {
        let groups = item
            .as_table_like()
            .ok_or_else(|| ParseError::invalid_field("dependency-groups", "a table"))?;
        for (name, group) in groups.iter() {
            let label = format!("dependency-groups.{name}");
            let array = group
                .as_array()
                .ok_or_else(|| ParseError::invalid_field(&label, "an array"))?;
            append_requirements(array, &label, true, &mut dependencies)?;
        }
    }

    Ok(dependencies)
}

/// Pins matching Python dependencies to their selected release versions.
pub(super) fn pin(contents: &str, updates: &[Update]) -> Result<String, ParseError> {
    let mut document = contents.parse::<DocumentMut>().map_err(ParseError::toml)?;

    if let Some(project) = document
        .get_mut("project")
        .and_then(Item::as_table_like_mut)
    {
        if let Some(array) = project.get_mut("dependencies").and_then(Item::as_array_mut) {
            pin_requirements(array, updates);
        }

        if let Some(optional) = project
            .get_mut("optional-dependencies")
            .and_then(Item::as_table_like_mut)
        {
            for (_, group) in optional.iter_mut() {
                if let Some(array) = group.as_array_mut() {
                    pin_requirements(array, updates);
                }
            }
        }
    }

    if let Some(groups) = document
        .get_mut("dependency-groups")
        .and_then(Item::as_table_like_mut)
    {
        for (_, group) in groups.iter_mut() {
            if let Some(array) = group.as_array_mut() {
                pin_requirements(array, updates);
            }
        }
    }

    Ok(restore_layout(contents, &document.to_string()))
}

fn pin_requirements(array: &mut toml_edit::Array, updates: &[Update]) {
    for value in array.iter_mut() {
        let Some(requirement) = value.as_str().and_then(parse_requirement) else {
            continue;
        };
        let Some(update) = updates.iter().find(|update| {
            update.dependency.name == requirement.dependency.name
                && update.dependency.requirement == requirement.dependency.requirement
        }) else {
            continue;
        };

        let pinned = format!(
            "{}=={}{}",
            requirement.prefix, update.versions.latest, requirement.marker
        );
        let decor = value.decor().clone();
        *value = toml_edit::Value::from(pinned);
        value.decor_mut().clone_from(&decor);
    }
}

fn append_requirements(
    array: &toml_edit::Array,
    label: &str,
    allow_include_group: bool,
    dependencies: &mut Vec<Dependency>,
) -> Result<(), ParseError> {
    for (index, value) in array.iter().enumerate() {
        let field = format!("{label}[{index}]");
        if let Some(requirement) = value.as_str() {
            if let Some(parsed) = parse_requirement(requirement) {
                dependencies.push(parsed.dependency);
            }
            continue;
        }

        if allow_include_group
            && let Some(include_group) = value.as_inline_table()
            && let Some(group) = include_group.get("include-group")
        {
            if group.as_str().is_some() {
                continue;
            }
            return Err(ParseError::invalid_field(
                format!("{field}.include-group"),
                "a string",
            ));
        }

        let expected = if allow_include_group {
            "a string or include-group table"
        } else {
            "a string"
        };
        return Err(ParseError::invalid_field(field, expected));
    }

    Ok(())
}

struct ParsedRequirement {
    dependency: Dependency,
    prefix: String,
    marker: String,
}

fn parse_requirement(requirement: &str) -> Option<ParsedRequirement> {
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

    let mut prefix_end = name_end;
    let rest = requirement[name_end..].trim_start();
    if let Some(extras) = rest.strip_prefix('[') {
        let closing = extras.find(']')?;
        prefix_end += requirement[name_end..].len() - rest.len() + closing + 2;
    }
    let rest = requirement[prefix_end..].trim_start();
    if rest.starts_with('@') {
        return None;
    }

    let (version, marker) = if let Some((version, _)) = rest.split_once(';') {
        (version, &rest[version.len()..])
    } else {
        (rest, "")
    };
    let version = version.trim();
    let version = version
        .strip_prefix('(')
        .and_then(|version| version.strip_suffix(')'))
        .unwrap_or(version)
        .trim();

    Some(ParsedRequirement {
        dependency: Dependency::new(name, version),
        prefix: requirement[..prefix_end].to_owned(),
        marker: marker.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;

    use crate::dependency::Dependency;
    use crate::registry::Release;
    use crate::update::{Outdated, Update};

    use super::{dependencies, is_manifest, normalize_name, outdated, parse_requirement, pin};

    fn update(name: &str, requirement: &str, latest: &str) -> Update {
        Update {
            dependency: Dependency::new(name, requirement),
            versions: Outdated {
                lowest: String::new(),
                latest: latest.to_owned(),
            },
        }
    }

    fn release(version: &str, yanked: bool) -> Release {
        Release {
            version: version.to_owned(),
            yanked,
        }
    }

    #[test]
    fn pins_keep_a_byte_order_mark() {
        let contents = "\u{feff}[project]\ndependencies = [\"requests>=2\"]\n";

        let pinned = pin(contents, &[update("requests", ">=2", "2.32.3")]).unwrap();

        assert_eq!(
            pinned,
            "\u{feff}[project]\ndependencies = [\"requests==2.32.3\"]\n"
        );
    }

    #[test]
    fn pins_keep_crlf_line_endings() {
        let contents = "[project]\r\nname = \"app\"\r\ndependencies = [\r\n  \"requests>=2\", # http\r\n  \"rich\",\r\n]\r\n";

        let pinned = pin(contents, &[update("requests", ">=2", "2.32.3")]).unwrap();

        assert_eq!(
            pinned,
            "[project]\r\nname = \"app\"\r\ndependencies = [\r\n  \"requests==2.32.3\", # http\r\n  \"rich\",\r\n]\r\n"
        );
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
                Dependency::new("requests", ">=2.31"),
                Dependency::new("attrs", ">=23.1"),
                Dependency::new("pytest", ">=7"),
                Dependency::new("ruff", ">=0.1"),
            ]
        );
    }

    #[test]
    fn reports_malformed_dependency_tables_and_entries() {
        for (contents, expected) in [
            ("project = 1", "`project` must be a table"),
            (
                "[project]\ndependencies = 1",
                "`project.dependencies` must be an array",
            ),
            (
                "[project]\noptional-dependencies = 1",
                "`project.optional-dependencies` must be a table",
            ),
            (
                "[project.optional-dependencies]\ntest = 1",
                "`project.optional-dependencies.test` must be an array",
            ),
            (
                "[project]\ndependencies = [1]",
                "`project.dependencies[0]` must be a string",
            ),
            (
                "dependency-groups = 1",
                "`dependency-groups` must be a table",
            ),
            (
                "[dependency-groups]\ndev = 1",
                "`dependency-groups.dev` must be an array",
            ),
            (
                "[dependency-groups]\ndev = [1]",
                "`dependency-groups.dev[0]` must be a string or include-group table",
            ),
            (
                "[dependency-groups]\ndev = [{ include-group = 1 }]",
                "`dependency-groups.dev[0].include-group` must be a string",
            ),
        ] {
            let error = dependencies(contents).unwrap_err().to_string();
            assert!(error.contains(expected), "{error}");
        }
    }

    #[test]
    fn parses_pep_508_requirements_and_skips_direct_references() {
        let parsed =
            parse_requirement(" package_name[extra]>=1.2 ; python_version < '4' ").unwrap();
        assert_eq!(parsed.dependency, Dependency::new("package_name", ">=1.2"));
        assert_eq!(parsed.prefix, "package_name[extra]");
        assert_eq!(parsed.marker, "; python_version < '4'");
        assert!(parse_requirement("local-package @ https://example.invalid/pkg").is_none());
    }

    #[test]
    fn bare_names_have_empty_requirements() {
        let parsed = parse_requirement("bare_name").unwrap();
        assert_eq!(parsed.dependency, Dependency::new("bare_name", ""));
        assert_eq!(parsed.prefix, "bare_name");
        assert_eq!(parsed.marker, "");
    }

    #[test]
    fn pins_project_dependencies_optionals_and_groups() {
        let contents = r#"
[project]
dependencies = ["requests>=2.31", "attrs (>=23.1)", "bare-name"]

[project.optional-dependencies]
test = ["pytest[asyncio]>=7; python_version >= '3.9'"]

[dependency-groups]
dev = ["ruff>=0.1"]
"#;
        let updates = [
            update("requests", ">=2.31", "2.32.3"),
            update("attrs", ">=23.1", "24.0"),
            update("bare-name", "", "1.2"),
            update("pytest", ">=7", "8.0"),
            update("ruff", ">=0.1", "0.2"),
        ];

        let pinned = pin(contents, &updates).unwrap();

        assert!(pinned.contains("requests==2.32.3"));
        assert!(pinned.contains("attrs==24.0"));
        assert!(pinned.contains("bare-name==1.2"));
        assert!(pinned.contains("pytest[asyncio]==8.0; python_version >= '3.9'"));
        assert!(pinned.contains("ruff==0.2"));
    }

    #[test]
    fn keeps_comments_and_unmatched_or_direct_reference_entries() {
        let contents = r#"
[project]
dependencies = [
    "requests>=2.31", # keep this comment
    "other>=1",
    "local-package @ https://example.invalid/pkg",
]
"#;

        let pinned = pin(contents, &[update("requests", ">=2.31", "2.32.3")]).unwrap();

        assert!(pinned.contains("requests==2.32.3"));
        assert!(pinned.contains("# keep this comment"));
        assert!(pinned.contains("other>=1"));
        assert!(pinned.contains("local-package @ https://example.invalid/pkg"));
    }

    #[test]
    fn pins_duplicate_requirements_in_every_source() {
        let contents = r#"
[project]
dependencies = ["requests>=2.31"]

[dependency-groups]
test = ["requests>=2.31"]
"#;

        let pinned = pin(contents, &[update("requests", ">=2.31", "2.32.3")]).unwrap();

        assert_eq!(pinned.matches("requests==2.32.3").count(), 2);
    }

    #[test]
    fn pin_reports_invalid_toml() {
        assert!(pin("[project\ndependencies = [", &[]).is_err());
    }

    #[test]
    fn missing_project_table_has_no_dependencies() {
        let dependencies = dependencies("[tool.black]\nline-length = 88").unwrap();

        assert!(dependencies.is_empty(), "{dependencies:?}");
    }

    #[test]
    fn invalid_toml_is_reported() {
        assert!(dependencies("[project\ndependencies = [").is_err());
    }
}
