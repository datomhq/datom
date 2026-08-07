//! Data source configuration model and on-disk persistence.
//!
//! Each data source is stored as a TOML file at
//! `datasources/<name>.datom.toml` inside a project. Secrets are never
//! written to these files: auth configurations reference the *names* of
//! environment variables that hold the secret material.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{CoreError, Result};

/// Directory inside a project that holds data source definitions.
pub const DATASOURCES_DIR: &str = "datasources";

/// File-name suffix for data source definition files.
pub const DATASOURCE_FILE_SUFFIX: &str = ".datom.toml";

/// A configured data source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Datasource {
    /// Unique name within a project.
    pub name: String,

    /// The kind of source this is, with its kind-specific configuration.
    #[serde(flatten)]
    pub kind: DatasourceKind,
}

/// Kind-specific configuration for a data source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DatasourceKind {
    /// An HTTP API.
    Api(ApiConfig),
}

/// Configuration for an HTTP API data source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiConfig {
    /// Base URL that requests are made against.
    pub base_url: String,

    /// How to authenticate against the API.
    pub auth: AuthConfig,

    /// The endpoints this API exposes, each destined to become one table.
    pub endpoints: Vec<Endpoint>,
}

/// One endpoint of an API data source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Endpoint {
    /// Unique name within the data source.
    pub name: String,

    /// Path joined onto the data source's base URL.
    pub path: String,
}

/// How to authenticate against an API.
///
/// Variants never hold secret values, only the names of environment
/// variables to read them from at run time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AuthConfig {
    /// No authentication.
    None,

    /// A static API key sent in a request header.
    ApiKey {
        /// Header the key is sent in (e.g. `X-Api-Key`).
        header_name: String,
        /// Environment variable holding the key.
        env_var: String,
    },

    /// A bearer token sent in the `Authorization` header.
    Bearer {
        /// Environment variable holding the token.
        env_var: String,
    },

    /// The OAuth2 client-credentials flow.
    #[serde(rename = "oauth2_client_credentials")]
    OAuth2ClientCredentials {
        /// Endpoint tokens are requested from.
        token_url: String,
        /// Environment variable holding the client id.
        client_id_env: String,
        /// Environment variable holding the client secret.
        client_secret_env: String,
        /// Scopes to request.
        scopes: Vec<String>,
    },
}

/// Path of the definition file for data source `name` inside `project_root`.
pub fn datasource_path(project_root: impl AsRef<Path>, name: &str) -> PathBuf {
    project_root
        .as_ref()
        .join(DATASOURCES_DIR)
        .join(format!("{name}{DATASOURCE_FILE_SUFFIX}"))
}

/// Write `datasource` to `datasources/<name>.datom.toml` inside
/// `project_root`, creating the directory if needed and overwriting any
/// existing definition. Returns the path written.
///
/// # Errors
///
/// Returns [`CoreError::InvalidDataSourceName`] if the name is empty or
/// contains path separators, [`CoreError::InvalidEndpoint`] if any endpoint
/// is invalid or a name is duplicated, or [`CoreError::Io`] /
/// [`CoreError::DataSourceSerialize`] on filesystem or encoding failures.
pub fn save_datasource(project_root: impl AsRef<Path>, datasource: &Datasource) -> Result<PathBuf> {
    validate_name(&datasource.name)?;
    let DatasourceKind::Api(api) = &datasource.kind;
    validate_auth(&datasource.name, &api.auth)?;
    validate_endpoints(&datasource.name, &api.endpoints)?;

    let contents =
        toml::to_string_pretty(datasource).map_err(|source| CoreError::DataSourceSerialize {
            name: datasource.name.clone(),
            source,
        })?;

    let dir = project_root.as_ref().join(DATASOURCES_DIR);
    fs::create_dir_all(&dir)?;

    let path = datasource_path(project_root, &datasource.name);
    fs::write(&path, contents)?;
    Ok(path)
}

/// Load the data source named `name` from `project_root`.
///
/// # Errors
///
/// Returns [`CoreError::DataSourceNotFound`] if no definition file exists,
/// [`CoreError::DataSourceParse`] if the file is not valid TOML,
/// [`CoreError::DataSourceNameMismatch`] if the file's `name` field disagrees
/// with the file name, or [`CoreError::InvalidEndpoint`] if any endpoint is
/// invalid or a name is duplicated.
pub fn load_datasource(project_root: impl AsRef<Path>, name: &str) -> Result<Datasource> {
    validate_name(name)?;
    let path = datasource_path(project_root, name);

    let contents = match fs::read_to_string(&path) {
        Ok(contents) => contents,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Err(CoreError::DataSourceNotFound(name.to_string()));
        }
        Err(err) => return Err(err.into()),
    };

    parse_datasource(&path, &contents, name)
}

