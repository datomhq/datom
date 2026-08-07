//! Implementation of the `datom datasource` commands.

use std::env;
use std::path::PathBuf;

use anyhow::{Context, Result, anyhow, bail};
use datom_core::datasource::{datasource_path, validate_datasource_name};
use datom_core::{
    ApiConfig, AuthConfig, CoreError, Datasource, DatasourceKind, Endpoint, EndpointOutcome,
    StepKind, StepOutcome,
};
use dialoguer::theme::ColorfulTheme;
use dialoguer::{Confirm, Input, Select};
use url::Url;

/// Locate the root of the enclosing datom project.
fn require_project_root() -> Result<PathBuf> {
    let cwd = env::current_dir().context("could not determine current directory")?;
    datom_core::find_project_root(&cwd).ok_or_else(|| {
        anyhow!(
            "not inside a datom project: no `datom.toml` found in `{}` or any parent directory.\n\
             Create a project with `datom init <name>` and run this command inside it.",
            cwd.display()
        )
    })
}

/// `datom datasource add <name> --api`: configure and save a new API data source.
pub fn add(name: &str, api: bool) -> Result<()> {
    if !api {
        bail!("specify the data source kind: pass `--api` (the only kind supported so far)");
    }

    let root = require_project_root()?;
    validate_datasource_name(name)?;
    if datasource_path(&root, name).exists() {
        return Err(CoreError::DataSourceExists(name.to_string()).into());
    }

    let theme = ColorfulTheme::default();
    let base_url = prompt_url(&theme, "Base URL")?;
    let auth = prompt_auth(&theme)?;
    let endpoints = prompt_endpoints(&theme)?;

    let datasource = Datasource {
        name: name.to_string(),
        kind: DatasourceKind::Api(ApiConfig {
            base_url,
            auth,
            endpoints,
        }),
    };
    let path = datom_core::save_datasource(&root, &datasource)?;
    println!("Created data source `{name}` at {}", path.display());
    Ok(())
}

/// `datom datasource introspect <name>`: fetch every endpoint, infer a
/// schema per endpoint, and write them all to
/// `datasources/<name>.types.datom`.
pub async fn introspect(name: &str) -> Result<()> {
    let root = require_project_root()?;
    let report = datom_core::introspect_datasource(&root, name)
        .await
        .with_context(|| format!("could not introspect data source `{name}`"))?;

    for endpoint in &report.endpoints {
        match &endpoint.outcome {
            EndpointOutcome::Inferred { record_count } => {
                let noun = if *record_count == 1 {
                    "record"
                } else {
                    "records"
                };
                println!("✓ {} ({record_count} {noun})", endpoint.name);
            }
            EndpointOutcome::NoRecords { kept_schema } => {
                let note = if *kept_schema {
                    "no records sampled; keeping the recorded schema"
                } else {
                    "no records sampled; nothing recorded yet"
                };
                println!("- {} ({note})", endpoint.name);
            }
            EndpointOutcome::Failed(message) => println!("✗ {} — {message}", endpoint.name),
        }
    }

    match &report.types_path {
        Some(path) => {
            println!("Types written to {}", path.display());
            Ok(())
        }
        None if report.passed() => {
            println!("No schemas recorded: no endpoint returned any records.");
            Ok(())
        }
        None => {
            // failed introspect but types file already exists
            if datom_core::types_path(&root, name).exists() {
                eprintln!(
                    "note: the existing types file was left unchanged and may now be out of date"
                );
            }
            bail!(
                "introspection failed for at least one endpoint of `{name}`; no types file written"
            )
        }
    }
}

/// `datom datasource endpoint add <datasource> <name> --path <path>`
pub fn endpoint_add(datasource: &str, name: &str, path: &str) -> Result<()> {
    let root = require_project_root()?;
    datom_core::add_endpoint(
        &root,
        datasource,
        Endpoint {
            name: name.to_string(),
            path: path.to_string(),
        },
    )?;
    println!("Added endpoint `{name}` to data source `{datasource}`.");
    Ok(())
}

/// `datom datasource endpoint remove <datasource> <name>`
pub fn endpoint_remove(datasource: &str, name: &str) -> Result<()> {
    let root = require_project_root()?;
    datom_core::remove_endpoint(&root, datasource, name)?;
    println!("Removed endpoint `{name}` from data source `{datasource}`.");
    Ok(())
}

/// `datom datasource endpoint list <datasource>`
pub fn endpoint_list(datasource: &str) -> Result<()> {
    let root = require_project_root()?;
    let loaded = datom_core::load_datasource(&root, datasource)?;
    let DatasourceKind::Api(api) = &loaded.kind;

    if api.endpoints.is_empty() {
        println!(
            "No endpoints configured. Add one with \
             `datom datasource endpoint add {datasource} <name> --path <path>`."
        );
        return Ok(());
    }

    let rows: Vec<Vec<String>> = api
        .endpoints
        .iter()
        .map(|endpoint| vec![endpoint.name.clone(), endpoint.path.clone()])
        .collect();
    print_table(&["NAME", "PATH"], &rows);
    Ok(())
}

