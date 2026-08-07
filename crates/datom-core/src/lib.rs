//! Core library for `datom-connect`.
//!
//! Houses the domain types and error definitions shared across the platform.
//! The CLI (`datom-cli`) is a thin front-end over the functionality exposed
//! here. For now the crate mostly defines the error type used by higher layers.

use thiserror::Error;

pub mod connectivity;
pub mod datasource;
pub mod db;
pub mod fetch;
pub mod introspect;
pub mod project;
pub mod schema;
pub mod schema_diff;
#[cfg(test)]
mod test_support;
pub mod types_format;

pub use connectivity::{
    EndpointTest, StepKind, StepOutcome, TestReport, TestStep, check_connectivity, test_datasource,
};
pub use datasource::{
    ApiConfig, AuthConfig, Datasource, DatasourceKind, DatasourceListing, Endpoint, add_endpoint,
    list_datasources, load_datasource, remove_endpoint, save_datasource, validate_auth,
    validate_datasource_name, validate_endpoints,
};
pub use fetch::{ApiFetcher, FetchResponse, fetch_api};
pub use introspect::{
    EndpointIntrospection, EndpointOutcome, IntrospectReport, introspect_datasource,
};
pub use project::{MANIFEST_FILE, SCHEMA_VERSION, find_project_root, init_project};
pub use schema::{
    InferredSchema, InferredType, RecordField, RecordType, infer_records, infer_response,
    infer_value, rename_as_inferred,
};
pub use schema_diff::{SchemaChange, diff_schemas};
pub use types_format::{
    TYPES_FILE_SUFFIX, load_tables, parse_tables, render_tables, save_tables, types_path,
};

/// Errors that can arise from core `datom-connect` operations.
#[derive(Debug, Error)]
pub enum CoreError {
    /// A project directory already exists at the target path.
    #[error("project directory `{0}` already exists")]
    ProjectExists(String),

    /// A named data source could not be found.
    #[error("data source `{0}` was not found")]
    DataSourceNotFound(String),

    /// A data source with the given name already exists.
    #[error("data source `{0}` already exists")]
    DataSourceExists(String),

    /// A data source name is empty or would escape the datasources directory.
    #[error("invalid data source name `{name}`: {reason}")]
    InvalidDataSourceName {
        /// The offending name.
        name: String,
        /// Why the name was rejected.
        reason: String,
    },

    /// An endpoint definition within a data source is invalid.
    #[error("invalid endpoint `{name}` in data source `{datasource}`: {reason}")]
    InvalidEndpoint {
        /// Name of the data source containing the endpoint.
        datasource: String,
        /// Name of the offending endpoint.
        name: String,
        /// Why the endpoint was rejected.
        reason: String,
    },

    /// A data source's authentication configuration is invalid.
    #[error("invalid auth configuration for data source `{datasource}`: {reason}")]
    InvalidAuth {
        /// Name of the offending data source.
        datasource: String,
        /// Why the configuration was rejected.
        reason: String,
    },

    /// A named endpoint could not be found in a data source.
    #[error("endpoint `{name}` was not found in data source `{datasource}`")]
    EndpointNotFound {
        /// Name of the data source that was searched.
        datasource: String,
        /// Name of the missing endpoint.
        name: String,
    },

    /// A data source definition failed to serialize to TOML.
    #[error("failed to serialize data source `{name}` to TOML")]
    DataSourceSerialize {
        /// Name of the data source being saved.
        name: String,
        /// Underlying TOML error.
        #[source]
        source: toml::ser::Error,
    },

    /// A data source definition file could not be parsed.
    #[error("failed to parse data source file `{path}`")]
    DataSourceParse {
        /// Path of the offending file.
        path: String,
        /// Underlying TOML error.
        #[source]
        source: toml::de::Error,
    },

    /// A definition file's `name` field disagrees with its file name.
    #[error("data source file `{path}` declares name `{found}`, expected `{expected}`")]
    DataSourceNameMismatch {
        /// Path of the offending file.
        path: String,
        /// Name implied by the file name.
        expected: String,
        /// Name declared inside the file.
        found: String,
    },

    /// Introspecting a data source failed.
    #[error("failed to introspect data source `{name}`: {reason}")]
    Introspection {
        /// Name of the data source being introspected.
        name: String,
        /// Human-readable reason the introspection failed.
        reason: String,
    },

    /// An underlying I/O error occurred.
    #[error(transparent)]
    Io(#[from] std::io::Error),

    /// A database operation failed.
    #[error(transparent)]
    Database(#[from] sqlx::Error),

    /// A required environment variable is not set.
    #[error(
        "environment variable `{0}` is not set (required by the data source auth configuration)"
    )]
    MissingEnvVar(String),

    /// A URL could not be parsed or joined.
    #[error("invalid URL `{url}`: {reason}")]
    InvalidUrl {
        /// The offending URL (or base URL and path being joined).
        url: String,
        /// Why it is invalid.
        reason: String,
    },

    /// An HTTP request could not be completed.
    #[error(transparent)]
    Http(#[from] reqwest::Error),

    /// A request did not finish within its deadline.
    #[error("request to `{url}` timed out after {timeout}")]
    HttpTimeout {
        /// URL that was requested.
        url: String,
        /// The deadline that elapsed, e.g. `30s`.
        timeout: String,
    },

    /// An HTTP request completed with a non-success status.
    #[error("request to `{url}` failed with status {status}; body: {body}")]
    HttpStatus {
        /// URL that was requested.
        url: String,
        /// The response status code.
        status: u16,
        /// Response body, truncated for readability.
        body: String,
    },

    /// An OAuth2 token endpoint returned an unusable response.
    #[error("OAuth2 token endpoint `{token_url}` returned an invalid response: {reason}")]
    OAuthToken {
        /// The token endpoint that was called.
        token_url: String,
        /// Why the response could not be used.
        reason: String,
    },

    /// A schema cannot be represented in the `.types.datom` text format.
    #[error("cannot render types file: {0}")]
    TypesRender(String),

    /// A `.types.datom` file failed to parse.
    #[error("types file parse error at line {line}: {message}")]
    TypesParse {
        /// 1-based line number of the offending input.
        line: usize,
        /// What went wrong.
        message: String,
    },
}

/// Convenient result alias for core operations.
pub type Result<T> = std::result::Result<T, CoreError>;

/// `err`'s message followed by its source chain, so failures show their
/// root cause (e.g. "connection refused" behind reqwest's generic
/// "error sending request", or the TOML error behind a config parse failure).
pub(crate) fn error_chain(err: &dyn std::error::Error) -> String {
    let mut message = err.to_string();
    let mut source = err.source();
    while let Some(cause) = source {
        message.push_str(": ");
        message.push_str(&cause.to_string());
        source = cause.source();
    }
    message
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_messages_render() {
        let err = CoreError::DataSourceNotFound("warehouse".to_string());
        assert_eq!(err.to_string(), "data source `warehouse` was not found");
    }
}