/// Everything found in a project's `datasources/` directory.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DatasourceListing {
    /// Definitions that loaded, sorted by name.
    pub datasources: Vec<Datasource>,

    /// Files that could not be loaded, as `(name, reason)`, sorted by name.
    pub invalid: Vec<(String, String)>,
}

/// List every data source defined in `project_root`, sorted by name.
///
/// Scans `datasources/` for `*.datom.toml` files; other entries are ignored.
/// A missing `datasources/` directory yields an empty listing.
///
/// # Errors
///
/// Returns [`CoreError::Io`] if the directory cannot be read. A file that
/// cannot be parsed or validated lands in
/// [`invalid`](DatasourceListing::invalid) instead of failing the call.
pub fn list_datasources(project_root: impl AsRef<Path>) -> Result<DatasourceListing> {
    let dir = project_root.as_ref().join(DATASOURCES_DIR);

    let entries = match fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Ok(DatasourceListing::default());
        }
        Err(err) => return Err(err.into()),
    };

    let mut listing = DatasourceListing::default();
    for entry in entries {
        let path = entry?.path();
        let Some(file_name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let Some(stem) = file_name.strip_suffix(DATASOURCE_FILE_SUFFIX) else {
            continue;
        };
        if stem.is_empty() || !path.is_file() {
            continue;
        }

        match fs::read_to_string(&path).map_err(CoreError::from) {
            Ok(contents) => match parse_datasource(&path, &contents, stem) {
                Ok(datasource) => listing.datasources.push(datasource),
                Err(err) => listing
                    .invalid
                    .push((stem.to_string(), crate::error_chain(&err))),
            },
            Err(err) => listing
                .invalid
                .push((stem.to_string(), crate::error_chain(&err))),
        }
    }

    listing.datasources.sort_by(|a, b| a.name.cmp(&b.name));
    listing.invalid.sort();
    Ok(listing)
}

/// Add `endpoint` to the `datasource`.
///
/// # Errors
///
/// Returns the errors of [`load_datasource`], or
/// [`CoreError::InvalidEndpoint`] if the endpoint's name is invalid.
pub fn add_endpoint(
    project_root: impl AsRef<Path>,
    datasource: &str,
    endpoint: Endpoint,
) -> Result<()> {
    let root = project_root.as_ref();
    let mut loaded = load_datasource(root, datasource)?;
    let DatasourceKind::Api(api) = &mut loaded.kind;
    api.endpoints.push(endpoint);
    save_datasource(root, &loaded)?;
    Ok(())
}

/// Remove `endpoint` from the `datasource`.
///
/// # Errors
///
/// Returns the errors of [`load_datasource`], or
/// [`CoreError::EndpointNotFound`] if no such endpoint exists.
pub fn remove_endpoint(
    project_root: impl AsRef<Path>,
    datasource: &str,
    endpoint: &str,
) -> Result<()> {
    let root = project_root.as_ref();
    let mut loaded = load_datasource(root, datasource)?;
    let DatasourceKind::Api(api) = &mut loaded.kind;
    let before = api.endpoints.len();
    api.endpoints.retain(|e| e.name != endpoint);
    if api.endpoints.len() == before {
        return Err(CoreError::EndpointNotFound {
            datasource: datasource.to_string(),
            name: endpoint.to_string(),
        });
    }
    save_datasource(root, &loaded)?;
    Ok(())
}

/// Parse a definition file's contents, checking that the declared name
/// matches the name implied by the file path and that its endpoints are
/// valid.
fn parse_datasource(path: &Path, contents: &str, expected_name: &str) -> Result<Datasource> {
    let datasource: Datasource =
        toml::from_str(contents).map_err(|source| CoreError::DataSourceParse {
            path: path.display().to_string(),
            source,
        })?;

    if datasource.name != expected_name {
        return Err(CoreError::DataSourceNameMismatch {
            path: path.display().to_string(),
            expected: expected_name.to_string(),
            found: datasource.name,
        });
    }

    let DatasourceKind::Api(api) = &datasource.kind;
    validate_auth(&datasource.name, &api.auth)?;
    validate_endpoints(&datasource.name, &api.endpoints)?;

    Ok(datasource)
}

