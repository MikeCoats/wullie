// SPDX-FileCopyrightText: 2026 Mike Coats <i.am@mikecoats.com>
// SPDX-License-Identifier: GPL-3.0-or-later

use std::ffi::OsStr;
use std::fmt;

use indexmap::IndexMap;
use serde::de::value::MapAccessDeserializer;
use serde::de::{MapAccess, Visitor};
use serde::{Deserialize, Deserializer};

use crate::dependency::{Dependency, ParseError};

/// Directories holding installed dependencies rather than projects.
/// Every directory with one of these names is pruned, including ones in
/// non-JavaScript trees.
pub const SKIP_DIRS: &[&str] = &["node_modules", "bower_components"];

/// Returns whether `file_name` is a `package.json` manifest.
pub fn is_manifest(file_name: &OsStr) -> bool {
    file_name == "package.json"
}

/// Leaves JavaScript package names unchanged.
pub fn normalize_name(name: &str) -> String {
    name.to_owned()
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct PackageJsonFields {
    #[serde(default)]
    dependencies: IndexMap<String, String>,
    #[serde(default)]
    dev_dependencies: IndexMap<String, String>,
    #[serde(default)]
    optional_dependencies: IndexMap<String, String>,
    #[serde(default)]
    peer_dependencies: IndexMap<String, String>,
}

struct PackageJson(PackageJsonFields);

impl<'de> Deserialize<'de> for PackageJson {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct PackageJsonVisitor;

        impl<'de> Visitor<'de> for PackageJsonVisitor {
            type Value = PackageJson;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a package.json object")
            }

            fn visit_map<A>(self, map: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                PackageJsonFields::deserialize(MapAccessDeserializer::new(map)).map(PackageJson)
            }
        }

        deserializer.deserialize_map(PackageJsonVisitor)
    }
}

/// Parses registry dependencies from the supported `package.json` fields.
pub fn dependencies(contents: &str) -> Result<Vec<Dependency>, ParseError> {
    let document = serde_json::from_str::<PackageJson>(contents).map_err(ParseError::json)?;
    let mut dependencies = Vec::new();

    for entries in [
        &document.0.dependencies,
        &document.0.dev_dependencies,
        &document.0.optional_dependencies,
        &document.0.peer_dependencies,
    ] {
        for (name, requirement) in entries {
            if let Some(dependency) = parse_requirement(name, requirement) {
                dependencies.push(dependency);
            }
        }
    }

    Ok(dependencies)
}

fn parse_requirement(name: &str, requirement: &str) -> Option<Dependency> {
    if let Some(alias) = requirement.strip_prefix("npm:") {
        let (name, requirement) = match alias.rsplit_once('@') {
            Some((name, requirement)) if !name.is_empty() => (name, requirement),
            _ => (alias, ""),
        };
        return (!name.is_empty()).then(|| Dependency::new(name, requirement));
    }

    if is_non_registry_requirement(requirement) {
        return None;
    }

    Some(Dependency::new(name, requirement))
}

fn is_non_registry_requirement(requirement: &str) -> bool {
    const NON_REGISTRY_PREFIXES: &[&str] = &[
        "file:",
        "link:",
        "workspace:",
        "portal:",
        "patch:",
        "catalog:",
        "github:",
        "gitlab:",
        "bitbucket:",
        "gist:",
        "git:",
    ];
    const LOCAL_PATH_PREFIXES: &[&str] = &["./", "../", "~/", "/"];

    requirement.starts_with("git+")
        || requirement.contains("://")
        || NON_REGISTRY_PREFIXES
            .iter()
            .any(|prefix| requirement.starts_with(prefix))
        || LOCAL_PATH_PREFIXES
            .iter()
            .any(|prefix| requirement.starts_with(prefix))
        || requirement.contains('/')
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;

    use crate::dependency::Dependency;

    use super::{dependencies, is_manifest, parse_requirement};

    #[test]
    fn detects_package_json_manifest() {
        assert!(is_manifest(OsStr::new("package.json")));
    }

    #[test]
    fn rejects_other_manifest_names() {
        assert!(!is_manifest(OsStr::new("package-lock.json")));
        assert!(!is_manifest(OsStr::new("npm-shrinkwrap.json")));
        assert!(!is_manifest(OsStr::new("Package.json")));
    }

    #[test]
    fn reads_dependency_fields_in_fixed_order_and_preserves_entry_order() {
        let contents = r#"{
            "peerDependencies": {"peer-b": "^2", "peer-a": "~1"},
            "dependencies": {"dep-b": "2", "renamed": "npm:actual@^1", "dep-a": "*"},
            "devDependencies": {"dev": ""},
            "optionalDependencies": {"optional": "latest"}
        }"#;

        assert_eq!(
            dependencies(contents).unwrap(),
            vec![
                Dependency::new("dep-b", "2"),
                Dependency::new("actual", "^1"),
                Dependency::new("dep-a", "*"),
                Dependency::new("dev", ""),
                Dependency::new("optional", "latest"),
                Dependency::new("peer-b", "^2"),
                Dependency::new("peer-a", "~1"),
            ]
        );
    }

    #[test]
    fn parses_scoped_and_unscoped_npm_aliases() {
        assert_eq!(
            parse_requirement("local-name", "npm:real-name@^2"),
            Some(Dependency::new("real-name", "^2"))
        );
        assert_eq!(
            parse_requirement("local-name", "npm:@scope/real-name@latest"),
            Some(Dependency::new("@scope/real-name", "latest"))
        );
        assert_eq!(
            parse_requirement("local-name", "npm:@scope/real-name"),
            Some(Dependency::new("@scope/real-name", ""))
        );
        assert_eq!(
            parse_requirement("local-name", "npm:real-name"),
            Some(Dependency::new("real-name", ""))
        );
    }

    #[test]
    fn skips_non_registry_and_local_requirements() {
        for requirement in [
            "file:../package",
            "link:../package",
            "workspace:*",
            "portal:../package",
            "patch:package@npm%3Apackage@^1",
            "catalog:default",
            "https://example.invalid/package.tgz",
            "git+ssh://example.invalid/package.git",
            "github:user/repo",
            "gitlab:user/repo",
            "bitbucket:user/repo",
            "gist:abc123",
            "user/repo",
            "./package",
            "../package",
            "~/package",
            "/absolute/package",
        ] {
            assert_eq!(
                parse_requirement("package", requirement),
                None,
                "{requirement}"
            );
        }
    }

    #[test]
    fn keeps_wildcard_empty_and_dist_tag_requirements() {
        assert_eq!(
            ["*", "", "latest"]
                .map(|requirement| parse_requirement("package", requirement).unwrap()),
            [
                Dependency::new("package", "*"),
                Dependency::new("package", ""),
                Dependency::new("package", "latest"),
            ]
        );
    }

    #[test]
    fn reports_invalid_json_and_dependency_shapes() {
        assert!(dependencies("not json").is_err());
        assert!(dependencies("[]").is_err());
        assert!(
            dependencies(r#"{"dependencies": []}"#)
                .unwrap_err()
                .to_string()
                .contains("invalid type: sequence, expected a map")
        );
        assert!(
            dependencies(r#"{"dependencies": {"package": 1}}"#)
                .unwrap_err()
                .to_string()
                .contains("invalid type: integer `1`, expected a string")
        );
    }

    #[test]
    fn a_manifest_without_dependency_fields_is_empty() {
        assert!(
            dependencies(r#"{"name": "empty-package", "version": "1.0.0"}"#)
                .unwrap()
                .is_empty()
        );
    }
}
