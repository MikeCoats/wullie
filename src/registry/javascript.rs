// SPDX-FileCopyrightText: 2026 Mike Coats <i.am@mikecoats.com>
// SPDX-License-Identifier: GPL-3.0-or-later

use indexmap::IndexMap;
use serde::Deserialize;

use crate::project::Ecosystem;

use super::{RegistryError, Release};

/// The public npm registry, the endpoint npm itself uses.
pub const INDEX_URL: &str = "https://registry.npmjs.org";
/// Requests the abbreviated package document npm uses for installs, which is
/// far smaller than the full document and still lists every version.
pub const ACCEPT: &str = "application/vnd.npm.install-v1+json";

/// The longest package name, including any scope, the npm registry accepts.
const MAX_NAME_LEN: usize = 214;

#[derive(Deserialize)]
struct Package {
    /// Fully unpublished packages have no `versions` field.
    #[serde(default)]
    versions: IndexMap<String, Manifest>,
}

#[derive(Deserialize)]
struct Manifest {
    #[serde(default)]
    deprecated: Option<serde_json::Value>,
}

impl Manifest {
    /// npm un-deprecates a version by setting an empty message.
    fn is_deprecated(&self) -> bool {
        self.deprecated
            .as_ref()
            .and_then(serde_json::Value::as_str)
            .is_some_and(|message| !message.is_empty())
    }
}

/// Returns whether `part` is a valid package name or scope: ASCII letters,
/// digits, `-`, `.`, `_` or `~`, not starting with `.` or `_`.
fn is_valid_part(part: &str) -> bool {
    let bytes = part.as_bytes();
    bytes.first().is_some_and(|first| !b"._".contains(first))
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-._~".contains(byte))
}

/// Returns whether `name` is a package name the npm registry accepts: a name,
/// or `@scope/name`, at most 214 characters long.
///
/// Legacy packages may contain capital letters, so they are accepted.
/// Names come from manifests that may be untrusted, and they become both URL
/// and cache paths, so anything else is rejected before it reaches either.
fn is_valid_name(name: &str) -> bool {
    if name.len() > MAX_NAME_LEN {
        return false;
    }
    match name.strip_prefix('@') {
        Some(scoped) => scoped
            .split_once('/')
            .is_some_and(|(scope, name)| is_valid_part(scope) && is_valid_part(name)),
        None => is_valid_part(name),
    }
}

/// Returns a package's path within the npm registry, e.g. `@types/node`.
pub fn index_path(name: &str) -> Result<String, RegistryError> {
    if !is_valid_name(name) {
        return Err(RegistryError::invalid_name());
    }
    Ok(Ecosystem::JavaScript.normalize_name(name))
}

/// Builds the full npm registry URL for a package path, encoding a scoped
/// package's `/` as npm does.
pub fn url(path: &str) -> String {
    format!("{INDEX_URL}/{}", path.replace('/', "%2f"))
}

/// Parses a package document into releases, treating deprecated versions as yanked.
pub fn parse_index(body: &str) -> Result<Vec<Release>, RegistryError> {
    let package: Package = serde_json::from_str(body).map_err(RegistryError::index)?;
    Ok(package
        .versions
        .into_iter()
        .map(|(version, manifest)| Release {
            yanked: manifest.is_deprecated(),
            version,
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use crate::registry::Release;

    use super::{index_path, parse_index, url};

    #[test]
    fn keeps_names_unchanged_in_paths_and_encodes_scoped_urls() {
        assert_eq!(index_path("left-pad").unwrap(), "left-pad");
        assert_eq!(index_path("JSONStream").unwrap(), "JSONStream");
        assert_eq!(index_path("@types/node").unwrap(), "@types/node");
        assert_eq!(index_path("lodash.merge").unwrap(), "lodash.merge");
        assert_eq!(url("left-pad"), "https://registry.npmjs.org/left-pad");
        assert_eq!(
            url("@types/node"),
            "https://registry.npmjs.org/@types%2fnode"
        );
    }

    #[test]
    fn accepts_names_up_to_the_npm_length_limit() {
        assert!(index_path(&"a".repeat(214)).is_ok());
        assert!(index_path(&"a".repeat(215)).is_err());
        assert!(index_path(&format!("@scope/{}", "a".repeat(207))).is_ok());
        assert!(index_path(&format!("@scope/{}", "a".repeat(208))).is_err());
    }

    #[test]
    fn rejects_names_npm_would_not_accept() {
        for name in [
            "",
            ".",
            "..",
            "../outside",
            ".hidden",
            "_leading",
            "a/b",
            "a\\b",
            "@",
            "@scope",
            "@scope/",
            "@/name",
            "@../outside",
            "@scope/../outside",
            "@scope/name/extra",
            "@scope/.hidden",
            "with space",
            "nul\0byte",
            "ünïcode",
        ] {
            let error = index_path(name).unwrap_err();
            assert_eq!(error.to_string(), "invalid package name", "{name:?}");
        }
    }

    #[test]
    fn parses_versions_in_order_and_treats_deprecated_versions_as_yanked() {
        assert_eq!(
            parse_index(
                r#"{
                    "name": "example",
                    "dist-tags": {"latest": "1.3.0"},
                    "versions": {
                        "1.0.0": {"name": "example", "version": "1.0.0"},
                        "1.1.0": {"deprecated": "use 1.2.0"},
                        "1.2.0": {"deprecated": ""},
                        "1.3.0": {"deprecated": null}
                    }
                }"#
            )
            .unwrap(),
            vec![
                Release {
                    version: "1.0.0".to_owned(),
                    yanked: false,
                },
                Release {
                    version: "1.1.0".to_owned(),
                    yanked: true,
                },
                Release {
                    version: "1.2.0".to_owned(),
                    yanked: false,
                },
                Release {
                    version: "1.3.0".to_owned(),
                    yanked: false,
                },
            ]
        );
    }

    #[test]
    fn a_fully_unpublished_package_has_no_releases() {
        assert!(
            parse_index(r#"{"name": "gone", "time": {"unpublished": {}}}"#)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn rejects_malformed_package_documents() {
        assert!(parse_index("not json").is_err());
        assert!(parse_index(r#"{"versions": []}"#).is_err());
    }
}