/// Validate the `endpoints` of the data source named `datasource`: every
/// endpoint name must be a valid identifier (non-empty lowercase ASCII
/// letters, digits, and underscores) and unique within the data source, and
/// every path must stay under the data source's base URL.
///
/// # Errors
///
/// Returns [`CoreError::InvalidEndpoint`] describing the first violation.
pub fn validate_endpoints(datasource: &str, endpoints: &[Endpoint]) -> Result<()> {
    let invalid = |name: &str, reason: String| CoreError::InvalidEndpoint {
        datasource: datasource.to_string(),
        name: name.to_string(),
        reason,
    };

    let mut seen = std::collections::HashSet::new();
    for endpoint in endpoints {
        if let Some(problem) = identifier_error(&endpoint.name) {
            return Err(invalid(&endpoint.name, format!("endpoint name {problem}")));
        }
        if let Some(problem) = path_error(&endpoint.path) {
            return Err(invalid(
                &endpoint.name,
                format!("path `{}` {problem}", endpoint.path),
            ));
        }
        if !seen.insert(endpoint.name.as_str()) {
            return Err(invalid(
                &endpoint.name,
                "duplicate endpoint name".to_string(),
            ));
        }
    }
    Ok(())
}

/// The environment variables `auth` names, as `(field, variable)`.
pub(crate) fn named_env_vars(auth: &AuthConfig) -> Vec<(&'static str, &str)> {
    match auth {
        AuthConfig::None => Vec::new(),
        AuthConfig::ApiKey { env_var, .. } | AuthConfig::Bearer { env_var } => {
            vec![("env_var", env_var.as_str())]
        }
        AuthConfig::OAuth2ClientCredentials {
            client_id_env,
            client_secret_env,
            ..
        } => vec![
            ("client_id_env", client_id_env.as_str()),
            ("client_secret_env", client_secret_env.as_str()),
        ],
    }
}

/// Why `name` cannot be an environment variable name, if it cannot.
fn env_var_name_error(name: &str) -> Option<&'static str> {
    let mut chars = name.chars();
    let first_ok = chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_');
    if first_ok && chars.all(|c| c.is_ascii_alphanumeric() || c == '_') {
        None
    } else {
        Some("must match [A-Za-z_][A-Za-z0-9_]*")
    }
}

/// Validate the authentication configuration of the data source named
/// `datasource`.
///
/// # Errors
///
/// Returns [`CoreError::InvalidAuth`] describing the violation.
pub fn validate_auth(datasource: &str, auth: &AuthConfig) -> Result<()> {
    // A name that cannot be an environment variable can never be read, so
    // the data source would fail at request time with an empty or mangled
    // name in the message instead of here, where the file is in hand.
    for (field, var) in named_env_vars(auth) {
        if let Some(problem) = env_var_name_error(var) {
            return Err(CoreError::InvalidAuth {
                datasource: datasource.to_string(),
                reason: format!("`{field}` names `{var}`, which {problem}"),
            });
        }
    }

    // The client id and secret are different credentials.
    if let AuthConfig::OAuth2ClientCredentials {
        client_id_env,
        client_secret_env,
        ..
    } = auth
        && client_id_env == client_secret_env
    {
        return Err(CoreError::InvalidAuth {
            datasource: datasource.to_string(),
            reason: format!(
                "`client_id_env` and `client_secret_env` both name `{client_id_env}`; \
                 the client id and secret must come from different environment variables"
            ),
        });
    }
    Ok(())
}

/// Message for the commands that cannot do anything without an endpoint,
/// telling the user how to add one to `datasource`.
pub(crate) fn no_endpoints_hint(datasource: &str) -> String {
    format!(
        "no endpoints configured; add one with \
         `datom datasource endpoint add {datasource} <name> --path <path>`"
    )
}

