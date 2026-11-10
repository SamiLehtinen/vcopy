use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(untagged)]
pub enum PathsValue {
    Single(PathBuf),
    Multiple(Vec<PathBuf>),
}

impl PathsValue {
    pub fn into_vec(self) -> Vec<PathBuf> {
        match self {
            PathsValue::Single(p) => vec![p],
            PathsValue::Multiple(v) => v,
        }
    }
}

#[derive(Debug, Default, Deserialize, Serialize, Clone)]
pub struct ConfigFile {
    #[serde(default, alias = "paths")]
    pub sources: Option<PathsValue>,

    #[serde(default, alias = "destination")]
    pub dest: Option<PathBuf>,

    #[serde(default, alias = "files_from", alias = "sources_from")]
    pub paths_from: Option<PathsValue>,

    #[serde(default)]
    pub max_versions: Option<usize>,

    #[serde(default)]
    pub keep_hourly: Option<usize>,

    #[serde(default)]
    pub keep_daily: Option<usize>,

    #[serde(default)]
    pub keep_weekly: Option<usize>,

    #[serde(default)]
    pub keep_monthly: Option<usize>,

    #[serde(default)]
    pub keep_yearly: Option<usize>,

    #[serde(default)]
    pub dry_run: Option<bool>,

    #[serde(default)]
    pub verbose: Option<bool>,

    #[serde(default)]
    pub flatten: Option<bool>,

    #[serde(default)]
    pub preserve_permissions: Option<bool>,

    #[serde(default)]
    pub target_time: Option<String>,

    #[serde(default, alias = "output", alias = "output_dir")]
    pub target_out: Option<PathBuf>,

    #[serde(default, alias = "action", alias = "subcommand")]
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
        serde_json::from_str::<ConfigFile>(&content).map_err(|e| {
            format!(
                "Failed to parse JSON configuration file '{}': {}",
                path.display(),
                e
            )
        })
    } else {
        match toml::from_str::<ConfigFile>(&content) {
            Ok(cfg) => Ok(cfg),
            Err(toml_err) => {
                if let Ok(cfg) = serde_json::from_str::<ConfigFile>(&content) {
                    Ok(cfg)
                } else {
                    Err(format!(
                        "Failed to parse configuration file '{}': {}",
                        path.display(),
                        toml_err
                    ))
                }
            }
        }
    }
}
