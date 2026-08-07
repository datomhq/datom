//! Fetching data from API data sources.
//!
//! An [`ApiFetcher`] resolves authentication once per data source: secrets
//! (API keys, tokens, OAuth2 client credentials) are read from the
//! environment variables named in the datasource's [`AuthConfig`] — they
//! never appear in configuration files — and, for OAuth2, exchanged for an
//! access token. The resolved material and one HTTP client are then reused
//! for every endpoint fetched from that data source. OAuth2 access tokens
//! are additionally cached for the lifetime of the process, keyed by token
//! endpoint and client id, so constructing another fetcher for the same
//! data source does not re-contact the token endpoint.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use serde::Deserialize;
use url::Url;

use crate::{ApiConfig, AuthConfig, CoreError, Endpoint, Result};

/// Maximum number of characters of an error response included in error messages.
const ERROR_BODY_MAX_CHARS: usize = 256;

/// Timeout for one request.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Deadline for establishing the connection alone, so an unreachable host
/// fails quickly instead of consuming the whole request budget.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// A fetched HTTP response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchResponse {
    /// The response status code (always in the 2xx range, since non-success
    /// statuses are reported as [`CoreError::HttpStatus`]).
    pub status: u16,

    /// The response body, decoded as text.
    pub body: String,

    /// The `Content-Type` header of the response, if present.
    pub content_type: Option<String>,
}

/// Fetches endpoints of one API data source, with authentication resolved
/// once up front and reused for every request.
pub struct ApiFetcher {
    base_url: String,
    client: reqwest::Client,
    auth: ResolvedAuth,
    /// The client's deadline, kept so a timeout can say what elapsed.
    timeout: Duration,
}

impl ApiFetcher {
    /// Resolve authentication for `config`.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::MissingEnvVar`] if a referenced environment
    /// variable is unset, [`CoreError::HttpStatus`] or
    /// [`CoreError::OAuthToken`] for token-endpoint failures, and
    /// [`CoreError::Http`] for transport failures.
    pub async fn new(config: &ApiConfig) -> Result<Self> {
        Self::with_secrets(config, &|var| std::env::var(var).ok()).await
    }

    /// [`ApiFetcher::new`] with an injectable secret source.
    async fn with_secrets(
        config: &ApiConfig,
        secrets: &(dyn Fn(&str) -> Option<String> + Sync),
    ) -> Result<Self> {
        Self::with_secrets_and_timeout(config, secrets, REQUEST_TIMEOUT).await
    }

    async fn with_secrets_and_timeout(
        config: &ApiConfig,
        secrets: &(dyn Fn(&str) -> Option<String> + Sync),
        timeout: Duration,
    ) -> Result<Self> {
        let client = reqwest::Client::builder()
            .timeout(timeout)
            // Never let the connect phase outlive the total deadline.
            .connect_timeout(CONNECT_TIMEOUT.min(timeout))
            .build()?;
        let auth = ResolvedAuth::resolve(&client, &config.auth, secrets, timeout).await?;
        Ok(Self {
            base_url: config.base_url.clone(),
            client,
            auth,
            timeout,
        })
    }

    /// Perform an authed GET against `endpoint`.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::InvalidUrl`] if the base URL and path cannot be
    /// joined, [`CoreError::HttpStatus`] for non-2xx responses, and
    /// [`CoreError::Http`] for transport failures.
    pub async fn fetch_endpoint(&self, endpoint: &Endpoint) -> Result<FetchResponse> {
        let url = join_url(&self.base_url, &endpoint.path)?;
        self.get(url.as_str()).await
    }

    /// Perform an authed GET against the data source's bare base URL.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::HttpStatus`] for non-2xx responses and
    /// [`CoreError::Http`] for transport failures.
    pub async fn fetch_base(&self) -> Result<FetchResponse> {
        self.get(self.base_url.as_str()).await
    }

    /// GET `url` with the resolved auth applied.
    async fn get(&self, url: &str) -> Result<FetchResponse> {
        let response = self
            .auth
            .apply(self.client.get(url))
            .send()
            .await
            .map_err(|err| name_timeout(err, url, self.timeout))?;
        let status = response.status();
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(String::from);
        let body = response
            .text()
            .await
            .map_err(|err| name_timeout(err, url, self.timeout))?;

        if !status.is_success() {
            return Err(CoreError::HttpStatus {
                url: url.to_string(),
                status: status.as_u16(),
                body: error_body_snippet(&body),
            });
        }

        Ok(FetchResponse {
            status: status.as_u16(),
            body,
            content_type,
        })
    }
}

