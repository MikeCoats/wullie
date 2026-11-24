// SPDX-FileCopyrightText: 2026 Mike Coats <i.am@mikecoats.com>
// SPDX-License-Identifier: GPL-3.0-or-later

mod cache;
mod javascript;
mod python;
mod rust;

pub use cache::{Cache, CacheError, MAX_AGE};

use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Duration;

/// The largest index body read from a registry.
///
/// ureq stops at 10 MiB by default, which some popular packages pass: npm's
/// abbreviated document for `next` was 25 MB in November 2026, and grows
/// with every canary release. This leaves plenty of room for growth while
/// still stopping a misbehaving server from exhausting memory.
const MAX_BODY_SIZE: u64 = 64 * 1024 * 1024;

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

/// Builds an ecosystem's full index URL.
///
/// Each ecosystem's index rules live in its own module; these functions
/// dispatch to them, like the methods on `Ecosystem` do for manifests.
fn url(ecosystem: Ecosystem, path: &str) -> Result<String, RegistryError> {
    match ecosystem {
        Ecosystem::Rust => Ok(rust::url(path)),
        Ecosystem::Python => Ok(python::url(path)),
        Ecosystem::JavaScript => Ok(javascript::url(path)),
    }
}

/// Returns the ecosystem-specific request media type, if one is needed.
fn accept(ecosystem: Ecosystem) -> Option<&'static str> {
    match ecosystem {
        Ecosystem::Rust => None,
        Ecosystem::Python => Some(python::ACCEPT),
        Ecosystem::JavaScript => Some(javascript::ACCEPT),
    }
}

/// A package's path within its ecosystem's index.
fn index_path(ecosystem: Ecosystem, name: &str) -> Result<String, RegistryError> {
    match ecosystem {
        Ecosystem::Rust => rust::index_path(name),
        Ecosystem::Python => python::index_path(name),
        Ecosystem::JavaScript => javascript::index_path(name),
    }
}

/// Parses an index body into releases.
fn parse_index(ecosystem: Ecosystem, body: &str) -> Result<Vec<Release>, RegistryError> {
    match ecosystem {
        Ecosystem::Rust => rust::parse_index(body),
        Ecosystem::Python => python::parse_index(body),
        Ecosystem::JavaScript => javascript::parse_index(body),
    }
}

/// The real registry client, fetching each ecosystem's index over HTTP.
///
/// With a `Cache`, fresh index bodies are read from disk instead, and fetched
/// bodies are written back.
pub struct HttpRegistry {
    agent: ureq::Agent,
    cache: Option<Cache>,
    cache_write_error: OnceLock<CacheError>,
}

impl HttpRegistry {
    /// Creates a client with a global request timeout and Wullie user agent.
    pub fn new(cache: Option<Cache>) -> Self {
        let config = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(30)))
            .user_agent(format!("wullie/{}", env!("CARGO_PKG_VERSION")))
            .build();

        Self {
            agent: config.into(),
            cache,
            cache_write_error: OnceLock::new(),
        }
    }

    /// Returns the first cache write error encountered by this registry.
    pub fn cache_write_error(&self) -> Option<&CacheError> {
        self.cache_write_error.get()
    }

    fn store(&self, key: &std::path::Path, body: &str) {
        if self.cache_write_error.get().is_some() {
            return;
        }
        if let Some(cache) = &self.cache
            && let Err(error) = cache.put(key, body)
        {
            let _ = self.cache_write_error.set(error);
        }
    }

    fn fetch(&self, url: &str, accept: Option<&str>) -> Result<String, RegistryError> {
        let request = self.agent.get(url);
        let request = if let Some(accept) = accept {
            request.header("Accept", accept)
        } else {
            request
        };
        let response = match request.call() {
            Ok(response) => response,
            Err(ureq::Error::StatusCode(404)) => return Err(RegistryError::not_found()),
            Err(error) => return Err(RegistryError::http(error)),
        };
        response
            .into_body()
            .with_config()
            .limit(MAX_BODY_SIZE)
            .read_to_string()
            .map_err(RegistryError::http)
    }
}

