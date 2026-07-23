//! `pixi-mise` — Pixi extension CLI (`pixi mise …`).

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use miette::{IntoDiagnostic, Result, WrapErr, miette};
use pixi_mise_core::assets::HostPlatform;
use pixi_mise_core::github::GithubClient;
use pixi_mise_core::pixi::{
    InstallTarget, global_env_name, list_tool_meta, pixi_home, read_tool_meta, remove_binaries,
    remove_tool_meta, resolve_prefix, unexpose_binaries,
};
use pixi_mise_core::{
    ConfigSource, Lockfile, ToolOptions, VersionSpec, add_tool_to_global_config,
    add_tool_to_pixi_toml, cache_root, clear_cache, find_workspace_root, import_mise_tools,
    install_tool, invalidate_cached_asset, load_global_tools, load_workspace_tools,
    normalize_tool_arg, parse_tool_spec, remove_tool_from_global_config,
    remove_tool_from_pixi_toml, resolve_tool, resolve_tool_with_lock, tool_request_from_spec,
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

    /// Pixi environment name (default: `default` for local; per-tool for global).
    #[arg(long, global = true, env = "PIXI_MISE_ENVIRONMENT")]
    environment: Option<String>,

    /// Platform for resolve/lock (e.g. `linux-64`). Defaults to host.
    #[arg(long, global = true)]
    platform: Option<String>,

    /// Print actions without changing config or the environment.
    #[arg(short = 'n', long, global = true)]
    dry_run: bool,

    /// Prefer lockfile URL/checksum when present (do not re-pick assets).
    #[arg(long, global = true)]
    locked: bool,

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
    /// Import `github:` tools from `mise.toml` into `pixi.toml`.
    #[command(name = "import-mise")]
    ImportMise,
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
        let level = match cli.verbose {
            1 => "info",
            _ => "debug",
        };
        let _ = tracing_subscriber::fmt()
            .with_env_filter(EnvFilter::new(level))
            .with_writer(std::io::stderr)
            .try_init();
    }

    let env = cli.environment.clone().unwrap_or_else(|| "default".into());
    let host = HostPlatform::detect();
    let dry_run = cli.dry_run;
    let locked = cli.locked;

    match cli.command {
        Commands::Add { tool } => cmd_add(&tool, &env, &host, dry_run, locked),
        Commands::Remove { tool } => cmd_remove(&tool, &env, dry_run),
        Commands::Install { tool } => cmd_install(tool.as_deref(), &env, &host, dry_run, locked),
        Commands::List => cmd_list(&env),
        Commands::Resolve { tool } => cmd_resolve(tool.as_deref(), &host, locked),
        Commands::Lock => cmd_lock(&host),
        Commands::ImportMise => cmd_import_mise(dry_run),
        Commands::Reinstall { tool } => cmd_reinstall(tool.as_deref(), &env, &host, dry_run),
        Commands::Update { tool } => cmd_update(tool.as_deref(), &env, &host, dry_run),
        Commands::Upgrade { tool } => cmd_upgrade(tool.as_deref(), &env, &host, dry_run),
        Commands::Search { tool } => cmd_search(&tool),
        Commands::Which { bin } => cmd_which(&bin, &env),
        Commands::Clean {
            command: CleanCommands::Cache,
        } => cmd_clean_cache(dry_run),
        Commands::Global { command } => {
            run_global(command, cli.environment.as_deref(), &host, dry_run, locked)
        }
    }
}

fn run_global(
    command: GlobalCommands,
    environment: Option<&str>,
    host: &HostPlatform,
    dry_run: bool,
    locked: bool,
) -> Result<()> {
    match command {
        GlobalCommands::Add { tool } => cmd_global_add(&tool, environment, host, dry_run, locked),
        GlobalCommands::Remove { tool } => cmd_global_remove(&tool, environment, dry_run),
        GlobalCommands::Install { tool } => {
            cmd_global_install(tool.as_deref(), environment, host, dry_run, locked)
        }
        GlobalCommands::List => cmd_global_list(environment),
        GlobalCommands::Update { tool } => {
            cmd_global_update(tool.as_deref(), environment, host, dry_run)
        }
    }
}

fn workspace_root() -> Result<PathBuf> {
    let cwd = std::env::current_dir().into_diagnostic()?;
    find_workspace_root(&cwd).into_diagnostic()
}

