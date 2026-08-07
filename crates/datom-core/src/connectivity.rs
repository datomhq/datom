//! Connectivity and contract testing for data sources
//! (`datom datasource test`).
//!
//! Config load and secret resolution are checked once per data source;
//! every endpoint then gets its own section: an authed GET (HTTP status,
//! latency, JSON parse) and a contract check of the freshly inferred schema
//! against the endpoint's table in `datasources/<name>.types.datom`
//! (compared with [`crate::schema_diff::diff_schemas`]).
//! The result is a structured [`TestReport`]; rendering and exit codes are
//! left to callers. Nothing here writes files.

use std::collections::HashSet;
use std::fs;
use std::path::Path;
use std::time::Instant;

use serde::de::IgnoredAny;

use crate::datasource::{named_env_vars, no_endpoints_hint};
use crate::fetch::ApiFetcher;
use crate::schema::{InferredType, infer_response};
use crate::schema_diff::diff_schemas;
use crate::types_format::{parse_tables, types_path};
use crate::{ApiConfig, AuthConfig, DatasourceKind, Endpoint, error_chain, load_datasource};

/// The data-source-wide steps, in report order.
const GLOBAL_STEPS: [StepKind; 3] = [
    StepKind::ConfigLoaded,
    StepKind::SecretsResolved,
    StepKind::Endpoints,
];

/// The per-endpoint steps, in report order.
const ENDPOINT_STEPS: [StepKind; 2] = [StepKind::Connected, StepKind::Contract];

/// Which check a [`TestStep`] reports on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepKind {
    /// The data source's definition file was read.
    ConfigLoaded,
    /// The environment variables its auth configuration names were read.
    SecretsResolved,
    /// The data source has endpoints to test.
    Endpoints,
    /// One endpoint answered, with a body that parses as JSON.
    Connected,
    /// One endpoint's schema still matches its recorded table.
    Contract,
}

impl StepKind {
    /// The label shown for this step.
    pub fn label(self) -> &'static str {
        match self {
            StepKind::ConfigLoaded => "Config loaded",
            StepKind::SecretsResolved => "Secrets resolved",
            StepKind::Endpoints => "Endpoints",
            StepKind::Connected => "Connected",
            StepKind::Contract => "Contract",
        }
    }
}

/// What happened when a test step ran (or why it did not run).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StepOutcome {
    /// The step succeeded, optionally with extra detail (e.g. `200, 89ms`).
    Passed(Option<String>),

    /// The step failed; the message says what to fix. A failed `Contract`
    /// step carries one schema change per line.
    Failed(String),

    /// The step did not run because an earlier step failed.
    Skipped,
}

/// One step of a connectivity test.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestStep {
    /// Which check this step reports on.
    pub kind: StepKind,

    /// What happened.
    pub outcome: StepOutcome,
}

impl TestStep {
    fn passed(&self) -> bool {
        matches!(self.outcome, StepOutcome::Passed(_))
    }
}

/// The test of one endpoint: connect, then check the contract.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EndpointTest {
    /// Endpoint name.
    pub name: String,

    /// Endpoint path, as configured.
    pub path: String,

    /// The endpoint's steps, in execution order.
    pub steps: Vec<TestStep>,
}

/// Structured result of a data source connectivity test.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestReport {
    /// Data-source-wide steps: config load and secret resolution.
    pub steps: Vec<TestStep>,

    /// Per-endpoint sections, in configuration order.
    pub endpoints: Vec<EndpointTest>,

    /// Tables in the types file with no matching endpoint.
    pub unmatched_tables: Vec<String>,
}

impl TestReport {
    /// Whether everything passed: the global steps, every endpoint step
    /// (contract checks included), and no unmatched tables.
    pub fn passed(&self) -> bool {
        self.steps.iter().all(TestStep::passed)
            && self
                .endpoints
                .iter()
                .all(|endpoint| endpoint.steps.iter().all(TestStep::passed))
            && self.unmatched_tables.is_empty()
    }

    /// Whether the data source is reachable: the global steps and every
    /// endpoint's `Connected` step passed. Contract drift does not count
    /// against reachability.
    pub fn connected(&self) -> bool {
        self.steps.iter().all(TestStep::passed)
            && self.endpoints.iter().all(|endpoint| {
                endpoint
                    .steps
                    .iter()
                    .filter(|step| step.kind == StepKind::Connected)
                    .all(TestStep::passed)
            })
    }