/// Why `path` is not a usable endpoint path, if it isn't.
///
/// Paths are joined onto the data source's base URL and every request
/// carries the data source's credentials, so a path must not be able to
/// point somewhere else: an absolute URL (`https://elsewhere/x`) or a
/// protocol-relative one (`//elsewhere/x`) would replace the host outright,
/// and `..` segments walk out of the base path.
fn path_error(path: &str) -> Option<&'static str> {
    // Only the path component can redirect the request; a query string or
    // fragment cannot.
    let path_part = path.split(['?', '#']).next().unwrap_or(path);

    if path.contains('\\') {
        // URL parsers map `\` onto `/` for http(s), so `\\host` is another
        // spelling of `//host`.
        Some("must not contain backslashes")
    } else if path_part.starts_with("//") {
        Some("must not start with `//`, which replaces the host")
    } else if has_scheme(path) {
        Some("must be a path relative to the base URL, not an absolute URL")
    } else if path_part.split('/').any(|segment| segment == "..") {
        Some("must not contain `..` segments")
    } else {
        None
    }
}

/// Whether `s` starts with a URL scheme (`https:`, `file:`, …).
fn has_scheme(s: &str) -> bool {
    let Some(colon) = s.find(':') else {
        return false;
    };
    let scheme = &s[..colon];
    scheme.starts_with(|c: char| c.is_ascii_alphabetic())
        && scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
}

/// Why `ident` is not a valid endpoint name, if it isn't.
fn identifier_error(ident: &str) -> Option<&'static str> {
    if ident.is_empty() {
        Some("must not be empty")
    } else if !ident
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
    {
        Some("may only contain lowercase letters, digits, and underscores")
    } else {
        None
    }
}

/// Validate a user-chosen data source name.
///
/// Names must be non-empty and consist only of lowercase ASCII letters, digits, and hyphens.
///
/// # Errors
///
/// Returns [`CoreError::InvalidDataSourceName`] describing the violation.
pub fn validate_datasource_name(name: &str) -> Result<()> {
    let reason = if name.is_empty() {
        Some("name must not be empty")
    } else if !name
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        Some("name may only contain lowercase letters, digits, and hyphens")
    } else {
        None
    };

    match reason {
        Some(reason) => Err(CoreError::InvalidDataSourceName {
            name: name.to_string(),
            reason: reason.to_string(),
        }),
        None => Ok(()),
    }
}

