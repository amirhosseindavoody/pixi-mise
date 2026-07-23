//! Parse and update `[tool.pixi-mise.tools]` in `pixi.toml`.

use std::fs;
use std::path::{Path, PathBuf};

use toml::Value;
use toml_edit::{Array, DocumentMut, InlineTable, Item, Table, value};

use crate::registry::RegistrySettings;
use crate::{
    ConfigSource, CoreError, ToolId, ToolOptions, ToolRequest, VersionSpec, parse_tool_spec,
};

/// Loaded workspace tool configuration.
#[derive(Debug, Clone)]
pub struct WorkspaceConfig {
    /// Workspace root (directory containing `pixi.toml`).
    pub root: PathBuf,
    /// Path to `pixi.toml`.
    pub pixi_toml: PathBuf,
    /// Declared tools.
    pub tools: Vec<ToolRequest>,
    /// Registry settings from `[tool.pixi-mise]`.
    pub registry: RegistrySettings,
}

/// Walk parents looking for `pixi.toml`.
pub fn find_workspace_root(start: &Path) -> Result<PathBuf, CoreError> {
    let mut dir = if start.is_file() {
        start
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| start.to_path_buf())
    } else {
        start.to_path_buf()
    };
    loop {
        let candidate = dir.join("pixi.toml");
        if candidate.is_file() {
            return Ok(dir);
        }
        if !dir.pop() {
            return Err(CoreError::NoWorkspace);
        }
    }
}

/// Load `[tool.pixi-mise.tools]` from the workspace `pixi.toml`.
pub fn load_workspace_tools(workspace_root: &Path) -> Result<WorkspaceConfig, CoreError> {
    let pixi_toml = workspace_root.join("pixi.toml");
    if !pixi_toml.is_file() {
        return Err(CoreError::NoWorkspace);
    }
    let text = fs::read_to_string(&pixi_toml).map_err(|e| CoreError::Config(e.to_string()))?;
    let value: Value = text
        .parse()
        .map_err(|e| CoreError::Config(format!("parse pixi.toml: {e}")))?;
    let tools = parse_tools_table(&value, &pixi_toml)?;
    let registry = parse_registry_settings(&value, workspace_root);
    Ok(WorkspaceConfig {
        root: workspace_root.to_path_buf(),
        pixi_toml,
        tools,
        registry,
    })
}

/// Registry settings for a workspace (defaults when no `pixi.toml` section).
pub fn workspace_registry_settings(workspace_root: &Path) -> RegistrySettings {
    let pixi_toml = workspace_root.join("pixi.toml");
    if !pixi_toml.is_file() {
        return RegistrySettings::default();
    }
    let Ok(text) = fs::read_to_string(&pixi_toml) else {
        return RegistrySettings::default();
    };
    let Ok(value) = text.parse::<Value>() else {
        return RegistrySettings::default();
    };
    parse_registry_settings(&value, workspace_root)
}

fn parse_registry_settings(doc: &Value, workspace_root: &Path) -> RegistrySettings {
    let mut settings = RegistrySettings::default();
    let Some(table) = doc
        .get("tool")
        .and_then(|t| t.get("pixi-mise"))
        .and_then(|t| t.as_table())
    else {
        // Auto-detect local slim registry beside pixi.toml.
        let local = workspace_root.join("pixi-mise-registry.toml");
        if local.is_file() {
            settings.local_path = Some(local);
        }
        return settings;
    };

    if let Some(Value::Boolean(b)) = table.get("registry") {
        settings.enabled = *b;
    } else if let Some(Value::String(s)) = table.get("registry") {
        match s.as_str() {
            "false" | "off" | "none" => settings.enabled = false,
            "aqua" | "true" | "on" => settings.enabled = true,
            other => {
                settings.enabled = true;
                settings.aqua_base_url = other.to_string();
            }
        }
    }

    if let Some(path) = table.get("registry_path").and_then(|v| v.as_str()) {
        let p = PathBuf::from(path);
        settings.local_path = Some(if p.is_absolute() {
            p
        } else {
            workspace_root.join(p)
        });
    } else {
        let local = workspace_root.join("pixi-mise-registry.toml");
        if local.is_file() {
            settings.local_path = Some(local);
        }
    }

    settings
}

