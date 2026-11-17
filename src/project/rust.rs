// SPDX-FileCopyrightText: 2026 Mike Coats <i.am@mikecoats.com>
// SPDX-License-Identifier: GPL-3.0-or-later

use std::ffi::OsStr;

use toml_edit::{DocumentMut, Item};

use crate::dependency::{Dependency, ParseError};

/// Cargo's build output directory. Every directory with this name is pruned,
/// including ones in non-Rust trees.
pub const SKIP_DIRS: &[&str] = &["target"];

/// Returns whether `file_name` is a Cargo manifest.
pub fn is_manifest(file_name: &OsStr) -> bool {
    file_name == "Cargo.toml"
}

/// Dependency table names Cargo reads at the top level and under each `target.<spec>`.
const TABLES: &[&str] = &["dependencies", "dev-dependencies", "build-dependencies"];

/// Parses Cargo dependency requirements from a manifest.
pub fn dependencies(contents: &str) -> Result<Vec<Dependency>, ParseError> {
    let document = contents.parse::<DocumentMut>().map_err(ParseError::toml)?;
    let mut dependencies = Vec::new();

    for table in dependency_tables(&document).filter_map(Item::as_table_like) {
        for (key, item) in table.iter() {
            if let Some(dependency) = parse_dependency(key, item) {
                dependencies.push(dependency);
            }
        }
    }

    Ok(dependencies)
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

fn parse_dependency(key: &str, item: &Item) -> Option<Dependency> {
    let (name, requirement) = if let Some(requirement) = item.as_str() {
        (key, requirement)
    } else {
        let table = item.as_table_like()?;
        if table.get("workspace").and_then(Item::as_bool) == Some(true) {
            return None;
        }
        let requirement = table.get("version")?.as_str()?;
        let name = table.get("package").and_then(Item::as_str).unwrap_or(key);
        (name, requirement)
    };

    Some(Dependency {
        name: name.to_owned(),
        requirement: requirement.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;

    use crate::dependency::Dependency;

    use super::{dependencies, is_manifest};

    fn dependency(name: &str, requirement: &str) -> Dependency {
        Dependency {
            name: name.to_owned(),
            requirement: requirement.to_owned(),
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
}
