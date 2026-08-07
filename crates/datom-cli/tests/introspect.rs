//! End-to-end tests of `datom datasource introspect`: spawns the real
//! binary in a temp project against a wiremock API and checks the
//! per-endpoint checklist and the written `.types.datom` file.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use datom_core::{ApiConfig, AuthConfig, Datasource, DatasourceKind, Endpoint};
use serde_json::json;
use tempfile::tempdir;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Run the compiled `datom` binary with `args` in `cwd`.
///
/// Runs on the blocking pool so the current-thread test runtime stays free
/// to drive the wiremock server while the child process talks to it.
async fn run_datom(args: &[&str], cwd: &Path) -> Output {
    let args: Vec<String> = args.iter().map(ToString::to_string).collect();
    let cwd = cwd.to_path_buf();
    tokio::task::spawn_blocking(move || {
        Command::new(env!("CARGO_BIN_EXE_datom"))
            .args(&args)
            .current_dir(&cwd)
            .output()
            .expect("failed to spawn datom binary")
    })
    .await
    .expect("blocking task panicked")
}

/// Create a temp project containing one no-auth API datasource named `name`
/// pointing at `base_url` with the given `(name, path)` endpoints; returns
/// the project root. The tempdir must be kept alive by the caller.
fn project_with_endpoints(
    tmp: &Path,
    name: &str,
    base_url: String,
    endpoints: &[(&str, &str)],
) -> PathBuf {
    let root = datom_core::init_project(tmp, "proj").unwrap();
    datom_core::save_datasource(
        &root,
        &Datasource {
            name: name.to_string(),
            kind: DatasourceKind::Api(ApiConfig {
                base_url,
                auth: AuthConfig::None,
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

/// Mount a JSON 200 response for `route`.
async fn mount(server: &MockServer, route: &str, body: serde_json::Value) {
    Mock::given(method("GET"))
        .and(path(route))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(server)
        .await;
}

fn types_file(root: &Path, datasource: &str) -> PathBuf {
    root.join("datasources")
        .join(format!("{datasource}.types.datom"))
}

#[tokio::test]
async fn introspect_writes_all_endpoint_schemas_into_one_file() {
    let server = MockServer::start().await;
    // `users` responds with an envelope; its nested `address` record
    // structurally differs from the one under `orders`.
    Mock::given(method("GET"))
        .and(path("/users"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": [
                {"id": 1, "email": "ada@example.com", "address": {"city": "London", "zip": "E1"}},
                {"id": 2, "email": null},
            ]
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/orders"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            {"id": 7, "address": {"zip": 11}},
        ])))
        .expect(1)
        .mount(&server)
        .await;

    let tmp = tempdir().unwrap();
    let root = project_with_endpoints(
        tmp.path(),
        "shop",
        server.uri(),
        &[("users", "/users"), ("orders", "/orders")],
    );

    let output = run_datom(&["datasource", "introspect", "shop"], &root).await;

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines[0], "✓ users (2 records)", "{stdout}");
    assert_eq!(lines[1], "✓ orders (1 record)", "{stdout}");
    assert!(lines[2].starts_with("Types written to "), "{stdout}");

    // One file holds both endpoint schemas; the colliding nested record
    // name is prefixed with its endpoint.
    let types = fs::read_to_string(types_file(&root, "shop")).unwrap();
    assert!(types.contains("table users {"), "{types}");
    assert!(types.contains("table orders {"), "{types}");
    // `address` is claimed by both endpoints with different shapes, so
    // neither keeps the bare name.
    assert!(!types.contains("record address {"), "{types}");
    assert!(types.contains("record users.address {"), "{types}");
    assert!(types.contains("record orders.address {"), "{types}");
    assert!(types.contains("address: orders.address\n"), "{types}");

    // The written file parses back into both table schemas.
    assert_eq!(datom_core::parse_tables(&types).unwrap().len(), 2);
}

#[tokio::test]
async fn introspect_continues_past_failures_and_writes_nothing() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/users"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([{"id": 1}])))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/boom"))
        .respond_with(ResponseTemplate::new(500).set_body_string("upstream exploded"))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/page"))
        .respond_with(ResponseTemplate::new(200).set_body_raw("<html>not json</html>", "text/html"))
        .expect(1)
        .mount(&server)
        .await;

    let tmp = tempdir().unwrap();
    let root = project_with_endpoints(
        tmp.path(),
        "mixed",
        server.uri(),
        &[
            ("users", "/users"),
            ("bad_http", "/boom"),
            ("bad_json", "/page"),
        ],
    );

    let output = run_datom(&["datasource", "introspect", "mixed"], &root).await;

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let lines: Vec<&str> = stdout.lines().collect();
    // Every endpoint is attempted and reported, failures included.
    assert_eq!(lines[0], "✓ users (1 record)", "{stdout}");
    assert!(lines[1].starts_with("✗ bad_http — "), "{stdout}");
    assert!(lines[1].contains("500"), "{stdout}");
    assert!(lines[1].contains("upstream exploded"), "{stdout}");
    assert!(lines[2].starts_with("✗ bad_json — "), "{stdout}");
    assert!(lines[2].contains("not valid JSON"), "{stdout}");
    assert!(lines[2].contains("text/html"), "{stdout}");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("no types file written"), "{stderr}");
    assert!(
        !types_file(&root, "mixed").exists(),
        "no partial types file may be written when an endpoint fails"
    );
}