fn parse_tools_table(doc: &Value, pixi_toml: &Path) -> Result<Vec<ToolRequest>, CoreError> {
    let Some(tools) = doc
        .get("tool")
        .and_then(|t| t.get("pixi-mise"))
        .and_then(|t| t.get("tools"))
    else {
        return Ok(Vec::new());
    };

    let table = tools
        .as_table()
        .ok_or_else(|| CoreError::Config("`tool.pixi-mise.tools` must be a table".into()))?;

    let source = ConfigSource {
        path: pixi_toml.to_path_buf(),
        table: "tool.pixi-mise.tools".into(),
    };

    let mut out = Vec::new();
    for (key, val) in table {
        let (id, default_version) = parse_tool_spec(key)?;
        let (version, options) = parse_tool_value(val, default_version)?;
        out.push(ToolRequest {
            backend: crate::BackendKind::Github,
            id,
            version,
            options,
            source: source.clone(),
        });
    }
    out.sort_by_key(|a| a.id.as_str());
    Ok(out)
}

fn parse_tool_value(
    val: &Value,
    default_version: VersionSpec,
) -> Result<(VersionSpec, ToolOptions), CoreError> {
    match val {
        Value::String(s) => Ok((parse_version_string(s), ToolOptions::default())),
        Value::Table(table) => {
            let version = table
                .get("version")
                .and_then(|v| v.as_str())
                .map(parse_version_string)
                .unwrap_or(default_version);
            let mut options = ToolOptions::default();
            if let Some(m) = table.get("matching").and_then(|v| v.as_str()) {
                options.matching = Some(m.to_string());
            }
            if let Some(m) = table.get("matching_regex").and_then(|v| v.as_str()) {
                options.matching_regex = Some(m.to_string());
            }
            if let Some(m) = table.get("asset_pattern").and_then(|v| v.as_str()) {
                options.asset_pattern = Some(m.to_string());
            }
            if let Some(m) = table.get("bin").and_then(|v| v.as_str()) {
                options.bin = Some(m.to_string());
            }
            if let Some(m) = table.get("rename_exe").and_then(|v| v.as_str()) {
                options.rename_exe = Some(m.to_string());
            }
            if let Some(m) = table.get("version_prefix").and_then(|v| v.as_str()) {
                options.version_prefix = Some(m.to_string());
            }
            if let Some(b) = table.get("prerelease").and_then(|v| v.as_bool()) {
                options.prerelease = b;
            }
            if let Some(m) = table.get("expose_as").and_then(|v| v.as_str()) {
                options.expose_as = Some(m.to_string());
            }
            if let Some(b) = table.get("registry").and_then(|v| v.as_bool()) {
                options.registry = Some(b);
            }
            if let Some(os_val) = table.get("os") {
                options.os = parse_os_filter(os_val)?;
            }
            Ok((version, options))
        }
        other => Err(CoreError::Config(format!(
            "unsupported tool value type: {other:?}"
        ))),
    }
}

fn parse_os_filter(val: &Value) -> Result<Vec<String>, CoreError> {
    match val {
        Value::String(s) => Ok(vec![s.clone()]),
        Value::Array(items) => {
            let mut out = Vec::new();
            for item in items {
                let Some(s) = item.as_str() else {
                    return Err(CoreError::Config(
                        "`os` entries must be strings (e.g. \"linux\", \"macos/arm64\")".into(),
                    ));
                };
                out.push(s.to_string());
            }
            Ok(out)
        }
        _ => Err(CoreError::Config(
            "`os` must be a string or array of strings".into(),
        )),
    }
}

fn parse_version_string(raw: &str) -> VersionSpec {
    crate::version::parse_version_spec(raw)
}

/// Add or update a tool entry in `pixi.toml`.
///
/// Uses `toml_edit` so existing key order, comments, and formatting are preserved.
/// New `[tool.pixi-mise.tools]` tables are appended; existing tables only gain/replace
/// the tool key.
pub fn add_tool_to_pixi_toml(
    pixi_toml: &Path,
    id: &ToolId,
    version: &VersionSpec,
    options: &ToolOptions,
) -> Result<(), CoreError> {
    let text = fs::read_to_string(pixi_toml).map_err(|e| CoreError::Config(e.to_string()))?;
    let mut doc: DocumentMut = text
        .parse()
        .map_err(|e| CoreError::Config(format!("parse pixi.toml: {e}")))?;

    let tools = ensure_dotted_table(&mut doc, &["tool", "pixi-mise", "tools"])?;
    let key = id.github_spec();
    tools[key.as_str()] = tool_item_for_write(version, options);

    fs::write(pixi_toml, doc.to_string()).map_err(|e| CoreError::Config(e.to_string()))?;
    Ok(())
}

