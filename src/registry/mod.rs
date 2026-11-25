// SPDX-FileCopyrightText: 2026 Mike Coats <i.am@mikecoats.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Registry clients that look up the releases published for a package.

mod cache;
mod javascript;
mod python;
mod rust;

pub(crate) use cache::{Cache, CacheError, MAX_AGE};

use std::cell::RefCell;
use std::collections::HashMap;
use std::io;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// The largest index body read from a registry.
///
/// ureq stops at 10 MiB by default, which some popular packages exceed. This
/// leaves plenty of room for growth while still stopping a misbehaving server
/// from exhausting memory.
const MAX_BODY_SIZE: u64 = 64 * 1024 * 1024;

/// How long a single read from a registry may take before the lookup fails.
///
/// This restarts with every read, so a slow but steady download of a large
/// index keeps going, while a stalled connection fails.
const READ_TIMEOUT: Duration = Duration::from_secs(30);

/// How long connecting to a registry, including TLS, may take.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// An upper bound on one attempt at a lookup, as a backstop in case a
/// registry keeps sending data just fast enough to beat [`READ_TIMEOUT`].
const ATTEMPT_TIMEOUT: Duration = Duration::from_mins(5);

/// How many times a lookup is tried before its failure is reported.
const MAX_ATTEMPTS: u32 = 3;

/// The longest Wullie waits for a registry before sending a request.
///
/// A registry that asks, with `Retry-After`, for a longer wait is not sent
/// anything more during this run; its lookups fail with that wait instead.
const MAX_WAIT: Duration = Duration::from_secs(10);

use crate::project::Ecosystem;

/// A published release and whether its registry has yanked it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Release {
    /// The release version as reported by its registry.
    pub version: String,
    /// Whether the release has been yanked.
    pub yanked: bool,
}

/// A source of published releases for an ecosystem.
pub(crate) trait Registry {
    /// Returns all published releases for a package.
    ///
    /// # Errors
    ///
    /// Fails if the name is invalid for its ecosystem, the package isn't
    /// found, or the lookup or its response fails.
    fn releases(&self, ecosystem: Ecosystem, name: &str) -> Result<Vec<Release>, RegistryError>;
}

