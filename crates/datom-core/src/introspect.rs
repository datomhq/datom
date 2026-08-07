//! Multi-endpoint introspection (`datom datasource introspect`).
//!
//! Fetches every endpoint of a data source — authentication is resolved
//! once via [`ApiFetcher`] — infers a table schema per endpoint, and writes
//! them all into the single `datasources/<name>.types.datom` file.
//! Endpoints fail independently: the report lists per-endpoint outcomes,
//! and the types file is only written when every endpoint succeeded, so a
//! failure never leaves a partial types file behind.

use std::fs;
use std::path::{Path, PathBuf};

use crate::datasource::no_endpoints_hint;
use crate::fetch::ApiFetcher;
use crate::schema::{InferredType, infer_response, rename_as_inferred};
use crate::types_format::{parse_tables, save_tables, types_path};
use crate::{CoreError, DatasourceKind, Endpoint, Result, error_chain, load_datasource};

/// Result of introspecting one data source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntrospectReport {
    /// Per-endpoint outcomes, in configuration order.
    pub endpoints: Vec<EndpointIntrospection>,

    /// Path of the written types file; `None` when any endpoint failed.
    pub types_path: Option<PathBuf>,
}

impl IntrospectReport {
    /// Whether no endpoint failed. An endpoint that returned no records
    /// did not fail — there was simply nothing to infer from.
    pub fn passed(&self) -> bool {
        !self
            .endpoints
            .iter()
            .any(|endpoint| matches!(endpoint.outcome, EndpointOutcome::Failed(_)))
    }
}

/// The introspection of one endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EndpointIntrospection {
    /// Endpoint name (also the name of the emitted table).
    pub name: String,

    /// What happened.
    pub outcome: EndpointOutcome,
}

/// Outcome of introspecting one endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EndpointOutcome {
    /// A schema was inferred.
    Inferred {
        /// Number of records the inference sampled.
        record_count: usize,
    },

    /// The endpoint responded, but with no records — an empty result set
    /// says nothing about the shape of the records it would contain.
    NoRecords {
        /// Whether a previously recorded schema was carried forward. When
        /// false, the endpoint has no table in the types file yet.
        kept_schema: bool,
    },

    /// Fetching or inference failed; the message says why.
    Failed(String),
}

/// Introspect every endpoint of the data source `name` inside
/// `project_root`, writing all inferred schemas to
/// `datasources/<name>.types.datom` (replacing its previous contents) when
/// — and only when — every endpoint succeeds.
///
/// # Errors
///
/// Returns an error for failures that affect the whole data source: it
/// cannot be loaded, it has no endpoints, authentication cannot be resolved
/// ([`ApiFetcher::new`]'s errors), or the types file cannot be written.
/// Per-endpoint failures do not error; they are reported in the
/// [`IntrospectReport`].
pub async fn introspect_datasource(
    project_root: impl AsRef<Path>,
    name: &str,
) -> Result<IntrospectReport> {
    let root = project_root.as_ref();
    let datasource = load_datasource(root, name)?;
    let DatasourceKind::Api(api) = &datasource.kind;

    if api.endpoints.is_empty() {
        return Err(CoreError::Introspection {
            name: name.to_string(),
            reason: no_endpoints_hint(name),
        });
    }

    let fetcher = ApiFetcher::new(api).await?;
    let recorded = recorded_tables(root, name);

    let mut endpoints = Vec::with_capacity(api.endpoints.len());
    let mut schemas = Vec::with_capacity(api.endpoints.len());
    let mut any_failed = false;
    for endpoint in &api.endpoints {
        let outcome = match introspect_endpoint(&fetcher, endpoint).await {
            Ok(Some((schema, record_count))) => {
                schemas.push(schema);
                EndpointOutcome::Inferred { record_count }
            }
            Ok(None) => {
                // An empty result set is no evidence, not a contradiction:
                // carry forward what was recorded before rather than
                // dropping the endpoint's schema on a quiet day.
                let kept = recorded.iter().find(|(table, _)| table == &endpoint.name);
                if let Some((_, schema)) = kept {
                    schemas.push(schema.clone());
                }
                EndpointOutcome::NoRecords {
                    kept_schema: kept.is_some(),
                }
            }
            Err(message) => {
                any_failed = true;
                EndpointOutcome::Failed(message)
            }
        };
        endpoints.push(EndpointIntrospection {
            name: endpoint.name.clone(),
            outcome,
        });
    }

    // Never a partial types file: write only when nothing failed, and only
    // when there is at least one schema to record.
    let types_path = if any_failed || schemas.is_empty() {
        None
    } else {
        Some(save_tables(root, name, &schemas)?)
    };

    Ok(IntrospectReport {
        endpoints,
        types_path,
    })
}

