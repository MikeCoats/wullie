// SPDX-FileCopyrightText: 2026 Mike Coats <i.am@mikecoats.com>
// SPDX-License-Identifier: GPL-3.0-or-later

use std::ffi::OsStr;

use semver::{Version, VersionReq};
use toml_edit::{DocumentMut, Item, Value};

use super::restore_line_endings;
use crate::dependency::{Dependency, ParseError};
use crate::registry::Release;
use crate::update::{self, CompareError, Outdated, Update};

/// Cargo's build output directory. Every directory with this name is pruned,
/// including ones in non-Rust trees.
pub(super) const SKIP_DIRS: &[&str] = &["target"];

/// Returns whether `file_name` is a Cargo manifest.
pub(super) fn is_manifest(file_name: &OsStr) -> bool {
    file_name == "Cargo.toml"
}

/// Normalizes crate names for registry lookups.
pub(super) fn normalize_name(name: &str) -> String {
    name.to_ascii_lowercase()
}

/// Compares a Cargo requirement with published releases.
pub(super) fn outdated(
    requirement: &str,
    releases: &[Release],
) -> Result<Option<Outdated>, CompareError> {
    let requirement = VersionReq::parse(requirement).map_err(CompareError::invalid_requirement)?;
    let includes_prereleases = requirement
        .comparators
        .iter()
        .any(|comparator| !comparator.pre.is_empty());
    let versions = update::published(releases, |version| Version::parse(version).ok());
    update::outdated(
        &versions,
        |version| requirement.matches(version),
        |version| includes_prereleases || version.pre.is_empty(),
    )
}

/// Dependency table names Cargo reads at the top level and under each `target.<spec>`.
const TABLES: &[&str] = &["dependencies", "dev-dependencies", "build-dependencies"];

/// Parses Cargo dependency requirements from a manifest.
pub(super) fn dependencies(contents: &str) -> Result<Vec<Dependency>, ParseError> {
    let document = contents.parse::<DocumentMut>().map_err(ParseError::toml)?;
    let mut dependencies = Vec::new();

    for (label, item) in dependency_tables(&document)? {
        let table = item
            .as_table_like()
            .ok_or_else(|| ParseError::invalid_field(&label, "a table"))?;
        for (key, item) in table.iter() {
            if let Some(dependency) = parse_dependency(&label, key, item)? {
                dependencies.push(dependency);
            }
        }
    }

    Ok(dependencies)
}

/// Pins matching Cargo dependencies to their selected release versions.
pub(super) fn pin(contents: &str, updates: &[Update]) -> Result<String, ParseError> {
    let mut document = contents.parse::<DocumentMut>().map_err(ParseError::toml)?;

    for (name, item) in document.iter_mut() {
        if TABLES.contains(&name.get()) {
            pin_dependency_table(name.get(), item, updates)?;
        }
    }

    if let Some(targets) = document.get_mut("target").and_then(Item::as_table_like_mut) {
        for (spec, target) in targets.iter_mut() {
            let Some(target) = target.as_table_like_mut() else {
                continue;
            };
            for (name, item) in target.iter_mut() {
                if TABLES.contains(&name.get()) {
                    let label = format!("target.{spec}.{name}");
                    pin_dependency_table(&label, item, updates)?;
                }
            }
        }
    }

    if let Some(workspace_dependencies) = document
        .get_mut("workspace")
        .and_then(Item::as_table_like_mut)
        .and_then(|workspace| workspace.get_mut("dependencies"))
    {
        pin_dependency_table("workspace.dependencies", workspace_dependencies, updates)?;
    }

    Ok(restore_line_endings(contents, &document.to_string()))
}

fn pin_dependency_table(
    label: &str,
    item: &mut Item,
    updates: &[Update],
) -> Result<(), ParseError> {
    let Some(table) = item.as_table_like_mut() else {
        return Err(ParseError::invalid_field(label, "a table"));
    };

    for (key, item) in table.iter_mut() {
        let Some(dependency) = parse_dependency(label, key.get(), item)? else {
            continue;
        };
        let Some(update) = updates.iter().find(|update| {
            update.dependency.name == dependency.name
                && update.dependency.requirement == dependency.requirement
        }) else {
            continue;
        };
        let requirement = format!("={}", without_build_metadata(&update.versions.latest));
        replace_requirement(item, &requirement);
    }

    Ok(())
}