fn resolve_request(
    client: &GithubClient,
    request: &pixi_mise_core::ToolRequest,
    host: &HostPlatform,
    lock_path: &std::path::Path,
    use_lock: bool,
) -> Result<pixi_mise_core::ToolVersion> {
    let lock = Lockfile::load(lock_path).into_diagnostic()?;
    let entry = if use_lock {
        lock.find(&request.id.github_spec(), &host.pixi_platform())
    } else {
        None
    };
    resolve_tool_with_lock(client, request, host, entry)
        .into_diagnostic()
        .wrap_err_with(|| format!("failed to resolve {}", request.id.github_spec()))
}

fn cmd_add(tool: &str, env: &str, host: &HostPlatform, dry_run: bool, locked: bool) -> Result<()> {
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

    let mut request = tool_request_from_spec(
        &id.github_spec(),
        ConfigSource {
            path: pixi_toml.clone(),
            table: "tool.pixi-mise.tools".into(),
        },
        options,
    )
    .into_diagnostic()?;
    request.version = version;

    let client = GithubClient::new();
    let lock_path = Lockfile::workspace_path(&root);
    let resolved = resolve_request(&client, &request, host, &lock_path, locked)?;
    println!(
        "resolved {} @ {} → {}",
        request.id.github_spec(),
        resolved.tag,
        resolved.asset.name
    );

    let target = InstallTarget::Local {
        workspace_root: root,
        env: env.to_string(),
    };
    let outcome = install_tool(&client, &resolved, &target, host, dry_run)
        .into_diagnostic()
        .wrap_err("failed to install tool")?;
    print_install_outcome(&outcome, dry_run);
    Ok(())
}

fn cmd_install(
    tool: Option<&str>,
    env: &str,
    host: &HostPlatform,
    dry_run: bool,
    locked: bool,
) -> Result<()> {
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
    let lock_path = Lockfile::workspace_path(&root);
    for request in &requests {
        let resolved = resolve_request(&client, request, host, &lock_path, locked)?;
        println!(
            "resolved {} @ {} → {}",
            request.id.github_spec(),
            resolved.tag,
            resolved.asset.name
        );
        let target = InstallTarget::Local {
            workspace_root: root.clone(),
            env: env.to_string(),
        };
        let outcome = install_tool(&client, &resolved, &target, host, dry_run)
            .into_diagnostic()
            .wrap_err_with(|| format!("failed to install {}", request.id.github_spec()))?;
        print_install_outcome(&outcome, dry_run);
    }
    Ok(())
}

fn print_install_outcome(outcome: &pixi_mise_core::InstallOutcome, dry_run: bool) {
    if dry_run {
        println!("would install into {}/bin", outcome.prefix.display());
        return;
    }
    for bin in &outcome.installed_bins {
        println!("installed {bin} → {}/bin/{bin}", outcome.prefix.display());
    }
    for bin in &outcome.exposed_bins {
        println!("exposed {bin} → {}/bin/{bin}", pixi_home().display());
    }
    if let Some(sum) = &outcome.checksum {
        println!("checksum {sum}");
    }
}