/// Ensure `a.b.c` exists as an explicit table chain, creating missing parents as needed.
fn ensure_dotted_table<'a>(
    doc: &'a mut DocumentMut,
    path: &[&str],
) -> Result<&'a mut Table, CoreError> {
    // Walk/create intermediate tables via Item indexing, then return the leaf.
    {
        let mut item = doc.as_item_mut();
        for (i, key) in path.iter().enumerate() {
            let is_leaf = i + 1 == path.len();
            if item.get(key).is_none() {
                let mut table = Table::new();
                // Leaf should render as `[tool.pixi-mise.tools]`; intermediates stay implicit.
                if !is_leaf {
                    table.set_implicit(true);
                }
                item[key] = Item::Table(table);
            } else if !item[key].is_table() && !item[key].is_none() {
                return Err(CoreError::Config(format!(
                    "`{}` must be a table",
                    path[..=i].join(".")
                )));
            }
            item = &mut item[key];
        }
    }

    let mut item = doc.as_item_mut();
    for key in path {
        item = &mut item[key];
    }
    item.as_table_mut()
        .ok_or_else(|| CoreError::Config(format!("`{}` must be a table", path.join("."))))
}

fn tool_item_for_write(version: &VersionSpec, options: &ToolOptions) -> Item {
    let has_options = options.matching.is_some()
        || options.matching_regex.is_some()
        || options.asset_pattern.is_some()
        || options.bin.is_some()
        || options.rename_exe.is_some()
        || options.version_prefix.is_some()
        || options.prerelease
        || options.expose_as.is_some()
        || options.registry.is_some()
        || !options.os.is_empty();

    if !has_options {
        return value(version.to_config_string());
    }

    let mut table = InlineTable::new();
    table.insert(
        "version",
        value(version.to_config_string()).into_value().unwrap(),
    );
    if let Some(m) = &options.matching {
        table.insert("matching", value(m.as_str()).into_value().unwrap());
    }
    if let Some(m) = &options.matching_regex {
        table.insert("matching_regex", value(m.as_str()).into_value().unwrap());
    }
    if let Some(m) = &options.asset_pattern {
        table.insert("asset_pattern", value(m.as_str()).into_value().unwrap());
    }
    if let Some(m) = &options.bin {
        table.insert("bin", value(m.as_str()).into_value().unwrap());
    }
    if let Some(m) = &options.rename_exe {
        table.insert("rename_exe", value(m.as_str()).into_value().unwrap());
    }
    if let Some(m) = &options.version_prefix {
        table.insert("version_prefix", value(m.as_str()).into_value().unwrap());
    }
    if options.prerelease {
        table.insert("prerelease", value(true).into_value().unwrap());
    }
    if let Some(m) = &options.expose_as {
        table.insert("expose_as", value(m.as_str()).into_value().unwrap());
    }
    if let Some(b) = options.registry {
        table.insert("registry", value(b).into_value().unwrap());
    }
    if !options.os.is_empty() {
        let mut arr = Array::new();
        for os in &options.os {
            arr.push(os.as_str());
        }
        table.insert("os", toml_edit::Value::Array(arr));
    }
    Item::Value(toml_edit::Value::InlineTable(table))
}

/// Remove a tool entry from `pixi.toml` without rewriting unrelated content.
pub fn remove_tool_from_pixi_toml(pixi_toml: &Path, id: &ToolId) -> Result<bool, CoreError> {
    let text = fs::read_to_string(pixi_toml).map_err(|e| CoreError::Config(e.to_string()))?;
    let mut doc: DocumentMut = text
        .parse()
        .map_err(|e| CoreError::Config(format!("parse pixi.toml: {e}")))?;

    let key = id.github_spec();
    let Some(tools) = doc
        .get_mut("tool")
        .and_then(|t| t.get_mut("pixi-mise"))
        .and_then(|t| t.get_mut("tools"))
        .and_then(|t| t.as_table_like_mut())
    else {
        return Ok(false);
    };
    let removed = tools.remove(&key).is_some();
    if removed {
        fs::write(pixi_toml, doc.to_string()).map_err(|e| CoreError::Config(e.to_string()))?;
    }
    Ok(removed)
}