/// Perform a GET request against the bare base URL of `config`,
/// authenticating as its [`AuthConfig`] describes.
///
/// # Errors
///
/// Returns [`CoreError::MissingEnvVar`] if a referenced environment variable
/// is unset, [`CoreError::HttpStatus`] for non-2xx responses (including from
/// the OAuth2 token endpoint), [`CoreError::OAuthToken`] for unusable token
/// responses, and [`CoreError::Http`] for transport failures.
pub async fn fetch_api(config: &ApiConfig) -> Result<FetchResponse> {
    ApiFetcher::new(config).await?.fetch_base().await
}

/// Join an endpoint `path` onto `base_url`.
///
/// A missing trailing slash on the base and any leading slashes on the path
/// are normalized away before [`Url::join`], so `http://h/api` + `/users`,
/// `http://h/api/` + `users`, and the other slash combinations all yield
/// `http://h/api/users`. An empty path yields the base URL itself.
///
/// The result is required to stay under `base_url` — same origin, and a
/// path below the base's. [`Url::join`] otherwise treats an absolute or
/// protocol-relative path as a *replacement*, which would send this data
/// source's credentials to whatever host the path named.
/// [`validate_endpoints`](crate::datasource::validate_endpoints) rejects
/// such paths when a definition file is written or loaded; this is the
/// backstop that keeps the request itself from ever leaving the base URL.
fn join_url(base_url: &str, path: &str) -> Result<Url> {
    let invalid = |url: String, reason: String| CoreError::InvalidUrl { url, reason };

    let mut base =
        Url::parse(base_url).map_err(|err| invalid(base_url.to_string(), err.to_string()))?;

    let relative = path.trim_start_matches('/');
    if relative.is_empty() {
        return Ok(base);
    }

    if !base.path().ends_with('/') {
        base.set_path(&format!("{}/", base.path()));
    }
    let joined = base
        .join(relative)
        .map_err(|err| invalid(format!("{base_url} joined with {path}"), err.to_string()))?;

    if !same_origin(&joined, &base) || !joined.path().starts_with(base.path()) {
        return Err(invalid(
            joined.to_string(),
            format!("endpoint path `{path}` escapes the base URL `{base_url}`"),
        ));
    }
    Ok(joined)
}

/// Report a transport failure, naming a timeout when one elapsed.
fn name_timeout(err: reqwest::Error, url: &str, timeout: Duration) -> CoreError {
    if err.is_timeout() {
        CoreError::HttpTimeout {
            url: url.to_string(),
            timeout: format!("{timeout:?}"),
        }
    } else {
        CoreError::Http(err)
    }
}

/// Whether two URLs address the same origin (scheme, host, and port).
///
/// Compared field by field rather than via [`Url::origin`], which is opaque
/// — and so never equal to itself — for non-special schemes.
fn same_origin(a: &Url, b: &Url) -> bool {
    a.scheme() == b.scheme()
        && a.host_str() == b.host_str()
        && a.port_or_known_default() == b.port_or_known_default()
}

/// Authentication material resolved from the environment.
enum ResolvedAuth {
    /// No authentication.
    None,

    /// A static header (API key).
    Header { name: String, value: String },

    /// A bearer token, static or obtained via OAuth2.
    Bearer(String),
}

impl ResolvedAuth {
    /// Read the secrets `auth` references.
    /// For OAuth2, exchange them for an access token.
    async fn resolve(
        client: &reqwest::Client,
        auth: &AuthConfig,
        secrets: &(dyn Fn(&str) -> Option<String> + Sync),
        timeout: Duration,
    ) -> Result<Self> {
        match auth {
            AuthConfig::None => Ok(Self::None),
            AuthConfig::ApiKey {
                header_name,
                env_var,
            } => Ok(Self::Header {
                name: header_name.clone(),
                value: require_secret(secrets, env_var)?,
            }),
            AuthConfig::Bearer { env_var } => Ok(Self::Bearer(require_secret(secrets, env_var)?)),
            AuthConfig::OAuth2ClientCredentials {
                token_url,
                client_id_env,
                client_secret_env,
                scopes,
            } => Ok(Self::Bearer(
                oauth2_token(
                    client,
                    token_url,
                    secrets,
                    client_id_env,
                    client_secret_env,
                    scopes,
                    timeout,
                )
                .await?,
            )),
        }
    }

