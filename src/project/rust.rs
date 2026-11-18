// SPDX-FileCopyrightText: 2026 Mike Coats <i.am@mikecoats.com>
// SPDX-License-Identifier: GPL-3.0-or-later

use std::ffi::OsStr;

use semver::{Version, VersionReq};
use toml_edit::{DocumentMut, Item, Value};

use crate::dependency::{Dependency, ParseError};
use crate::registry::Release;
use crate::update::{CompareError, Outdated, Update};

/// Cargo's build output directory. Every directory with this name is pruned,
/// including ones in non-Rust trees.
pub const SKIP_DIRS: &[&str] = &["target"];

/// Returns whether `file_name` is a Cargo manifest.
pub fn is_manifest(file_name: &OsStr) -> bool {
    file_name == "Cargo.toml"
}

/// Normalizes crate names for registry lookups.
pub fn normalize_name(name: &str) -> String {
    name.to_ascii_lowercase()
}

/// Compares a Cargo requirement with published releases.
pub fn outdated(requirement: &str, releases: &[Release]) -> Result<Option<Outdated>, CompareError> {
    let requirement = VersionReq::parse(requirement).map_err(CompareError::invalid_requirement)?;
    let includes_prereleases = requirement
        .comparators
        .iter()
        .any(|comparator| !comparator.pre.is_empty());
    let versions: Vec<_> = releases
        .iter()
        .filter(|release| !release.yanked)
        .filter_map(|release| {
            Version::parse(&release.version)
                .ok()
                .map(|version| (version, release.version.as_str()))
        })
        .collect();
    let lowest = versions
        .iter()
        .filter(|(version, _)| requirement.matches(version))
        .min_by(|(left, _), (right, _)| left.cmp(right))
        .ok_or_else(CompareError::no_matching_release)?;
    let latest = versions
        .iter()
        .filter(|(version, _)| includes_prereleases || version.pre.is_empty())
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

/// Dependency table names Cargo reads at the top level and under each `target.<spec>`.
const TABLES: &[&str] = &["dependencies", "dev-dependencies", "build-dependencies"];

/// Parses Cargo dependency requirements from a manifest.
pub fn dependencies(contents: &str) -> Result<Vec<Dependency>, ParseError> {
    let document = contents.parse::<DocumentMut>().map_err(ParseError::toml)?;
    let mut dependencies = Vec::new();

    for table in dependency_tables(&document).filter_map(Item::as_table_like) {
        for (key, item) in table.iter() {
            if let Some(dependency) = parse_dependency(key, item)? {
                dependencies.push(dependency);
            }
        }
    }

    Ok(dependencies)
}

/// Pins matching Cargo dependencies to their selected release versions.
pub fn pin(contents: &str, updates: &[Update]) -> Result<String, ParseError> {
    let mut document = contents.parse::<DocumentMut>().map_err(ParseError::toml)?;

    for (name, item) in document.iter_mut() {
        if TABLES.contains(&name.get()) {
            pin_dependency_table(item, updates)?;
        }
    }

    if let Some(targets) = document.get_mut("target").and_then(Item::as_table_like_mut) {
        for (_, target) in targets.iter_mut() {
            let Some(target) = target.as_table_like_mut() else {
                continue;
            };
            for (name, item) in target.iter_mut() {
                if TABLES.contains(&name.get()) {
                    pin_dependency_table(item, updates)?;
                }
            }
        }
    }

    if let Some(workspace_dependencies) = document
        .get_mut("workspace")
        .and_then(Item::as_table_like_mut)
        .and_then(|workspace| workspace.get_mut("dependencies"))
    {
        pin_dependency_table(workspace_dependencies, updates)?;
    }

    Ok(document.to_string())
}

fn pin_dependency_table(item: &mut Item, updates: &[Update]) -> Result<(), ParseError> {
    let Some(table) = item.as_table_like_mut() else {
        return Ok(());
    };

    for (key, item) in table.iter_mut() {
        let Some(dependency) = parse_dependency(key.get(), item)? else {
            continue;
        };
        let Some(update) = updates.iter().find(|update| {
            update.dependency.name == dependency.name
                && update.dependency.requirement == dependency.requirement
        }) else {
            continue;
        };
        let requirement = format!("={}", update.latest);
        replace_requirement(item, &requirement);
    }

    Ok(())
}

fn replace_requirement(item: &mut Item, requirement: &str) {
    let version = if item.as_str().is_some() {
        Some(item)
    } else {
        item.as_table_like_mut()
            .and_then(|table| table.get_mut("version"))
    };
    let Some(value) = version.and_then(Item::as_value_mut) else {
        return;
    };

    let decor = value.decor().clone();
    *value = Value::from(requirement.to_owned());
    value.decor_mut().clone_from(&decor);
}

/// Yields every dependency table Cargo reads: the top-level tables, then those
/// under each `target.<spec>`, then `workspace.dependencies`.
fn dependency_tables(document: &DocumentMut) -> impl Iterator<Item = &Item> {
    let top_level = document
        .iter()
        .filter(|(name, _)| TABLES.contains(name))
        .map(|(_, item)| item);
    let targets = document
        .get("target")
        .and_then(Item::as_table_like)
        .into_iter()
        .flat_map(|targets| targets.iter())
        .filter_map(|(_, target)| target.as_table_like())
        .flat_map(|target| target.iter())
        .filter(|(name, _)| TABLES.contains(name))
        .map(|(_, item)| item);
    let workspace = document
        .get("workspace")
        .and_then(Item::as_table_like)
        .and_then(|workspace| workspace.get("dependencies"));

    top_level.chain(targets).chain(workspace)
}

fn parse_dependency(key: &str, item: &Item) -> Result<Option<Dependency>, ParseError> {
    let (name, requirement) = if let Some(requirement) = item.as_str() {
        (key, requirement)
    } else {
        let Some(table) = item.as_table_like() else {
            return Ok(None);
        };
        if table.get("workspace").and_then(Item::as_bool) == Some(true)
            || table.get("registry").is_some()
            || table.get("registry-index").is_some()
        {
            return Ok(None);
        }
        let Some(version) = table.get("version") else {
            if table.get("path").is_some() || table.get("git").is_some() {
                return Ok(None);
            }
            return Err(ParseError::missing_source(key));
        };
        let Some(requirement) = version.as_str() else {
            return Ok(None);
        };
        let name = table.get("package").and_then(Item::as_str).unwrap_or(key);
        (name, requirement)
    };

    Ok(Some(Dependency::new(name, requirement)))
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;

    use crate::dependency::Dependency;
    use crate::registry::Release;
    use crate::update::{Outdated, Update};

    use super::{dependencies, is_manifest, outdated, pin};

    fn dependency(name: &str, requirement: &str) -> Dependency {
        Dependency::new(name, requirement)
    }

    fn release(version: &str, yanked: bool) -> Release {
        Release {
            version: version.to_owned(),
            yanked,
        }
    }

    fn update(name: &str, requirement: &str, latest: &str) -> Update {
        Update {
            dependency: dependency(name, requirement),
            lowest: String::new(),
            latest: latest.to_owned(),
        }
    }

    #[test]
    fn detects_cargo_manifest() {
        assert!(is_manifest(OsStr::new("Cargo.toml")));
    }

    #[test]
    fn rejects_other_manifest_names() {
        assert!(!is_manifest(OsStr::new("Cargo.lock")));
        assert!(!is_manifest(OsStr::new("cargo.toml")));
    }

    #[test]
    fn parses_string_inline_table_and_section_forms() {
        let dependencies = dependencies(
            r#"
                [dependencies]
                serde = "1"
                foo = { package = "bar", version = "^0.4", path = "../bar" }

                [dependencies.section_crate]
                version = "=2.1.3"
            "#,
        )
        .unwrap();

        assert_eq!(
            dependencies,
            vec![
                dependency("serde", "1"),
                dependency("bar", "^0.4"),
                dependency("section_crate", "=2.1.3"),
            ]
        );
    }

    #[test]
    fn pins_string_inline_section_target_and_workspace_dependencies() {
        let contents = r#"# manifest comment
[dependencies]
serde = "1" # serde comment
renamed = { package = "renamed-crate", version = "^0.4", path = "../renamed" }

[dependencies.section_crate]
version = "=2.1.3" # section comment

[target.'cfg(unix)'.dependencies]
libc = "0.2"

[workspace.dependencies]
shared = "3"
"#;
        let updates = [
            update("serde", "1", "1.2.0"),
            update("renamed-crate", "^0.4", "0.9.0"),
            update("section_crate", "=2.1.3", "2.2.0"),
            update("libc", "0.2", "0.2.9"),
            update("shared", "3", "3.1.0"),
        ];

        let pinned = pin(contents, &updates).unwrap();

        assert!(pinned.contains("serde = \"=1.2.0\" # serde comment"));
        assert!(pinned.contains("version = \"=0.9.0\""));
        assert!(pinned.contains("version = \"=2.2.0\" # section comment"));
        assert!(pinned.contains("libc = \"=0.2.9\""));
        assert!(pinned.contains("shared = \"=3.1.0\""));
        assert!(pinned.starts_with("# manifest comment\n"));
    }

    #[test]
    fn pins_duplicate_names_with_distinct_requirements_and_leaves_unmatched_entries() {
        let contents = r#"
            [dependencies]
            shared = "1"
            unmatched = "4"

            [dev-dependencies]
            shared_alias = { package = "shared", version = "2" }
        "#;
        let updates = [
            update("shared", "1", "3.0.0"),
            update("shared", "2", "3.0.0"),
        ];

        let pinned = pin(contents, &updates).unwrap();

        assert!(pinned.contains("shared = \"=3.0.0\""));
        assert!(pinned.contains("version = \"=3.0.0\""));
        assert!(pinned.contains("unmatched = \"4\""));
    }

    #[test]
    fn collects_every_dependency_table_in_manifest_order_including_optional() {
        let dependencies = dependencies(
            r#"
                [dependencies]
                normal = "1"
                optional = { version = "2", optional = true }

                [dev-dependencies]
                dev = "3"

                [build-dependencies]
                build = "4"

                [target.'cfg(unix)'.dependencies]
                target_normal = "5"

                [target.'cfg(unix)'.dev-dependencies]
                target_dev = "6"

                [target.'cfg(windows)'.build-dependencies]
                target_build = "7"

                [workspace.dependencies]
                shared = "2"
            "#,
        )
        .unwrap();

        assert_eq!(
            dependencies,
            vec![
                dependency("normal", "1"),
                dependency("optional", "2"),
                dependency("dev", "3"),
                dependency("build", "4"),
                dependency("target_normal", "5"),
                dependency("target_dev", "6"),
                dependency("target_build", "7"),
                dependency("shared", "2"),
            ]
        );
    }

    #[test]
    fn lists_top_level_tables_before_target_and_workspace_tables() {
        let dependencies = dependencies(
            r#"
                [workspace.dependencies]
                shared = "1"

                [target.'cfg(unix)'.dependencies]
                libc = "0.2"

                [dependencies]
                serde = "1"
            "#,
        )
        .unwrap();

        assert_eq!(
            dependencies,
            vec![
                dependency("serde", "1"),
                dependency("libc", "0.2"),
                dependency("shared", "1"),
            ]
        );
    }

    #[test]
    fn returns_duplicate_pairs_for_shared_deduplication() {
        let dependencies = dependencies(
            r#"
                [dependencies]
                shared = "1"
                other = "1"

                [dev-dependencies]
                shared = "1"
                shared_v2 = { package = "shared", version = "2" }

                [build-dependencies]
                shared_alias = { package = "shared", version = "2" }

                [target.'cfg(unix)'.dependencies]
                shared = "3"

                [workspace.dependencies]
                shared = "1"
            "#,
        )
        .unwrap();

        assert_eq!(
            dependencies,
            vec![
                dependency("shared", "1"),
                dependency("other", "1"),
                dependency("shared", "1"),
                dependency("shared", "2"),
                dependency("shared", "2"),
                dependency("shared", "3"),
                dependency("shared", "1"),
            ]
        );
    }

    #[test]
    fn skips_dependencies_from_alternative_registries() {
        let dependencies = dependencies(
            r#"
                [dependencies]
                public = "1"
                named_registry = { version = "2", registry = "internal" }
                explicit_registry = { version = "3", registry-index = "https://example.invalid/index" }
            "#,
        )
        .unwrap();

        assert_eq!(dependencies, vec![dependency("public", "1")]);
    }

    #[test]
    fn rejects_dependency_tables_without_a_source() {
        for dependency in ["thiserror = {}", "thiserror = { optional = true }"] {
            let error = dependencies(&format!("[dependencies]\n{dependency}"))
                .unwrap_err()
                .to_string();
            assert!(error.contains("thiserror"));
            assert!(error.contains("no version, path, git, or workspace source"));
        }
    }

    #[test]
    fn keeps_git_and_path_dependencies_with_versions_and_skips_unversioned_entries() {
        let dependencies = dependencies(
            r#"
                [dependencies]
                path_with_version = { path = "../path", version = "1" }
                git_with_version = { git = "https://example.invalid/repo", version = "^2" }
                workspace_inherited = { workspace = true }
                path_only = { path = "../path" }
                git_only = { git = "https://example.invalid/repo" }
                non_string_version = { version = 1 }
                non_string_requirement = 1
            "#,
        )
        .unwrap();

        assert_eq!(
            dependencies,
            vec![
                dependency("path_with_version", "1"),
                dependency("git_with_version", "^2"),
            ]
        );
    }

    #[test]
    fn rejects_invalid_toml() {
        assert!(dependencies("[dependencies\nserde = \"1\"").is_err());
    }

    #[test]
    fn reports_lowest_matching_release_and_latest_for_a_range() {
        assert_eq!(
            outdated(
                "1",
                &[
                    release("1.0.0", false),
                    release("1.5.0", false),
                    release("1.9.0", false)
                ]
            )
            .unwrap(),
            Some(Outdated {
                lowest: "1.0.0".to_owned(),
                latest: "1.9.0".to_owned(),
            })
        );
    }

    #[test]
    fn does_not_report_an_exact_current_pin() {
        assert_eq!(
            outdated(
                "=1.9.0",
                &[release("1.5.0", false), release("1.9.0", false)]
            )
            .unwrap(),
            None
        );
    }

    #[test]
    fn reports_an_exact_old_pin() {
        assert_eq!(
            outdated(
                "=1.5.0",
                &[release("1.5.0", false), release("1.9.0", false)]
            )
            .unwrap(),
            Some(Outdated {
                lowest: "1.5.0".to_owned(),
                latest: "1.9.0".to_owned(),
            })
        );
    }

    #[test]
    fn ignores_yanked_releases_for_lowest_and_latest() {
        assert_eq!(
            outdated(
                "1",
                &[
                    release("1.0.0", true),
                    release("1.0.1", false),
                    release("1.5.0", false),
                    release("2.0.0", true),
                ]
            )
            .unwrap(),
            Some(Outdated {
                lowest: "1.0.1".to_owned(),
                latest: "1.5.0".to_owned(),
            })
        );
    }

    #[test]
    fn applies_cargo_caret_rules_to_zero_major_versions() {
        assert_eq!(
            outdated(
                "0.4",
                &[
                    release("0.4.0", false),
                    release("0.4.8", false),
                    release("0.5.0", false)
                ]
            )
            .unwrap(),
            Some(Outdated {
                lowest: "0.4.0".to_owned(),
                latest: "0.5.0".to_owned(),
            })
        );
    }

    #[test]
    fn uses_the_first_published_release_above_a_greater_than_requirement() {
        assert_eq!(
            outdated(
                ">1.2.3",
                &[
                    release("1.2.3", false),
                    release("1.2.4", false),
                    release("1.3.0", false),
                ]
            )
            .unwrap(),
            Some(Outdated {
                lowest: "1.2.4".to_owned(),
                latest: "1.3.0".to_owned(),
            })
        );
    }

    #[test]
    fn stable_requirements_ignore_prereleases_as_latest() {
        assert_eq!(
            outdated(
                "*",
                &[
                    release("1.0.0", false),
                    release("1.9.0", false),
                    release("2.0.0-rc.1", false),
                ]
            )
            .unwrap(),
            Some(Outdated {
                lowest: "1.0.0".to_owned(),
                latest: "1.9.0".to_owned(),
            })
        );
    }

    #[test]
    fn requirements_naming_a_prerelease_include_it_in_latest() {
        assert_eq!(
            outdated("=2.0.0-rc.1", &[release("2.0.0-rc.1", false)]).unwrap(),
            None
        );
    }

    #[test]
    fn rejects_invalid_requirements() {
        assert!(outdated("not-semver", &[release("1.0.0", false)]).is_err());
    }

    #[test]
    fn reports_no_matching_non_yanked_release() {
        assert!(outdated("=2.0.0", &[release("1.0.0", false), release("2.0.0", true)]).is_err());
    }

    #[test]
    fn skips_unparseable_release_versions() {
        assert_eq!(
            outdated(
                "1",
                &[
                    release("not-semver", false),
                    release("1.0.0", false),
                    release("1.1.0", false)
                ]
            )
            .unwrap(),
            Some(Outdated {
                lowest: "1.0.0".to_owned(),
                latest: "1.1.0".to_owned(),
            })
        );
    }
}