fn tool_value_for_write(version: &VersionSpec, options: &ToolOptions) -> Value {
    let has_options = options.matching.is_some()
        || options.matching_regex.is_some()
        || options.asset_pattern.is_some()
        || options.bin.is_some()
        || options.rename_exe.is_some()
        || options.version_prefix.is_some()
        || options.prerelease
        || options.expose_as.is_some()
        || options.registry.is_some()
        || !options.os.is_empty();

    if !has_options {
        return Value::String(version.to_config_string());
    }

    let mut table = toml::map::Map::new();
    table.insert("version".into(), Value::String(version.to_config_string()));
    if let Some(m) = &options.matching {
        table.insert("matching".into(), Value::String(m.clone()));
    }
    if let Some(m) = &options.matching_regex {
        table.insert("matching_regex".into(), Value::String(m.clone()));
    }
    if let Some(m) = &options.asset_pattern {
        table.insert("asset_pattern".into(), Value::String(m.clone()));
    }
    if let Some(m) = &options.bin {
        table.insert("bin".into(), Value::String(m.clone()));
    }
    if let Some(m) = &options.rename_exe {
        table.insert("rename_exe".into(), Value::String(m.clone()));
    }
    if let Some(m) = &options.version_prefix {
        table.insert("version_prefix".into(), Value::String(m.clone()));
    }
    if options.prerelease {
        table.insert("prerelease".into(), Value::Boolean(true));
    }
    if let Some(m) = &options.expose_as {
        table.insert("expose_as".into(), Value::String(m.clone()));
    }
    if let Some(b) = options.registry {
        table.insert("registry".into(), Value::Boolean(b));
    }
    if !options.os.is_empty() {
        table.insert(
            "os".into(),
            Value::Array(options.os.iter().cloned().map(Value::String).collect()),
        );
    }
    Value::Table(table)
}

/// Loaded global tool configuration (`$PIXI_HOME/pixi-mise.toml`).
#[derive(Debug, Clone)]
pub struct GlobalConfig {
    /// Path to the global config file.
    pub path: PathBuf,
    /// Declared tools.
    pub tools: Vec<ToolRequest>,
}

/// Load `[tools]` from `$PIXI_HOME/pixi-mise.toml` (empty if missing).
pub fn load_global_tools() -> Result<GlobalConfig, CoreError> {
    let path = pixi_mise_pixi::global_config_path();
    if !path.is_file() {
        return Ok(GlobalConfig {
            path,
            tools: Vec::new(),
        });
    }
    let text = fs::read_to_string(&path).map_err(|e| CoreError::Config(e.to_string()))?;
    let value: Value = text
        .parse()
        .map_err(|e| CoreError::Config(format!("parse global config: {e}")))?;
    let tools = parse_global_tools_table(&value, &path)?;
    Ok(GlobalConfig { path, tools })
}

fn parse_global_tools_table(doc: &Value, path: &Path) -> Result<Vec<ToolRequest>, CoreError> {
    let Some(tools) = doc.get("tools") else {
        return Ok(Vec::new());
    };
    let table = tools
        .as_table()
        .ok_or_else(|| CoreError::Config("`tools` must be a table in global config".into()))?;
    let source = ConfigSource {
        path: path.to_path_buf(),
        table: "tools".into(),
    };
    let mut out = Vec::new();
    for (key, val) in table {
        let (id, default_version) = parse_tool_spec(key)?;
        let (version, options) = parse_tool_value(val, default_version)?;
        out.push(ToolRequest {
            backend: crate::BackendKind::Github,
            id,
            version,
            options,
            source: source.clone(),
        });
    }
    out.sort_by_key(|a| a.id.as_str());
    Ok(out)
}

/// Add or update a tool in `$PIXI_HOME/pixi-mise.toml`.
pub fn add_tool_to_global_config(
    id: &ToolId,
    version: &VersionSpec,
    options: &ToolOptions,
) -> Result<PathBuf, CoreError> {
    let path = pixi_mise_pixi::global_config_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| CoreError::Config(e.to_string()))?;
    }
    let mut doc: Value = if path.is_file() {
        let text = fs::read_to_string(&path).map_err(|e| CoreError::Config(e.to_string()))?;
        text.parse()
            .map_err(|e| CoreError::Config(format!("parse global config: {e}")))?
    } else {
        Value::Table(Default::default())
    };

    let tools = doc
        .as_table_mut()
        .ok_or_else(|| CoreError::Config("global config root must be a table".into()))?
        .entry("tools")
        .or_insert_with(|| Value::Table(Default::default()));
    let tools_table = tools
        .as_table_mut()
        .ok_or_else(|| CoreError::Config("`tools` must be a table".into()))?;
    tools_table.insert(id.github_spec(), tool_value_for_write(version, options));

    let rendered = toml::to_string_pretty(&doc).map_err(|e| CoreError::Config(e.to_string()))?;
    fs::write(&path, rendered).map_err(|e| CoreError::Config(e.to_string()))?;
    Ok(path)
}

