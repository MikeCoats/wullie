// SPDX-FileCopyrightText: 2026 Mike Coats <i.am@mikecoats.com>
// SPDX-License-Identifier: GPL-3.0-or-later

use serde::Deserialize;

use super::{RegistryError, Release};

/// The crates.io sparse index, the endpoint Cargo itself uses.
pub const INDEX_URL: &str = "https://index.crates.io";

/// Builds the full crates.io sparse-index URL for a package path.
pub fn url(path: &str) -> String {
    format!("{INDEX_URL}/{path}")
}

#[derive(Deserialize)]
struct IndexEntry {
    vers: String,
    yanked: bool,
}

/// Returns a crate's path within the sparse index, e.g. `se/rd/serde`.
pub fn index_path(name: &str) -> String {
    let name = crate::project::Ecosystem::Rust.normalize_name(name);
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

/// Parses a sparse-index body, one JSON object per line.
pub fn parse_index(body: &str) -> Result<Vec<Release>, RegistryError> {
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
