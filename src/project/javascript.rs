// SPDX-FileCopyrightText: 2026 Mike Coats <i.am@mikecoats.com>
// SPDX-License-Identifier: GPL-3.0-or-later

use std::ffi::OsStr;
use std::fmt;
use std::ops::Range as Span;

use indexmap::IndexMap;
use nodejs_semver::{Range, Version};
use serde::de::value::MapAccessDeserializer;
use serde::de::{DeserializeOwned, Error as _, IgnoredAny, MapAccess, Visitor};
use serde::{Deserialize, Deserializer};

use crate::dependency::{Dependency, ParseError};
use crate::registry::Release;
use crate::update::{self, CompareError, Outdated, Update};

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

/// Compares an npm requirement with published releases, using npm's range rules.
///
/// A requirement that is not a range but is a valid dist-tag, such as
/// `latest`, names no version, so it is treated like `*`.
pub fn outdated(requirement: &str, releases: &[Release]) -> Result<Option<Outdated>, CompareError> {
    let range = match Range::parse(requirement) {
        Ok(range) => range,
        Err(_) if is_dist_tag(requirement) => Range::any(),
        Err(source) => return Err(CompareError::invalid_javascript_requirement(source)),
    };
    let versions = update::published(releases, |version| Version::parse(version).ok());
    // npm ranges only match a pre-release when one of their comparators names
    // a pre-release of the same version, so a matching pre-release means the
    // requirement names one.
    let includes_prereleases = versions
        .iter()
        .any(|(version, _)| version.is_prerelease() && range.satisfies(version));
    update::outdated(
        &versions,
        |version| range.satisfies(version),
        |version| includes_prereleases || !version.is_prerelease(),
    )
}

/// Returns whether `requirement` could be a dist-tag: npm requires tags to be
/// URL-safe, and this accepts the conventional ASCII letters, digits, `-`, `.`
/// and `_`.
fn is_dist_tag(requirement: &str) -> bool {
    !requirement.is_empty()
        && requirement
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-._".contains(&byte))
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

/// The `package.json` fields holding dependencies.
const FIELDS: &[&str] = &[
    "dependencies",
    "devDependencies",
    "optionalDependencies",
    "peerDependencies",
];

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

/// Pins matching `package.json` dependencies to their selected release versions.
///
/// Only the matching requirement strings are rewritten, so the rest of the
/// file keeps its formatting, key order and indentation.
pub fn pin(contents: &str, updates: &[Update]) -> Result<String, ParseError> {
    // Report malformed manifests exactly as the Dependency Finder does.
    dependencies(contents)?;

    let mut replacements = Vec::new();
    let mut scanner = Scanner::new(contents);
    scanner.members(|scanner, field| {
        if !FIELDS.contains(&field.as_str()) {
            return scanner.skip_value();
        }
        scanner.members(|scanner, name| {
            let (span, requirement) = scanner.value::<String>()?;
            if let Some(pinned) = pinned_requirement(&name, &requirement, updates) {
                let pinned = serde_json::to_string(&pinned).map_err(ParseError::json)?;
                replacements.push((span, pinned));
            }
            Ok(())
        })
    })?;

    let mut pinned = String::with_capacity(contents.len());
    let mut end = 0;
    for (span, replacement) in replacements {
        pinned.push_str(&contents[end..span.start]);
        pinned.push_str(&replacement);
        end = span.end;
    }
    pinned.push_str(&contents[end..]);
    Ok(pinned)
}

/// Returns the exact requirement for a matching dependency: a bare version,
/// which npm treats as exact, keeping any `npm:` alias prefix.
fn pinned_requirement(name: &str, requirement: &str, updates: &[Update]) -> Option<String> {
    let dependency = parse_requirement(name, requirement)?;
    let update = updates
        .iter()
        .find(|update| update.dependency == dependency)?;
    Some(if requirement.starts_with("npm:") {
        format!("npm:{}@{}", dependency.name, update.versions.latest)
    } else {
        update.versions.latest.clone()
    })
}

/// Walks JSON text, recording where values sit so they can be replaced in place.
///
/// Each value is read by `serde_json`, so strings, escapes and numbers follow
/// its rules; the scanner only steps over the object punctuation between them.
struct Scanner<'a> {
    contents: &'a str,
    position: usize,
}

impl<'a> Scanner<'a> {
    fn new(contents: &'a str) -> Self {
        Self {
            contents,
            position: 0,
        }
    }

    fn skip_whitespace(&mut self) {
        let bytes = self.contents.as_bytes();
        while bytes
            .get(self.position)
            .is_some_and(|byte| b" \t\n\r".contains(byte))
        {
            self.position += 1;
        }
    }

    /// Consumes `byte` if it is the next non-whitespace byte.
    fn eat(&mut self, byte: u8) -> bool {
        self.skip_whitespace();
        let found = self.contents.as_bytes().get(self.position) == Some(&byte);
        if found {
            self.position += 1;
        }
        found
    }

    fn expect(&mut self, byte: u8) -> Result<(), ParseError> {
        if self.eat(byte) {
            Ok(())
        } else {
            Err(ParseError::json(serde_json::Error::custom(format!(
                "expected `{}` at byte {}",
                char::from(byte),
                self.position
            ))))
        }
    }