/// Remove a tool from `$PIXI_HOME/pixi-mise.toml`.
pub fn remove_tool_from_global_config(id: &ToolId) -> Result<bool, CoreError> {
    let path = pixi_mise_pixi::global_config_path();
    if !path.is_file() {
        return Ok(false);
    }
    let text = fs::read_to_string(&path).map_err(|e| CoreError::Config(e.to_string()))?;
    let mut doc: Value = text
        .parse()
        .map_err(|e| CoreError::Config(format!("parse global config: {e}")))?;
    let key = id.github_spec();
    let removed = doc
        .get_mut("tools")
        .and_then(|t| t.as_table_mut())
        .map(|tools| tools.remove(&key).is_some())
        .unwrap_or(false);
    if removed {
        let rendered =
            toml::to_string_pretty(&doc).map_err(|e| CoreError::Config(e.to_string()))?;
        fs::write(&path, rendered).map_err(|e| CoreError::Config(e.to_string()))?;
    }
    Ok(removed)
}

/// Result of importing tools from a `mise.toml`.
#[derive(Debug, Clone, Default)]
pub struct MiseImportReport {
    /// Tools added to `pixi.toml`.
    pub added: Vec<String>,
    /// Tools already present (skipped).
    pub skipped: Vec<String>,
    /// Non-github mise tools ignored.
    pub ignored: Vec<String>,
}