    fn record_global(&mut self, kind: StepKind, outcome: StepOutcome) {
        self.steps.push(TestStep { kind, outcome });
    }

    /// Record a global failure and mark every global step that has not run
    /// as skipped.
    fn fail_global(&mut self, kind: StepKind, message: String) {
        self.record_global(kind, StepOutcome::Failed(message));
        let recorded: Vec<StepKind> = self.steps.iter().map(|step| step.kind).collect();
        for &skipped in GLOBAL_STEPS.iter().filter(|k| !recorded.contains(k)) {
            self.record_global(skipped, StepOutcome::Skipped);
        }
    }
}

/// Whether every endpoint of `config` answers with a JSON body.
///
/// Takes an already-loaded [`ApiConfig`] so a caller that just parsed the
/// definition file does not pay to parse it again.
pub async fn check_connectivity(config: &ApiConfig) -> bool {
    // Nothing was requested, so nothing is reachable.
    if config.endpoints.is_empty() {
        return false;
    }
    if required_env_vars(&config.auth)
        .iter()
        .any(|var| env_var_problem(var).is_some())
    {
        return false;
    }

    let Ok(fetcher) = ApiFetcher::new(config).await else {
        return false;
    };
    for endpoint in &config.endpoints {
        let Ok(response) = fetcher.fetch_endpoint(endpoint).await else {
            return false;
        };
        // Validated without building the value tree: the verdict depends
        // only on whether the body is JSON, not on what it contains.
        if serde_json::from_str::<IgnoredAny>(&response.body).is_err() {
            return false;
        }
    }
    true
}

/// Test connectivity and contracts of the data source `name` inside
/// `project_root`.
///
/// Never returns an error: failures are recorded in the report so callers
/// can show exactly which step of which endpoint broke.
pub async fn test_datasource(project_root: impl AsRef<Path>, name: &str) -> TestReport {
    let root = project_root.as_ref();
    let mut report = TestReport {
        steps: Vec::new(),
        endpoints: Vec::new(),
        unmatched_tables: Vec::new(),
    };

    // Config loaded
    let datasource = match load_datasource(root, name) {
        Ok(datasource) => datasource,
        Err(err) => {
            report.fail_global(StepKind::ConfigLoaded, error_chain(&err));
            return report;
        }
    };
    report.record_global(StepKind::ConfigLoaded, StepOutcome::Passed(None));
    let DatasourceKind::Api(api) = &datasource.kind;

    // Secrets resolved
    let env_vars = required_env_vars(&api.auth);
    let problems: Vec<String> = env_vars
        .iter()
        .filter_map(|var| env_var_problem(var))
        .collect();
    if !problems.is_empty() {
        report.fail_global(
            StepKind::SecretsResolved,
            format!(
                "cannot read the secrets this data source needs: {}",
                problems.join("; ")
            ),
        );
        report.endpoints = api.endpoints.iter().map(skipped_endpoint).collect();
        return report;
    }
    let detail = match env_vars.len() {
        0 => "none required".to_string(),
        1 => "1 env var".to_string(),
        n => format!("{n} env vars"),
    };
    report.record_global(StepKind::SecretsResolved, StepOutcome::Passed(Some(detail)));

    if api.endpoints.is_empty() {
        report.fail_global(StepKind::Endpoints, no_endpoints_hint(name));
        return report;
    }
    report.record_global(
        StepKind::Endpoints,
        StepOutcome::Passed(Some(api.endpoints.len().to_string())),
    );

    // The recorded contract, if any.
    let stored = load_stored_tables(root, name);
    if let StoredTypes::Tables(tables) = &stored {
        report.unmatched_tables = tables
            .iter()
            .map(|(table, _)| table.clone())
            .filter(|table| !api.endpoints.iter().any(|e| &e.name == table))
            .collect();
    }

    // Auth is resolved once; a failure (e.g. the OAuth2 token exchange)
    // affects every endpoint's connection.
    let fetcher = match ApiFetcher::new(api).await {
        Ok(fetcher) => fetcher,
        Err(err) => {
            let message = error_chain(&err);
            report.endpoints = api
                .endpoints
                .iter()
                .map(|endpoint| EndpointTest {
                    name: endpoint.name.clone(),
                    path: endpoint.path.clone(),
                    steps: vec![
                        TestStep {
                            kind: StepKind::Connected,
                            outcome: StepOutcome::Failed(message.clone()),
                        },
                        TestStep {
                            kind: StepKind::Contract,
                            outcome: StepOutcome::Skipped,
                        },
                    ],
                })
                .collect();
            return report;
        }
    };

    for endpoint in &api.endpoints {
        report
            .endpoints
            .push(test_endpoint(&fetcher, endpoint, &stored, name).await);
    }
    report
}