/// Builds an ecosystem's full index URL.
///
/// Each ecosystem's index rules live in its own module; these functions
/// dispatch to them, like the methods on `Ecosystem` do for manifests.
fn url(ecosystem: Ecosystem, path: &str) -> String {
    match ecosystem {
        Ecosystem::Rust => rust::url(path),
        Ecosystem::Python => python::url(path),
        Ecosystem::JavaScript => javascript::url(path),
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
pub(crate) struct HttpRegistry {
    agent: ureq::Agent,
    cache: Option<Cache>,
    cache_write_error: OnceLock<CacheError>,
    /// What this run has learned about each registry, by host.
    hosts: RefCell<HashMap<String, HostState>>,
}

/// What a run has learned about a registry that limits sending it requests.
#[derive(Debug, Clone, Copy)]
enum HostState {
    /// It asked, with `Retry-After`, not to be sent requests until then.
    WaitUntil(Instant),
    /// It couldn't be reached, so it isn't sent anything more this run.
    Unreachable,
}

impl HttpRegistry {
    /// Creates a client with Wullie's timeouts and user agent.
    ///
    /// Error statuses are returned as responses rather than errors, so
    /// `fetch` can read a `Retry-After` header before retrying.
    pub(crate) fn new(cache: Option<Cache>) -> Self {
        let config = ureq::Agent::config_builder()
            .https_only(true)
            .http_status_as_error(false)
            .timeout_connect(Some(CONNECT_TIMEOUT))
            .timeout_per_read(Some(READ_TIMEOUT))
            .timeout_per_call(Some(ATTEMPT_TIMEOUT))
            .user_agent(format!("wullie/{}", env!("CARGO_PKG_VERSION")))
            .build();

        Self {
            agent: config.into(),
            cache,
            cache_write_error: OnceLock::new(),
            hosts: RefCell::default(),
        }
    }

    /// Returns the first cache write error encountered by this registry.
    pub(crate) fn cache_write_error(&self) -> Option<&CacheError> {
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

    /// Fetches `url`, retrying failures that are likely to be temporary.
    ///
    /// A dropped connection, a timeout, a rate limit (429) or a server error
    /// (500, 502, 503, 504, or 408) is tried again, up to [`MAX_ATTEMPTS`]
    /// times in all. Anything else, such as a missing package, fails at once.
    ///
    /// A `Retry-After` header is always respected: no request is sent to that
    /// registry before the time it gives, for this lookup or any later one.
    /// Without one, attempts are spaced by [`backoff`].
    ///
    /// If the registry can't be reached at all, even after retrying, it is
    /// marked unreachable and every later lookup on it fails at once, rather
    /// than each waiting through the same timeouts and retries.
    fn fetch(&self, url: &str, accept: Option<&str>) -> Result<String, RegistryError> {
        let host = host(url);
        let mut attempt = 1;
        loop {
            self.wait_for(&host)?;
            let failure = match self.fetch_once(url, accept) {
                Ok(body) => return Ok(body),
                Err(failure) => failure,
            };
            if let Some(retry_after) = failure.retry_after {
                self.hosts.borrow_mut().insert(
                    host.clone(),
                    HostState::WaitUntil(Instant::now() + retry_after),
                );
            }
            if !failure.retryable || attempt == MAX_ATTEMPTS {
                if failure.unreachable {
                    self.hosts
                        .borrow_mut()
                        .insert(host.clone(), HostState::Unreachable);
                }
                return Err(failure.error);
            }
            if failure.retry_after.is_none() {
                thread::sleep(backoff(attempt));
            }
            attempt += 1;
        }
    }

    /// Waits until `host` may be sent a request, or fails if its registry
    /// asked for a longer wait than [`MAX_WAIT`] or couldn't be reached.
    fn wait_for(&self, host: &str) -> Result<(), RegistryError> {
        let Some(state) = self.hosts.borrow().get(host).copied() else {
            return Ok(());
        };
        let until = match state {
            HostState::Unreachable => return Err(RegistryError::unreachable(host)),
            HostState::WaitUntil(until) => until,
        };
        let wait = until.saturating_duration_since(Instant::now());
        if wait > MAX_WAIT {
            return Err(RegistryError::rate_limited(wait));
        }
        thread::sleep(wait);
        Ok(())
    }

    fn fetch_once(&self, url: &str, accept: Option<&str>) -> Result<String, Failure> {
        let request = self.agent.get(url);
        let request = if let Some(accept) = accept {
            request.header("Accept", accept)
        } else {
            request
        };
        let response = request.call().map_err(Failure::from)?;
        let status = response.status().as_u16();
        if !response.status().is_success() {
            let retry_after = response
                .headers()
                .get("Retry-After")
                .and_then(|value| value.to_str().ok())
                .and_then(|value| parse_retry_after(value, SystemTime::now()));
            return Err(Failure {
                retry_after,
                ..Failure::from(ureq::Error::StatusCode(status))
            });
        }
        response
            .into_body()
            .with_config()
            .limit(MAX_BODY_SIZE)
            .read_to_string()
            .map_err(Failure::from)
    }
}

/// A failed attempt at a lookup, and whether it is worth trying again.
struct Failure {
    error: RegistryError,
    retryable: bool,
    /// Whether the registry was never reached, so no response came back.
    unreachable: bool,
    retry_after: Option<Duration>,
}

impl From<ureq::Error> for Failure {
    fn from(error: ureq::Error) -> Self {
        let retryable = match &error {
            ureq::Error::StatusCode(status) => matches!(status, 408 | 429 | 500 | 502 | 503 | 504),
            ureq::Error::Io(_) | ureq::Error::ConnectionFailed | ureq::Error::Timeout(_) => true,
            _ => false,
        };
        let unreachable = is_unreachable(&error);
        let error = if matches!(error, ureq::Error::StatusCode(404)) {
            RegistryError::not_found()
        } else {
            RegistryError::http(error)
        };
        Self {
            error,
            retryable,
            unreachable,
            retry_after: None,
        }
    }
}

/// Whether `error` means the registry was never reached: its name didn't
/// resolve, or connecting to it failed or timed out.
///
/// A failure after connecting, such as a reset partway through a download,
/// doesn't count: the registry is up, even if that one lookup failed.
fn is_unreachable(error: &ureq::Error) -> bool {
    match error {
        ureq::Error::HostNotFound | ureq::Error::ConnectionFailed => true,
        ureq::Error::Timeout(timeout) => {
            matches!(timeout, ureq::Timeout::Resolve | ureq::Timeout::Connect)
        }
        ureq::Error::Io(error) => matches!(
            error.kind(),
            io::ErrorKind::ConnectionRefused
                | io::ErrorKind::HostUnreachable
                | io::ErrorKind::NetworkUnreachable
                | io::ErrorKind::NetworkDown
        ),
        _ => false,
    }
}

/// The registry a URL belongs to, by host and port.
fn host(url: &str) -> String {
    url.parse::<ureq::http::Uri>()
        .ok()
        .and_then(|uri| uri.authority().map(ToString::to_string))
        .unwrap_or_else(|| url.to_owned())
}

/// How long to wait before the attempt after `attempt` when the registry
/// didn't say: one second, then two.
fn backoff(attempt: u32) -> Duration {
    Duration::from_secs(1 << (attempt - 1).min(4))
}

/// Reads a `Retry-After` header as a wait from `now`.
///
/// The header is either a number of seconds or an HTTP date, which is in the
/// past or present when there is nothing to wait for. A value in neither form
/// is ignored, and the usual [`backoff`] applies.
fn parse_retry_after(value: &str, now: SystemTime) -> Option<Duration> {
    let value = value.trim();
    if let Ok(seconds) = value.parse() {
        return Some(Duration::from_secs(seconds));
    }
    let date = parse_http_date(value)?;
    Some(date.duration_since(now).unwrap_or(Duration::ZERO))
}

/// Parses an HTTP date in the IMF-fixdate form every server must send, such
/// as `Sun, 06 Nov 1994 08:49:37 GMT` (RFC 9110, section 5.6.7).
fn parse_http_date(value: &str) -> Option<SystemTime> {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];

    let (_weekday, rest) = value.split_once(", ")?;
    let mut parts = rest.split(' ');
    let day: i64 = parts.next()?.parse().ok()?;
    let month_name = parts.next()?;
    let month = MONTHS.iter().position(|name| *name == month_name)?;
    let year: i64 = parts.next()?.parse().ok()?;
    let mut time = parts.next()?.split(':');
    let hour: i64 = time.next()?.parse().ok()?;
    let minute: i64 = time.next()?.parse().ok()?;
    let second: i64 = time.next()?.parse().ok()?;
    if parts.next()? != "GMT"
        || parts.next().is_some()
        || time.next().is_some()
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return None;
    }

    let month = i64::try_from(month).ok()? + 1;
    let days = days_since_epoch(year, month, day);
    let seconds = u64::try_from(days * 86_400 + hour * 3_600 + minute * 60 + second).ok()?;
    Some(UNIX_EPOCH + Duration::from_secs(seconds))
}

/// Days from 1970-01-01 to a date in the proleptic Gregorian calendar, using
/// Howard Hinnant's `days_from_civil` algorithm.
fn days_since_epoch(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let month_from_march = (month + 9) % 12;
    let day_of_year = (153 * month_from_march + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

impl Registry for HttpRegistry {
    fn releases(&self, ecosystem: Ecosystem, name: &str) -> Result<Vec<Release>, RegistryError> {
        let path = index_path(ecosystem, name)?;
        let key = PathBuf::from(ecosystem.to_string()).join(&path);

        // An entry that doesn't parse (cut short, or edited by hand) is a
        // miss, so it is fetched again and overwritten rather than failing
        // every lookup until it expires.
        if let Some(releases) = self
            .cache
            .as_ref()
            .and_then(|cache| cache.get(&key))
            .and_then(|body| parse_index(ecosystem, &body).ok())
        {
            return Ok(releases);
        }

        let body = self.fetch(&url(ecosystem, &path), accept(ecosystem))?;
        let releases = parse_index(ecosystem, &body)?;
        self.store(&key, &body);
        Ok(releases)
    }
}

/// An opaque registry lookup or index parsing error.
#[derive(Debug, thiserror::Error)]
#[error(transparent)]
pub(crate) struct RegistryError(RegistryErrorKind);

#[derive(Debug, thiserror::Error)]
enum RegistryErrorKind {
    #[error("package was not found in the registry")]
    NotFound,
    #[error("invalid package name")]
    InvalidName,
    #[error("registry request failed")]
    Http(#[source] ureq::Error),
    #[error("registry asked not to be sent requests for another {0} seconds")]
    RateLimited(u64),
    #[error("registry {0} couldn't be reached earlier in this run")]
    Unreachable(String),
    #[error("cannot parse registry index data")]
    Index(#[source] serde_json::Error),
}

impl RegistryError {
    pub(crate) fn not_found() -> Self {
        Self(RegistryErrorKind::NotFound)
    }

    /// The registry asked, with `Retry-After`, for a longer wait than Wullie
    /// makes, shown in whole seconds rounded up.
    fn rate_limited(wait: Duration) -> Self {
        let seconds = wait.as_secs() + u64::from(wait.subsec_nanos() > 0);
        Self(RegistryErrorKind::RateLimited(seconds))
    }

    /// A lookup skipped because its registry, `host`, couldn't be reached
    /// earlier in the run.
    pub(crate) fn unreachable(host: &str) -> Self {
        Self(RegistryErrorKind::Unreachable(host.to_owned()))
    }

    /// The registry this lookup was skipped for, if it was skipped because
    /// the registry couldn't be reached earlier in the run.
    pub(crate) fn unreachable_registry(&self) -> Option<&str> {
        match &self.0 {
            RegistryErrorKind::Unreachable(host) => Some(host),
            _ => None,
        }
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
    use std::cell::RefCell;
    use std::error::Error;
    use std::net::TcpListener;
    use std::path::Path;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, OnceLock};
    use std::time::{Duration, Instant, UNIX_EPOCH};

    use tempfile::TempDir;

    use crate::project::Ecosystem;

    use super::{
        Cache, HttpRegistry, MAX_AGE, READ_TIMEOUT, Registry, Release, accept, backoff, index_path,
        is_unreachable, parse_http_date, parse_retry_after, url,
    };

    /// Serves `responses` on a local port, one per connection in order,
    /// returning its URL and a count of the requests received.
    fn serve(responses: Vec<Vec<u8>>) -> (String, Arc<AtomicUsize>) {
        use std::io::{BufRead, BufReader, Write};

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        let requests = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&requests);
        std::thread::spawn(move || {
            for response in responses {
                let (stream, _) = listener.accept().unwrap();
                let mut reader = BufReader::new(stream);
                let mut line = String::new();
                while reader.read_line(&mut line).unwrap() > 2 {
                    line.clear();
                }
                counter.fetch_add(1, Ordering::SeqCst);
                reader.into_inner().write_all(&response).unwrap();
            }
        });
        (url, requests)
    }

    /// An HTTP response that closes its connection, so each request in a
    /// test arrives on a new one.
    fn response(status: &str, headers: &str, body: &[u8]) -> Vec<u8> {
        let mut response = format!(
            "HTTP/1.1 {status}\r\nConnection: close\r\nContent-Length: {}\r\n{headers}\r\n",
            body.len()
        )
        .into_bytes();
        response.extend_from_slice(body);
        response
    }

    /// A registry that talks plain HTTP to local test servers, configured
    /// like the real one otherwise.
    fn local_registry() -> HttpRegistry {
        HttpRegistry {
            agent: ureq::Agent::config_builder()
                .http_status_as_error(false)
                .build()
                .into(),
            cache: None,
            cache_write_error: OnceLock::new(),
            hosts: RefCell::default(),
        }
    }

    #[test]
    fn the_agent_reports_error_statuses_as_responses_and_times_out_reads() {
        let registry = HttpRegistry::new(None);
        let config = registry.agent.config();

        assert!(!config.http_status_as_error());
        assert_eq!(config.timeout_per_read(), Some(READ_TIMEOUT));
    }

    #[test]
    fn retries_a_temporary_failure() {
        let (url, requests) = serve(vec![
            response("503 Service Unavailable", "Retry-After: 0\r\n", b""),
            response("200 OK", "", b"body"),
        ]);

        let body = local_registry().fetch(&url, None).unwrap();

        assert_eq!(body, "body");
        assert_eq!(requests.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn gives_up_after_the_last_attempt() {
        let busy = response("429 Too Many Requests", "Retry-After: 0\r\n", b"");
        let (url, requests) = serve(vec![busy; 3]);

        let error = local_registry().fetch(&url, None).unwrap_err();

        assert_eq!(error.to_string(), "registry request failed");
        assert_eq!(error.source().unwrap().to_string(), "http status: 429");
        assert_eq!(requests.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn does_not_retry_a_missing_package_or_a_client_error() {
        for (status, message) in [
            ("404 Not Found", "package was not found in the registry"),
            ("400 Bad Request", "registry request failed"),
        ] {
            let (url, requests) = serve(vec![response(status, "", b"")]);

            let error = local_registry().fetch(&url, None).unwrap_err();

            assert_eq!(error.to_string(), message);
            assert_eq!(requests.load(Ordering::SeqCst), 1, "{status}");
        }
    }

    #[test]
    fn does_not_retry_sooner_than_retry_after_allows() {
        let (url, requests) = serve(vec![response(
            "503 Service Unavailable",
            "Retry-After: 60\r\n",
            b"",
        )]);
        let registry = local_registry();

        let error = registry.fetch(&url, None).unwrap_err();

        assert_eq!(
            error.to_string(),
            "registry asked not to be sent requests for another 60 seconds"
        );
        assert_eq!(requests.load(Ordering::SeqCst), 1);

        // Later lookups on the same registry aren't sent either.
        let error = registry.fetch(&url, None).unwrap_err();

        assert!(
            error
                .to_string()
                .starts_with("registry asked not to be sent")
        );
        assert_eq!(requests.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn waits_out_a_short_retry_after_before_retrying() {
        let (url, requests) = serve(vec![
            response("429 Too Many Requests", "Retry-After: 1\r\n", b""),
            response("200 OK", "", b"body"),
        ]);
        let started = Instant::now();

        let body = local_registry().fetch(&url, None).unwrap();

        assert_eq!(body, "body");
        assert_eq!(requests.load(Ordering::SeqCst), 2);
        assert!(started.elapsed() >= Duration::from_secs(1));
    }

    #[test]
    fn treats_only_failures_to_reach_a_registry_as_unreachable() {
        use std::io;

        for error in [
            ureq::Error::HostNotFound,
            ureq::Error::ConnectionFailed,
            ureq::Error::Timeout(ureq::Timeout::Connect),
            ureq::Error::Io(io::ErrorKind::ConnectionRefused.into()),
            ureq::Error::Io(io::ErrorKind::NetworkUnreachable.into()),
        ] {
            assert!(is_unreachable(&error), "{error}");
        }
        for error in [
            ureq::Error::StatusCode(503),
            ureq::Error::Timeout(ureq::Timeout::PerRead),
            ureq::Error::Io(io::ErrorKind::ConnectionReset.into()),
        ] {
            assert!(!is_unreachable(&error), "{error}");
        }
    }

    #[test]
    fn stops_sending_requests_to_a_registry_that_cannot_be_reached() {
        // Nothing listens on a port once its listener is dropped, so
        // connecting is refused.
        let closed = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap();
        let url = format!("http://{closed}/");
        let registry = local_registry();

        let first = registry.fetch(&url, None).unwrap_err();
        let started = Instant::now();
        let second = registry.fetch(&url, None).unwrap_err();

        assert_eq!(first.to_string(), "registry request failed");
        assert_eq!(first.unreachable_registry(), None);
        assert_eq!(
            second.unreachable_registry(),
            Some(closed.to_string().as_str())
        );
        assert!(started.elapsed() < Duration::from_millis(500));
    }

    #[test]
    fn reads_retry_after_as_seconds_or_an_http_date() {
        let now = UNIX_EPOCH + Duration::from_secs(784_111_777);

        assert_eq!(parse_retry_after(" 5 ", now), Some(Duration::from_secs(5)));
        assert_eq!(
            parse_retry_after("Sun, 06 Nov 1994 08:50:07 GMT", now),
            Some(Duration::from_secs(30))
        );
        assert_eq!(
            parse_retry_after("Sun, 06 Nov 1994 08:00:00 GMT", now),
            Some(Duration::ZERO)
        );
        assert_eq!(parse_retry_after("soon", now), None);
    }

    #[test]
    fn parses_imf_fixdates() {
        assert_eq!(
            parse_http_date("Sun, 06 Nov 1994 08:49:37 GMT"),
            Some(UNIX_EPOCH + Duration::from_secs(784_111_777))
        );
        assert_eq!(
            parse_http_date("Thu, 01 Jan 1970 00:00:00 GMT"),
            Some(UNIX_EPOCH)
        );
        assert_eq!(
            parse_http_date("Tue, 29 Feb 2028 23:59:59 GMT"),
            Some(UNIX_EPOCH + Duration::from_secs(1_835_481_599))
        );
        for invalid in [
            "Sun, 06 Nov 1994 08:49:37 UTC",
            "Sun, 06 Foo 1994 08:49:37 GMT",
            "Sun, 32 Nov 1994 08:49:37 GMT",
            "Sun, 06 Nov 1994 24:00:00 GMT",
            "Sunday, 06-Nov-94 08:49:37 GMT",
            "Sun Nov  6 08:49:37 1994",
        ] {
            assert_eq!(parse_http_date(invalid), None, "{invalid}");
        }
    }

    #[test]
    fn backs_off_one_second_then_two() {
        assert_eq!(backoff(1), Duration::from_secs(1));
        assert_eq!(backoff(2), Duration::from_secs(2));
    }

    #[test]
    fn the_agent_is_https_only() {
        assert!(HttpRegistry::new(None).agent.config().https_only());
    }

    #[test]
    fn refuses_plain_http_without_connecting() {
        // Nothing accepts on this listener, so a connection would hang until
        // the timeout instead of failing at once.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());

        let error = HttpRegistry::new(None).fetch(&url, None).unwrap_err();

        let source = error.source().expect("a ureq error");
        assert!(
            source.to_string().starts_with("configured for https only"),
            "{source}"
        );
        assert!(listener.accept().is_err(), "a connection was made");
    }

    #[test]
    fn reads_bodies_larger_than_the_ureq_default_limit() {
        let size = 11 * 1024 * 1024;
        let (url, _) = serve(vec![response("200 OK", "", &vec![b'a'; size])]);

        let body = local_registry().fetch(&url, None).unwrap();

        assert_eq!(body.len(), size);
    }

    #[test]
    fn rust_dispatches_to_its_index_rules() {
        assert_eq!(index_path(Ecosystem::Rust, "Serde").unwrap(), "se/rd/serde");
        assert_eq!(
            url(Ecosystem::Rust, "se/rd/serde"),
            "https://index.crates.io/se/rd/serde"
        );
        assert_eq!(accept(Ecosystem::Rust), None);
    }

    #[test]
    fn python_dispatches_to_pypi_index_rules() {
        assert_eq!(index_path(Ecosystem::Python, "Foo_Bar").unwrap(), "foo-bar");
        assert_eq!(
            url(Ecosystem::Python, "foo-bar"),
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
            url(Ecosystem::JavaScript, "@types/node"),
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
    fn unparseable_cache_entries_are_fetched_again() {
        let temp = TempDir::new().unwrap();
        let cache = Cache::new(temp.path().to_path_buf(), MAX_AGE);
        cache
            .put(Path::new("rust/se/rd/serde"), "{\"vers\":")
            .unwrap();
        // Send requests through a local proxy that refuses them, so the fetch
        // fails at once, without retrying or touching the network.
        let (proxy_url, _) = serve(vec![response("403 Forbidden", "", b"")]);
        let proxy = ureq::Proxy::new(proxy_url.trim_end_matches('/')).unwrap();
        let registry = HttpRegistry {
            agent: ureq::Agent::config_builder()
                .proxy(Some(proxy))
                .build()
                .into(),
            cache: Some(cache),
            cache_write_error: OnceLock::new(),
            hosts: RefCell::default(),
        };

        let error = registry.releases(Ecosystem::Rust, "serde").unwrap_err();

        // The damaged entry was skipped and a fetch was attempted, rather
        // than the parse error being returned.
        assert_eq!(error.to_string(), "registry request failed");
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
        // Joined part by part, so the separators match on Windows too.
        let first_path = temp.path().join("blocker").join("first");
        assert!(first_error.contains(&first_path.display().to_string()));

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