    /// Apply this auth to `request`.
    fn apply(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match self {
            Self::None => request,
            Self::Header { name, value } => request.header(name.as_str(), value.as_str()),
            Self::Bearer(token) => request.bearer_auth(token),
        }
    }
}

/// Read the secret named `var`, erroring if it is unset.
fn require_secret(secrets: &(dyn Fn(&str) -> Option<String> + Sync), var: &str) -> Result<String> {
    secrets(var).ok_or_else(|| CoreError::MissingEnvVar(var.to_string()))
}

/// Successful response from an OAuth2 token endpoint.
#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
}

/// Process-lifetime cache of OAuth2 access tokens, keyed by token endpoint
/// and client id.
fn token_cache() -> &'static Mutex<HashMap<String, String>> {
    static CACHE: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Obtain an access token via the OAuth2 client-credentials flow, reusing a
/// previously fetched token when one is cached.
async fn oauth2_token(
    client: &reqwest::Client,
    token_url: &str,
    secrets: &(dyn Fn(&str) -> Option<String> + Sync),
    client_id_env: &str,
    client_secret_env: &str,
    scopes: &[String],
    timeout: Duration,
) -> Result<String> {
    let client_id = require_secret(secrets, client_id_env)?;
    let client_secret = require_secret(secrets, client_secret_env)?;

    let cache_key = format!("{token_url}\u{1f}{client_id}");
    let cached = token_cache().lock().unwrap().get(&cache_key).cloned();
    if let Some(token) = cached {
        return Ok(token);
    }

    let mut form = vec![("grant_type", "client_credentials".to_string())];
    if !scopes.is_empty() {
        form.push(("scope", scopes.join(" ")));
    }

    let response = client
        .post(token_url)
        .basic_auth(&client_id, Some(&client_secret))
        .form(&form)
        .send()
        .await
        .map_err(|err| name_timeout(err, token_url, timeout))?;
    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|err| name_timeout(err, token_url, timeout))?;

    if !status.is_success() {
        return Err(CoreError::HttpStatus {
            url: token_url.to_string(),
            status: status.as_u16(),
            body: error_body_snippet(&body),
        });
    }

    let token: TokenResponse =
        serde_json::from_str(&body).map_err(|err| CoreError::OAuthToken {
            token_url: token_url.to_string(),
            reason: err.to_string(),
        })?;

    token_cache()
        .lock()
        .unwrap()
        .insert(cache_key, token.access_token.clone());
    Ok(token.access_token)
}