/// Run one endpoint's steps: fetch and parse, then diff the contract.
async fn test_endpoint(
    fetcher: &ApiFetcher,
    endpoint: &Endpoint,
    stored: &StoredTypes,
    datasource: &str,
) -> EndpointTest {
    let step = |kind, outcome| TestStep { kind, outcome };
    let section = |steps| EndpointTest {
        name: endpoint.name.clone(),
        path: endpoint.path.clone(),
        steps,
    };
    let connected_failed = |message: String| {
        section(vec![
            step(StepKind::Connected, StepOutcome::Failed(message)),
            step(StepKind::Contract, StepOutcome::Skipped),
        ])
    };

    // Connected: HTTP status, latency, and a JSON body.
    let started = Instant::now();
    let response = match fetcher.fetch_endpoint(endpoint).await {
        Ok(response) => response,
        Err(err) => return connected_failed(error_chain(&err)),
    };
    let elapsed_ms = started.elapsed().as_millis();

    let json: serde_json::Value = match serde_json::from_str(&response.body) {
        Ok(json) => json,
        Err(_) => {
            return connected_failed(format!(
                "HTTP {} but the response is not valid JSON (content type: {})",
                response.status,
                response.content_type.as_deref().unwrap_or("unknown")
            ));
        }
    };

    section(vec![
        step(
            StepKind::Connected,
            StepOutcome::Passed(Some(format!("{}, {elapsed_ms}ms", response.status))),
        ),
        step(
            StepKind::Contract,
            contract_outcome(endpoint, &json, stored, datasource),
        ),
    ])
}

/// Compare the endpoint's freshly inferred schema against its stored table.
fn contract_outcome(
    endpoint: &Endpoint,
    json: &serde_json::Value,
    stored: &StoredTypes,
    datasource: &str,
) -> StepOutcome {
    let schema = infer_response(&endpoint.name, json);

    let recorded = match stored {
        StoredTypes::Missing => None,
        StoredTypes::Unreadable(message) => {
            return StepOutcome::Failed(format!("cannot read the types file: {message}"));
        }
        StoredTypes::Tables(tables) => tables
            .iter()
            .find(|(table, _)| table == &endpoint.name)
            .map(|(_, ty)| ty),
    };

    // An empty result set can neither confirm nor contradict a contract, so
    // it is reported rather than treated as every field disappearing.
    if schema.record_count == 0 {
        return match recorded {
            Some(_) => StepOutcome::Passed(Some("no records sampled".to_string())),
            None => StepOutcome::Failed(format!(
                "nothing to verify: no records sampled, and no table recorded for `{}` yet",
                endpoint.name
            )),
        };
    }

    let Some(stored_ty) = recorded else {
        return StepOutcome::Failed(match stored {
            StoredTypes::Missing => format!(
                "no types file; run `datom datasource introspect {datasource}` to record the contract"
            ),
            _ => format!(
                "table `{}` is missing from the types file; run `datom datasource introspect {datasource}` to update it",
                endpoint.name
            ),
        });
    };

    let fresh = schema.ty;
    if !matches!(fresh, InferredType::Record(_)) {
        return StepOutcome::Failed(
            "response does not describe records (cannot infer a table schema)".to_string(),
        );
    }

    let changes = diff_schemas(stored_ty, &fresh);
    if changes.is_empty() {
        StepOutcome::Passed(None)
    } else {
        StepOutcome::Failed(
            changes
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("\n"),
        )
    }
}

/// The tables recorded in the data source's types file.
enum StoredTypes {
    /// No types file exists yet.
    Missing,
    /// The types file exists but could not be read or parsed.
    Unreadable(String),
    /// Parsed tables as `(name, schema)`, in file order.
    Tables(Vec<(String, InferredType)>),
}

/// Load and parse the types file of data source `name`.
fn load_stored_tables(root: &Path, name: &str) -> StoredTypes {
    let contents = match fs::read_to_string(types_path(root, name)) {
        Ok(contents) => contents,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return StoredTypes::Missing,
        Err(err) => return StoredTypes::Unreadable(err.to_string()),
    };
    match parse_tables(&contents) {
        Ok(tables) => StoredTypes::Tables(
            tables
                .into_iter()
                .map(|ty| {
                    let name = match &ty {
                        InferredType::Record(record) => record.name.clone(),
                        _ => unreachable!("parsed tables are always records"),
                    };
                    (name, ty)
                })
                .collect(),
        ),
        Err(err) => StoredTypes::Unreadable(error_chain(&err)),
    }
}

