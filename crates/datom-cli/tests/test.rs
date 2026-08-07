//! End-to-end tests of `datom datasource test`: spawns the real binary in a
//! temp project against a wiremock API and checks the per-endpoint
//! checklist, contract diffs, and exit codes.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use datom_core::{ApiConfig, AuthConfig, Datasource, DatasourceKind, Endpoint};
use serde_json::json;
use tempfile::tempdir;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Environment variable the bearer-auth test datasources read their token
/// from. [`run_datom`] removes it from the child environment so each test
/// controls whether it is set.
const TOKEN_VAR: &str = "DATOM_TEST_CONNECTIVITY_TOKEN";

/// Run the compiled `datom` binary with `args` in `cwd`, with `envs` set,
/// on the blocking pool so the current-thread test runtime stays free to
/// drive the wiremock server while the child process talks to it.
async fn run_datom(args: &[&str], cwd: &Path, envs: &[(&str, &str)]) -> Output {
    let args: Vec<String> = args.iter().map(ToString::to_string).collect();
    let envs: Vec<(String, String)> = envs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    let cwd = cwd.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_datom"));
        cmd.args(&args).current_dir(&cwd).env_remove(TOKEN_VAR);
        for (key, value) in &envs {
            cmd.env(key, value);
        }
        cmd.output().expect("failed to spawn datom binary")
    })
    .await
    .expect("blocking task panicked")
}

/// Create a temp project containing one API datasource named `name` with
/// `auth`, pointing at `base_url`, with the given `(name, path)` endpoints;
/// returns the project root. The tempdir must be kept alive by the caller.
fn project_with_endpoints(
    tmp: &Path,
    name: &str,
    base_url: String,
    auth: AuthConfig,
    endpoints: &[(&str, &str)],
) -> PathBuf {
    let root = datom_core::init_project(tmp, "proj").unwrap();
    datom_core::save_datasource(
        &root,
        &Datasource {
            name: name.to_string(),
            kind: DatasourceKind::Api(ApiConfig {
                base_url,
                auth,
                endpoints: endpoints
                    .iter()
                    .map(|(name, path)| Endpoint {
                        name: name.to_string(),
                        path: path.to_string(),
                    })
                    .collect(),
            }),
        },
    )
    .unwrap();
    root
}

fn bearer() -> AuthConfig {
    AuthConfig::Bearer {
        env_var: TOKEN_VAR.to_string(),
    }
}

/// The checklist lines printed to stdout.
fn stdout_lines(output: &Output) -> Vec<String> {
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(String::from)
        .collect()
}

/// The step lines of `endpoint`'s section, header excluded.
///
/// Anchoring on the section header keeps these assertions independent of
/// how many data-source-wide steps happen to precede it.
fn section(lines: &[String], endpoint: &str) -> Vec<String> {
    let header = format!("endpoint {endpoint} (");
    let start = lines
        .iter()
        .position(|line| line.starts_with(&header))
        .unwrap_or_else(|| panic!("no section for endpoint `{endpoint}` in {lines:?}"));
    lines[start + 1..]
        .iter()
        .take_while(|line| line.starts_with("    "))
        .cloned()
        .collect()
}

/// Mount a JSON 200 response for `route`.
async fn mount_json(server: &MockServer, route: &str, body: serde_json::Value) {
    Mock::given(method("GET"))
        .and(path(route))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(server)
        .await;
}

#[tokio::test]
async fn test_passes_when_all_contracts_match() {
    let server = MockServer::start().await;
    mount_json(&server, "/users", json!([{"id": 1}])).await;
    mount_json(&server, "/orders", json!({"data": [{"total": 2.5}]})).await;

    let tmp = tempdir().unwrap();
    let root = project_with_endpoints(
        tmp.path(),
        "shop",
        server.uri(),
        AuthConfig::None,
        &[("users", "/users"), ("orders", "/orders")],
    );

    // Record the contract, then test against it.
    let introspect = run_datom(&["datasource", "introspect", "shop"], &root, &[]).await;
    assert!(introspect.status.success());
    let output = run_datom(&["datasource", "test", "shop"], &root, &[]).await;

    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let lines = stdout_lines(&output);
    assert_eq!(lines.len(), 9, "{lines:?}");
    assert_eq!(lines[0], "✓ Config loaded");
    assert_eq!(lines[1], "✓ Secrets resolved (none required)");
    assert_eq!(lines[2], "✓ Endpoints (2)");

    let users = section(&lines, "users");
    assert!(users[0].starts_with("    ✓ Connected (200, "), "{lines:?}");
    assert!(users[0].ends_with("ms)"), "{lines:?}");
    assert_eq!(users[1], "    ✓ Contract unchanged");

    let orders = section(&lines, "orders");
    assert!(orders[0].starts_with("    ✓ Connected (200, "), "{lines:?}");
    assert_eq!(orders[1], "    ✓ Contract unchanged");
}