/// Drops any `+build` suffix from a release version. Cargo ignores build
/// metadata in requirements (and warns about it), and crates.io rejects
/// versions that differ only by build metadata, so the pin stays unambiguous.
fn without_build_metadata(version: &str) -> String {
    match Version::parse(version) {
        Ok(mut version) => {
            version.build = semver::BuildMetadata::EMPTY;
            version.to_string()
        }
        Err(_) => version.to_owned(),
    }
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

/// Yields every dependency table Cargo reads, along with its manifest path.
fn dependency_tables(document: &DocumentMut) -> Result<Vec<(String, &Item)>, ParseError> {
    let mut tables = Vec::new();

    for (name, item) in document.iter() {
        if TABLES.contains(&name) {
            tables.push((name.to_owned(), item));
        }
    }

    if let Some(targets_item) = document.get("target") {
        let targets = targets_item
            .as_table_like()
            .ok_or_else(|| ParseError::invalid_field("target", "a table"))?;
        for (spec, target_item) in targets.iter() {
            let target = target_item
                .as_table_like()
                .ok_or_else(|| ParseError::invalid_field(format!("target.{spec}"), "a table"))?;
            for (name, item) in target.iter() {
                if TABLES.contains(&name) {
                    tables.push((format!("target.{spec}.{name}"), item));
                }
            }
        }
    }

    if let Some(workspace_item) = document.get("workspace") {
        let workspace = workspace_item
            .as_table_like()
            .ok_or_else(|| ParseError::invalid_field("workspace", "a table"))?;
        if let Some(item) = workspace.get("dependencies") {
            tables.push(("workspace.dependencies".to_owned(), item));
        }
    }

    Ok(tables)
}

fn parse_dependency(
    table_label: &str,
    key: &str,
    item: &Item,
) -> Result<Option<Dependency>, ParseError> {
    let field = format!("{table_label}.{key}");
    let (name, requirement) = if let Some(requirement) = item.as_str() {
        (key, requirement)
    } else {
        let Some(table) = item.as_table_like() else {
            return Err(ParseError::invalid_field(
                field,
                "a version string or table",
            ));
        };
        let name = match table.get("package") {
            Some(package) => package
                .as_str()
                .ok_or_else(|| ParseError::invalid_field(format!("{field}.package"), "a string"))?,
            None => key,
        };
        let version = match table.get("version") {
            Some(version) => Some(version.as_str().ok_or_else(|| {
                ParseError::invalid_field(format!("{field}.version"), "a string")
            })?),
            None => None,
        };
        if table.get("workspace").and_then(Item::as_bool) == Some(true)
            || table.get("registry").is_some()
            || table.get("registry-index").is_some()
        {
            return Ok(None);
        }
        let Some(requirement) = version else {
            if table.get("path").is_some() || table.get("git").is_some() {
                return Ok(None);
            }
            return Err(ParseError::missing_source(key));
        };
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
            versions: Outdated {
                lowest: String::new(),
                latest: latest.to_owned(),
            },
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
    fn pins_keep_crlf_line_endings() {
        let contents = "[package]\r\nname = \"a\"\r\n\r\n[dependencies]\r\nserde = \"1\" # c\r\nanyhow = \"1\"\r\n";

        let pinned = pin(contents, &[update("serde", "1", "1.0.229")]).unwrap();

        assert_eq!(
            pinned,
            "[package]\r\nname = \"a\"\r\n\r\n[dependencies]\r\nserde = \"=1.0.229\" # c\r\nanyhow = \"1\"\r\n"
        );
    }

    #[test]
    fn pins_keep_a_missing_final_newline() {
        let contents = "[dependencies]\nserde = \"1\"";

        let pinned = pin(contents, &[update("serde", "1", "1.0.229")]).unwrap();

        assert_eq!(pinned, "[dependencies]\nserde = \"=1.0.229\"");
    }

    #[test]
    fn pins_without_build_metadata() {
        let contents = r#"
            [dependencies]
            toml_edit = "0.25.19"
            pre = "1.0.0-alpha.1"
        "#;
        let updates = [
            update("toml_edit", "0.25.19", "0.25.20+spec-1.1.0"),
            update("pre", "1.0.0-alpha.1", "1.0.0-alpha.2+build.7"),
        ];

        let pinned = pin(contents, &updates).unwrap();

        assert!(pinned.contains("toml_edit = \"=0.25.20\""));
        assert!(pinned.contains("pre = \"=1.0.0-alpha.2\""));
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
    fn reports_malformed_dependency_tables_and_entries() {
        for (contents, expected) in [
            ("dependencies = 1", "`dependencies` must be a table"),
            (
                "[dependencies]\ninvalid = 1",
                "`dependencies.invalid` must be a version string or table",
            ),
            (
                "[dependencies]\ninvalid = { version = 1 }",
                "`dependencies.invalid.version` must be a string",
            ),
            (
                "[dependencies]\ninvalid = { package = 1, version = \"1\" }",
                "`dependencies.invalid.package` must be a string",
            ),
            ("[target]\nunix = 1", "`target.unix` must be a table"),
            ("target = 1", "`target` must be a table"),
            (
                "[workspace]\ndependencies = 1",
                "`workspace.dependencies` must be a table",
            ),
        ] {
            let error = dependencies(contents).unwrap_err().to_string();
            assert!(error.contains(expected), "{error}");
        }
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