/// `datom datasource test <name>`: check config and secrets once, then
/// connectivity and schema contract per endpoint.
pub async fn test(name: &str) -> Result<()> {
    let root = require_project_root()?;
    let report = datom_core::test_datasource(&root, name).await;

    for step in &report.steps {
        match &step.outcome {
            StepOutcome::Passed(None) => println!("✓ {}", step.kind.label()),
            StepOutcome::Passed(Some(detail)) => println!("✓ {} ({detail})", step.kind.label()),
            StepOutcome::Failed(message) => println!("✗ {} — {message}", step.kind.label()),
            StepOutcome::Skipped => println!("- {} (skipped)", step.kind.label()),
        }
    }

    for endpoint in &report.endpoints {
        println!("endpoint {} ({})", endpoint.name, endpoint.path);
        for step in &endpoint.steps {
            print_endpoint_step(step);
        }
    }

    for table in &report.unmatched_tables {
        println!("✗ table `{table}` has no matching endpoint");
    }

    if !report.passed() {
        bail!("data source `{name}` failed its connectivity test");
    }
    Ok(())
}

/// Print one per-endpoint step, indented under its `endpoint` header. A
/// failed contract check lists one schema change per line.
fn print_endpoint_step(step: &datom_core::TestStep) {
    if step.kind == StepKind::Contract {
        match &step.outcome {
            StepOutcome::Passed(None) => println!("    ✓ Contract unchanged"),
            StepOutcome::Passed(Some(detail)) => println!("    ✓ Contract ({detail})"),
            StepOutcome::Failed(message) => {
                println!("    ✗ Contract changed:");
                for line in message.lines() {
                    println!("        {line}");
                }
            }
            StepOutcome::Skipped => println!("    - Contract (skipped)"),
        }
        return;
    }
    match &step.outcome {
        StepOutcome::Passed(None) => println!("    ✓ {}", step.kind.label()),
        StepOutcome::Passed(Some(detail)) => println!("    ✓ {} ({detail})", step.kind.label()),
        StepOutcome::Failed(message) => println!("    ✗ {} — {message}", step.kind.label()),
        StepOutcome::Skipped => println!("    - {} (skipped)", step.kind.label()),
    }
}

/// `datom datasource list`: print the data sources of the enclosing project.
/// With `test_connection`, also run each source's connectivity test (live
/// requests) and report the result in a CONNECTED column.
pub async fn list(test_connection: bool) -> Result<()> {
    let root = require_project_root()?;
    let listing = datom_core::list_datasources(&root)?;
    let datasources = &listing.datasources;

    if datasources.is_empty() && listing.invalid.is_empty() {
        println!("No data sources configured. Add one with `datom datasource add <name> --api`.");
        return Ok(());
    }

    // Check every datasource concurrently so one slow API does not stall
    // the whole listing; results land in `connected` in datasource order.
    // The already-parsed config is handed over rather than re-read, and
    // the contract half is skipped — this column only reports reachability.
    let connected = if test_connection {
        let mut checks = tokio::task::JoinSet::new();
        for (index, datasource) in datasources.iter().enumerate() {
            let DatasourceKind::Api(api) = &datasource.kind;
            let api = api.clone();
            checks.spawn(async move { (index, datom_core::check_connectivity(&api).await) });
        }
        let mut connected = vec![false; datasources.len()];
        while let Some(result) = checks.join_next().await {
            let (index, passed) = result.expect("connectivity check task panicked");
            connected[index] = passed;
        }
        Some(connected)
    } else {
        None
    };

    let yes_no = |value: bool| if value { "yes" } else { "no" }.to_string();
    let mut headers = vec!["NAME", "KIND", "URL", "ENDPOINTS", "TYPES"];
    if test_connection {
        headers.push("CONNECTED");
    }
    let rows: Vec<Vec<String>> = datasources
        .iter()
        .enumerate()
        .map(|(index, datasource)| {
            let DatasourceKind::Api(api) = &datasource.kind;
            let mut row = vec![
                datasource.name.clone(),
                "api".to_string(),
                api.base_url.clone(),
                api.endpoints.len().to_string(),
                yes_no(datom_core::types_path(&root, &datasource.name).exists()),
            ];
            if let Some(connected) = &connected {
                row.push(yes_no(connected[index]));
            }
            row
        })
        .collect();

    if !rows.is_empty() {
        print_table(&headers, &rows);
    }

    // A file that cannot be read is reported beside the ones that can.
    for (name, reason) in &listing.invalid {
        eprintln!("✗ {name}: {reason}");
    }
    if !listing.invalid.is_empty() {
        bail!(
            "{} data source file(s) could not be read",
            listing.invalid.len()
        );
    }
    Ok(())
}

