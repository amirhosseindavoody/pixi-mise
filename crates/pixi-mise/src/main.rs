//! `pixi-mise` — Pixi extension CLI (`pixi mise …`).

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use miette::{IntoDiagnostic, Result, WrapErr, miette};
use pixi_mise_core::assets::HostPlatform;
use pixi_mise_core::github::GithubClient;
use pixi_mise_core::pixi::{
    InstallTarget, list_tool_meta, read_tool_meta, remove_binaries, remove_tool_meta,
    resolve_prefix,
};
use pixi_mise_core::{
    ConfigSource, ToolOptions, add_tool_to_pixi_toml, find_workspace_root, install_tool,
    load_workspace_tools, normalize_tool_arg, parse_tool_spec, remove_tool_from_pixi_toml,
    resolve_tool, tool_request_from_spec,
};
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
        // Re-init with a more verbose filter when `-v` is passed.
        let level = match cli.verbose {
            1 => "info",
            _ => "debug",
        };
        let _ = tracing_subscriber::fmt()
            .with_env_filter(EnvFilter::new(level))
            .with_writer(std::io::stderr)
            .try_init();
        tracing::debug!(
            environment = ?cli.environment,
            platform = ?cli.platform,
            dry_run = cli.dry_run,
            verbose = cli.verbose,
            "cli options"
        );
    }

    let env = cli.environment.clone().unwrap_or_else(|| "default".into());
    let host = HostPlatform::detect();

    match cli.command {
        Commands::Add { tool } => cmd_add(&tool, &env, &host, cli.dry_run),
        Commands::Remove { tool } => cmd_remove(&tool, &env, cli.dry_run),
        Commands::Install { tool } => cmd_install(tool.as_deref(), &env, &host, cli.dry_run),
        Commands::List => cmd_list(&env),
        Commands::Resolve { tool } => cmd_resolve(tool.as_deref(), &host),
        Commands::Reinstall { tool } => stub("reinstall", tool.as_deref()),
        Commands::Update { tool } => stub("update", tool.as_deref()),
        Commands::Upgrade { tool } => stub("upgrade", tool.as_deref()),
        Commands::Search { tool } => stub("search", Some(&tool)),
        Commands::Lock => stub("lock", None),
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
    if let Some(spec) = tool
        && (spec.contains(':') || spec.contains('/'))
    {
        let _ = parse_tool_spec(&normalize_tool_arg(spec)).into_diagnostic()?;
    }

    let detail = match tool {
        Some(t) => format!(" for `{t}`"),
        None => String::new(),
    };

    miette::bail!(
        "`pixi mise {command}`{detail} is not implemented yet.\n\
         Phase 1 covers workspace add / install / list / remove.\n\
         See docs/DESIGN.md."
    );
}

fn workspace_root() -> Result<PathBuf> {
    let cwd = std::env::current_dir().into_diagnostic()?;
    find_workspace_root(&cwd).into_diagnostic()
}

fn cmd_add(tool: &str, env: &str, host: &HostPlatform, dry_run: bool) -> Result<()> {
    let root = workspace_root()?;
    let pixi_toml = root.join("pixi.toml");
    let (id, version) = parse_tool_spec(&normalize_tool_arg(tool)).into_diagnostic()?;
    let options = ToolOptions::default();

    if dry_run {
        println!(
            "would add {} @ {} to {}",
            id.github_spec(),
            version.to_config_string(),
            pixi_toml.display()
        );
    } else {
        add_tool_to_pixi_toml(&pixi_toml, &id, &version, &options)
            .into_diagnostic()
            .wrap_err("failed to update pixi.toml")?;
        println!(
            "added {} = \"{}\"",
            id.github_spec(),
            version.to_config_string()
        );
    }

    let request = tool_request_from_spec(
        &id.github_spec(),
        ConfigSource {
            path: pixi_toml.clone(),
            table: "tool.pixi-mise.tools".into(),
        },
        options,
    )
    .into_diagnostic()?;
    // Preserve the version from the CLI (@14 etc.); tool_request_from_spec on
    // the bare id would default to latest.
    let mut request = request;
    request.version = version;

    let client = GithubClient::new();
    let resolved = resolve_tool(&client, &request, host)
        .into_diagnostic()
        .wrap_err("failed to resolve tool")?;
    println!(
        "resolved {} @ {} → {}",
        request.id.github_spec(),
        resolved.tag,
        resolved.asset.name
    );

    let outcome = install_tool(&client, &resolved, &root, env, host, dry_run)
        .into_diagnostic()
        .wrap_err("failed to install tool")?;
    if dry_run {
        println!("would install into {}/bin", outcome.prefix.display());
    } else {
        for bin in &outcome.installed_bins {
            println!("installed {bin} → {}/bin/{bin}", outcome.prefix.display());
        }
    }
    Ok(())
}