/// Reject names that are empty or would escape the `datasources/` directory.
fn validate_name(name: &str) -> Result<()> {
    let reason = if name.is_empty() {
        Some("name must not be empty")
    } else if name.contains(['/', '\\']) {
        Some("name must not contain path separators")
    } else if name == "." || name == ".." {
        Some("name must not be `.` or `..`")
    } else {
        None
    };

    match reason {
        Some(reason) => Err(CoreError::InvalidDataSourceName {
            name: name.to_string(),
            reason: reason.to_string(),
        }),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn api_datasource(name: &str, auth: AuthConfig) -> Datasource {
        Datasource {
            name: name.to_string(),
            kind: DatasourceKind::Api(ApiConfig {
                base_url: "https://api.example.com".to_string(),
                auth,
                endpoints: Vec::new(),
            }),
        }
    }

    fn endpoint(name: &str, path: &str) -> Endpoint {
        Endpoint {
            name: name.to_string(),
            path: path.to_string(),
        }
    }

    fn with_endpoints(mut datasource: Datasource, endpoints: Vec<Endpoint>) -> Datasource {
        let DatasourceKind::Api(api) = &mut datasource.kind;
        api.endpoints = endpoints;
        datasource
    }

    fn all_auth_variants() -> Vec<AuthConfig> {
        vec![
            AuthConfig::None,
            AuthConfig::ApiKey {
                header_name: "X-Api-Key".to_string(),
                env_var: "EXAMPLE_API_KEY".to_string(),
            },
            AuthConfig::Bearer {
                env_var: "EXAMPLE_TOKEN".to_string(),
            },
            AuthConfig::OAuth2ClientCredentials {
                token_url: "https://auth.example.com/token".to_string(),
                client_id_env: "EXAMPLE_CLIENT_ID".to_string(),
                client_secret_env: "EXAMPLE_CLIENT_SECRET".to_string(),
                scopes: vec!["read".to_string(), "write".to_string()],
            },
        ]
    }

    fn assert_round_trips(auth: AuthConfig) {
        let original = api_datasource("example", auth);
        let toml = toml::to_string_pretty(&original).unwrap();
        let parsed: Datasource = toml::from_str(&toml).unwrap();
        assert_eq!(parsed, original, "round-trip through TOML:\n{toml}");
    }

    #[test]
    fn round_trips_auth_none() {
        assert_round_trips(AuthConfig::None);
    }

    #[test]
    fn round_trips_auth_api_key() {
        assert_round_trips(AuthConfig::ApiKey {
            header_name: "X-Api-Key".to_string(),
            env_var: "EXAMPLE_API_KEY".to_string(),
        });
    }

    #[test]
    fn round_trips_auth_bearer() {
        assert_round_trips(AuthConfig::Bearer {
            env_var: "EXAMPLE_TOKEN".to_string(),
        });
    }

    #[test]
    fn round_trips_auth_oauth2_client_credentials() {
        assert_round_trips(AuthConfig::OAuth2ClientCredentials {
            token_url: "https://auth.example.com/token".to_string(),
            client_id_env: "EXAMPLE_CLIENT_ID".to_string(),
            client_secret_env: "EXAMPLE_CLIENT_SECRET".to_string(),
            scopes: vec!["read".to_string(), "write".to_string()],
        });
    }

    #[test]
    fn round_trips_multiple_endpoints() {
        let original = with_endpoints(
            api_datasource("example", AuthConfig::None),
            vec![
                endpoint("users", "/v1/users"),
                endpoint("orders", "/v1/orders"),
                endpoint("audit_log2", "/v1/audit"),
            ],
        );

        let toml = toml::to_string_pretty(&original).unwrap();
        assert!(toml.contains("[[endpoints]]"), "{toml}");

        let parsed: Datasource = toml::from_str(&toml).unwrap();
        assert_eq!(parsed, original, "round-trip through TOML:\n{toml}");
    }

    #[test]
    fn round_trips_endpoints_through_save_and_load() {
        let tmp = tempdir().unwrap();
        let original = with_endpoints(
            api_datasource("example", AuthConfig::None),
            vec![
                endpoint("users", "/v1/users"),
                endpoint("orders", "/v1/orders"),
            ],
        );

        save_datasource(tmp.path(), &original).unwrap();
        assert_eq!(load_datasource(tmp.path(), "example").unwrap(), original);
    }

    #[test]
    fn save_rejects_duplicate_endpoint_names() {
        let tmp = tempdir().unwrap();
        let datasource = with_endpoints(
            api_datasource("dup", AuthConfig::None),
            vec![endpoint("users", "/a"), endpoint("users", "/b")],
        );

        let err = save_datasource(tmp.path(), &datasource).unwrap_err();

        assert!(matches!(err, CoreError::InvalidEndpoint { .. }), "{err}");
        assert!(err.to_string().contains("duplicate"), "{err}");
        assert!(
            !datasource_path(tmp.path(), "dup").exists(),
            "nothing should be written on validation failure"
        );
    }

    #[test]
    fn load_rejects_duplicate_endpoint_names() {
        let tmp = tempdir().unwrap();
        let datasource = with_endpoints(
            api_datasource("dup", AuthConfig::None),
            vec![endpoint("users", "/v1/users")],
        );
        let path = save_datasource(tmp.path(), &datasource).unwrap();

        // Sneak a duplicate past save's validation by editing the file.
        let mut contents = fs::read_to_string(&path).unwrap();
        contents.push_str("\n[[endpoints]]\nname = \"users\"\npath = \"/v2/users\"\n");
        fs::write(&path, contents).unwrap();

        let err = load_datasource(tmp.path(), "dup").unwrap_err();

        assert!(matches!(err, CoreError::InvalidEndpoint { .. }), "{err}");
        assert!(err.to_string().contains("duplicate"), "{err}");
    }

    /// The endpoint names of `datasource` as loaded from disk.
    fn endpoint_names(root: &Path, datasource: &str) -> Vec<String> {
        let loaded = load_datasource(root, datasource).unwrap();
        let DatasourceKind::Api(api) = &loaded.kind;
        api.endpoints.iter().map(|e| e.name.clone()).collect()
    }

    #[test]
    fn add_endpoint_appends_and_persists() {
        let tmp = tempdir().unwrap();
        let datasource = with_endpoints(
            api_datasource("example", AuthConfig::None),
            vec![endpoint("users", "/v1/users")],
        );
        save_datasource(tmp.path(), &datasource).unwrap();
        add_endpoint(tmp.path(), "example", endpoint("orders", "/v1/orders")).unwrap();

        assert_eq!(endpoint_names(tmp.path(), "example"), ["users", "orders"]);
    }

    #[test]
    fn add_endpoint_rejects_duplicate_name_and_leaves_file_unchanged() {
        let tmp = tempdir().unwrap();
        let datasource = with_endpoints(
            api_datasource("example", AuthConfig::None),
            vec![endpoint("users", "/v1/users")],
        );
        save_datasource(tmp.path(), &datasource).unwrap();

        let err = add_endpoint(tmp.path(), "example", endpoint("users", "/v2/users")).unwrap_err();

        assert!(matches!(err, CoreError::InvalidEndpoint { .. }), "{err}");
        assert!(err.to_string().contains("duplicate"), "{err}");
        assert_eq!(endpoint_names(tmp.path(), "example"), ["users"]);
    }

    #[test]
    fn add_endpoint_rejects_invalid_name_and_leaves_file_unchanged() {
        let tmp = tempdir().unwrap();
        save_datasource(tmp.path(), &api_datasource("example", AuthConfig::None)).unwrap();

        let err = add_endpoint(tmp.path(), "example", endpoint("Bad-Name", "/x")).unwrap_err();

        assert!(matches!(err, CoreError::InvalidEndpoint { .. }), "{err}");
        assert!(endpoint_names(tmp.path(), "example").is_empty());
    }

    #[test]
    fn add_endpoint_errors_when_datasource_missing() {
        let tmp = tempdir().unwrap();
        let err = add_endpoint(tmp.path(), "nope", endpoint("users", "/x")).unwrap_err();
        assert!(matches!(err, CoreError::DataSourceNotFound(_)), "{err}");
    }

    #[test]
    fn remove_endpoint_deletes_and_persists() {
        let tmp = tempdir().unwrap();
        let datasource = with_endpoints(
            api_datasource("example", AuthConfig::None),
            vec![
                endpoint("users", "/v1/users"),
                endpoint("orders", "/v1/orders"),
            ],
        );
        save_datasource(tmp.path(), &datasource).unwrap();
        remove_endpoint(tmp.path(), "example", "users").unwrap();

        assert_eq!(endpoint_names(tmp.path(), "example"), ["orders"]);
    }

    #[test]
    fn remove_endpoint_errors_when_endpoint_missing() {
        let tmp = tempdir().unwrap();
        let datasource = with_endpoints(
            api_datasource("example", AuthConfig::None),
            vec![endpoint("users", "/v1/users")],
        );
        save_datasource(tmp.path(), &datasource).unwrap();

        let err = remove_endpoint(tmp.path(), "example", "orders").unwrap_err();

        assert!(matches!(err, CoreError::EndpointNotFound { .. }), "{err}");
        assert_eq!(
            err.to_string(),
            "endpoint `orders` was not found in data source `example`"
        );
        assert_eq!(endpoint_names(tmp.path(), "example"), ["users"]);
    }

    #[test]
    fn rejects_unusable_environment_variable_names() {
        let tmp = tempdir().unwrap();
        for bad in ["", "has space", "has-hyphen", "9leading", "$dollar"] {
            let auth = AuthConfig::Bearer {
                env_var: bad.to_string(),
            };
            let err = save_datasource(tmp.path(), &api_datasource("ds", auth))
                .expect_err(&format!("`{bad}` must be rejected"));
            assert!(matches!(err, CoreError::InvalidAuth { .. }), "{err}");
        }
        for good in ["TOKEN", "_private", "Api_Key_2", "lowercase"] {
            let auth = AuthConfig::Bearer {
                env_var: good.to_string(),
            };
            assert!(
                save_datasource(tmp.path(), &api_datasource("ds", auth)).is_ok(),
                "`{good}` must be accepted"
            );
        }
    }

    #[test]
    fn rejects_one_variable_serving_as_both_id_and_secret() {
        let tmp = tempdir().unwrap();
        let shared = AuthConfig::OAuth2ClientCredentials {
            token_url: "https://auth.example.com/token".to_string(),
            client_id_env: "API_CREDS".to_string(),
            client_secret_env: "API_CREDS".to_string(),
            scopes: Vec::new(),
        };

        let err = save_datasource(tmp.path(), &api_datasource("shared", shared)).unwrap_err();

        assert!(matches!(err, CoreError::InvalidAuth { .. }), "{err}");
        assert!(err.to_string().contains("API_CREDS"), "{err}");
        assert!(
            !datasource_path(tmp.path(), "shared").exists(),
            "nothing should be written on validation failure"
        );
    }

    #[test]
    fn load_rejects_a_hand_written_shared_credential_variable() {
        let tmp = tempdir().unwrap();
        let good = AuthConfig::OAuth2ClientCredentials {
            token_url: "https://auth.example.com/token".to_string(),
            client_id_env: "API_ID".to_string(),
            client_secret_env: "API_SECRET".to_string(),
            scopes: Vec::new(),
        };
        let path = save_datasource(tmp.path(), &api_datasource("shared", good)).unwrap();
        let contents = fs::read_to_string(&path)
            .unwrap()
            .replace("API_SECRET", "API_ID");
        fs::write(&path, contents).unwrap();

        let err = load_datasource(tmp.path(), "shared").unwrap_err();

        assert!(matches!(err, CoreError::InvalidAuth { .. }), "{err}");
    }

    #[test]
    fn rejects_endpoint_paths_that_escape_the_base_url() {
        // A path that names another host would send the data source's
        // credentials there; `..` walks out of the base path.
        let escapes = [
            "http://evil.example/steal",
            "https://evil.example/steal",
            "file:///etc/passwd",
            "//evil.example/steal",
            "\\\\evil.example/steal",
            "/v1/../../elsewhere",
            "../elsewhere",
        ];
        for path in escapes {
            let err = validate_endpoints("ds", &[endpoint("users", path)])
                .expect_err(&format!("`{path}` must be rejected"));
            assert!(matches!(err, CoreError::InvalidEndpoint { .. }), "{err}");
            assert!(err.to_string().contains(path), "{err}");
        }
    }

    #[test]
    fn accepts_ordinary_endpoint_paths() {
        for path in [
            "/v1/users",
            "v1/users",
            "",
            "/",
            "users?page=2&sort=id",
            "/a.b/c-d_e",
            "/search#frag",
        ] {
            assert!(
                validate_endpoints("ds", &[endpoint("users", path)]).is_ok(),
                "`{path}` must be accepted"
            );
        }
    }

    #[test]
    fn add_endpoint_rejects_escaping_path_and_leaves_file_unchanged() {
        let tmp = tempdir().unwrap();
        save_datasource(tmp.path(), &api_datasource("example", AuthConfig::None)).unwrap();

        let err = add_endpoint(
            tmp.path(),
            "example",
            endpoint("exfil", "http://evil.example/steal"),
        )
        .unwrap_err();

        assert!(matches!(err, CoreError::InvalidEndpoint { .. }), "{err}");
        assert!(endpoint_names(tmp.path(), "example").is_empty());
    }

    #[test]
    fn load_rejects_hand_written_escaping_path() {
        // The definition file is the attack surface: it carries no secrets
        // and so gets committed and reviewed casually. Refuse to load one
        // that would redirect credentials elsewhere.
        let tmp = tempdir().unwrap();
        let datasource = with_endpoints(
            api_datasource("sneaky", AuthConfig::None),
            vec![endpoint("users", "/v1/users")],
        );
        let path = save_datasource(tmp.path(), &datasource).unwrap();
        let mut contents = fs::read_to_string(&path).unwrap();
        contents
            .push_str("\n[[endpoints]]\nname = \"exfil\"\npath = \"https://evil.example/steal\"\n");
        fs::write(&path, contents).unwrap();

        let err = load_datasource(tmp.path(), "sneaky").unwrap_err();

        assert!(matches!(err, CoreError::InvalidEndpoint { .. }), "{err}");
        assert!(err.to_string().contains("evil.example"), "{err}");
    }

    #[test]
    fn validates_endpoint_names() {
        for good in ["users", "users_v2", "_private", "a1"] {
            assert!(
                validate_endpoints("ds", &[endpoint(good, "/x")]).is_ok(),
                "{good}"
            );
        }
        for bad in ["", "Users", "has-hyphen", "has space", "dots.io"] {
            assert!(
                matches!(
                    validate_endpoints("ds", &[endpoint(bad, "/x")]),
                    Err(CoreError::InvalidEndpoint { .. })
                ),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn serialized_form_uses_stable_tags() {
        let toml = toml::to_string_pretty(&api_datasource(
            "example",
            AuthConfig::ApiKey {
                header_name: "X-Api-Key".to_string(),
                env_var: "EXAMPLE_API_KEY".to_string(),
            },
        ))
        .unwrap();

        assert!(toml.contains("kind = \"api\""), "{toml}");
        assert!(toml.contains("type = \"api_key\""), "{toml}");
    }

    #[test]
    fn save_then_load_round_trips_every_variant() {
        let tmp = tempdir().unwrap();

        for (i, auth) in all_auth_variants().into_iter().enumerate() {
            let original = api_datasource(&format!("source-{i}"), auth);
            let path = save_datasource(tmp.path(), &original).unwrap();

            assert_eq!(path, datasource_path(tmp.path(), &original.name));
            assert!(path.is_file());
            assert_eq!(
                load_datasource(tmp.path(), &original.name).unwrap(),
                original
            );
        }
    }

    #[test]
    fn load_missing_returns_not_found() {
        let tmp = tempdir().unwrap();
        let err = load_datasource(tmp.path(), "nope").unwrap_err();
        assert!(matches!(err, CoreError::DataSourceNotFound(_)), "{err}");
    }

    #[test]
    fn list_returns_empty_without_datasources_dir() {
        let tmp = tempdir().unwrap();
        assert_eq!(
            list_datasources(tmp.path()).unwrap(),
            DatasourceListing::default()
        );
    }

    #[test]
    fn list_scans_and_sorts_by_name() {
        let tmp = tempdir().unwrap();
        for name in ["zeta", "alpha", "mid"] {
            save_datasource(tmp.path(), &api_datasource(name, AuthConfig::None)).unwrap();
        }
        // Non-datasource files are ignored.
        fs::write(tmp.path().join(DATASOURCES_DIR).join("notes.txt"), "hi").unwrap();

        let listing = list_datasources(tmp.path()).unwrap();
        let names: Vec<String> = listing.datasources.into_iter().map(|d| d.name).collect();
        assert_eq!(names, ["alpha", "mid", "zeta"]);
        assert!(listing.invalid.is_empty());
    }

    #[test]
    fn listing_reports_a_broken_file_beside_the_healthy_ones() {
        let tmp = tempdir().unwrap();
        save_datasource(tmp.path(), &api_datasource("good", AuthConfig::None)).unwrap();
        fs::write(
            tmp.path()
                .join(DATASOURCES_DIR)
                .join(format!("broken{DATASOURCE_FILE_SUFFIX}")),
            "this is not toml {{{",
        )
        .unwrap();

        let listing = list_datasources(tmp.path()).unwrap();

        // The healthy one is still listed — a broken file must not hide it.
        let names: Vec<&str> = listing
            .datasources
            .iter()
            .map(|d| d.name.as_str())
            .collect();
        assert_eq!(names, ["good"]);

        assert_eq!(listing.invalid.len(), 1);
        let (name, reason) = &listing.invalid[0];
        assert_eq!(name, "broken");
        assert!(reason.contains("parse"), "{reason}");
    }

    #[test]
    fn rejects_names_with_path_separators() {
        let tmp = tempdir().unwrap();
        let err =
            save_datasource(tmp.path(), &api_datasource("../evil", AuthConfig::None)).unwrap_err();
        assert!(
            matches!(err, CoreError::InvalidDataSourceName { .. }),
            "{err}"
        );
    }

    #[test]
    fn strict_name_validation() {
        for name in ["github", "my-api2", "a", "0-day"] {
            assert!(validate_datasource_name(name).is_ok(), "{name}");
        }
        for name in ["", "GitHub", "my_api", "has space", "../evil", "dots.io"] {
            assert!(
                matches!(
                    validate_datasource_name(name),
                    Err(CoreError::InvalidDataSourceName { .. })
                ),
                "{name}"
            );
        }
    }

    #[test]
    fn detects_name_mismatch() {
        let tmp = tempdir().unwrap();
        save_datasource(tmp.path(), &api_datasource("real", AuthConfig::None)).unwrap();
        let dir = tmp.path().join(DATASOURCES_DIR);
        fs::rename(
            dir.join(format!("real{DATASOURCE_FILE_SUFFIX}")),
            dir.join(format!("renamed{DATASOURCE_FILE_SUFFIX}")),
        )
        .unwrap();

        let err = load_datasource(tmp.path(), "renamed").unwrap_err();
        assert!(
            matches!(err, CoreError::DataSourceNameMismatch { .. }),
            "{err}"
        );
    }
}
