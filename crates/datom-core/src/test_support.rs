//! Helpers shared by the core test modules.
//!
//! The modules that orchestrate work — [`crate::connectivity`] and
//! [`crate::introspect`] — need a project on disk and an API to talk to, so
//! their tests all start the same way.

use std::path::PathBuf;

use serde_json::Value;
use tempfile::{TempDir, tempdir};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use crate::datasource::{ApiConfig, AuthConfig, Datasource, DatasourceKind, Endpoint};
use crate::project::init_project;
use crate::save_datasource;

/// Name of the data source every helper here creates.
pub(crate) const DATASOURCE: &str = "api";

/// A temp project holding one API data source named [`DATASOURCE`],
/// pointing at `base_url` with the given `(name, path)` endpoints.
///
/// The returned [`TempDir`] must be kept alive for the project to exist.
pub(crate) fn project(
    base_url: &str,
    auth: AuthConfig,
    endpoints: &[(&str, &str)],
) -> (TempDir, PathBuf) {
    let tmp = tempdir().unwrap();
    let root = init_project(tmp.path(), "proj").unwrap();
    save_datasource(
        &root,
        &Datasource {
            name: DATASOURCE.to_string(),
            kind: DatasourceKind::Api(ApiConfig {
                base_url: base_url.to_string(),
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
    (tmp, root)
}

/// Mount a JSON 200 response for `route`.
pub(crate) async fn mount_json(server: &MockServer, route: &str, body: Value) {
    Mock::given(method("GET"))
        .and(path(route))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(server)
        .await;
}
