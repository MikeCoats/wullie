// SPDX-FileCopyrightText: 2026 Mike Coats <i.am@mikecoats.com>
// SPDX-License-Identifier: GPL-3.0-or-later

use serde::Deserialize;

use super::{RegistryError, Release};

/// The crates.io sparse index, the endpoint Cargo itself uses.
pub(super) const INDEX_URL: &str = "https://index.crates.io";

/// Builds the full crates.io sparse-index URL for a package path.
pub(super) fn url(path: &str) -> String {
    format!("{INDEX_URL}/{path}")
}

#[derive(Deserialize)]
struct IndexEntry {
    vers: String,
    yanked: bool,
}

/// The longest crate name crates.io accepts.
const MAX_NAME_LEN: usize = 64;

/// Returns whether `name` is a name crates.io accepts: an ASCII letter followed
/// by ASCII letters, digits, `-` or `_`, at most 64 characters long.
///
/// Names come from manifests that may be untrusted, and they become both URL
/// and cache paths, so anything else is rejected before it reaches either.
fn is_valid_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    name.len() <= MAX_NAME_LEN
        && bytes
            .next()
            .is_some_and(|first| first.is_ascii_alphabetic())
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

/// Returns a crate's path within the sparse index, e.g. `se/rd/serde`.
pub(super) fn index_path(name: &str) -> Result<String, RegistryError> {
    if !is_valid_name(name) {
        return Err(RegistryError::invalid_name());
    }
    // Valid names are ASCII, so byte slicing is safe.
    let name = crate::project::Ecosystem::Rust.normalize_name(name);
    Ok(match name.len() {
        1 => format!("1/{name}"),
        2 => format!("2/{name}"),
        3 => format!("3/{}/{name}", &name[..1]),
        _ => format!("{}/{}/{name}", &name[..2], &name[2..4]),
    })
}

/// Parses a sparse-index body, one JSON object per line.
pub(super) fn parse_index(body: &str) -> Result<Vec<Release>, RegistryError> {
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
        assert_eq!(index_path("a").unwrap(), "1/a");
        assert_eq!(index_path("ab").unwrap(), "2/ab");
        assert_eq!(index_path("abc").unwrap(), "3/a/abc");
        assert_eq!(index_path("Serde").unwrap(), "se/rd/serde");
        assert_eq!(index_path("serde_json").unwrap(), "se/rd/serde_json");
        assert_eq!(index_path("tokio-util").unwrap(), "to/ki/tokio-util");
    }

    #[test]
    fn accepts_names_up_to_the_crates_io_length_limit() {
        assert!(index_path(&"a".repeat(64)).is_ok());
        assert!(index_path(&"a".repeat(65)).is_err());
    }

    #[test]
    fn rejects_names_crates_io_would_not_accept() {
        for name in [
            "",
            "../../../../se/rd/serde",
            "se/rd",
            "se\\rd",
            ".hidden",
            "1password",
            "-leading",
            "_leading",
            "with space",
            "dotted.name",
            "nul\0byte",
            "ünïcode",
        ] {
            let error = index_path(name).unwrap_err();
            assert_eq!(error.to_string(), "invalid package name", "{name:?}");
        }
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
