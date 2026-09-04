//! `datom` — command-line interface for the datom-connect data platform.

mod datasource;
mod parse;

use std::env;
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

/// datom-connect: a CLI data platform tool.
#[derive(Debug, Parser)]
#[command(name = "datom", version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Initialize a new datom-connect project.
    Init {
        /// Name of the project to create.
        name: String,
    },

    /// Validate a datom source file's syntax and print its AST.
    Parse {
        /// Path of the source file to parse.
        file: PathBuf,
    },

    /// Manage data sources.
    #[command(subcommand)]
    Datasource(DatasourceCommand),

    /// Database maintenance commands.
    #[command(subcommand, hide = true)]
    Db(DbCommand),
}

#[derive(Debug, Subcommand)]
enum DbCommand {
    /// Verify database connectivity by running `SELECT 1`.
    Ping,
}

#[derive(Debug, Subcommand)]
enum DatasourceCommand {
    /// Add a new data source (interactive).
    Add {
        /// Name of the data source to add.
        name: String,

        /// Add an API data source.
        #[arg(long)]
        api: bool,
    },

    /// Manage a data source's endpoints.
    #[command(subcommand)]
    Endpoint(EndpointCommand),

    /// Introspect a data source's schema.
    Introspect {
        /// Name of the data source to introspect.
        name: String,
    },

    /// List configured data sources.
    List {
        /// Also run each data source's connectivity test (performs live
        /// requests) and report the result in a CONNECTED column.
        #[arg(long)]
        test_connection: bool,
    },

    /// Test connectivity and contracts of a data source.
    Test {
        /// Name of the data source to test.
        name: String,
    },
}

#[derive(Debug, Subcommand)]
enum EndpointCommand {
    /// Add an endpoint to a data source.
    Add {
        /// Data source to add the endpoint to.
        datasource: String,

        /// Name of the new endpoint (becomes a table name).
        name: String,

        /// Path joined onto the data source's base URL.
        #[arg(long)]
        path: String,
    },

    /// Remove an endpoint from a data source.
    Remove {
        /// Data source to remove the endpoint from.
        datasource: String,

        /// Name of the endpoint to remove.
        name: String,
    },

    /// List a data source's endpoints.
    List {
        /// Data source whose endpoints to list.
        datasource: String,
    },
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Init { name } => {
            let cwd = env::current_dir().context("could not determine current directory")?;
            datom_core::init_project(&cwd, &name)
                .with_context(|| format!("failed to create project `{name}`"))?;
            println!("Project {name} created!");
        }
        Command::Parse { file } => parse::parse(&file)?,
        Command::Datasource(cmd) => match cmd {
            DatasourceCommand::Add { name, api } => datasource::add(&name, api)?,
            DatasourceCommand::Endpoint(cmd) => match cmd {
                EndpointCommand::Add {
                    datasource,
                    name,
                    path,
                } => datasource::endpoint_add(&datasource, &name, &path)?,
                EndpointCommand::Remove { datasource, name } => {
                    datasource::endpoint_remove(&datasource, &name)?
                }
                EndpointCommand::List { datasource } => datasource::endpoint_list(&datasource)?,
            },
            DatasourceCommand::Introspect { name } => datasource::introspect(&name).await?,
            DatasourceCommand::List { test_connection } => {
                datasource::list(test_connection).await?
            }
            DatasourceCommand::Test { name } => datasource::test(&name).await?,
        },
        Command::Db(cmd) => match cmd {
            DbCommand::Ping => {
                let url = datom_core::db::database_url();
                datom_core::db::ping().await.with_context(|| {
                    format!(
                        "could not connect to the database at `{url}`.\n\
                         Is Postgres running? Start it with `docker compose up -d`."
                    )
                })?;
                println!("Database connection OK — SELECT 1 succeeded.");
            }
        },
    }

    Ok(())
}