impl Registry for HttpRegistry {
    fn releases(&self, ecosystem: Ecosystem, name: &str) -> Result<Vec<Release>, RegistryError> {
        let path = index_path(ecosystem, name)?;
        let key = PathBuf::from(ecosystem.to_string()).join(&path);

        if let Some(body) = self.cache.as_ref().and_then(|cache| cache.get(&key)) {
            return parse_index(ecosystem, &body);
        }

        let body = self.fetch(&url(ecosystem, &path)?, accept(ecosystem))?;
        let releases = parse_index(ecosystem, &body)?;
        self.store(&key, &body);
        Ok(releases)
    }
}

/// An opaque registry lookup or index parsing error.
#[derive(Debug, thiserror::Error)]
#[error(transparent)]
pub struct RegistryError(RegistryErrorKind);

#[derive(Debug, thiserror::Error)]
enum RegistryErrorKind {
    #[error("package was not found in the registry")]
    NotFound,
    #[error("invalid package name")]
    InvalidName,
    #[error("registry request failed")]
    Http(#[source] ureq::Error),
    #[error("cannot parse registry index data")]
    Index(#[source] serde_json::Error),
}

impl RegistryError {
    pub(crate) fn not_found() -> Self {
        Self(RegistryErrorKind::NotFound)
    }

    fn invalid_name() -> Self {
        Self(RegistryErrorKind::InvalidName)
    }

    fn http(source: ureq::Error) -> Self {
        Self(RegistryErrorKind::Http(source))
    }

    fn index(source: serde_json::Error) -> Self {
        Self(RegistryErrorKind::Index(source))
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use tempfile::TempDir;

    use crate::project::Ecosystem;

    use super::{Cache, HttpRegistry, MAX_AGE, Registry, Release, accept, index_path, url};

    /// Serves one HTTP response with `body` on a local port, returning its URL.
    fn serve_once(body: Vec<u8>) -> String {
        use std::io::{BufRead, BufReader, Write};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream);
            let mut line = String::new();
            while reader.read_line(&mut line).unwrap() > 2 {
                line.clear();
            }
            let mut stream = reader.into_inner();
            let header = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", body.len());
            stream.write_all(header.as_bytes()).unwrap();
            stream.write_all(&body).unwrap();
        });
        url
    }

    #[test]
    fn reads_bodies_larger_than_the_ureq_default_limit() {
        let size = 11 * 1024 * 1024;
        let url = serve_once(vec![b'a'; size]);

        let body = HttpRegistry::new(None).fetch(&url, None).unwrap();

        assert_eq!(body.len(), size);
    }

    #[test]
    fn rust_dispatches_to_its_index_rules() {
        assert_eq!(index_path(Ecosystem::Rust, "Serde").unwrap(), "se/rd/serde");
        assert_eq!(
            url(Ecosystem::Rust, "se/rd/serde").unwrap(),
            "https://index.crates.io/se/rd/serde"
        );
        assert_eq!(accept(Ecosystem::Rust), None);
    }

    #[test]
    fn python_dispatches_to_pypi_index_rules() {
        assert_eq!(index_path(Ecosystem::Python, "Foo_Bar").unwrap(), "foo-bar");
        assert_eq!(
            url(Ecosystem::Python, "foo-bar").unwrap(),
            "https://pypi.org/simple/foo-bar/"
        );
        assert_eq!(
            accept(Ecosystem::Python),
            Some("application/vnd.pypi.simple.v1+json")
        );
    }

    #[test]
    fn javascript_dispatches_to_npm_registry_rules() {
        assert_eq!(
            index_path(Ecosystem::JavaScript, "@types/node").unwrap(),
            "@types/node"
        );
        assert_eq!(
            url(Ecosystem::JavaScript, "@types/node").unwrap(),
            "https://registry.npmjs.org/@types%2fnode"
        );
        assert_eq!(
            accept(Ecosystem::JavaScript),
            Some("application/vnd.npm.install-v1+json")
        );
    }