/// At most [`ERROR_BODY_MAX_CHARS`] characters of `body`, for error messages.
fn error_body_snippet(body: &str) -> String {
    if body.trim().is_empty() {
        return "(empty body)".to_string();
    }
    let mut snippet: String = body.chars().take(ERROR_BODY_MAX_CHARS).collect();
    if snippet.len() < body.len() {
        snippet.push('…');
    }
    snippet
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Instant;
    use wiremock::matchers::{body_string_contains, header, header_exists, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn no_secrets(_: &str) -> Option<String> {
        None
    }

    fn api(base_url: String, auth: AuthConfig) -> ApiConfig {
        ApiConfig {
            base_url,
            auth,
            endpoints: Vec::new(),
        }
    }

    fn endpoint(name: &str, path: &str) -> Endpoint {
        Endpoint {
            name: name.to_string(),
            path: path.to_string(),
        }
    }

    #[test]
    fn join_url_handles_slash_combinations() {
        let cases = [
            ("http://h/api", "users", "http://h/api/users"),
            ("http://h/api/", "users", "http://h/api/users"),
            ("http://h/api", "/users", "http://h/api/users"),
            ("http://h/api/", "/users", "http://h/api/users"),
            ("http://h", "users", "http://h/users"),
            ("http://h", "/users", "http://h/users"),
            ("http://h/api/", "v2/users", "http://h/api/v2/users"),
            ("http://h/api", "users/", "http://h/api/users/"),
            ("http://h/api", "", "http://h/api"),
            ("http://h/api/", "", "http://h/api/"),
        ];
        for (base, path, expected) in cases {
            assert_eq!(
                join_url(base, path).unwrap().as_str(),
                expected,
                "{base} + {path}"
            );
        }
    }

    #[test]
    fn join_url_rejects_invalid_base() {
        let err = join_url("not a url", "users").unwrap_err();
        assert!(matches!(err, CoreError::InvalidUrl { .. }), "{err}");
    }

    #[test]
    fn join_url_never_leaves_the_base_url() {
        // Each of these tries to point the request — and with it the data
        // source's credentials — somewhere the base URL never named. The
        // invariant is what matters, not which mechanism enforces it: a
        // hostile path is either rejected outright, or defused into a URL
        // that is still under the base. (`//host` takes the second route:
        // the leading-slash trim turns it into an ordinary relative path.)
        let base = "http://api.example/v1";
        let hostile = [
            "http://evil.example/steal",
            "https://evil.example/steal",
            "//evil.example/steal",
            "/../../elsewhere",
            "../../elsewhere",
            "..",
            "\\\\evil.example/steal",
            "/v1/../../elsewhere",
        ];

        for path in hostile {
            let Ok(url) = join_url(base, path) else {
                continue; // rejected outright
            };
            assert_eq!(
                url.host_str(),
                Some("api.example"),
                "`{path}` reached another host as `{url}`"
            );
            assert!(
                url.path().starts_with("/v1/"),
                "`{path}` escaped the base path as `{url}`"
            );
        }
    }

    #[test]
    fn join_url_allows_paths_within_the_base_url() {
        let allowed = [
            (
                "http://api.example/v1",
                "/users",
                "http://api.example/v1/users",
            ),
            (
                "http://api.example/v1",
                "users?page=2",
                "http://api.example/v1/users?page=2",
            ),
            ("http://api.example", "/users", "http://api.example/users"),
        ];
        for (base, path, expected) in allowed {
            assert_eq!(join_url(base, path).unwrap().as_str(), expected, "{path}");
        }
    }

    #[tokio::test]
    async fn fetches_with_no_auth() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string("hello")
                    .insert_header("content-type", "text/plain"),
            )
            .expect(1)
            .mount(&server)
            .await;

        let response = ApiFetcher::with_secrets(&api(server.uri(), AuthConfig::None), &no_secrets)
            .await
            .unwrap()
            .fetch_base()
            .await
            .unwrap();

        assert_eq!(response.status, 200);
        assert_eq!(response.body, "hello");
        assert_eq!(response.content_type.as_deref(), Some("text/plain"));
    }

    #[tokio::test]
    async fn sends_api_key_header() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/"))
            .and(header("x-api-key", "sekrit"))
            .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
            .expect(1)
            .mount(&server)
            .await;

        let config = api(
            server.uri(),
            AuthConfig::ApiKey {
                header_name: "x-api-key".to_string(),
                env_var: "FETCH_TEST_API_KEY".to_string(),
            },
        );
        let secrets = |var: &str| (var == "FETCH_TEST_API_KEY").then(|| "sekrit".to_string());

        let response = ApiFetcher::with_secrets(&config, &secrets)
            .await
            .unwrap()
            .fetch_base()
            .await
            .unwrap();
        assert_eq!(response.body, "ok");
    }

    #[tokio::test]
    async fn sends_bearer_token() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/"))
            .and(header("authorization", "Bearer tok-123"))
            .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
            .expect(1)
            .mount(&server)
            .await;

        let config = api(
            server.uri(),
            AuthConfig::Bearer {
                env_var: "FETCH_TEST_BEARER".to_string(),
            },
        );
        let secrets = |var: &str| (var == "FETCH_TEST_BEARER").then(|| "tok-123".to_string());

        let response = ApiFetcher::with_secrets(&config, &secrets)
            .await
            .unwrap()
            .fetch_base()
            .await
            .unwrap();
        assert_eq!(response.body, "ok");
    }

    #[tokio::test]
    async fn oauth2_exchanges_token_then_calls_api_and_caches_token() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/token"))
            .and(header_exists("authorization"))
            .and(body_string_contains("grant_type=client_credentials"))
            .and(body_string_contains("scope=read+write"))
            .respond_with(
                ResponseTemplate::new(200).set_body_string(r#"{"access_token":"oauth-tok"}"#),
            )
            .expect(1) // the second fetch must reuse the cached token
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/"))
            .and(header("authorization", "Bearer oauth-tok"))
            .respond_with(ResponseTemplate::new(200).set_body_string("data"))
            .expect(2)
            .mount(&server)
            .await;

        let config = api(
            server.uri(),
            AuthConfig::OAuth2ClientCredentials {
                token_url: format!("{}/token", server.uri()),
                client_id_env: "FETCH_TEST_CLIENT_ID".to_string(),
                client_secret_env: "FETCH_TEST_CLIENT_SECRET".to_string(),
                scopes: vec!["read".to_string(), "write".to_string()],
            },
        );
        let secrets = |var: &str| match var {
            "FETCH_TEST_CLIENT_ID" => Some("cid".to_string()),
            "FETCH_TEST_CLIENT_SECRET" => Some("csec".to_string()),
            _ => None,
        };

        for _ in 0..2 {
            let response = ApiFetcher::with_secrets(&config, &secrets)
                .await
                .unwrap()
                .fetch_base()
                .await
                .unwrap();
            assert_eq!(response.body, "data");
        }
    }

    #[tokio::test]
    async fn surfaces_500_with_status_and_truncated_body() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/"))
            .respond_with(ResponseTemplate::new(500).set_body_string("boom ".repeat(200)))
            .mount(&server)
            .await;

        let err = ApiFetcher::with_secrets(&api(server.uri(), AuthConfig::None), &no_secrets)
            .await
            .unwrap()
            .fetch_base()
            .await
            .unwrap_err();

        match err {
            CoreError::HttpStatus { url, status, body } => {
                assert_eq!(url, server.uri());
                assert_eq!(status, 500);
                assert!(body.starts_with("boom "));
                assert!(body.ends_with('…'));
                assert_eq!(body.chars().count(), ERROR_BODY_MAX_CHARS + 1);
            }
            other => panic!("unexpected error: {other}"),
        }
    }

    #[tokio::test]
    async fn endpoint_fetches_share_one_resolved_auth() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/users"))
            .and(header("authorization", "Bearer tok-1"))
            .respond_with(ResponseTemplate::new(200).set_body_string("users"))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/v1/orders"))
            .and(header("authorization", "Bearer tok-1"))
            .respond_with(ResponseTemplate::new(200).set_body_string("orders"))
            .expect(1)
            .mount(&server)
            .await;

        let config = api(
            server.uri(),
            AuthConfig::Bearer {
                env_var: "FETCH_TEST_SHARED".to_string(),
            },
        );
        let reads = AtomicUsize::new(0);
        let secrets = |var: &str| {
            reads.fetch_add(1, Ordering::SeqCst);
            (var == "FETCH_TEST_SHARED").then(|| "tok-1".to_string())
        };

        let fetcher = ApiFetcher::with_secrets(&config, &secrets).await.unwrap();
        // One path with a leading slash, one without: both join correctly.
        let users = fetcher
            .fetch_endpoint(&endpoint("users", "/v1/users"))
            .await
            .unwrap();
        let orders = fetcher
            .fetch_endpoint(&endpoint("orders", "v1/orders"))
            .await
            .unwrap();

        assert_eq!(users.body, "users");
        assert_eq!(orders.body, "orders");
        assert_eq!(
            reads.load(Ordering::SeqCst),
            1,
            "secrets must be resolved once per data source, not per endpoint"
        );
    }

    #[tokio::test]
    async fn oauth2_token_fetched_once_across_endpoint_fetches() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/token"))
            .and(header_exists("authorization"))
            .and(body_string_contains("grant_type=client_credentials"))
            .respond_with(
                ResponseTemplate::new(200).set_body_string(r#"{"access_token":"once-tok"}"#),
            )
            .expect(1) // a single exchange must cover every endpoint fetch
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/a"))
            .and(header("authorization", "Bearer once-tok"))
            .respond_with(ResponseTemplate::new(200).set_body_string("a"))
            .expect(2)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/b"))
            .and(header("authorization", "Bearer once-tok"))
            .respond_with(ResponseTemplate::new(200).set_body_string("b"))
            .expect(1)
            .mount(&server)
            .await;

        let config = api(
            server.uri(),
            AuthConfig::OAuth2ClientCredentials {
                token_url: format!("{}/token", server.uri()),
                client_id_env: "FETCH_ONCE_CLIENT_ID".to_string(),
                client_secret_env: "FETCH_ONCE_CLIENT_SECRET".to_string(),
                scopes: Vec::new(),
            },
        );
        let secrets = |var: &str| match var {
            "FETCH_ONCE_CLIENT_ID" => Some("cid".to_string()),
            "FETCH_ONCE_CLIENT_SECRET" => Some("csec".to_string()),
            _ => None,
        };

        let fetcher = ApiFetcher::with_secrets(&config, &secrets).await.unwrap();
        fetcher.fetch_endpoint(&endpoint("a", "/a")).await.unwrap();
        fetcher.fetch_endpoint(&endpoint("b", "/b")).await.unwrap();

        // A second fetcher for the same data source reuses the process-wide
        // token cache instead of contacting the token endpoint again.
        let second = ApiFetcher::with_secrets(&config, &secrets).await.unwrap();
        second.fetch_endpoint(&endpoint("a", "/a")).await.unwrap();
    }

    #[tokio::test]
    async fn fetch_endpoint_joins_base_path_and_endpoint_path() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v2/users"))
            .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
            .expect(1)
            .mount(&server)
            .await;

        // Base URL without a trailing slash, path with a leading slash.
        let config = api(format!("{}/api", server.uri()), AuthConfig::None);

        let response = ApiFetcher::with_secrets(&config, &no_secrets)
            .await
            .unwrap()
            .fetch_endpoint(&endpoint("users", "/v2/users"))
            .await
            .unwrap();

        assert_eq!(response.body, "ok");
    }

    #[tokio::test]
    async fn a_stalled_response_times_out_instead_of_hanging() {
        let server = MockServer::start().await;
        // Accepts the request, then never answers in time.
        Mock::given(method("GET"))
            .and(path("/"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string("too late")
                    .set_delay(Duration::from_secs(30)),
            )
            .mount(&server)
            .await;

        let started = Instant::now();
        let err = ApiFetcher::with_secrets_and_timeout(
            &api(server.uri(), AuthConfig::None),
            &no_secrets,
            Duration::from_millis(150),
        )
        .await
        .unwrap()
        .fetch_base()
        .await
        .unwrap_err();

        assert!(matches!(err, CoreError::HttpTimeout { .. }), "{err}");
        assert!(err.to_string().contains("timed out after 150ms"), "{err}");
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "gave up after {:?}, so the deadline did not apply",
            started.elapsed()
        );
    }

    #[tokio::test]
    async fn the_oauth2_token_request_is_also_bounded() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(30)))
            .mount(&server)
            .await;

        let config = api(
            server.uri(),
            AuthConfig::OAuth2ClientCredentials {
                token_url: format!("{}/token", server.uri()),
                client_id_env: "FETCH_TIMEOUT_CLIENT_ID".to_string(),
                client_secret_env: "FETCH_TIMEOUT_CLIENT_SECRET".to_string(),
                scopes: Vec::new(),
            },
        );
        let secrets = |var: &str| match var {
            "FETCH_TIMEOUT_CLIENT_ID" => Some("cid".to_string()),
            "FETCH_TIMEOUT_CLIENT_SECRET" => Some("csec".to_string()),
            _ => None,
        };

        let started = Instant::now();
        // A stalled token endpoint must not hang auth resolution either.
        let err =
            ApiFetcher::with_secrets_and_timeout(&config, &secrets, Duration::from_millis(150))
                .await
                .err()
                .expect("a stalled token endpoint must fail");

        assert!(matches!(err, CoreError::HttpTimeout { .. }), "{err}");
        assert!(err.to_string().contains("/token"), "{err}");
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[tokio::test]
    async fn missing_env_var_error_names_the_variable() {
        let config = api(
            "http://localhost:9".to_string(),
            AuthConfig::Bearer {
                env_var: "FETCH_TEST_ABSENT".to_string(),
            },
        );

        // `.err()` instead of `.unwrap_err()`: ApiFetcher deliberately does
        // not implement Debug, so resolved secrets can never be printed.
        let err = ApiFetcher::with_secrets(&config, &no_secrets)
            .await
            .err()
            .expect("resolving auth without the env var must fail");

        assert!(
            matches!(err, CoreError::MissingEnvVar(ref var) if var == "FETCH_TEST_ABSENT"),
            "{err}"
        );
        assert!(err.to_string().contains("FETCH_TEST_ABSENT"));
    }
}