/// Import `github:` tools from `mise.toml` / `.mise.toml` into workspace `pixi.toml`.
pub fn import_mise_tools(
    workspace_root: &Path,
    dry_run: bool,
) -> Result<MiseImportReport, CoreError> {
    let mise_path = ["mise.toml", ".mise.toml", ".config/mise.toml"]
        .iter()
        .map(|p| workspace_root.join(p))
        .find(|p| p.is_file())
        .ok_or_else(|| {
            CoreError::Config("no mise.toml / .mise.toml found in workspace root".into())
        })?;

    let text = fs::read_to_string(&mise_path).map_err(|e| CoreError::Config(e.to_string()))?;
    let doc: Value = text
        .parse()
        .map_err(|e| CoreError::Config(format!("parse {}: {e}", mise_path.display())))?;

    let Some(tools) = doc.get("tools").and_then(|t| t.as_table()) else {
        return Ok(MiseImportReport::default());
    };

    let existing = load_workspace_tools(workspace_root)?;
    let pixi_toml = workspace_root.join("pixi.toml");
    let mut report = MiseImportReport::default();

    for (key, val) in tools {
        let key = key.trim();
        // mise forms: "github:owner/repo" = "1.2.3" or table with version
        if !key.starts_with("github:") {
            // Also accept backend-style "owner/repo" under tools with github: prefix missing —
            // only import explicit github: keys.
            report.ignored.push(key.to_string());
            continue;
        }
        let (id, default_version) = parse_tool_spec(key)?;
        let (version, mut options) = parse_tool_value(val, default_version)?;
        // mise uses `version` / string; map common option aliases if present in table.
        if let Value::Table(table) = val
            && options.matching.is_none()
            && let Some(m) = table.get("matching").and_then(|v| v.as_str())
        {
            options.matching = Some(m.to_string());
        }
        if existing.tools.iter().any(|t| t.id == id) {
            report.skipped.push(id.github_spec());
            continue;
        }
        report.added.push(format!(
            "{} = \"{}\"",
            id.github_spec(),
            version.to_config_string()
        ));
        if !dry_run {
            add_tool_to_pixi_toml(&pixi_toml, &id, &version, &options)?;
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_workspace(contents: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("pixi-mise-cfg-{nanos}"));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("pixi.toml"), contents).unwrap();
        dir
    }

    #[test]
    fn parse_string_and_table_tools() {
        let root = temp_workspace(
            r#"
[workspace]
name = "demo"

[tool.pixi-mise.tools]
"github:BurntSushi/ripgrep" = "14.1.1"
"github:cli/cli" = { version = "latest", matching = "gh_" }
"#,
        );
        let cfg = load_workspace_tools(&root).unwrap();
        assert_eq!(cfg.tools.len(), 2);
        let rg = cfg.tools.iter().find(|t| t.id.repo == "ripgrep").unwrap();
        assert_eq!(rg.version, VersionSpec::Exact("14.1.1".into()));
        let gh = cfg.tools.iter().find(|t| t.id.repo == "cli").unwrap();
        assert_eq!(gh.version, VersionSpec::Latest);
        assert_eq!(gh.options.matching.as_deref(), Some("gh_"));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn add_and_remove_tool() {
        let root = temp_workspace(
            r#"
[workspace]
name = "demo"
"#,
        );
        let toml_path = root.join("pixi.toml");
        let id = ToolId {
            owner: "BurntSushi".into(),
            repo: "ripgrep".into(),
        };
        add_tool_to_pixi_toml(
            &toml_path,
            &id,
            &VersionSpec::Prefix("14".into()),
            &ToolOptions::default(),
        )
        .unwrap();
        let cfg = load_workspace_tools(&root).unwrap();
        assert_eq!(cfg.tools.len(), 1);
        assert!(remove_tool_from_pixi_toml(&toml_path, &id).unwrap());
        let cfg = load_workspace_tools(&root).unwrap();
        assert!(cfg.tools.is_empty());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn add_tool_preserves_existing_pixi_toml_order() {
        let original = r#"# keep this comment
[workspace]
name = "demo"
channels = ["conda-forge"]
platforms = ["linux-64"]

[dependencies]
python = "3.12.*"

[tasks]
hello = "echo hi"
"#;
        let root = temp_workspace(original);
        let toml_path = root.join("pixi.toml");
        let id = ToolId {
            owner: "BurntSushi".into(),
            repo: "ripgrep".into(),
        };
        add_tool_to_pixi_toml(
            &toml_path,
            &id,
            &VersionSpec::Prefix("14".into()),
            &ToolOptions::default(),
        )
        .unwrap();
        let updated = fs::read_to_string(&toml_path).unwrap();
        assert!(
            updated.starts_with("# keep this comment\n[workspace]\nname = \"demo\""),
            "prefix/order changed:\n{updated}"
        );
        assert!(
            updated.contains("[dependencies]\npython = \"3.12.*\""),
            "dependencies reordered:\n{updated}"
        );
        assert!(
            updated.contains("[tasks]\nhello = \"echo hi\""),
            "tasks missing/reordered:\n{updated}"
        );
        assert!(
            updated.contains("[tool.pixi-mise.tools]"),
            "tools section missing:\n{updated}"
        );
        assert!(
            updated.contains("\"github:BurntSushi/ripgrep\" = \"14\""),
            "tool entry missing:\n{updated}"
        );
        // Existing sections should still appear before the newly appended tools table.
        let deps_at = updated.find("[dependencies]").unwrap();
        let tools_at = updated.find("[tool.pixi-mise.tools]").unwrap();
        assert!(
            deps_at < tools_at,
            "tools section was not appended:\n{updated}"
        );

        // Second add should only append to the tools table.
        let id2 = ToolId {
            owner: "cli".into(),
            repo: "cli".into(),
        };
        add_tool_to_pixi_toml(
            &toml_path,
            &id2,
            &VersionSpec::Latest,
            &ToolOptions {
                matching: Some("gh_".into()),
                ..ToolOptions::default()
            },
        )
        .unwrap();
        let updated2 = fs::read_to_string(&toml_path).unwrap();
        assert_eq!(
            updated2.matches("[tool.pixi-mise.tools]").count(),
            1,
            "duplicate tools headers:\n{updated2}"
        );
        assert!(updated2.contains("\"github:cli/cli\""));
        assert!(
            updated2.starts_with("# keep this comment\n[workspace]\nname = \"demo\""),
            "second add rewrote file:\n{updated2}"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn import_mise_github_tools() {
        let root = temp_workspace(
            r#"
[workspace]
name = "demo"
"#,
        );
        fs::write(
            root.join("mise.toml"),
            r#"
[tools]
"github:BurntSushi/ripgrep" = "14"
node = "20"
"github:cli/cli" = { version = "2.67.0" }
"#,
        )
        .unwrap();
        let report = import_mise_tools(&root, false).unwrap();
        assert_eq!(report.added.len(), 2);
        assert!(report.ignored.iter().any(|s| s == "node"));
        let cfg = load_workspace_tools(&root).unwrap();
        assert_eq!(cfg.tools.len(), 2);
        let _ = fs::remove_dir_all(&root);
    }
}