/// Print a two-space-separated table, padding every column but the last so
/// lines carry no trailing whitespace.
fn print_table(headers: &[&str], rows: &[Vec<String>]) {
    let widths: Vec<usize> = headers
        .iter()
        .enumerate()
        .map(|(col, header)| {
            rows.iter()
                .map(|row| row[col].len())
                .chain([header.len()])
                .max()
                .expect("headers make the iterator non-empty")
        })
        .collect();

    let render = |cells: &mut dyn Iterator<Item = &str>| -> String {
        let mut line = String::new();
        for (cell, &width) in cells.zip(&widths) {
            line.push_str(&format!("{cell:<width$}  "));
        }
        line.truncate(line.trim_end().len());
        line
    };

    println!("{}", render(&mut headers.iter().copied()));
    for row in rows {
        println!("{}", render(&mut row.iter().map(String::as_str)));
    }
}

/// Ask which auth scheme to use, then collect its variant-specific fields.
fn prompt_auth(theme: &ColorfulTheme) -> Result<AuthConfig> {
    let options = [
        "None",
        "API Key",
        "Bearer token",
        "OAuth2 client credentials",
    ];
    let selection = Select::with_theme(theme)
        .with_prompt("Authentication")
        .items(options)
        .default(0)
        .interact()?;

    let auth = match selection {
        0 => AuthConfig::None,
        1 => AuthConfig::ApiKey {
            header_name: Input::with_theme(theme)
                .with_prompt("Header name")
                .default("X-Api-Key".to_string())
                .interact_text()?,
            env_var: prompt_env_var(theme, "API key environment variable")?,
        },
        2 => AuthConfig::Bearer {
            env_var: prompt_env_var(theme, "Token environment variable")?,
        },
        3 => AuthConfig::OAuth2ClientCredentials {
            token_url: prompt_url(theme, "Token URL")?,
            client_id_env: prompt_env_var(theme, "Client ID environment variable")?,
            client_secret_env: prompt_env_var(theme, "Client secret environment variable")?,
            scopes: prompt_scopes(theme)?,
        },
        _ => unreachable!("select returned out-of-range index"),
    };
    Ok(auth)
}

/// Repeatedly offer to add endpoints (name and path). When none are added,
/// default to a single `default` endpoint with an empty path.
fn prompt_endpoints(theme: &ColorfulTheme) -> Result<Vec<Endpoint>> {
    let mut endpoints: Vec<Endpoint> = Vec::new();
    loop {
        let prompt = if endpoints.is_empty() {
            "Add an endpoint?"
        } else {
            "Add another endpoint?"
        };
        if !Confirm::with_theme(theme)
            .with_prompt(prompt)
            .default(endpoints.is_empty())
            .interact()?
        {
            break;
        }

        let name: String = Input::with_theme(theme)
            .with_prompt("Endpoint name")
            .validate_with(|input: &String| {
                if !input.is_empty()
                    && input
                        .chars()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
                {
                    Ok(())
                } else {
                    Err("endpoint names must be lowercase letters, digits, and underscores")
                }
            })
            .interact_text()?;
        if endpoints.iter().any(|endpoint| endpoint.name == name) {
            eprintln!("endpoint `{name}` is already defined; choose another name");
            continue;
        }

        let path: String = Input::with_theme(theme)
            .with_prompt("Path")
            .allow_empty(true)
            .interact_text()?;
        endpoints.push(Endpoint { name, path });
    }

    if endpoints.is_empty() {
        endpoints.push(Endpoint {
            name: "default".to_string(),
            path: String::new(),
        });
    }
    Ok(endpoints)
}

/// Prompt for a URL, re-prompting until it parses.
fn prompt_url(theme: &ColorfulTheme, prompt: &str) -> Result<String> {
    Ok(Input::with_theme(theme)
        .with_prompt(prompt)
        .validate_with(|input: &String| Url::parse(input).map(|_| ()).map_err(|e| e.to_string()))
        .interact_text()?)
}

/// Prompt for an environment variable *name* (never a secret value).
fn prompt_env_var(theme: &ColorfulTheme, prompt: &str) -> Result<String> {
    Ok(Input::with_theme(theme)
        .with_prompt(prompt)
        .validate_with(|input: &String| {
            let mut chars = input.chars();
            let first_ok = chars
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic() || c == '_');
            if first_ok && chars.all(|c| c.is_ascii_alphanumeric() || c == '_') {
                Ok(())
            } else {
                Err("environment variable names must match [A-Za-z_][A-Za-z0-9_]*")
            }
        })
        .interact_text()?)
}

/// Prompt for a space-separated list of OAuth2 scopes; empty means none.
fn prompt_scopes(theme: &ColorfulTheme) -> Result<Vec<String>> {
    let raw: String = Input::with_theme(theme)
        .with_prompt("Scopes (space-separated, leave empty for none)")
        .allow_empty(true)
        .interact_text()?;
    Ok(raw.split_whitespace().map(String::from).collect())
}