#[tokio::test]
async fn test_reports_drift_on_one_endpoint() {
    let server = MockServer::start().await;
    mount_json(&server, "/users", json!([{"id": 1}])).await;
    mount_json(&server, "/orders", json!([{"total": 2.5}])).await;

    let tmp = tempdir().unwrap();
    let root = project_with_endpoints(
        tmp.path(),
        "shop",
        server.uri(),
        AuthConfig::None,
        &[("users", "/users"), ("orders", "/orders")],
    );
    let introspect = run_datom(&["datasource", "introspect", "shop"], &root, &[]).await;
    assert!(introspect.status.success());

    // The API drifts: `users.id` becomes a string.
    server.reset().await;
    mount_json(&server, "/users", json!([{"id": "u-1"}])).await;
    mount_json(&server, "/orders", json!([{"total": 2.5}])).await;

    let output = run_datom(&["datasource", "test", "shop"], &root, &[]).await;

    assert!(!output.status.success());
    let lines = stdout_lines(&output);
    let users = section(&lines, "users");
    assert_eq!(users[1], "    ✗ Contract changed:", "{lines:?}");
    assert_eq!(users[2], "        ~ id: int -> string", "{lines:?}");
    assert_eq!(
        section(&lines, "orders")[1],
        "    ✓ Contract unchanged",
        "{lines:?}"
    );
}

#[tokio::test]
async fn test_flags_endpoint_missing_from_types_file() {
    let server = MockServer::start().await;
    mount_json(&server, "/users", json!([{"id": 1}])).await;
    mount_json(&server, "/orders", json!([{"total": 2.5}])).await;

    let tmp = tempdir().unwrap();
    let root = project_with_endpoints(
        tmp.path(),
        "shop",
        server.uri(),
        AuthConfig::None,
        &[("users", "/users")],
    );
    let introspect = run_datom(&["datasource", "introspect", "shop"], &root, &[]).await;
    assert!(introspect.status.success());

    // A new endpoint appears after the contract was recorded.
    let add = run_datom(
        &[
            "datasource",
            "endpoint",
            "add",
            "shop",
            "orders",
            "--path",
            "/orders",
        ],
        &root,
        &[],
    )
    .await;
    assert!(add.status.success());

    let output = run_datom(&["datasource", "test", "shop"], &root, &[]).await;

    assert!(!output.status.success());
    let lines = stdout_lines(&output);
    assert_eq!(
        section(&lines, "users")[1],
        "    ✓ Contract unchanged",
        "{lines:?}"
    );
    let orders = section(&lines, "orders");
    assert_eq!(orders[1], "    ✗ Contract changed:", "{lines:?}");
    assert!(
        orders[2].contains("table `orders` is missing from the types file"),
        "{lines:?}"
    );
}

#[tokio::test]
async fn test_flags_recorded_table_without_endpoint() {
    let server = MockServer::start().await;
    mount_json(&server, "/users", json!([{"id": 1}])).await;
    mount_json(&server, "/orders", json!([{"total": 2.5}])).await;

    let tmp = tempdir().unwrap();
    let root = project_with_endpoints(
        tmp.path(),
        "shop",
        server.uri(),
        AuthConfig::None,
        &[("users", "/users"), ("orders", "/orders")],
    );
    let introspect = run_datom(&["datasource", "introspect", "shop"], &root, &[]).await;
    assert!(introspect.status.success());

    // The endpoint goes away but its table stays in the types file.
    let remove = run_datom(
        &["datasource", "endpoint", "remove", "shop", "orders"],
        &root,
        &[],
    )
    .await;
    assert!(remove.status.success());

    let output = run_datom(&["datasource", "test", "shop"], &root, &[]).await;

    assert!(!output.status.success());
    let lines = stdout_lines(&output);
    assert_eq!(
        section(&lines, "users")[1],
        "    ✓ Contract unchanged",
        "{lines:?}"
    );
    assert!(
        lines.contains(&"✗ table `orders` has no matching endpoint".to_string()),
        "{lines:?}"
    );
}

#[tokio::test]
async fn test_skips_endpoints_when_env_var_is_missing() {
    let server = MockServer::start().await;
    // Secrets are checked before any request, so the API is never hit.
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&server)
        .await;

    let tmp = tempdir().unwrap();
    let root = project_with_endpoints(
        tmp.path(),
        "api",
        server.uri(),
        bearer(),
        &[("users", "/users")],
    );

    let output = run_datom(&["datasource", "test", "api"], &root, &[]).await;

    assert!(!output.status.success());
    let lines = stdout_lines(&output);
    assert_eq!(lines[0], "✓ Config loaded");
    assert_eq!(
        lines[1],
        format!(
            "✗ Secrets resolved — cannot read the secrets this data source needs: \
             `{TOKEN_VAR}` is not set"
        )
    );
    assert_eq!(lines[2], "- Endpoints (skipped)");
    let users = section(&lines, "users");
    assert_eq!(users[0], "    - Connected (skipped)");
    assert_eq!(users[1], "    - Contract (skipped)");
}