    #[test]
    fn invalid_names_are_rejected_before_the_cache_or_network() {
        let temp = TempDir::new().unwrap();
        let outside = temp.path().join("outside");
        let cache_dir = temp.path().join("cache");
        let cache = Cache::new(cache_dir, MAX_AGE);
        let registry = HttpRegistry::new(Some(cache));

        for (ecosystem, name) in [
            (Ecosystem::Rust, "../../../outside/se/rd/serde"),
            (Ecosystem::Python, "../outside"),
            (Ecosystem::JavaScript, "../outside"),
            (Ecosystem::JavaScript, "@scope/../../outside"),
        ] {
            let error = registry.releases(ecosystem, name).unwrap_err();
            assert_eq!(error.to_string(), "invalid package name");
        }
        assert!(!outside.exists());
        assert!(registry.cache_write_error().is_none());
    }

    #[test]
    fn fresh_cache_entries_are_used_without_the_network() {
        let temp = TempDir::new().unwrap();
        let cache = Cache::new(temp.path().to_path_buf(), MAX_AGE);
        cache
            .put(
                Path::new("rust/se/rd/serde"),
                "{\"vers\":\"1.0.0\",\"yanked\":false}\n",
            )
            .unwrap();
        let registry = HttpRegistry::new(Some(cache));

        assert_eq!(
            registry.releases(Ecosystem::Rust, "Serde").unwrap(),
            vec![Release {
                version: "1.0.0".to_owned(),
                yanked: false,
            }]
        );
    }

    #[test]
    fn scoped_javascript_packages_are_cached_under_their_scope() {
        let temp = TempDir::new().unwrap();
        let cache = Cache::new(temp.path().to_path_buf(), MAX_AGE);
        cache
            .put(
                Path::new("javascript/@types/node"),
                r#"{"versions":{"22.0.0":{}}}"#,
            )
            .unwrap();
        let registry = HttpRegistry::new(Some(cache));

        assert_eq!(
            registry
                .releases(Ecosystem::JavaScript, "@types/node")
                .unwrap(),
            vec![Release {
                version: "22.0.0".to_owned(),
                yanked: false,
            }]
        );
    }

    #[test]
    fn records_only_the_first_cache_write_error_and_stops_writing() {
        let temp = TempDir::new().unwrap();
        let blocker = temp.path().join("blocker");
        std::fs::write(&blocker, "not a directory").unwrap();
        let cache = Cache::new(temp.path().to_path_buf(), MAX_AGE);
        let registry = HttpRegistry::new(Some(cache));

        registry.store(Path::new("blocker/first"), "first");
        let first_error = registry.cache_write_error().unwrap().to_string();
        assert!(first_error.contains(&temp.path().join("blocker/first").display().to_string()));

        registry.store(Path::new("blocker/second"), "second");
        assert_eq!(
            registry.cache_write_error().unwrap().to_string(),
            first_error
        );

        std::fs::remove_file(blocker).unwrap();
        registry.store(Path::new("later"), "body");
        assert!(!temp.path().join("later").exists());
        assert_eq!(
            registry.cache_write_error().unwrap().to_string(),
            first_error
        );
    }

    #[test]
    fn successful_cache_writes_do_not_record_an_error() {
        let temp = TempDir::new().unwrap();
        let cache = Cache::new(temp.path().to_path_buf(), MAX_AGE);
        let registry = HttpRegistry::new(Some(cache));

        registry.store(Path::new("rust/1/a"), "body");

        assert!(registry.cache_write_error().is_none());
        assert_eq!(
            std::fs::read_to_string(temp.path().join("rust/1/a")).unwrap(),
            "body"
        );
    }
}