    /// Reads the next value, returning it with its span in the contents.
    fn value<T: DeserializeOwned>(&mut self) -> Result<(Span<usize>, T), ParseError> {
        self.skip_whitespace();
        let start = self.position;
        let mut stream =
            serde_json::Deserializer::from_str(&self.contents[start..]).into_iter::<T>();
        let value = stream
            .next()
            .ok_or_else(|| ParseError::json(serde_json::Error::custom("expected a value")))?
            .map_err(ParseError::json)?;
        self.position = start + stream.byte_offset();
        Ok((start..self.position, value))
    }

    fn skip_value(&mut self) -> Result<(), ParseError> {
        self.value::<IgnoredAny>().map(drop)
    }

    /// Steps through an object, calling `member` with each key once the scanner
    /// is positioned at that key's value; `member` must consume the value.
    fn members(
        &mut self,
        mut member: impl FnMut(&mut Self, String) -> Result<(), ParseError>,
    ) -> Result<(), ParseError> {
        self.expect(b'{')?;
        if self.eat(b'}') {
            return Ok(());
        }
        loop {
            let (_, key) = self.value::<String>()?;
            self.expect(b':')?;
            member(self, key)?;
            if !self.eat(b',') {
                return self.expect(b'}');
            }
        }
    }
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
    use crate::registry::Release;
    use crate::update::{Outdated, Update};

    use super::{dependencies, is_manifest, outdated, parse_requirement, pin};

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

    fn outdated_versions(lowest: &str, latest: &str) -> Option<Outdated> {
        Some(Outdated {
            lowest: lowest.to_owned(),
            latest: latest.to_owned(),
        })
    }

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

    #[test]
    fn reports_lowest_matching_release_and_latest_for_a_caret_range() {
        assert_eq!(
            outdated(
                "^1.2",
                &[
                    release("1.1.0", false),
                    release("1.2.0", false),
                    release("1.9.0", false),
                ]
            )
            .unwrap(),
            outdated_versions("1.2.0", "1.9.0")
        );
    }

    #[test]
    fn a_bare_version_is_an_exact_requirement_unlike_cargo() {
        assert_eq!(
            outdated("1.2.0", &[release("1.2.0", false), release("1.2.1", false)]).unwrap(),
            outdated_versions("1.2.0", "1.2.1")
        );
        assert_eq!(
            outdated("1.2.1", &[release("1.2.0", false), release("1.2.1", false)]).unwrap(),
            None
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
    fn applies_npm_tilde_x_range_hyphen_and_or_rules() {
        let releases = [
            release("1.2.0", false),
            release("1.2.5", false),
            release("1.3.0", false),
            release("2.0.0", false),
            release("3.0.0", false),
        ];

        assert_eq!(
            outdated("~1.2.3", &releases).unwrap(),
            outdated_versions("1.2.5", "3.0.0")
        );
        assert_eq!(
            outdated("1.3.x", &releases).unwrap(),
            outdated_versions("1.3.0", "3.0.0")
        );
        assert_eq!(
            outdated(">=1.2.1 <2", &releases).unwrap(),
            outdated_versions("1.2.5", "3.0.0")
        );
        assert_eq!(
            outdated("2.0.0 - 3.0.0", &releases).unwrap(),
            outdated_versions("2.0.0", "3.0.0")
        );
        assert_eq!(
            outdated("^3 || ^2", &releases).unwrap(),
            outdated_versions("2.0.0", "3.0.0")
        );
    }

    #[test]
    fn an_unspecified_version_or_dist_tag_matches_every_stable_release() {
        let releases = [
            release("0.9.0", true),
            release("not-a-version", false),
            release("1.0.0-rc.1", false),
            release("1.0.0", false),
            release("1.1.0", false),
            release("2.0.0-beta.1", false),
        ];

        for requirement in ["", "*", "x", "latest", "next"] {
            assert_eq!(
                outdated(requirement, &releases).unwrap(),
                outdated_versions("1.0.0", "1.1.0"),
                "{requirement:?}"
            );
        }
    }

    #[test]
    fn ignores_yanked_releases_for_lowest_and_latest() {
        assert_eq!(
            outdated(
                "^1",
                &[
                    release("1.0.0", true),
                    release("1.0.1", false),
                    release("1.5.0", false),
                    release("2.0.0", true),
                ]
            )
            .unwrap(),
            outdated_versions("1.0.1", "1.5.0")
        );
    }

    #[test]
    fn requirements_naming_a_prerelease_include_prereleases_in_latest() {
        let releases = [
            release("2.0.0-rc.1", false),
            release("2.0.0-rc.2", false),
            release("2.1.0-beta.1", false),
        ];

        assert_eq!(
            outdated("=2.0.0-rc.2", &releases).unwrap(),
            outdated_versions("2.0.0-rc.2", "2.1.0-beta.1")
        );
        assert_eq!(
            outdated(
                "^2.0.0-rc.1",
                &[release("2.0.0-rc.1", false), release("2.0.0", false)]
            )
            .unwrap(),
            outdated_versions("2.0.0-rc.1", "2.0.0")
        );
    }

    #[test]
    fn rejects_invalid_requirements_and_missing_matches() {
        let invalid = outdated(">>2", &[release("2.0.0", false)]).unwrap_err();
        assert_eq!(invalid.to_string(), "invalid version requirement");

        let no_match =
            outdated("^2", &[release("1.9.0", false), release("2.0.0", true)]).unwrap_err();
        assert_eq!(
            no_match.to_string(),
            "no non-yanked release matches the requirement"
        );
    }

    #[test]
    fn pins_every_dependency_field_to_a_bare_exact_version() {
        let contents = r#"{
  "name": "example",
  "dependencies": {"dep": "^1.2", "tagged": "latest"},
  "devDependencies": {"dev": "~2.0.0"},
  "optionalDependencies": {"optional": "*"},
  "peerDependencies": {"peer": ">=3 <4"}
}
"#;
        let updates = [
            update("dep", "^1.2", "1.9.0"),
            update("tagged", "latest", "4.0.0"),
            update("dev", "~2.0.0", "2.1.0"),
            update("optional", "*", "5.0.0"),
            update("peer", ">=3 <4", "3.5.0"),
        ];

        assert_eq!(
            pin(contents, &updates).unwrap(),
            r#"{
  "name": "example",
  "dependencies": {"dep": "1.9.0", "tagged": "4.0.0"},
  "devDependencies": {"dev": "2.1.0"},
  "optionalDependencies": {"optional": "5.0.0"},
  "peerDependencies": {"peer": "3.5.0"}
}
"#
        );
    }