#[tokio::test]
async fn test_reports_auth_failure_per_endpoint() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/users"))
        .respond_with(ResponseTemplate::new(401).set_body_string("invalid token"))
        .expect(1)
        .mount(&server)
        .await;

    let tmp = tempdir().unwrap();
    let root = project_with_endpoints(
        tmp.path(),
        "api",
        server.uri(),
        bearer(),
        &[("users", "/users")],
    );

    let output = run_datom(
        &["datasource", "test", "api"],
        &root,
        &[(TOKEN_VAR, "expired")],
    )
    .await;

    assert!(!output.status.success());
    let lines = stdout_lines(&output);
    let users = section(&lines, "users");
    assert!(users[0].starts_with("    ✗ Connected — "), "{lines:?}");
    assert!(users[0].contains("401"), "{lines:?}");
    assert!(users[0].contains("invalid token"), "{lines:?}");
    assert_eq!(users[1], "    - Contract (skipped)");
}

#[tokio::test]
async fn test_reports_non_json_and_missing_types_file() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/page"))
        .respond_with(ResponseTemplate::new(200).set_body_raw("<html>not json</html>", "text/html"))
        .expect(1)
        .mount(&server)
        .await;
    mount_json(&server, "/users", json!([{"id": 1}])).await;

    let tmp = tempdir().unwrap();
    // Never introspected: no types file exists.
    let root = project_with_endpoints(
        tmp.path(),
        "api",
        server.uri(),
        AuthConfig::None,
        &[("page", "/page"), ("users", "/users")],
    );

    let output = run_datom(&["datasource", "test", "api"], &root, &[]).await;

    assert!(!output.status.success());
    let lines = stdout_lines(&output);
    let page = section(&lines, "page");
    assert!(
        page[0].starts_with("    ✗ Connected — HTTP 200 but"),
        "{lines:?}"
    );
    assert!(page[0].contains("not valid JSON"), "{lines:?}");
    assert!(page[0].contains("text/html"), "{lines:?}");
    assert_eq!(page[1], "    - Contract (skipped)");

    let users = section(&lines, "users");
    assert_eq!(users[1], "    ✗ Contract changed:", "{lines:?}");
    assert!(users[2].contains("no types file"), "{lines:?}");
    assert!(users[2].contains("introspect"), "{lines:?}");
}

#[tokio::test]
async fn test_treats_an_empty_result_set_as_no_evidence() {
    let server = MockServer::start().await;
    mount_json(&server, "/users", json!([{"id": 1}])).await;

    let tmp = tempdir().unwrap();
    let root = project_with_endpoints(
        tmp.path(),
        "shop",
        server.uri(),
        AuthConfig::None,
        &[("users", "/users")],
    );
    let introspect = run_datom(&["datasource", "introspect", "shop"], &root, &[]).await;
    assert!(introspect.status.success());

    // A quiet day: the endpoint returns no records at all.
    server.reset().await;
    mount_json(&server, "/users", json!([])).await;

    let output = run_datom(&["datasource", "test", "shop"], &root, &[]).await;

    // Zero records cannot contradict the contract, so this is not drift —
    // reporting `- id: int` here would be a false alarm.
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let lines = stdout_lines(&output);
    let users = section(&lines, "users");
    assert!(users[0].starts_with("    ✓ Connected (200, "), "{lines:?}");
    assert_eq!(users[1], "    ✓ Contract (no records sampled)", "{lines:?}");
}

#[tokio::test]
async fn test_fails_when_no_endpoints_are_configured() {
    let server = MockServer::start().await;
    // Nothing to test means nothing to request.
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&server)
        .await;

    let tmp = tempdir().unwrap();
    let root = project_with_endpoints(tmp.path(), "empty", server.uri(), AuthConfig::None, &[]);

    let output = run_datom(&["datasource", "test", "empty"], &root, &[]).await;

    // A data source with no endpoints must not report a vacuous pass.
    assert!(!output.status.success());
    let lines = stdout_lines(&output);
    assert_eq!(lines[0], "✓ Config loaded");
    assert_eq!(lines[1], "✓ Secrets resolved (none required)");
    assert!(lines[2].starts_with("✗ Endpoints — "), "{lines:?}");
    assert!(lines[2].contains("no endpoints configured"), "{lines:?}");
    assert!(
        lines[2].contains("datom datasource endpoint add empty"),
        "{lines:?}"
    );
    assert_eq!(lines.len(), 3, "{lines:?}");
}

#[tokio::test]
async fn test_fails_config_step_when_datasource_missing() {
    let tmp = tempdir().unwrap();
    let root = datom_core::init_project(tmp.path(), "proj").unwrap();

    let output = run_datom(&["datasource", "test", "nope"], &root, &[]).await;

    assert!(!output.status.success());
    let lines = stdout_lines(&output);
    assert_eq!(
        lines[0],
        "✗ Config loaded — data source `nope` was not found"
    );
    assert_eq!(lines[1], "- Secrets resolved (skipped)");
    assert_eq!(lines[2], "- Endpoints (skipped)");
    assert_eq!(lines.len(), 3, "no endpoint sections without a config");
}
