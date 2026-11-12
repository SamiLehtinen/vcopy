use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(untagged)]
pub enum PathsValue {
    Single(PathBuf),
    Multiple(Vec<PathBuf>),
}

impl PathsValue {
    pub fn into_vec(self) -> Vec<PathBuf> {
        match self {
            PathsValue::Single(p) => {
                let s = p.to_string_lossy();
                if s.contains(',') {
                    s.split(',')
                        .map(|part| part.trim())
                        .filter(|part| !part.is_empty())
                        .map(PathBuf::from)
                        .collect()
                } else {
                    vec![p]
                }
            }
            PathsValue::Multiple(v) => v,
        }
    }
}

#[derive(Debug, Default, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub struct ConfigFile {
    #[serde(
        default,
        alias = "sources",
        alias = "source",
        alias = "paths",
        alias = "--sources",
        alias = "--source",
        alias = "-s"
    )]
    pub sources: Option<PathsValue>,

    #[serde(
        default,
        alias = "dest",
        alias = "destination",
        alias = "--dest",
        alias = "--destination",
        alias = "-d"
    )]
    pub dest: Option<PathBuf>,

    #[serde(
        default,
        alias = "paths-from",
        alias = "paths_from",
        alias = "files-from",
        alias = "files_from",
        alias = "sources-from",
        alias = "sources_from",
        alias = "--paths-from",
        alias = "--files-from",
        alias = "--sources-from",
        alias = "-f"
    )]
    pub paths_from: Option<PathsValue>,

    #[serde(
        default,
        alias = "max-versions",
        alias = "max_versions",
        alias = "--max-versions",
        alias = "-n"
    )]
    pub max_versions: Option<usize>,

    #[serde(
        default,
        alias = "keep-hourly",
        alias = "keep_hourly",
        alias = "hourly",
        alias = "--keep-hourly",
        alias = "--hourly"
    )]
    pub keep_hourly: Option<usize>,

    #[serde(
        default,
        alias = "keep-daily",
        alias = "keep_daily",
        alias = "daily",
        alias = "--keep-daily",
        alias = "--daily"
    )]
    pub keep_daily: Option<usize>,

    #[serde(
        default,
        alias = "keep-weekly",
        alias = "keep_weekly",
        alias = "weekly",
        alias = "--keep-weekly",
        alias = "--weekly"
    )]
    pub keep_weekly: Option<usize>,

    #[serde(
        default,
        alias = "keep-monthly",
        alias = "keep_monthly",
        alias = "monthly",
        alias = "--keep-monthly",
        alias = "--monthly"
    )]
    pub keep_monthly: Option<usize>,

    #[serde(
        default,
        alias = "keep-yearly",
        alias = "keep_yearly",
        alias = "yearly",
        alias = "--keep-yearly",
        alias = "--yearly"
    )]
    pub keep_yearly: Option<usize>,

    #[serde(default, alias = "dry-run", alias = "dry_run", alias = "--dry-run")]
    pub dry_run: Option<bool>,

    #[serde(default, alias = "verbose", alias = "--verbose", alias = "-v")]
    pub verbose: Option<bool>,

    #[serde(
        default,
        alias = "flatten",
        alias = "flat",
        alias = "--flatten",
        alias = "--flat"
    )]
    pub flatten: Option<bool>,

    #[serde(
        default,
        alias = "preserve-permissions",
        alias = "preserve_permissions",
        alias = "permissions",
        alias = "perms",
        alias = "--preserve-permissions",
        alias = "--permissions",
        alias = "--perms",
        alias = "-p"
    )]
    pub preserve_permissions: Option<bool>,

    #[serde(default, alias = "force", alias = "--force", alias = "-F")]
    pub force: Option<bool>,

    #[serde(
        default,
        alias = "allow-existing",
        alias = "allow_existing",
        alias = "--allow-existing"
    )]
    pub allow_existing: Option<bool>,

    #[serde(
        default,
        alias = "allow-empty-source",
        alias = "allow_empty_source",
        alias = "--allow-empty-source"
    )]
    pub allow_empty_source: Option<bool>,

    #[serde(
        default,
        alias = "allow-missing-source",
        alias = "allow_missing_source",
        alias = "--allow-missing-source"
    )]
    pub allow_missing_source: Option<bool>,

    #[serde(
        default,
        alias = "target-time",
        alias = "target_time",
        alias = "time",
        alias = "snapshot",
        alias = "--target-time",
        alias = "--time",
        alias = "--snapshot",
        alias = "-t"
    )]
    pub target_time: Option<String>,

    #[serde(
        default,
        alias = "target-out",
        alias = "target_out",
        alias = "output",
        alias = "output-dir",
        alias = "output_dir",
        alias = "--target-out",
        alias = "--output"
    )]
    pub target_out: Option<PathBuf>,

    #[serde(
        default,
        alias = "command",
        alias = "action",
        alias = "subcommand",
        alias = "mode"
    )]
    pub command: Option<String>,
}