#[tokio::test]
async fn introspect_writes_a_types_file_test_can_read_back() {
    let server = MockServer::start().await;
    // The records of the `users` endpoint contain a nested `users` object,
    // so the hoisted record's name would collide with the table's.
    Mock::given(method("GET"))
        .and(path("/users"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            {"id": 1, "users": {"x": 2}},
        ])))
        .mount(&server)
        .await;

    let tmp = tempdir().unwrap();
    let root = project_with_endpoints(tmp.path(), "shop", server.uri(), &[("users", "/users")]);

    let introspect = run_datom(&["datasource", "introspect", "shop"], &root).await;
    assert!(
        introspect.status.success(),
        "stdout: {}",
        String::from_utf8_lossy(&introspect.stdout)
    );

    // The written file must be readable by the contract check; before the
    // record was renamed this failed with a bogus "recursive reference".
    let types = fs::read_to_string(types_file(&root, "shop")).unwrap();
    datom_core::parse_tables(&types)
        .unwrap_or_else(|err| panic!("introspect wrote an unreadable types file: {err}\n{types}"));

    let test = run_datom(&["datasource", "test", "shop"], &root).await;
    let stdout = String::from_utf8_lossy(&test.stdout);
    assert!(
        test.status.success(),
        "stdout: {stdout}\nstderr: {}",
        String::from_utf8_lossy(&test.stderr)
    );
    assert!(stdout.contains("✓ Contract unchanged"), "{stdout}");
}

#[tokio::test]
async fn introspect_tolerates_an_endpoint_with_no_records() {
    let server = MockServer::start().await;
    mount(&server, "/users", json!([{"id": 1}])).await;
    // An empty result set: well-formed, but nothing to learn from.
    mount(&server, "/orders", json!([])).await;

    let tmp = tempdir().unwrap();
    let root = project_with_endpoints(
        tmp.path(),
        "shop",
        server.uri(),
        &[("users", "/users"), ("orders", "/orders")],
    );

    let output = run_datom(&["datasource", "introspect", "shop"], &root).await;

    // The empty endpoint must not sink the whole run.
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("✓ users (1 record)"), "{stdout}");
    assert!(
        stdout.contains("- orders (no records sampled; nothing recorded yet)"),
        "{stdout}"
    );

    // The endpoint that did return records is recorded; the empty one has
    // no block, because inventing one would assert a shape we never saw.
    let types = fs::read_to_string(types_file(&root, "shop")).unwrap();
    assert!(types.contains("table users {"), "{types}");
    assert!(!types.contains("table orders {"), "{types}");
}