fn cmd_install(tool: Option<&str>, env: &str, host: &HostPlatform, dry_run: bool) -> Result<()> {
    let root = workspace_root()?;
    let cfg = load_workspace_tools(&root)
        .into_diagnostic()
        .wrap_err("failed to load workspace tools")?;

    let requests = if let Some(spec) = tool {
        let (id, _) = parse_tool_spec(&normalize_tool_arg(spec)).into_diagnostic()?;
        let found =
            cfg.tools.into_iter().find(|t| t.id == id).ok_or_else(|| {
                miette!("tool `{}` is not configured in pixi.toml", id.github_spec())
            })?;
        vec![found]
    } else {
        cfg.tools
    };

    if requests.is_empty() {
        println!("no tools configured under [tool.pixi-mise.tools]");
        return Ok(());
    }

    let client = GithubClient::new();
    for request in &requests {
        let resolved = resolve_tool(&client, request, host)
            .into_diagnostic()
            .wrap_err_with(|| format!("failed to resolve {}", request.id.github_spec()))?;
        println!(
            "resolved {} @ {} → {}",
            request.id.github_spec(),
            resolved.tag,
            resolved.asset.name
        );
        let outcome = install_tool(&client, &resolved, &root, env, host, dry_run)
            .into_diagnostic()
            .wrap_err_with(|| format!("failed to install {}", request.id.github_spec()))?;
        if dry_run {
            println!("would install into {}/bin", outcome.prefix.display());
        } else {
            for bin in &outcome.installed_bins {
                println!("installed {bin}");
            }
        }
    }
    Ok(())
}

fn cmd_list(env: &str) -> Result<()> {
    let root = workspace_root()?;
    let cfg = load_workspace_tools(&root).into_diagnostic()?;
    let installed = list_tool_meta(&root, env).into_diagnostic()?;

    if cfg.tools.is_empty() && installed.is_empty() {
        println!("(no pixi-mise tools configured or installed)");
        return Ok(());
    }

    println!(
        "{:<36} {:<12} {:<12} BINS",
        "TOOL", "REQUESTED", "INSTALLED"
    );
    for req in &cfg.tools {
        let id = req.id.github_spec();
        let meta = installed.iter().find(|m| m.id == id);
        let installed_ver = meta.map(|m| m.version.as_str()).unwrap_or("-");
        let bins = meta
            .map(|m| m.installed_bins.join(", "))
            .unwrap_or_else(|| "-".into());
        println!(
            "{:<36} {:<12} {:<12} {}",
            id,
            req.version.to_config_string(),
            installed_ver,
            bins
        );
    }

    // Show installed tools no longer in config.
    for meta in &installed {
        if !cfg.tools.iter().any(|t| t.id.github_spec() == meta.id) {
            println!(
                "{:<36} {:<12} {:<12} {} (orphaned)",
                meta.id,
                "-",
                meta.version,
                meta.installed_bins.join(", ")
            );
        }
    }
    Ok(())
}

fn cmd_remove(tool: &str, env: &str, dry_run: bool) -> Result<()> {
    let root = workspace_root()?;
    let pixi_toml = root.join("pixi.toml");
    let (id, _) = parse_tool_spec(&normalize_tool_arg(tool)).into_diagnostic()?;
    let tool_id = id.github_spec();

    let meta = read_tool_meta(&root, env, &tool_id).into_diagnostic()?;
    if let Some(meta) = &meta {
        let prefix = resolve_prefix(&InstallTarget::Local {
            workspace_root: root.clone(),
            env: env.to_string(),
        })
        .into_diagnostic()?;
        if dry_run {
            println!(
                "would remove bins {:?} from {}",
                meta.installed_bins,
                prefix.join("bin").display()
            );
        } else {
            remove_binaries(&prefix, &meta.installed_bins).into_diagnostic()?;
            remove_tool_meta(&root, env, &tool_id).into_diagnostic()?;
            for bin in &meta.installed_bins {
                println!("removed {bin}");
            }
        }
    } else {
        println!("no install metadata for {tool_id} (config entry will still be removed)");
    }

    if dry_run {
        println!("would remove {tool_id} from {}", pixi_toml.display());
    } else {
        let removed = remove_tool_from_pixi_toml(&pixi_toml, &id).into_diagnostic()?;
        if removed {
            println!("removed {tool_id} from pixi.toml");
        } else {
            println!("{tool_id} was not present in pixi.toml");
        }
    }
    Ok(())
}

fn cmd_resolve(tool: Option<&str>, host: &HostPlatform) -> Result<()> {
    let root = workspace_root()?;
    let cfg = load_workspace_tools(&root).into_diagnostic()?;
    let client = GithubClient::new();

    let requests = if let Some(spec) = tool {
        let normalized = normalize_tool_arg(spec);
        // Prefer config entry when present so options apply; otherwise resolve the CLI spec.
        let (id, version) = parse_tool_spec(&normalized).into_diagnostic()?;
        if let Some(found) = cfg.tools.into_iter().find(|t| t.id == id) {
            vec![found]
        } else {
            let mut request = tool_request_from_spec(
                &normalized,
                ConfigSource {
                    path: root.join("pixi.toml"),
                    table: "cli".into(),
                },
                ToolOptions::default(),
            )
            .into_diagnostic()?;
            request.version = version;
            vec![request]
        }
    } else {
        cfg.tools
    };

    if requests.is_empty() {
        println!("no tools to resolve");
        return Ok(());
    }

    for request in &requests {
        let resolved = resolve_tool(&client, request, host)
            .into_diagnostic()
            .wrap_err_with(|| format!("failed to resolve {}", request.id.github_spec()))?;
        println!(
            "{} @ {} ({})\n  asset: {}\n  url:   {}\n  host:  {}/{} ({})",
            request.id.github_spec(),
            resolved.version,
            resolved.tag,
            resolved.asset.name,
            resolved.asset.download_url,
            host.os,
            host.arch,
            host.pixi_platform()
        );
    }
    Ok(())
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