/// An endpoint section whose steps were all skipped by an earlier failure.
fn skipped_endpoint(endpoint: &Endpoint) -> EndpointTest {
    EndpointTest {
        name: endpoint.name.clone(),
        path: endpoint.path.clone(),
        steps: ENDPOINT_STEPS
            .iter()
            .map(|&kind| TestStep {
                kind,
                outcome: StepOutcome::Skipped,
            })
            .collect(),
    }
}

/// Why `var` cannot be used as a secret, if it cannot.
fn env_var_problem(var: &str) -> Option<String> {
    match std::env::var(var) {
        Ok(_) => None,
        Err(std::env::VarError::NotPresent) => Some(format!("`{var}` is not set")),
        Err(std::env::VarError::NotUnicode(_)) => {
            Some(format!("`{var}` is set but is not valid UTF-8"))
        }
    }
}

/// Names of the environment variables `auth` reads secrets from, without
/// repeats.
///
/// Derived from [`named_env_vars`] rather than matching on [`AuthConfig`]
/// again, so this cannot fall out of step with what validation checks.
fn required_env_vars(auth: &AuthConfig) -> Vec<&str> {
    let mut seen = HashSet::new();
    named_env_vars(auth)
        .into_iter()
        .map(|(_, var)| var)
        .filter(|var| seen.insert(*var))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::datasource::AuthConfig;
    use crate::schema::rename_as_inferred;
    use crate::test_support::{DATASOURCE, mount_json, project};
    use crate::types_format::save_tables;
    use serde_json::json;
    use wiremock::MockServer;
    use wiremock::matchers::method;
    use wiremock::{Mock, ResponseTemplate};

    /// The outcome of the `kind` step, from a step list.
    fn outcome(steps: &[TestStep], kind: StepKind) -> &StepOutcome {
        &steps
            .iter()
            .find(|step| step.kind == kind)
            .unwrap_or_else(|| panic!("no {kind:?} step in {steps:?}"))
            .outcome
    }

    /// The section for `endpoint`.
    fn endpoint<'a>(report: &'a TestReport, name: &str) -> &'a EndpointTest {
        report
            .endpoints
            .iter()
            .find(|e| e.name == name)
            .unwrap_or_else(|| panic!("no section for `{name}`"))
    }

    /// Record a contract for the data source, one table per sample.
    fn record_contract(root: &Path, tables: &[(&str, serde_json::Value)]) {
        let schemas: Vec<_> = tables
            .iter()
            .map(|(name, sample)| {
                let mut schema = crate::schema::infer_response(name, sample).ty;
                rename_as_inferred(&mut schema, name);
                schema
            })
            .collect();
        save_tables(root, DATASOURCE, &schemas).unwrap();
    }

    /// The config of the data source the helpers create.
    fn api_config(root: &Path) -> ApiConfig {
        let DatasourceKind::Api(api) = load_datasource(root, DATASOURCE).unwrap().kind;
        api
    }

    #[tokio::test]
    async fn connectivity_check_agrees_with_the_full_report() {
        // The cheap path and the thorough one must never disagree about
        // reachability, or the CONNECTED column starts lying.
        let healthy = MockServer::start().await;
        mount_json(&healthy, "/users", json!([{"id": 1}])).await;

        let broken = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&broken)
            .await;

        let not_json = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_raw("<html>", "text/html"))
            .mount(&not_json)
            .await;

        let cases = [
            (
                healthy.uri(),
                AuthConfig::None,
                &[("users", "/users")][..],
                true,
            ),
            (
                broken.uri(),
                AuthConfig::None,
                &[("users", "/users")][..],
                false,
            ),
            (
                not_json.uri(),
                AuthConfig::None,
                &[("users", "/users")][..],
                false,
            ),
            // No endpoints: nothing was requested, so nothing is reachable.
            (healthy.uri(), AuthConfig::None, &[][..], false),
            // A secret that is not set stops the request from being made.
            (
                healthy.uri(),
                AuthConfig::Bearer {
                    env_var: "DATOM_CORE_TEST_ABSENT_SECRET".to_string(),
                },
                &[("users", "/users")][..],
                false,
            ),
        ];

        for (uri, auth, endpoints, expected) in cases {
            let (_tmp, root) = project(&uri, auth, endpoints);
            let cheap = check_connectivity(&api_config(&root)).await;
            let thorough = test_datasource(&root, DATASOURCE).await.connected();

            assert_eq!(
                cheap, expected,
                "check_connectivity for {uri} {endpoints:?}"
            );
            assert_eq!(
                cheap, thorough,
                "the two paths disagreed for {uri} {endpoints:?}"
            );
        }
    }

    #[tokio::test]
    async fn connectivity_check_ignores_the_contract_entirely() {
        let server = MockServer::start().await;
        mount_json(&server, "/users", json!([{"id": 1}])).await;
        let (_tmp, root) = project(&server.uri(), AuthConfig::None, &[("users", "/users")]);
        // A types file so broken it cannot be parsed.
        fs::write(types_path(&root, DATASOURCE), "this is not a schema {{{").unwrap();

        assert!(check_connectivity(&api_config(&root)).await);

        // The full report still fails, but on the contract.
        let report = test_datasource(&root, DATASOURCE).await;
        assert!(!report.passed());
        assert!(report.connected());
    }

    #[tokio::test]
    async fn drift_fails_the_report_but_the_source_is_still_connected() {
        // The distinction `list --test-connection` depends on: a schema
        // that moved is not the same as an API that is down.
        let server = MockServer::start().await;
        mount_json(&server, "/users", json!([{"id": "now-a-string"}])).await;
        let (_tmp, root) = project(&server.uri(), AuthConfig::None, &[("users", "/users")]);
        record_contract(&root, &[("users", json!([{"id": 1}]))]);

        let report = test_datasource(&root, DATASOURCE).await;

        let users = endpoint(&report, "users");
        assert!(matches!(
            outcome(&users.steps, StepKind::Connected),
            StepOutcome::Passed(_)
        ));
        let StepOutcome::Failed(message) = outcome(&users.steps, StepKind::Contract) else {
            panic!(
                "expected drift, got {:?}",
                outcome(&users.steps, StepKind::Contract)
            );
        };
        assert_eq!(message, "~ id: int -> string");

        assert!(!report.passed(), "drift must fail the report");
        assert!(report.connected(), "drift must not mean unreachable");
    }

    #[tokio::test]
    async fn zero_endpoints_is_never_a_vacuous_pass() {
        let server = MockServer::start().await;
        let (_tmp, root) = project(&server.uri(), AuthConfig::None, &[]);

        let report = test_datasource(&root, DATASOURCE).await;

        let StepOutcome::Failed(message) = outcome(&report.steps, StepKind::Endpoints) else {
            panic!("expected the Endpoints step to fail");
        };
        assert!(message.contains("no endpoints configured"), "{message}");
        assert!(report.endpoints.is_empty());
        // Nothing was requested, so neither predicate may be true.
        assert!(!report.passed());
        assert!(!report.connected());
    }

    #[tokio::test]
    async fn a_missing_secret_skips_every_endpoint_without_requesting() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200))
            .expect(0)
            .mount(&server)
            .await;
        let auth = AuthConfig::Bearer {
            env_var: "DATOM_CORE_TEST_ABSENT_SECRET".to_string(),
        };
        let (_tmp, root) = project(
            &server.uri(),
            auth,
            &[("users", "/users"), ("orders", "/orders")],
        );

        let report = test_datasource(&root, DATASOURCE).await;

        assert!(matches!(
            outcome(&report.steps, StepKind::SecretsResolved),
            StepOutcome::Failed(_)
        ));
        assert!(matches!(
            outcome(&report.steps, StepKind::Endpoints),
            StepOutcome::Skipped
        ));
        // Both endpoints are still listed, so the reader sees what was not
        // checked rather than a silently shorter report.
        assert_eq!(report.endpoints.len(), 2);
        for section in &report.endpoints {
            assert!(
                section
                    .steps
                    .iter()
                    .all(|step| matches!(step.outcome, StepOutcome::Skipped))
            );
        }
        assert!(!report.connected());
    }

    #[tokio::test]
    async fn an_empty_result_set_with_nothing_recorded_verifies_nothing() {
        // Every endpoint quiet and no contract on disk: the run must not
        // come back green having checked nothing at all.
        let server = MockServer::start().await;
        mount_json(&server, "/users", json!([])).await;
        let (_tmp, root) = project(&server.uri(), AuthConfig::None, &[("users", "/users")]);

        let report = test_datasource(&root, DATASOURCE).await;

        let StepOutcome::Failed(message) =
            outcome(&endpoint(&report, "users").steps, StepKind::Contract)
        else {
            panic!("expected the Contract step to fail");
        };
        assert!(message.contains("nothing to verify"), "{message}");
        assert!(!report.passed());
        // It did answer, though, so it is still reachable.
        assert!(report.connected());
    }

    #[test]
    fn a_variable_named_twice_is_counted_once() {
        // `validate_auth` now rejects this config on the way in, so it can
        // only arrive from a value built in memory. Deduping here keeps the
        // count and the failure message honest if one ever does.
        let auth = AuthConfig::OAuth2ClientCredentials {
            token_url: "https://auth.example.com/token".to_string(),
            client_id_env: "SHARED".to_string(),
            client_secret_env: "SHARED".to_string(),
            scopes: Vec::new(),
        };

        assert_eq!(required_env_vars(&auth), ["SHARED"]);
    }

    #[tokio::test]
    async fn an_empty_result_set_is_not_reported_as_drift() {
        let server = MockServer::start().await;
        mount_json(&server, "/users", json!([])).await;
        let (_tmp, root) = project(&server.uri(), AuthConfig::None, &[("users", "/users")]);
        record_contract(&root, &[("users", json!([{"id": 1}]))]);

        let report = test_datasource(&root, DATASOURCE).await;

        let contract = outcome(&endpoint(&report, "users").steps, StepKind::Contract);
        let StepOutcome::Passed(Some(detail)) = contract else {
            panic!("expected a passing note, got {contract:?}");
        };
        assert_eq!(detail, "no records sampled");
        assert!(report.passed());
    }

    #[tokio::test]
    async fn a_recorded_table_with_no_endpoint_is_reported() {
        let server = MockServer::start().await;
        mount_json(&server, "/users", json!([{"id": 1}])).await;
        let (_tmp, root) = project(&server.uri(), AuthConfig::None, &[("users", "/users")]);
        // The contract remembers an endpoint the config no longer has.
        record_contract(
            &root,
            &[
                ("users", json!([{"id": 1}])),
                ("orders", json!([{"total": 2.5}])),
            ],
        );

        let report = test_datasource(&root, DATASOURCE).await;

        assert_eq!(report.unmatched_tables, ["orders"]);
        assert!(!report.passed(), "an orphan table is a contract violation");
        // ...but every endpoint that exists still connected.
        assert!(report.connected());
    }

    #[tokio::test]
    async fn a_non_json_body_fails_connected_and_skips_the_contract() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_raw("<html>nope</html>", "text/html"))
            .mount(&server)
            .await;
        let (_tmp, root) = project(&server.uri(), AuthConfig::None, &[("page", "/page")]);

        let report = test_datasource(&root, DATASOURCE).await;

        let page = endpoint(&report, "page");
        let StepOutcome::Failed(message) = outcome(&page.steps, StepKind::Connected) else {
            panic!("expected the Connected step to fail");
        };
        assert!(message.contains("not valid JSON"), "{message}");
        assert!(message.contains("text/html"), "{message}");
        assert!(matches!(
            outcome(&page.steps, StepKind::Contract),
            StepOutcome::Skipped
        ));
        assert!(!report.connected());
    }

    #[tokio::test]
    async fn a_missing_data_source_reports_only_the_global_steps() {
        let (_tmp, root) = project("http://127.0.0.1:1", AuthConfig::None, &[("a", "/a")]);

        let report = test_datasource(&root, "nope").await;

        assert!(matches!(
            outcome(&report.steps, StepKind::ConfigLoaded),
            StepOutcome::Failed(_)
        ));
        assert!(matches!(
            outcome(&report.steps, StepKind::SecretsResolved),
            StepOutcome::Skipped
        ));
        assert!(
            report.endpoints.is_empty(),
            "no config means no endpoints to report"
        );
    }

    #[tokio::test]
    async fn a_missing_types_file_points_at_introspect() {
        let server = MockServer::start().await;
        mount_json(&server, "/users", json!([{"id": 1}])).await;
        let (_tmp, root) = project(&server.uri(), AuthConfig::None, &[("users", "/users")]);

        let report = test_datasource(&root, DATASOURCE).await;

        let StepOutcome::Failed(message) =
            outcome(&endpoint(&report, "users").steps, StepKind::Contract)
        else {
            panic!("expected the Contract step to fail");
        };
        assert!(message.contains("no types file"), "{message}");
        assert!(message.contains("datom datasource introspect"), "{message}");
    }
}