#[tokio::test]
async fn introspect_keeps_a_recorded_schema_when_an_endpoint_goes_empty() {
    let server = MockServer::start().await;
    mount(&server, "/orders", json!([{"total": 2.5}])).await;

    let tmp = tempdir().unwrap();
    let root = project_with_endpoints(tmp.path(), "shop", server.uri(), &[("orders", "/orders")]);

    let first = run_datom(&["datasource", "introspect", "shop"], &root).await;
    assert!(first.status.success());
    let recorded = fs::read_to_string(types_file(&root, "shop")).unwrap();
    assert!(recorded.contains("total: float"), "{recorded}");

    // A quiet day: the endpoint now returns nothing.
    server.reset().await;
    mount(&server, "/orders", json!([])).await;
    let second = run_datom(&["datasource", "introspect", "shop"], &root).await;

    assert!(
        second.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&second.stderr)
    );
    let stdout = String::from_utf8_lossy(&second.stdout);
    assert!(
        stdout.contains("- orders (no records sampled; keeping the recorded schema)"),
        "{stdout}"
    );
    // The contract survives rather than being erased by an empty response.
    assert_eq!(
        fs::read_to_string(types_file(&root, "shop")).unwrap(),
        recorded
    );
}

#[tokio::test]
async fn an_empty_endpoint_does_not_rename_another_endpoints_records() {
    // Both endpoints nest an `address` of different shapes, so both are
    // qualified. When one goes empty its block is carried forward — and
    // must not disturb the other's naming.
    let server = MockServer::start().await;
    mount(&server, "/users", json!([{"address": {"city": "L"}}])).await;
    mount(&server, "/orders", json!([{"address": {"zip": 3}}])).await;

    let tmp = tempdir().unwrap();
    let root = project_with_endpoints(
        tmp.path(),
        "shop",
        server.uri(),
        &[("users", "/users"), ("orders", "/orders")],
    );

    assert!(
        run_datom(&["datasource", "introspect", "shop"], &root)
            .await
            .status
            .success()
    );
    let before = fs::read_to_string(types_file(&root, "shop")).unwrap();
    assert!(before.contains("record users.address {"), "{before}");
    assert!(before.contains("record orders.address {"), "{before}");

    // `users` goes quiet; `orders` is untouched upstream.
    server.reset().await;
    mount(&server, "/users", json!([])).await;
    mount(&server, "/orders", json!([{"address": {"zip": 3}}])).await;
    assert!(
        run_datom(&["datasource", "introspect", "shop"], &root)
            .await
            .status
            .success()
    );

    // Nothing changed upstream that the file should reflect, so the file
    // must not change at all.
    let after = fs::read_to_string(types_file(&root, "shop")).unwrap();
    assert_eq!(after, before, "an empty endpoint churned the types file");
}

#[tokio::test]
async fn introspect_still_rejects_a_response_that_is_not_records() {
    let server = MockServer::start().await;
    // Non-empty, but scalars: there is no table schema here.
    mount(&server, "/nums", json!([1, 2, 3])).await;

    let tmp = tempdir().unwrap();
    let root = project_with_endpoints(tmp.path(), "api", server.uri(), &[("nums", "/nums")]);

    let output = run_datom(&["datasource", "introspect", "api"], &root).await;

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("✗ nums — "), "{stdout}");
    assert!(stdout.contains("does not describe records"), "{stdout}");
    assert!(!types_file(&root, "api").exists());
}

#[tokio::test]
async fn introspect_errors_when_datasource_missing() {
    let tmp = tempdir().unwrap();
    let root = datom_core::init_project(tmp.path(), "proj").unwrap();

    let output = run_datom(&["datasource", "introspect", "nope"], &root).await;

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("was not found"), "stderr: {stderr}");
}

#[tokio::test]
async fn introspect_errors_when_datasource_has_no_endpoints() {
    let tmp = tempdir().unwrap();
    let root = project_with_endpoints(tmp.path(), "empty", "http://127.0.0.1:1".to_string(), &[]);

    let output = run_datom(&["datasource", "introspect", "empty"], &root).await;

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("no endpoints configured"),
        "stderr: {stderr}"
    );
    assert!(!types_file(&root, "empty").exists());
}