impl ConfigFile {
    pub fn get_sources(&self) -> Vec<PathBuf> {
        self.sources
            .as_ref()
            .map(|s| s.clone().into_vec())
            .unwrap_or_default()
    }

    pub fn get_paths_from(&self) -> Vec<String> {
        self.paths_from
            .as_ref()
            .map(|s| {
                s.clone()
                    .into_vec()
                    .into_iter()
                    .map(|p| p.to_string_lossy().to_string())
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// Tokenize string taking shell-style quotes ("..." and '...') and # comments into account
fn tokenize_config(content: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    for raw_line in content.lines() {
        let line = raw_line.trim();
        if line.is_empty()
            || line.starts_with('#')
            || line.starts_with("//")
            || line.starts_with(';')
        {
            continue;
        }

        let mut chars = line.chars().peekable();
        let mut cur = String::new();
        let mut in_quote = None;

        while let Some(c) = chars.next() {
            if let Some(q) = in_quote {
                if c == q {
                    in_quote = None;
                } else if c == '\\' {
                    if let Some(next) = chars.next() {
                        cur.push(next);
                    }
                } else {
                    cur.push(c);
                }
            } else if c == '"' || c == '\'' {
                in_quote = Some(c);
            } else if c == '#' || (c == '/' && chars.peek() == Some(&'/')) || c == ';' {
                break;
            } else if c.is_whitespace() {
                if !cur.is_empty() {
                    tokens.push(cur);
                    cur = String::new();
                }
            } else if c == '=' || c == ':' {
                // If '=' or ':' is encountered unquoted
                if !cur.is_empty() {
                    tokens.push(cur);
                    cur = String::new();
                }
                tokens.push("=".to_string());
            } else {
                cur.push(c);
            }
        }
        if !cur.is_empty() {
            tokens.push(cur);
        }
    }
    tokens
}

/// Parse configuration written directly in CLI arguments format or key-value format
pub fn parse_cli_or_keyval_config(content: &str) -> Result<ConfigFile, String> {
    let raw_tokens = tokenize_config(content);
    if raw_tokens.is_empty() {
        return Ok(ConfigFile::default());
    }

    let mut cfg = ConfigFile::default();
    let mut positional = Vec::new();
    let mut explicit_sources = Vec::new();
    let mut explicit_paths_from = Vec::new();

    let mut i = 0;
    while i < raw_tokens.len() {
        let tok = &raw_tokens[i];
        if tok == "=" {
            i += 1;
            continue;
        }

        let (key, inline_val) = if let Some(eq_idx) = tok.find('=') {
            let (k, v) = tok.split_at(eq_idx);
            (k, Some(&v[1..]))
        } else {
            (tok.as_str(), None)
        };

        let norm_key = key.trim_start_matches('-');

        let get_val = |i: &mut usize, inline: Option<&str>| -> Result<String, String> {
            if let Some(v) = inline {
                Ok(v.to_string())
            } else if *i + 1 < raw_tokens.len() && raw_tokens[*i + 1] != "=" {
                *i += 1;
                Ok(raw_tokens[*i].clone())
            } else if *i + 2 < raw_tokens.len() && raw_tokens[*i + 1] == "=" {
                *i += 2;
                Ok(raw_tokens[*i].clone())
            } else {
                Err(format!("Missing value for configuration option '{}'", tok))
            }
        };

        match norm_key {
            "dest" | "destination" | "d" => {
                let v = get_val(&mut i, inline_val)?;
                cfg.dest = Some(PathBuf::from(v));
            }
            "sources" | "source" | "paths" | "s" => {
                let v = get_val(&mut i, inline_val)?;
                for p in v.split(',').map(|s| s.trim()).filter(|s| !s.is_empty()) {
                    explicit_sources.push(PathBuf::from(p));
                }
            }
            "paths-from" | "paths_from" | "files-from" | "files_from" | "sources-from"
            | "sources_from" | "f" => {
                let v = get_val(&mut i, inline_val)?;
                for p in v.split(',').map(|s| s.trim()).filter(|s| !s.is_empty()) {
                    explicit_paths_from.push(PathBuf::from(p));
                }
            }
            "flatten" | "flat" => {
                if let Some(v) = inline_val {
                    cfg.flatten = Some(v.parse::<bool>().unwrap_or(true));
                } else if i + 1 < raw_tokens.len()
                    && (raw_tokens[i + 1] == "true" || raw_tokens[i + 1] == "false")
                {
                    i += 1;
                    cfg.flatten = Some(raw_tokens[i] == "true");
                } else if i + 2 < raw_tokens.len()
                    && raw_tokens[i + 1] == "="
                    && (raw_tokens[i + 2] == "true" || raw_tokens[i + 2] == "false")
                {
                    i += 2;
                    cfg.flatten = Some(raw_tokens[i] == "true");
                } else {
                    cfg.flatten = Some(true);
                }
            }
            "preserve-permissions" | "preserve_permissions" | "permissions" | "perms" | "p" => {
                if let Some(v) = inline_val {
                    cfg.preserve_permissions = Some(v.parse::<bool>().unwrap_or(true));
                } else if i + 1 < raw_tokens.len()
                    && (raw_tokens[i + 1] == "true" || raw_tokens[i + 1] == "false")
                {
                    i += 1;
                    cfg.preserve_permissions = Some(raw_tokens[i] == "true");
                } else if i + 2 < raw_tokens.len()
                    && raw_tokens[i + 1] == "="
                    && (raw_tokens[i + 2] == "true" || raw_tokens[i + 2] == "false")
                {
                    i += 2;
                    cfg.preserve_permissions = Some(raw_tokens[i] == "true");
                } else {
                    cfg.preserve_permissions = Some(true);
                }
            }
            "force" | "F" => {
                if let Some(v) = inline_val {
                    cfg.force = Some(v.parse::<bool>().unwrap_or(true));
                } else if i + 1 < raw_tokens.len()
                    && (raw_tokens[i + 1] == "true" || raw_tokens[i + 1] == "false")
                {
                    i += 1;
                    cfg.force = Some(raw_tokens[i] == "true");
                } else if i + 2 < raw_tokens.len()
                    && raw_tokens[i + 1] == "="
                    && (raw_tokens[i + 2] == "true" || raw_tokens[i + 2] == "false")
                {
                    i += 2;
                    cfg.force = Some(raw_tokens[i] == "true");
                } else {
                    cfg.force = Some(true);
                }
            }
            "allow-existing" | "allow_existing" => {
                if let Some(v) = inline_val {
                    cfg.allow_existing = Some(v.parse::<bool>().unwrap_or(true));
                } else if i + 1 < raw_tokens.len()
                    && (raw_tokens[i + 1] == "true" || raw_tokens[i + 1] == "false")
                {
                    i += 1;
                    cfg.allow_existing = Some(raw_tokens[i] == "true");
                } else if i + 2 < raw_tokens.len()
                    && raw_tokens[i + 1] == "="
                    && (raw_tokens[i + 2] == "true" || raw_tokens[i + 2] == "false")
                {
                    i += 2;
                    cfg.allow_existing = Some(raw_tokens[i] == "true");
                } else {
                    cfg.allow_existing = Some(true);
                }
            }
            "allow-empty-source" | "allow_empty_source" => {
                if let Some(v) = inline_val {
                    cfg.allow_empty_source = Some(v.parse::<bool>().unwrap_or(true));
                } else if i + 1 < raw_tokens.len()
                    && (raw_tokens[i + 1] == "true" || raw_tokens[i + 1] == "false")
                {
                    i += 1;
                    cfg.allow_empty_source = Some(raw_tokens[i] == "true");
                } else if i + 2 < raw_tokens.len()
                    && raw_tokens[i + 1] == "="
                    && (raw_tokens[i + 2] == "true" || raw_tokens[i + 2] == "false")
                {
                    i += 2;
                    cfg.allow_empty_source = Some(raw_tokens[i] == "true");
                } else {
                    cfg.allow_empty_source = Some(true);
                }
            }
            "allow-missing-source" | "allow_missing_source" => {
                if let Some(v) = inline_val {
                    cfg.allow_missing_source = Some(v.parse::<bool>().unwrap_or(true));
                } else if i + 1 < raw_tokens.len()
                    && (raw_tokens[i + 1] == "true" || raw_tokens[i + 1] == "false")
                {
                    i += 1;
                    cfg.allow_missing_source = Some(raw_tokens[i] == "true");
                } else if i + 2 < raw_tokens.len()
                    && raw_tokens[i + 1] == "="
                    && (raw_tokens[i + 2] == "true" || raw_tokens[i + 2] == "false")
                {
                    i += 2;
                    cfg.allow_missing_source = Some(raw_tokens[i] == "true");
                } else {
                    cfg.allow_missing_source = Some(true);
                }
            }
            "dry-run" | "dry_run" => {
                if let Some(v) = inline_val {
                    cfg.dry_run = Some(v.parse::<bool>().unwrap_or(true));
                } else if i + 1 < raw_tokens.len()
                    && (raw_tokens[i + 1] == "true" || raw_tokens[i + 1] == "false")
                {
                    i += 1;
                    cfg.dry_run = Some(raw_tokens[i] == "true");
                } else if i + 2 < raw_tokens.len()
                    && raw_tokens[i + 1] == "="
                    && (raw_tokens[i + 2] == "true" || raw_tokens[i + 2] == "false")
                {
                    i += 2;
                    cfg.dry_run = Some(raw_tokens[i] == "true");
                } else {
                    cfg.dry_run = Some(true);
                }
            }
            "verbose" | "v" => {
                if let Some(v) = inline_val {
                    cfg.verbose = Some(v.parse::<bool>().unwrap_or(true));
                } else if i + 1 < raw_tokens.len()
                    && (raw_tokens[i + 1] == "true" || raw_tokens[i + 1] == "false")
                {
                    i += 1;
                    cfg.verbose = Some(raw_tokens[i] == "true");
                } else if i + 2 < raw_tokens.len()
                    && raw_tokens[i + 1] == "="
                    && (raw_tokens[i + 2] == "true" || raw_tokens[i + 2] == "false")
                {
                    i += 2;
                    cfg.verbose = Some(raw_tokens[i] == "true");
                } else {
                    cfg.verbose = Some(true);
                }
            }
            "max-versions" | "max_versions" | "n" => {
                let v = get_val(&mut i, inline_val)?;
                cfg.max_versions = Some(
                    v.parse::<usize>()
                        .map_err(|e| format!("Invalid max-versions '{}': {}", v, e))?,
                );
            }
            "keep-hourly" | "keep_hourly" | "hourly" => {
                let v = get_val(&mut i, inline_val)?;
                cfg.keep_hourly = Some(
                    v.parse::<usize>()
                        .map_err(|e| format!("Invalid keep-hourly '{}': {}", v, e))?,
                );
            }
            "keep-daily" | "keep_daily" | "daily" => {
                let v = get_val(&mut i, inline_val)?;
                cfg.keep_daily = Some(
                    v.parse::<usize>()
                        .map_err(|e| format!("Invalid keep-daily '{}': {}", v, e))?,
                );
            }
            "keep-weekly" | "keep_weekly" | "weekly" => {
                let v = get_val(&mut i, inline_val)?;
                cfg.keep_weekly = Some(
                    v.parse::<usize>()
                        .map_err(|e| format!("Invalid keep-weekly '{}': {}", v, e))?,
                );
            }
            "keep-monthly" | "keep_monthly" | "monthly" => {
                let v = get_val(&mut i, inline_val)?;
                cfg.keep_monthly = Some(
                    v.parse::<usize>()
                        .map_err(|e| format!("Invalid keep-monthly '{}': {}", v, e))?,
                );
            }
            "keep-yearly" | "keep_yearly" | "yearly" => {
                let v = get_val(&mut i, inline_val)?;
                cfg.keep_yearly = Some(
                    v.parse::<usize>()
                        .map_err(|e| format!("Invalid keep-yearly '{}': {}", v, e))?,
                );
            }
            "target-time" | "target_time" | "time" | "t" | "snapshot" => {
                let v = get_val(&mut i, inline_val)?;
                cfg.target_time = Some(v);
            }
            "target-out" | "target_out" | "output" | "output-dir" | "output_dir" => {
                let v = get_val(&mut i, inline_val)?;
                cfg.target_out = Some(PathBuf::from(v));
            }
            "command" | "action" | "subcommand" | "mode" => {
                let v = get_val(&mut i, inline_val)?;
                cfg.command = Some(v);
            }
            "auto" | "create" | "prune" | "restore" | "list" | "ls" | "manual" | "man"
                if !tok.starts_with('-') =>
            {
                cfg.command = Some(tok.to_string());
            }
            _ => {
                if tok.starts_with('-') {
                    return Err(format!("Unknown configuration option '{}'", tok));
                }
                positional.push(PathBuf::from(tok));
            }
        }
        i += 1;
    }

    if !positional.is_empty() {
        if cfg.dest.is_some() {
            explicit_sources.extend(positional);
        } else if positional.len() == 1 {
            cfg.dest = Some(positional.remove(0));
        } else {
            let d = positional.pop().unwrap();
            cfg.dest = Some(d);
            explicit_sources.extend(positional);
        }
    }

    if !explicit_sources.is_empty() {
        cfg.sources = Some(PathsValue::Multiple(explicit_sources));
    }
    if !explicit_paths_from.is_empty() {
        cfg.paths_from = Some(PathsValue::Multiple(explicit_paths_from));
    }

    Ok(cfg)
}

pub fn load_config(path: &Path) -> Result<ConfigFile, String> {
    if !path.exists() {
        return Err(format!("Configuration file not found: {}", path.display()));
    }

    let content = fs::read_to_string(path).map_err(|e| {
        format!(
            "Failed to read configuration file '{}': {}",
            path.display(),
            e
        )
    })?;

    let trimmed = content.trim_start();
    let is_json = path.extension().is_some_and(|ext| ext == "json") || trimmed.starts_with('{');

    if is_json {
        if let Ok(cfg) = serde_json::from_str::<ConfigFile>(&content) {
            return Ok(cfg);
        }
    } else if let Ok(cfg) = toml::from_str::<ConfigFile>(&content) {
        return Ok(cfg);
    }

    // Direct CLI arguments format or unquoted key-value format
    match parse_cli_or_keyval_config(&content) {
        Ok(cfg) => Ok(cfg),
        Err(cli_err) => {
            if is_json {
                serde_json::from_str::<ConfigFile>(&content).map_err(|e| {
                    format!(
                        "Failed to parse JSON configuration file '{}': {}",
                        path.display(),
                        e
                    )
                })
            } else {
                Err(format!(
                    "Failed to parse configuration file '{}': {}\n(Note: Configuration files can be formatted as CLI arguments, TOML, or JSON)",
                    path.display(),
                    cli_err
                ))
            }
        }
    }
}