/// Tables already recorded for `datasource`, as `(name, schema)`.
///
/// Empty when no types file exists or it cannot be parsed; introspect
/// replaces the file either way, so an unreadable one is treated as absent.
fn recorded_tables(root: &Path, datasource: &str) -> Vec<(String, InferredType)> {
    let Ok(contents) = fs::read_to_string(types_path(root, datasource)) else {
        return Vec::new();
    };
    let Ok(tables) = parse_tables(&contents) else {
        return Vec::new();
    };
    tables
        .into_iter()
        .filter_map(|mut ty| {
            let InferredType::Record(record) = &ty else {
                return None;
            };
            let name = record.name.clone();
            rename_as_inferred(&mut ty, &name);
            Some((name, ty))
        })
        .collect()
}

/// Fetch one endpoint and infer its table schema, named after the endpoint.
///
/// Returns the schema and the sampled record count, `None` when the
/// endpoint returned no records at all, or a message describing the
/// failure.
async fn introspect_endpoint(
    fetcher: &ApiFetcher,
    endpoint: &Endpoint,
) -> std::result::Result<Option<(InferredType, usize)>, String> {
    let response = fetcher
        .fetch_endpoint(endpoint)
        .await
        .map_err(|err| error_chain(&err))?;

    let json: serde_json::Value = serde_json::from_str(&response.body).map_err(|_| {
        format!(
            "response is not valid JSON (content type: {})",
            response.content_type.as_deref().unwrap_or("unknown")
        )
    })?;

    let schema = infer_response(&endpoint.name, &json);
    if schema.record_count == 0 {
        // An empty array (or an empty envelope) — a well-formed response
        // that happens to carry no records to learn from.
        return Ok(None);
    }
    match schema.ty {
        InferredType::Record(mut record) => {
            record.name = endpoint.name.clone();
            Ok(Some((InferredType::Record(record), schema.record_count)))
        }
        _ => Err("response does not describe records (cannot infer a table schema)".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::datasource::AuthConfig;
    use crate::test_support::{DATASOURCE, mount_json, project};
    use serde_json::json;
    use wiremock::MockServer;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, ResponseTemplate};

    /// The outcome recorded for `name`.
    fn outcome<'a>(report: &'a IntrospectReport, name: &str) -> &'a EndpointOutcome {
        &report
            .endpoints
            .iter()
            .find(|endpoint| endpoint.name == name)
            .unwrap_or_else(|| panic!("no outcome for `{name}`"))
            .outcome
    }

    /// The types file's contents, if one was written.
    fn types(root: &Path) -> Option<String> {
        fs::read_to_string(types_path(root, DATASOURCE)).ok()
    }

    #[tokio::test]
    async fn records_a_table_per_endpoint_that_returned_data() {
        let server = MockServer::start().await;
        mount_json(&server, "/users", json!([{"id": 1}, {"id": 2}])).await;
        mount_json(&server, "/orders", json!({"data": [{"total": 2.5}]})).await;
        let (_tmp, root) = project(
            &server.uri(),
            AuthConfig::None,
            &[("users", "/users"), ("orders", "/orders")],
        );

        let report = introspect_datasource(&root, DATASOURCE).await.unwrap();

        assert!(report.passed());
        assert_eq!(
            outcome(&report, "users"),
            &EndpointOutcome::Inferred { record_count: 2 }
        );
        // The envelope is unwrapped, so the count is the inner array's.
        assert_eq!(
            outcome(&report, "orders"),
            &EndpointOutcome::Inferred { record_count: 1 }
        );
        let written = types(&root).expect("a types file");
        assert!(written.contains("table users {"), "{written}");
        assert!(written.contains("table orders {"), "{written}");
        assert_eq!(report.types_path, Some(types_path(&root, DATASOURCE)));
    }

    #[tokio::test]
    async fn one_failed_endpoint_writes_nothing_at_all() {
        let server = MockServer::start().await;
        mount_json(&server, "/users", json!([{"id": 1}])).await;
        Mock::given(method("GET"))
            .and(path("/boom"))
            .respond_with(ResponseTemplate::new(500).set_body_string("upstream exploded"))
            .mount(&server)
            .await;
        let (_tmp, root) = project(
            &server.uri(),
            AuthConfig::None,
            &[("users", "/users"), ("broken", "/boom")],
        );

        let report = introspect_datasource(&root, DATASOURCE).await.unwrap();

        // The healthy endpoint was still attempted and reported...
        assert_eq!(
            outcome(&report, "users"),
            &EndpointOutcome::Inferred { record_count: 1 }
        );
        let EndpointOutcome::Failed(message) = outcome(&report, "broken") else {
            panic!("expected a failure");
        };
        assert!(message.contains("500"), "{message}");
        // ...but a partial file is never left behind.
        assert!(!report.passed());
        assert_eq!(report.types_path, None);
        assert!(types(&root).is_none(), "no partial types file may exist");
    }

    #[tokio::test]
    async fn an_empty_endpoint_is_not_a_failure() {
        let server = MockServer::start().await;
        mount_json(&server, "/users", json!([{"id": 1}])).await;
        mount_json(&server, "/orders", json!([])).await;
        let (_tmp, root) = project(
            &server.uri(),
            AuthConfig::None,
            &[("users", "/users"), ("orders", "/orders")],
        );

        let report = introspect_datasource(&root, DATASOURCE).await.unwrap();

        assert!(report.passed(), "no records is not a failure");
        assert_eq!(
            outcome(&report, "orders"),
            &EndpointOutcome::NoRecords { kept_schema: false }
        );
        // Nothing is invented for it: asserting an empty shape would be a
        // claim we never observed.
        let written = types(&root).expect("a types file");
        assert!(written.contains("table users {"), "{written}");
        assert!(!written.contains("table orders {"), "{written}");
    }

    #[tokio::test]
    async fn an_empty_endpoint_keeps_the_schema_recorded_earlier() {
        let server = MockServer::start().await;
        mount_json(&server, "/orders", json!([{"total": 2.5}])).await;
        let (_tmp, root) = project(&server.uri(), AuthConfig::None, &[("orders", "/orders")]);

        introspect_datasource(&root, DATASOURCE).await.unwrap();
        let recorded = types(&root).expect("a types file");
        assert!(recorded.contains("total: float"), "{recorded}");

        // The endpoint goes quiet.
        server.reset().await;
        mount_json(&server, "/orders", json!([])).await;
        let report = introspect_datasource(&root, DATASOURCE).await.unwrap();

        assert_eq!(
            outcome(&report, "orders"),
            &EndpointOutcome::NoRecords { kept_schema: true }
        );
        assert_eq!(
            types(&root).as_deref(),
            Some(recorded.as_str()),
            "an empty response must not erase the contract"
        );
    }

    #[tokio::test]
    async fn nothing_is_written_when_no_endpoint_returned_records() {
        let server = MockServer::start().await;
        mount_json(&server, "/users", json!([])).await;
        let (_tmp, root) = project(&server.uri(), AuthConfig::None, &[("users", "/users")]);

        let report = introspect_datasource(&root, DATASOURCE).await.unwrap();

        // Nothing failed, but there was nothing to record either.
        assert!(report.passed());
        assert_eq!(report.types_path, None);
        assert!(types(&root).is_none());
    }

    #[tokio::test]
    async fn a_response_that_is_not_records_fails_the_endpoint() {
        let server = MockServer::start().await;
        // Non-empty, but scalars: there is no table schema here.
        mount_json(&server, "/nums", json!([1, 2, 3])).await;
        let (_tmp, root) = project(&server.uri(), AuthConfig::None, &[("nums", "/nums")]);

        let report = introspect_datasource(&root, DATASOURCE).await.unwrap();

        let EndpointOutcome::Failed(message) = outcome(&report, "nums") else {
            panic!("expected a failure");
        };
        assert!(message.contains("does not describe records"), "{message}");
        assert!(!report.passed());
    }

    #[tokio::test]
    async fn a_data_source_without_endpoints_errors() {
        let (_tmp, root) = project("http://127.0.0.1:1", AuthConfig::None, &[]);

        let err = introspect_datasource(&root, DATASOURCE).await.unwrap_err();

        assert!(matches!(err, CoreError::Introspection { .. }), "{err}");
        assert!(err.to_string().contains("no endpoints configured"), "{err}");
    }

    #[tokio::test]
    async fn a_missing_data_source_errors() {
        let (_tmp, root) = project("http://127.0.0.1:1", AuthConfig::None, &[("a", "/a")]);

        let err = introspect_datasource(&root, "nope").await.unwrap_err();

        assert!(matches!(err, CoreError::DataSourceNotFound(_)), "{err}");
    }
}