fn cmd_list(env: &str) -> Result<()> {
    let root = workspace_root()?;
    let cfg = load_workspace_tools(&root).into_diagnostic()?;
    let meta_root = root.join(".pixi");
    let installed = list_tool_meta(&meta_root, env).into_diagnostic()?;

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
    let meta_root = root.join(".pixi");

    let meta = read_tool_meta(&meta_root, env, &tool_id).into_diagnostic()?;
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
            remove_tool_meta(&meta_root, env, &tool_id).into_diagnostic()?;
            for bin in &meta.installed_bins {
                println!("removed {bin}");
            }
            let mut lock = Lockfile::load(&Lockfile::workspace_path(&root)).into_diagnostic()?;
            lock.remove_id(&tool_id);
            lock.save(&Lockfile::workspace_path(&root))
                .into_diagnostic()?;
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

fn cmd_resolve(tool: Option<&str>, host: &HostPlatform, locked: bool) -> Result<()> {
    let root = workspace_root()?;
    let cfg = load_workspace_tools(&root).into_diagnostic()?;
    let client = GithubClient::new();
    let lock_path = Lockfile::workspace_path(&root);

    let requests = if let Some(spec) = tool {
        let normalized = normalize_tool_arg(spec);
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
        let resolved = resolve_request(&client, request, host, &lock_path, locked)?;
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
        if let Some(sum) = &resolved.asset.digest {
            println!("  lock:  {sum}");
        }
    }
    Ok(())
}

fn cmd_lock(host: &HostPlatform) -> Result<()> {
    let root = workspace_root()?;
    let cfg = load_workspace_tools(&root).into_diagnostic()?;
    if cfg.tools.is_empty() {
        println!("no tools configured under [tool.pixi-mise.tools]");
        return Ok(());
    }
    let client = GithubClient::new();
    let lock_path = Lockfile::workspace_path(&root);
    let mut lock = Lockfile::default();
    for request in &cfg.tools {
        // Fresh resolve (ignore existing lock) so `lock` refreshes pins.
        let resolved = resolve_tool(&client, request, host)
            .into_diagnostic()
            .wrap_err_with(|| format!("failed to resolve {}", request.id.github_spec()))?;
        println!(
            "locked {} @ {} → {}",
            request.id.github_spec(),
            resolved.tag,
            resolved.asset.name
        );
        lock.upsert(pixi_mise_core::LockEntry {
            id: request.id.github_spec(),
            version: resolved.version.clone(),
            tag: resolved.tag.clone(),
            asset: resolved.asset.name.clone(),
            url: resolved.asset.download_url.clone(),
            checksum: resolved.asset.digest.clone(),
            platform: host.pixi_platform(),
            installed_bins: Vec::new(),
        });
    }
    lock.save(&lock_path).into_diagnostic()?;
    println!("wrote {}", lock_path.display());
    Ok(())
}

fn cmd_global_add(
    tool: &str,
    environment: Option<&str>,
    host: &HostPlatform,
    dry_run: bool,
    locked: bool,
) -> Result<()> {
    let (id, version) = parse_tool_spec(&normalize_tool_arg(tool)).into_diagnostic()?;
    let options = ToolOptions::default();
    let env = environment
        .map(str::to_string)
        .unwrap_or_else(|| global_env_name(&id.github_spec()));

    if dry_run {
        println!(
            "would add {} @ {} to {}",
            id.github_spec(),
            version.to_config_string(),
            pixi_mise_core::pixi::global_config_path().display()
        );
    } else {
        let path = add_tool_to_global_config(&id, &version, &options).into_diagnostic()?;
        println!(
            "added {} = \"{}\" → {}",
            id.github_spec(),
            version.to_config_string(),
            path.display()
        );
    }

    let mut request = tool_request_from_spec(
        &id.github_spec(),
        ConfigSource {
            path: pixi_mise_core::pixi::global_config_path(),
            table: "tools".into(),
        },
        options,
    )
    .into_diagnostic()?;
    request.version = version;

    let client = GithubClient::new();
    let lock_path = Lockfile::global_path(&pixi_home());
    let resolved = resolve_request(&client, &request, host, &lock_path, locked)?;
    println!(
        "resolved {} @ {} → {}",
        request.id.github_spec(),
        resolved.tag,
        resolved.asset.name
    );

    let target = InstallTarget::Global { env };
    let outcome = install_tool(&client, &resolved, &target, host, dry_run)
        .into_diagnostic()
        .wrap_err("failed to install global tool")?;
    print_install_outcome(&outcome, dry_run);
    Ok(())
}

fn cmd_global_install(
    tool: Option<&str>,
    environment: Option<&str>,
    host: &HostPlatform,
    dry_run: bool,
    locked: bool,
) -> Result<()> {
    let cfg = load_global_tools().into_diagnostic()?;
    let requests = if let Some(spec) = tool {
        let (id, _) = parse_tool_spec(&normalize_tool_arg(spec)).into_diagnostic()?;
        let found = cfg.tools.into_iter().find(|t| t.id == id).ok_or_else(|| {
            miette!(
                "tool `{}` is not configured in {}",
                id.github_spec(),
                cfg.path.display()
            )
        })?;
        vec![found]
    } else {
        cfg.tools
    };

    if requests.is_empty() {
        println!(
            "no tools configured under [tools] in {}",
            pixi_mise_core::pixi::global_config_path().display()
        );
        return Ok(());
    }

    let client = GithubClient::new();
    let lock_path = Lockfile::global_path(&pixi_home());
    for request in &requests {
        let env = environment
            .map(str::to_string)
            .unwrap_or_else(|| global_env_name(&request.id.github_spec()));
        let resolved = resolve_request(&client, request, host, &lock_path, locked)?;
        println!(
            "resolved {} @ {} → {}",
            request.id.github_spec(),
            resolved.tag,
            resolved.asset.name
        );
        let target = InstallTarget::Global { env };
        let outcome = install_tool(&client, &resolved, &target, host, dry_run)
            .into_diagnostic()
            .wrap_err_with(|| format!("failed to install {}", request.id.github_spec()))?;
        print_install_outcome(&outcome, dry_run);
    }
    Ok(())
}

fn cmd_global_list(environment: Option<&str>) -> Result<()> {
    let cfg = load_global_tools().into_diagnostic()?;
    let home = pixi_home();

    // Collect metadata across per-tool envs (or a single overridden env).
    let mut installed = Vec::new();
    if let Some(env) = environment {
        installed.extend(list_tool_meta(&home, env).into_diagnostic()?);
    } else {
        let mise_root = home.join("mise");
        if mise_root.is_dir() {
            for entry in std::fs::read_dir(&mise_root).into_diagnostic()? {
                let entry = entry.into_diagnostic()?;
                if entry.file_type().into_diagnostic()?.is_dir() {
                    let name = entry.file_name().to_string_lossy().into_owned();
                    installed.extend(list_tool_meta(&home, &name).into_diagnostic()?);
                }
            }
        }
    }
    installed.sort_by(|a, b| a.id.cmp(&b.id));
    installed.dedup_by(|a, b| a.id == b.id);

    if cfg.tools.is_empty() && installed.is_empty() {
        println!("(no global pixi-mise tools configured or installed)");
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
            .map(|m| {
                if m.exposed_bins.is_empty() {
                    m.installed_bins.join(", ")
                } else {
                    m.exposed_bins.join(", ")
                }
            })
            .unwrap_or_else(|| "-".into());
        println!(
            "{:<36} {:<12} {:<12} {}",
            id,
            req.version.to_config_string(),
            installed_ver,
            bins
        );
    }
    for meta in &installed {
        if !cfg.tools.iter().any(|t| t.id.github_spec() == meta.id) {
            println!(
                "{:<36} {:<12} {:<12} {} (orphaned)",
                meta.id,
                "-",
                meta.version,
                meta.exposed_bins.join(", ")
            );
        }
    }
    Ok(())
}

fn cmd_global_remove(tool: &str, environment: Option<&str>, dry_run: bool) -> Result<()> {
    let (id, _) = parse_tool_spec(&normalize_tool_arg(tool)).into_diagnostic()?;
    let tool_id = id.github_spec();
    let env = environment
        .map(str::to_string)
        .unwrap_or_else(|| global_env_name(&tool_id));
    let home = pixi_home();

    let meta = read_tool_meta(&home, &env, &tool_id).into_diagnostic()?;
    if let Some(meta) = &meta {
        let prefix =
            resolve_prefix(&InstallTarget::Global { env: env.clone() }).into_diagnostic()?;
        if dry_run {
            println!(
                "would remove bins {:?} and unexpose {:?}",
                meta.installed_bins, meta.exposed_bins
            );
        } else {
            unexpose_binaries(&meta.exposed_bins).into_diagnostic()?;
            remove_binaries(&prefix, &meta.installed_bins).into_diagnostic()?;
            remove_tool_meta(&home, &env, &tool_id).into_diagnostic()?;
            let mut lock = Lockfile::load(&Lockfile::global_path(&home)).into_diagnostic()?;
            lock.remove_id(&tool_id);
            lock.save(&Lockfile::global_path(&home)).into_diagnostic()?;
            for bin in &meta.exposed_bins {
                println!("unexposed {bin}");
            }
            for bin in &meta.installed_bins {
                println!("removed {bin}");
            }
        }
    } else {
        println!("no install metadata for {tool_id}");
    }

    if dry_run {
        println!(
            "would remove {tool_id} from {}",
            pixi_mise_core::pixi::global_config_path().display()
        );
    } else {
        let removed = remove_tool_from_global_config(&id).into_diagnostic()?;
        if removed {
            println!(
                "removed {tool_id} from {}",
                pixi_mise_core::pixi::global_config_path().display()
            );
        } else {
            println!("{tool_id} was not present in global config");
        }
    }
    Ok(())
}

fn filter_requests(
    mut tools: Vec<pixi_mise_core::ToolRequest>,
    tool: Option<&str>,
    missing_msg: impl FnOnce(&str) -> String,
) -> Result<Vec<pixi_mise_core::ToolRequest>> {
    if let Some(spec) = tool {
        let (id, _) = parse_tool_spec(&normalize_tool_arg(spec)).into_diagnostic()?;
        let found = tools
            .into_iter()
            .find(|t| t.id == id)
            .ok_or_else(|| miette!("{}", missing_msg(&id.github_spec())))?;
        Ok(vec![found])
    } else {
        tools.sort_by_key(|t| t.id.as_str());
        Ok(tools)
    }
}

fn cmd_search(tool: &str) -> Result<()> {
    let (id, _) = parse_tool_spec(&normalize_tool_arg(tool)).into_diagnostic()?;
    let client = GithubClient::new();
    let releases = client
        .list_releases(&id.owner, &id.repo)
        .into_diagnostic()
        .wrap_err_with(|| format!("failed to list releases for {}", id.github_spec()))?;
    if releases.is_empty() {
        println!("no releases found for {}", id.github_spec());
        return Ok(());
    }
    println!("Recent releases for {}:", id.github_spec());
    for release in releases.into_iter().take(30) {
        let kind = if release.prerelease {
            "prerelease"
        } else {
            "stable"
        };
        println!("  {:24} ({kind})", release.tag_name);
    }
    Ok(())
}

fn cmd_update(tool: Option<&str>, env: &str, host: &HostPlatform, dry_run: bool) -> Result<()> {
    let root = workspace_root()?;
    let cfg = load_workspace_tools(&root)
        .into_diagnostic()
        .wrap_err("failed to load workspace tools")?;
    let requests = filter_requests(cfg.tools, tool, |id| {
        format!("tool `{id}` is not configured in pixi.toml")
    })?;
    if requests.is_empty() {
        println!("no tools configured under [tool.pixi-mise.tools]");
        return Ok(());
    }

    let client = GithubClient::new();
    let meta_root = root.join(".pixi");
    for request in &requests {
        let previous = read_tool_meta(&meta_root, env, &request.id.github_spec())
            .into_diagnostic()?
            .map(|m| m.tag);
        // Ignore lock so we re-resolve within the current version spec.
        let resolved = resolve_tool(&client, request, host)
            .into_diagnostic()
            .wrap_err_with(|| format!("failed to resolve {}", request.id.github_spec()))?;
        match &previous {
            Some(old) if old == &resolved.tag => {
                println!(
                    "{}: already up to date at {}",
                    request.id.github_spec(),
                    resolved.tag
                );
            }
            Some(old) => {
                println!("{}: {} -> {}", request.id.github_spec(), old, resolved.tag);
            }
            None => {
                println!("{}: installing {}", request.id.github_spec(), resolved.tag);
            }
        }
        let target = InstallTarget::Local {
            workspace_root: root.clone(),
            env: env.to_string(),
        };
        let outcome = install_tool(&client, &resolved, &target, host, dry_run)
            .into_diagnostic()
            .wrap_err_with(|| format!("failed to install {}", request.id.github_spec()))?;
        print_install_outcome(&outcome, dry_run);
    }
    Ok(())
}

fn cmd_upgrade(tool: Option<&str>, env: &str, host: &HostPlatform, dry_run: bool) -> Result<()> {
    let root = workspace_root()?;
    let pixi_toml = root.join("pixi.toml");
    let cfg = load_workspace_tools(&root)
        .into_diagnostic()
        .wrap_err("failed to load workspace tools")?;
    let requests = filter_requests(cfg.tools, tool, |id| {
        format!("tool `{id}` is not configured in pixi.toml")
    })?;
    if requests.is_empty() {
        println!("no tools configured under [tool.pixi-mise.tools]");
        return Ok(());
    }

    let client = GithubClient::new();
    for mut request in requests {
        let previous_spec = request.version.to_config_string();
        // Upgrade loosens the pin: resolve latest, then write Exact into config.
        request.version = VersionSpec::Latest;
        let resolved = resolve_tool(&client, &request, host)
            .into_diagnostic()
            .wrap_err_with(|| format!("failed to resolve {}", request.id.github_spec()))?;
        let new_spec = VersionSpec::Exact(resolved.version.clone());
        println!(
            "{}: config `{}` -> `{}` (tag {})",
            request.id.github_spec(),
            previous_spec,
            new_spec.to_config_string(),
            resolved.tag
        );
        if !dry_run {
            add_tool_to_pixi_toml(&pixi_toml, &request.id, &new_spec, &request.options)
                .into_diagnostic()
                .wrap_err("failed to update pixi.toml")?;
        }
        request.version = new_spec;
        let target = InstallTarget::Local {
            workspace_root: root.clone(),
            env: env.to_string(),
        };
        let outcome = install_tool(&client, &resolved, &target, host, dry_run)
            .into_diagnostic()
            .wrap_err_with(|| format!("failed to install {}", request.id.github_spec()))?;
        print_install_outcome(&outcome, dry_run);
    }
    Ok(())
}

fn cmd_reinstall(tool: Option<&str>, env: &str, host: &HostPlatform, dry_run: bool) -> Result<()> {
    let root = workspace_root()?;
    let cfg = load_workspace_tools(&root)
        .into_diagnostic()
        .wrap_err("failed to load workspace tools")?;
    let requests = filter_requests(cfg.tools, tool, |id| {
        format!("tool `{id}` is not configured in pixi.toml")
    })?;
    if requests.is_empty() {
        println!("no tools configured under [tool.pixi-mise.tools]");
        return Ok(());
    }

    let client = GithubClient::new();
    let lock_path = Lockfile::workspace_path(&root);
    let lock = Lockfile::load(&lock_path).into_diagnostic()?;
    let meta_root = root.join(".pixi");

    for request in &requests {
        let tool_id = request.id.github_spec();
        if let Some(meta) = read_tool_meta(&meta_root, env, &tool_id).into_diagnostic()? {
            invalidate_cached_asset(&request.id.owner, &request.id.repo, &meta.tag, &meta.asset)
                .into_diagnostic()?;
        } else if let Some(entry) = lock.find(&tool_id, &host.pixi_platform()) {
            invalidate_cached_asset(
                &request.id.owner,
                &request.id.repo,
                &entry.tag,
                &entry.asset,
            )
            .into_diagnostic()?;
        }
        let resolved = resolve_tool(&client, request, host)
            .into_diagnostic()
            .wrap_err_with(|| format!("failed to resolve {tool_id}"))?;
        println!("reinstalling {tool_id} @ {}", resolved.tag);
        let target = InstallTarget::Local {
            workspace_root: root.clone(),
            env: env.to_string(),
        };
        let outcome = install_tool(&client, &resolved, &target, host, dry_run)
            .into_diagnostic()
            .wrap_err_with(|| format!("failed to reinstall {tool_id}"))?;
        print_install_outcome(&outcome, dry_run);
    }
    Ok(())
}

fn cmd_which(bin: &str, env: &str) -> Result<()> {
    // Prefer workspace install, then global expose.
    if let Ok(root) = workspace_root() {
        let local = root
            .join(".pixi")
            .join("envs")
            .join(env)
            .join("bin")
            .join(bin);
        if local.is_file() {
            println!("{}", local.display());
            return Ok(());
        }
        // Fall back to configured rename/expose names when `bin` is a tool id key.
        if let Ok(cfg) = load_workspace_tools(&root) {
            for req in &cfg.tools {
                if req.id.github_spec() == bin
                    || req.id.repo == bin
                    || format!("github:{}", req.id.as_str()) == bin
                {
                    let name = req
                        .options
                        .expose_as
                        .as_deref()
                        .or(req.options.rename_exe.as_deref())
                        .or(req.options.bin.as_deref())
                        .unwrap_or(req.id.repo.as_str());
                    let path = root
                        .join(".pixi")
                        .join("envs")
                        .join(env)
                        .join("bin")
                        .join(name);
                    if path.is_file() {
                        println!("{}", path.display());
                        return Ok(());
                    }
                }
            }
        }
    }

    let global = pixi_home().join("bin").join(bin);
    if global.is_file() {
        println!("{}", global.display());
        return Ok(());
    }

    Err(miette!(
        "`{bin}` not found in workspace or global pixi-mise bins"
    ))
}

fn cmd_clean_cache(dry_run: bool) -> Result<()> {
    let root = cache_root();
    if dry_run {
        println!("would clear cache at {}", root.display());
        return Ok(());
    }
    let cleared = clear_cache().into_diagnostic()?;
    println!("cleared cache at {}", cleared.display());
    Ok(())
}

fn cmd_import_mise(dry_run: bool) -> Result<()> {
    let root = workspace_root()?;
    let report = import_mise_tools(&root, dry_run)
        .into_diagnostic()
        .wrap_err("failed to import mise.toml tools")?;
    if report.added.is_empty() && report.skipped.is_empty() && report.ignored.is_empty() {
        println!("no tools found in mise.toml");
        return Ok(());
    }
    for entry in &report.added {
        if dry_run {
            println!("would import {entry}");
        } else {
            println!("imported {entry}");
        }
    }
    for id in &report.skipped {
        println!("skipped {id} (already configured)");
    }
    for id in &report.ignored {
        println!("ignored {id} (not a github: tool)");
    }
    Ok(())
}

fn cmd_global_update(
    tool: Option<&str>,
    environment: Option<&str>,
    host: &HostPlatform,
    dry_run: bool,
) -> Result<()> {
    let cfg = load_global_tools().into_diagnostic()?;
    let requests = filter_requests(cfg.tools, tool, |id| {
        format!(
            "tool `{id}` is not configured in {}",
            pixi_mise_core::pixi::global_config_path().display()
        )
    })?;
    if requests.is_empty() {
        println!(
            "no tools configured under [tools] in {}",
            pixi_mise_core::pixi::global_config_path().display()
        );
        return Ok(());
    }

    let client = GithubClient::new();
    let home = pixi_home();
    for request in &requests {
        let env = environment
            .map(str::to_string)
            .unwrap_or_else(|| global_env_name(&request.id.github_spec()));
        let previous = read_tool_meta(&home, &env, &request.id.github_spec())
            .into_diagnostic()?
            .map(|m| m.tag);
        let resolved = resolve_tool(&client, request, host)
            .into_diagnostic()
            .wrap_err_with(|| format!("failed to resolve {}", request.id.github_spec()))?;
        match &previous {
            Some(old) if old == &resolved.tag => {
                println!(
                    "{}: already up to date at {}",
                    request.id.github_spec(),
                    resolved.tag
                );
            }
            Some(old) => {
                println!("{}: {} -> {}", request.id.github_spec(), old, resolved.tag);
            }
            None => {
                println!("{}: installing {}", request.id.github_spec(), resolved.tag);
            }
        }
        let target = InstallTarget::Global { env };
        let outcome = install_tool(&client, &resolved, &target, host, dry_run)
            .into_diagnostic()
            .wrap_err_with(|| format!("failed to install {}", request.id.github_spec()))?;
        print_install_outcome(&outcome, dry_run);
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

    #[test]
    fn parses_lock_and_locked_flag() {
        let cli = Cli::try_parse_from(["pixi-mise", "--locked", "lock"]).unwrap();
        assert!(cli.locked);
        assert!(matches!(cli.command, Commands::Lock));
    }

    #[test]
    fn parses_phase3_commands() {
        let search = Cli::try_parse_from(["pixi-mise", "search", "github:cli/cli"]).unwrap();
        assert!(matches!(search.command, Commands::Search { .. }));
        let update = Cli::try_parse_from(["pixi-mise", "update"]).unwrap();
        assert!(matches!(update.command, Commands::Update { tool: None }));
        let upgrade = Cli::try_parse_from(["pixi-mise", "upgrade", "github:cli/cli"]).unwrap();
        assert!(matches!(upgrade.command, Commands::Upgrade { .. }));
        let reinstall = Cli::try_parse_from(["pixi-mise", "reinstall"]).unwrap();
        assert!(matches!(
            reinstall.command,
            Commands::Reinstall { tool: None }
        ));
        let which = Cli::try_parse_from(["pixi-mise", "which", "rg"]).unwrap();
        assert!(matches!(which.command, Commands::Which { .. }));
        let clean = Cli::try_parse_from(["pixi-mise", "clean", "cache"]).unwrap();
        assert!(matches!(
            clean.command,
            Commands::Clean {
                command: CleanCommands::Cache
            }
        ));
        let import = Cli::try_parse_from(["pixi-mise", "import-mise"]).unwrap();
        assert!(matches!(import.command, Commands::ImportMise));
        let gupdate = Cli::try_parse_from(["pixi-mise", "global", "update"]).unwrap();
        assert!(matches!(
            gupdate.command,
            Commands::Global {
                command: GlobalCommands::Update { tool: None }
            }
        ));
    }
}
