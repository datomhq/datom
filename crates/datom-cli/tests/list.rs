//! End-to-end tests of `datom datasource list`: spawns the real binary in a
//! temp project and checks the table output — offline by default, with a
//! live CONNECTED column under `--test-connection`.

use std::path::Path;
use std::process::{Command, Output};

use datom_core::{ApiConfig, AuthConfig, Datasource, DatasourceKind, Endpoint};
use serde_json::json;
use tempfile::tempdir;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Run the compiled `datom` binary with `args` in `cwd` on the blocking
/// pool, so the current-thread test runtime stays free to drive the
/// wiremock servers while the child process talks to them.
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

fn save(root: &Path, name: &str, base_url: &str, endpoints: &[(&str, &str)]) {
    datom_core::save_datasource(
        root,
        &Datasource {
            name: name.to_string(),
            kind: DatasourceKind::Api(ApiConfig {
                base_url: base_url.to_string(),
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
}

/// The table cells of each stdout line.
fn table(output: &Output) -> Vec<Vec<String>> {
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|line| line.split_whitespace().map(String::from).collect())
        .collect()
}

#[tokio::test]
async fn list_shows_name_kind_url_and_types_without_touching_the_network() {
    let server = MockServer::start().await;
    // Without --test-connection, listing must not make any request.
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&server)
        .await;

    let tmp = tempdir().unwrap();
    let root = datom_core::init_project(tmp.path(), "proj").unwrap();
    let users_url = format!("{}/users", server.uri());
    save(&root, "users", &users_url, &[("a", "/a"), ("b", "/b")]);
    save(&root, "github", &server.uri(), &[]);

    // Only `users` has been introspected.
    let ty = datom_core::infer_value("users", &json!({"id": 1}));
    datom_core::save_tables(&root, "users", std::slice::from_ref(&ty)).unwrap();

    let output = run_datom(&["datasource", "list"], &root).await;

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let lines = table(&output);
    assert_eq!(lines.len(), 3, "{lines:?}");
    assert_eq!(lines[0], ["NAME", "KIND", "URL", "ENDPOINTS", "TYPES"]);
    assert_eq!(lines[1], ["github", "api", &server.uri(), "0", "no"]);
    assert_eq!(lines[2], ["users", "api", &users_url, "2", "yes"]);
}

#[tokio::test]
async fn list_with_test_connection_adds_connected_column() {
    // `users` answers with JSON and passes the connectivity test; `github`
    // answers 500 and fails it.
    let healthy = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
        .expect(1)
        .mount(&healthy)
        .await;
    let broken = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(500).set_body_string("down"))
        .expect(1)
        .mount(&broken)
        .await;

    let tmp = tempdir().unwrap();
    let root = datom_core::init_project(tmp.path(), "proj").unwrap();
    save(&root, "users", &healthy.uri(), &[("root", "/")]);
    save(&root, "github", &broken.uri(), &[("root", "/")]);
    // Nothing was ever requested for this one, so it cannot be "connected".
    save(&root, "empty", &healthy.uri(), &[]);

    // Only `users` has been introspected.
    let ty = datom_core::infer_value("users", &json!({"id": 1}));
    datom_core::save_tables(&root, "users", std::slice::from_ref(&ty)).unwrap();

    let output = run_datom(&["datasource", "list", "--test-connection"], &root).await;

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let lines = table(&output);
    assert_eq!(lines.len(), 4, "{lines:?}");
    assert_eq!(
        lines[0],
        ["NAME", "KIND", "URL", "ENDPOINTS", "TYPES", "CONNECTED"]
    );
    assert_eq!(lines[1], ["empty", "api", &healthy.uri(), "0", "no", "no"]);
    assert_eq!(lines[2], ["github", "api", &broken.uri(), "1", "no", "no"]);
    assert_eq!(
        lines[3],
        ["users", "api", &healthy.uri(), "1", "yes", "yes"]
    );
}

#[tokio::test]
async fn list_reports_empty_project() {
    let tmp = tempdir().unwrap();
    let root = datom_core::init_project(tmp.path(), "proj").unwrap();

    let output = run_datom(&["datasource", "list"], &root).await;

    assert!(output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("No data sources configured"),
        "stdout: {}",
        String::from_utf8_lossy(&output.stdout)
    );
}