    #[test]
    fn keeps_formatting_other_fields_and_unmatched_or_non_registry_entries() {
        let contents = "{\n\t\"version\" : 1.5e3,\n\t\"scripts\": {\"dependencies\": \"^1\"},\n\t\"nested\": [{\"dependencies\": {\"dep\": \"^1\"}}, null, true],\n\t\"dependencies\" : {\n\t\t\"dep\"  :  \"^1\" ,\n\t\t\"other\": \"^2\",\n\t\t\"local\": \"file:../local\",\n\t\t\"escaped\\u0021\": \"\\u005e3\"\n\t}\n}";
        let updates = [
            update("dep", "^1", "1.4.0"),
            update("local", "file:../local", "9.9.9"),
            update("escaped!", "^3", "3.1.0"),
        ];

        assert_eq!(
            pin(contents, &updates).unwrap(),
            "{\n\t\"version\" : 1.5e3,\n\t\"scripts\": {\"dependencies\": \"^1\"},\n\t\"nested\": [{\"dependencies\": {\"dep\": \"^1\"}}, null, true],\n\t\"dependencies\" : {\n\t\t\"dep\"  :  \"1.4.0\" ,\n\t\t\"other\": \"^2\",\n\t\t\"local\": \"file:../local\",\n\t\t\"escaped\\u0021\": \"3.1.0\"\n\t}\n}"
        );
    }

    #[test]
    fn pins_npm_aliases_keeping_the_alias_prefix() {
        let contents = r#"{"dependencies": {
            "renamed": "npm:actual@^1",
            "scoped": "npm:@scope/actual@~2",
            "bare": "npm:bare-actual"
        }}"#;
        let updates = [
            update("actual", "^1", "1.5.0"),
            update("@scope/actual", "~2", "2.3.0"),
            update("bare-actual", "", "0.4.0"),
        ];

        let pinned = pin(contents, &updates).unwrap();

        assert!(pinned.contains(r#""renamed": "npm:actual@1.5.0""#));
        assert!(pinned.contains(r#""scoped": "npm:@scope/actual@2.3.0""#));
        assert!(pinned.contains(r#""bare": "npm:bare-actual@0.4.0""#));
    }

    #[test]
    fn pins_duplicate_requirements_in_every_field() {
        let contents = r#"{"dependencies": {"shared": "^1"}, "devDependencies": {"shared": "^1", "other": "^1"}, "peerDependencies": {"shared": "^2"}}"#;

        let pinned = pin(contents, &[update("shared", "^1", "2.0.0")]).unwrap();

        assert_eq!(
            pinned,
            r#"{"dependencies": {"shared": "2.0.0"}, "devDependencies": {"shared": "2.0.0", "other": "^1"}, "peerDependencies": {"shared": "^2"}}"#
        );
    }

    #[test]
    fn pin_leaves_manifests_without_matches_or_dependencies_unchanged() {
        for contents in [
            r#"{}"#,
            " { \"name\": \"x\" } \n",
            r#"{"dependencies": {}}"#,
        ] {
            assert_eq!(
                pin(contents, &[update("dep", "^1", "1.0.0")]).unwrap(),
                contents
            );
        }
    }

    #[test]
    fn pin_reports_invalid_json_and_dependency_shapes() {
        assert!(pin("not json", &[]).is_err());
        assert!(pin("[]", &[]).is_err());
        assert!(pin(r#"{"dependencies": {"dep": 1}}"#, &[]).is_err());
    }
}
