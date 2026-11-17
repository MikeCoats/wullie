// SPDX-FileCopyrightText: 2026 Mike Coats <i.am@mikecoats.com>
// SPDX-License-Identifier: GPL-3.0-or-later

use std::time::Duration;

use serde::Deserialize;

use crate::project::Ecosystem;

/// A published release and whether its registry has yanked it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    /// The release version as reported by its registry.
    pub version: String,
    /// Whether the release has been yanked.
    pub yanked: bool,
}

/// A source of published releases for an ecosystem.
pub trait Registry {
    /// Returns all published releases for a package.
    fn releases(&self, ecosystem: Ecosystem, name: &str) -> Result<Vec<Release>, RegistryError>;
}

/// A crates.io registry client using the sparse index.
pub struct CratesIo {
    agent: ureq::Agent,
}

impl CratesIo {
    /// Creates a client with a global request timeout and Wullie user agent.
    pub fn new() -> Self {
        let config = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(30)))
            .user_agent(format!("wullie/{}", env!("CARGO_PKG_VERSION")))
            .build();

        Self {
            agent: config.into(),
        }
    }
}

impl Default for CratesIo {
    fn default() -> Self {
        Self::new()
    }
}

impl Registry for CratesIo {
    fn releases(&self, ecosystem: Ecosystem, name: &str) -> Result<Vec<Release>, RegistryError> {
        if ecosystem != Ecosystem::Rust {
            return Err(RegistryError::unsupported(ecosystem));
        }

        let url = format!("https://index.crates.io/{}", index_path(name));
        let response = match self.agent.get(&url).call() {
            Ok(response) => response,
            Err(ureq::Error::StatusCode(404)) => return Err(RegistryError::not_found()),
            Err(error) => return Err(RegistryError::http(error)),
        };
        let body = response
            .into_body()
            .read_to_string()
            .map_err(RegistryError::http)?;

        parse_index(&body)
    }
}

/// An opaque registry lookup or index parsing error.
#[derive(Debug, thiserror::Error)]
#[error(transparent)]
pub struct RegistryError(RegistryErrorKind);

#[derive(Debug, thiserror::Error)]
enum RegistryErrorKind {
    #[error("crate was not found on crates.io")]
    NotFound,
    #[error("crates.io does not support the {0} ecosystem")]
    Unsupported(Ecosystem),
    #[error("crates.io request failed")]
    Http(#[source] ureq::Error),
    #[error("cannot parse crates.io sparse-index data")]
    Index(#[source] serde_json::Error),
}

impl RegistryError {
    pub(crate) fn not_found() -> Self {
        Self(RegistryErrorKind::NotFound)
    }

    pub(crate) fn unsupported(ecosystem: Ecosystem) -> Self {
        Self(RegistryErrorKind::Unsupported(ecosystem))
    }

    fn http(source: ureq::Error) -> Self {
        Self(RegistryErrorKind::Http(source))
    }

    fn index(source: serde_json::Error) -> Self {
        Self(RegistryErrorKind::Index(source))
    }
}

#[derive(Deserialize)]
struct IndexEntry {
    vers: String,
    yanked: bool,
}

fn index_path(name: &str) -> String {
    let name = name.to_ascii_lowercase();
    let characters: Vec<_> = name.chars().collect();
    match characters.len() {
        0 => String::new(),
        1 => format!("1/{name}"),
        2 => format!("2/{name}"),
        3 => format!("3/{}/{name}", characters[0]),
        _ => {
            let first: String = characters.iter().take(2).copied().collect();
            let second: String = characters.iter().skip(2).take(2).copied().collect();
            format!("{first}/{second}/{name}")
        }
    }
}

fn parse_index(body: &str) -> Result<Vec<Release>, RegistryError> {
    body.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let entry: IndexEntry = serde_json::from_str(line).map_err(RegistryError::index)?;
            Ok(Release {
                version: entry.vers,
                yanked: entry.yanked,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{Release, index_path, parse_index};

    #[test]
    fn builds_crates_io_sparse_index_paths() {
        assert_eq!(index_path("a"), "1/a");
        assert_eq!(index_path("ab"), "2/ab");
        assert_eq!(index_path("abc"), "3/a/abc");
        assert_eq!(index_path("Serde"), "se/rd/serde");
    }

    #[test]
    fn parses_release_versions_and_yanked_flags_from_index_lines() {
        assert_eq!(
            parse_index(
                r#"{"vers":"1.0.0","yanked":false}
                   
                   {"vers":"1.1.0","yanked":true}
                "#
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
            ]
        );
    }

    #[test]
    fn rejects_malformed_index_lines() {
        assert!(parse_index("not json").is_err());
    }
}
