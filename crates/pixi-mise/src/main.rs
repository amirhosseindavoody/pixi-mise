//! `pixi-mise` — Pixi extension CLI (`pixi mise …`).

use std::process::ExitCode;

use clap::{Parser, Subcommand};
use miette::{IntoDiagnostic, Result};
use tracing_subscriber::EnvFilter;

fn main() -> ExitCode {
    if let Err(err) = try_main() {
        eprintln!("{err:?}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

fn try_main() -> Result<()> {
    init_tracing();
    let cli = Cli::parse();
    run(cli)
}

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .try_init();
}

/// Install GitHub release binaries into Pixi environments.
///
/// Invoked as a Pixi extension: `pixi mise <subcommand> …`
#[derive(Debug, Parser)]
#[command(
    name = "pixi-mise",
    version,
    about = "Install GitHub release binaries into Pixi environments",
    long_about = None,
    propagate_version = true
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// Pixi environment name (default: `default`).
    #[arg(long, global = true, env = "PIXI_MISE_ENVIRONMENT")]
    environment: Option<String>,

    /// Platform for resolve/lock (e.g. `linux-64`). Defaults to host.
    #[arg(long, global = true)]
    platform: Option<String>,

    /// Print actions without changing config or the environment.
    #[arg(short = 'n', long, global = true)]
    dry_run: bool,

    /// Increase logging verbosity.
    #[arg(short, long, global = true, action = clap::ArgAction::Count)]
    verbose: u8,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Add a tool to workspace config and install it.
    #[command(visible_alias = "a")]
    Add {
        /// Tool spec, e.g. `github:BurntSushi/ripgrep@14`.
        tool: String,
    },
    /// Remove a tool from config and uninstall binaries.
    #[command(visible_alias = "rm")]
    Remove {
        /// Tool id, e.g. `github:BurntSushi/ripgrep`.
        tool: String,
    },
    /// Install tools from workspace config.
    #[command(visible_alias = "i")]
    Install {
        /// Optional single tool to install.
        tool: Option<String>,
    },
    /// Force re-download and re-link binaries.
    Reinstall {
        /// Optional single tool to reinstall.
        tool: Option<String>,
    },
    /// Re-resolve within version specs; refresh lock + env.
    Update {
        /// Optional single tool to update.
        tool: Option<String>,
    },
    /// Bump version specs in config and refresh lock.
    Upgrade {
        /// Optional single tool to upgrade.
        tool: Option<String>,
    },
    /// List configured / installed tools.
    #[command(visible_alias = "ls")]
    List,
    /// List remote versions / releases for a tool.
    Search {
        /// Tool id, e.g. `github:cli/cli`.
        tool: String,
    },
    /// Rewrite lockfile from current resolution (no install).
    Lock,
    /// Show resolved assets without installing.
    Resolve {
        /// Optional tool spec to resolve.
        tool: Option<String>,
    },
    /// Print path to an installed binary.
    Which {
        /// Binary name.
        bin: String,
    },
    /// Cache maintenance.
    Clean {
        #[command(subcommand)]
        command: CleanCommands,
    },
    /// Manage globally installed tools (`$PIXI_HOME`).
    Global {
        #[command(subcommand)]
        command: GlobalCommands,
    },
}

#[derive(Debug, Subcommand)]
enum CleanCommands {
    /// Clear the download cache.
    Cache,
}

#[derive(Debug, Subcommand)]
enum GlobalCommands {
    /// Add + install into a global Pixi env / expose on `$PIXI_HOME/bin`.
    #[command(visible_alias = "a")]
    Add {
        /// Tool spec, e.g. `github:cli/cli`.
        tool: String,
    },
    /// Remove a global tool.
    #[command(visible_alias = "rm")]
    Remove {
        /// Tool id, e.g. `github:cli/cli`.
        tool: String,
    },
    /// Install from global config.
    #[command(visible_alias = "i")]
    Install {
        /// Optional single tool to install.
        tool: Option<String>,
    },
    /// List global tools.
    #[command(visible_alias = "ls")]
    List,
    /// Update global tools within specs.
    Update {
        /// Optional single tool to update.
        tool: Option<String>,
    },
}

fn run(cli: Cli) -> Result<()> {
    if cli.verbose > 0 {
        tracing::debug!(
            environment = ?cli.environment,
            platform = ?cli.platform,
            dry_run = cli.dry_run,
            verbose = cli.verbose,
            "cli options"
        );
    }

    match cli.command {
        Commands::Add { tool } => stub("add", Some(&tool)),
        Commands::Remove { tool } => stub("remove", Some(&tool)),
        Commands::Install { tool } => stub("install", tool.as_deref()),
        Commands::Reinstall { tool } => stub("reinstall", tool.as_deref()),
        Commands::Update { tool } => stub("update", tool.as_deref()),
        Commands::Upgrade { tool } => stub("upgrade", tool.as_deref()),
        Commands::List => stub("list", None),
        Commands::Search { tool } => stub("search", Some(&tool)),
        Commands::Lock => stub("lock", None),
        Commands::Resolve { tool } => stub("resolve", tool.as_deref()),
        Commands::Which { bin } => stub("which", Some(&bin)),
        Commands::Clean {
            command: CleanCommands::Cache,
        } => stub("clean cache", None),
        Commands::Global { command } => run_global(command),
    }
}

fn run_global(command: GlobalCommands) -> Result<()> {
    match command {
        GlobalCommands::Add { tool } => stub("global add", Some(&tool)),
        GlobalCommands::Remove { tool } => stub("global remove", Some(&tool)),
        GlobalCommands::Install { tool } => stub("global install", tool.as_deref()),
        GlobalCommands::List => stub("global list", None),
        GlobalCommands::Update { tool } => stub("global update", tool.as_deref()),
    }
}

fn stub(command: &str, tool: Option<&str>) -> Result<()> {
    // Validate tool specs early so CLI parsing of github:… is exercised in Phase 0.
    if let Some(spec) = tool
        && (spec.contains(':') || spec.contains('/'))
    {
        let _ = pixi_mise_core::parse_tool_spec(&normalize_spec(spec)).into_diagnostic()?;
    }

    let detail = match tool {
        Some(t) => format!(" for `{t}`"),
        None => String::new(),
    };

    miette::bail!(
        "`pixi mise {command}`{detail} is not implemented yet.\n\
         Phase 0 ships the CLI skeleton only; GitHub install arrives in Phase 1.\n\
         See docs/DESIGN.md."
    );
}

fn normalize_spec(spec: &str) -> String {
    if spec.starts_with("github:") {
        spec.to_string()
    } else {
        format!("github:{spec}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_debug_assert() {
        Cli::command().debug_assert();
    }

    #[test]
    fn parses_add() {
        let cli = Cli::try_parse_from(["pixi-mise", "add", "github:cli/cli@2"]).unwrap();
        match cli.command {
            Commands::Add { tool } => assert_eq!(tool, "github:cli/cli@2"),
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn parses_global_list() {
        let cli = Cli::try_parse_from(["pixi-mise", "global", "list"]).unwrap();
        match cli.command {
            Commands::Global {
                command: GlobalCommands::List,
            } => {}
            other => panic!("unexpected: {other:?}"),
        }
    }
}
