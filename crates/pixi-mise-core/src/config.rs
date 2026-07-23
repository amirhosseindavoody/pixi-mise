//! Parse and update `[tool.pixi-mise.tools]` in `pixi.toml`.

use std::fs;
use std::path::{Path, PathBuf};

use toml::Value;

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
    Ok(WorkspaceConfig {
        root: workspace_root.to_path_buf(),
        pixi_toml,
        tools,
    })
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
            Ok((version, options))
        }
        other => Err(CoreError::Config(format!(
            "unsupported tool value type: {other:?}"
        ))),
    }
}

fn parse_version_string(raw: &str) -> VersionSpec {
    if raw.is_empty() || raw.eq_ignore_ascii_case("latest") {
        VersionSpec::Latest
    } else {
        let stripped = raw.trim_start_matches('v');
        if !stripped.is_empty() && stripped.chars().all(|c| c.is_ascii_digit()) {
            VersionSpec::Prefix(stripped.to_string())
        } else {
            VersionSpec::Exact(raw.to_string())
        }
    }
}

/// Add or update a tool entry in `pixi.toml`.
pub fn add_tool_to_pixi_toml(
    pixi_toml: &Path,
    id: &ToolId,
    version: &VersionSpec,
    options: &ToolOptions,
) -> Result<(), CoreError> {
    let text = fs::read_to_string(pixi_toml).map_err(|e| CoreError::Config(e.to_string()))?;
    let mut doc: Value = text
        .parse()
        .map_err(|e| CoreError::Config(format!("parse pixi.toml: {e}")))?;

    let tool = doc
        .as_table_mut()
        .ok_or_else(|| CoreError::Config("pixi.toml root must be a table".into()))?
        .entry("tool")
        .or_insert_with(|| Value::Table(Default::default()));
    let tool_table = tool
        .as_table_mut()
        .ok_or_else(|| CoreError::Config("`tool` must be a table".into()))?;
    let pixi_mise = tool_table
        .entry("pixi-mise")
        .or_insert_with(|| Value::Table(Default::default()));
    let pixi_mise_table = pixi_mise
        .as_table_mut()
        .ok_or_else(|| CoreError::Config("`tool.pixi-mise` must be a table".into()))?;
    let tools = pixi_mise_table
        .entry("tools")
        .or_insert_with(|| Value::Table(Default::default()));
    let tools_table = tools
        .as_table_mut()
        .ok_or_else(|| CoreError::Config("`tool.pixi-mise.tools` must be a table".into()))?;

    let key = id.github_spec();
    tools_table.insert(key, tool_value_for_write(version, options));

    let rendered = toml::to_string_pretty(&doc).map_err(|e| CoreError::Config(e.to_string()))?;
    fs::write(pixi_toml, rendered).map_err(|e| CoreError::Config(e.to_string()))?;
    Ok(())
}

fn tool_value_for_write(version: &VersionSpec, options: &ToolOptions) -> Value {
    let has_options = options.matching.is_some()
        || options.matching_regex.is_some()
        || options.asset_pattern.is_some()
        || options.bin.is_some()
        || options.rename_exe.is_some()
        || options.version_prefix.is_some()
        || options.prerelease
        || options.expose_as.is_some();

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
    Value::Table(table)
}

/// Remove a tool entry from `pixi.toml`.
pub fn remove_tool_from_pixi_toml(pixi_toml: &Path, id: &ToolId) -> Result<bool, CoreError> {
    let text = fs::read_to_string(pixi_toml).map_err(|e| CoreError::Config(e.to_string()))?;
    let mut doc: Value = text
        .parse()
        .map_err(|e| CoreError::Config(format!("parse pixi.toml: {e}")))?;

    let key = id.github_spec();
    let removed = doc
        .get_mut("tool")
        .and_then(|t| t.get_mut("pixi-mise"))
        .and_then(|t| t.get_mut("tools"))
        .and_then(|t| t.as_table_mut())
        .map(|tools| tools.remove(&key).is_some())
        .unwrap_or(false);

    if removed {
        let rendered =
            toml::to_string_pretty(&doc).map_err(|e| CoreError::Config(e.to_string()))?;
        fs::write(pixi_toml, rendered).map_err(|e| CoreError::Config(e.to_string()))?;
    }
    Ok(removed)
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
}
