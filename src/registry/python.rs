// SPDX-FileCopyrightText: 2026 Mike Coats <i.am@mikecoats.com>
// SPDX-License-Identifier: GPL-3.0-or-later

use std::collections::HashSet;
use std::str::FromStr;

use pep440_rs::Version;
use serde::Deserialize;

use crate::project::Ecosystem;

use super::{RegistryError, Release};

/// The PyPI Simple API endpoint.
pub(super) const INDEX_URL: &str = "https://pypi.org/simple";
/// Requests the JSON representation of the Simple API.
pub(super) const ACCEPT: &str = "application/vnd.pypi.simple.v1+json";

const ARCHIVE_SUFFIXES: &[&str] = &[
    ".tar.gz",
    ".tar.bz2",
    ".tar.xz",
    ".tar.zst",
    ".tar.lz",
    ".tar.lzma",
    ".tar.br",
    ".tgz",
    ".tbz",
    ".tbz2",
    ".txz",
    ".tlz",
    ".zip",
    ".tar",
];

#[derive(Deserialize)]
struct Project {
    versions: Vec<String>,
    files: Vec<File>,
}

#[derive(Deserialize)]
struct File {
    filename: String,
    #[serde(default)]
    yanked: Yanked,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Yanked {
    Flag(bool),
    Reason(#[expect(dead_code, reason = "any reason marks the release as yanked")] String),
}

impl Default for Yanked {
    fn default() -> Self {
        Self::Flag(false)
    }
}

impl Yanked {
    fn is_yanked(&self) -> bool {
        match self {
            Self::Flag(yanked) => *yanked,
            Self::Reason(_) => true,
        }
    }
}

/// Returns whether `name` is a valid PEP 508 project name: ASCII letters and
/// digits, optionally separated by `-`, `_` or `.`, starting and ending with a
/// letter or digit.
///
/// Names come from manifests that may be untrusted, and they become both URL
/// and cache paths, so anything else is rejected before it reaches either.
fn is_valid_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    bytes.first().is_some_and(u8::is_ascii_alphanumeric)
        && bytes.last().is_some_and(u8::is_ascii_alphanumeric)
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_.".contains(byte))
}

/// Returns a project's normalized path within PyPI's Simple API.
pub(super) fn index_path(name: &str) -> Result<String, RegistryError> {
    if !is_valid_name(name) {
        return Err(RegistryError::invalid_name());
    }
    Ok(Ecosystem::Python.normalize_name(name))
}

/// Builds the full PyPI Simple API URL for a normalized package path.
pub(super) fn url(path: &str) -> String {
    format!("{INDEX_URL}/{path}/")
}

/// Parses the JSON Simple API response into releases with per-version yanked status.
pub(super) fn parse_index(body: &str) -> Result<Vec<Release>, RegistryError> {
    let project: Project = serde_json::from_str(body).map_err(RegistryError::index)?;
    let available_versions: HashSet<_> = project
        .files
        .iter()
        .filter(|file| !file.yanked.is_yanked())
        .filter_map(|file| version_from_filename(&file.filename))
        .filter_map(|version| Version::from_str(version).ok())
        .collect();

    Ok(project
        .versions
        .into_iter()
        .map(|version| {
            let yanked = Version::from_str(&version)
                .map(|version| !available_versions.contains(&version))
                .unwrap_or(true);
            Release { version, yanked }
        })
        .collect())
}

fn version_from_filename(filename: &str) -> Option<&str> {
    if let Some(stem) = filename
        .strip_suffix(".whl")
        .or_else(|| filename.strip_suffix(".egg"))
    {
        return stem.split('-').nth(1);
    }

    let stem = ARCHIVE_SUFFIXES
        .iter()
        .find_map(|suffix| filename.strip_suffix(suffix))?;
    stem.rsplit_once('-').map(|(_, version)| version)
}

#[cfg(test)]
mod tests {
    use crate::registry::Release;

    use super::{index_path, parse_index, url};

    #[test]
    fn normalizes_names_in_pypi_paths_and_builds_simple_api_urls() {
        assert_eq!(index_path("Foo_Bar").unwrap(), "foo-bar");
        assert_eq!(index_path("foo.bar").unwrap(), "foo-bar");
        assert_eq!(index_path("zope.interface").unwrap(), "zope-interface");
        assert_eq!(index_path("1password").unwrap(), "1password");
        assert_eq!(url("foo-bar"), "https://pypi.org/simple/foo-bar/");
    }

    #[test]
    fn rejects_names_that_are_not_valid_pep_508_names() {
        for name in [
            "",
            "..",
            "../../etc",
            "a/b",
            "a\\b",
            "-leading",
            "trailing.",
            "with space",
            "nul\0byte",
            "ünïcode",
        ] {
            let error = index_path(name).unwrap_err();
            assert_eq!(error.to_string(), "invalid package name", "{name:?}");
        }
    }

    #[test]
    fn derives_yanked_versions_from_files() {
        assert_eq!(
            parse_index(
                r#"{
                    "versions": ["1.0", "1.1", "1.2", "1.3", "1.4", "1.5", "1.6"],
                    "files": [
                        {"filename": "foo-1.0-py3-none-any.whl"},
                        {"filename": "foo-1.1-py3-none-any.whl", "yanked": true},
                        {"filename": "foo-1.1.tar.gz", "yanked": "withdrawn"},
                        {"filename": "foo-1.2-py3-none-any.whl", "yanked": true},
                        {"filename": "foo-1.2.zip", "yanked": false},
                        {"filename": "foo-bar-1.4.tar.gz"},
                        {"filename": "foo-1.5-py3.egg"},
                        {"filename": "malformed.zip"}
                    ]
                }"#
            )
            .unwrap(),
            vec![
                Release {
                    version: "1.0".to_owned(),
                    yanked: false,
                },
                Release {
                    version: "1.1".to_owned(),
                    yanked: true,
                },
                Release {
                    version: "1.2".to_owned(),
                    yanked: false,
                },
                Release {
                    version: "1.3".to_owned(),
                    yanked: true,
                },
                Release {
                    version: "1.4".to_owned(),
                    yanked: false,
                },
                Release {
                    version: "1.5".to_owned(),
                    yanked: false,
                },
                Release {
                    version: "1.6".to_owned(),
                    yanked: true,
                },
            ]
        );
    }

    #[test]
    fn rejects_malformed_simple_api_json() {
        assert!(parse_index("not json").is_err());
    }
}
