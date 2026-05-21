use anyhow::Result;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Target environment configuration
#[derive(Debug, Clone, Deserialize)]
pub struct TargetConfig {
    /// Orion WebSocket server URL
    pub server_ws: String,
    /// Scorpio base URL (replaces base_url in scorpio.toml)
    pub scorpio_base_url: String,
    /// Scorpio LFS URL (replaces lfs_url in scorpio.toml)
    pub scorpio_lfs_url: String,
}

/// Target configuration store loaded from JSON file
#[derive(Debug, Clone)]
pub struct Config {
    /// Map from target name (e.g., "aws-gitmega") to its configuration
    targets: HashMap<String, TargetConfig>,
    /// Directory to save Orion logs
    log_dir: String,
}

impl Config {
    /// Create a new Config with the given log directory and empty targets
    #[cfg(test)]
    pub fn new(log_dir: String) -> Self {
        Self {
            targets: Default::default(),
            log_dir,
        }
    }

    /// Load configuration from a JSON file
    pub async fn load(path: impl AsRef<std::path::Path>) -> Result<Self> {
        let content = tokio::fs::read_to_string(path).await?;
        let parsed: ConfigFile = serde_json::from_str(&content)?;

        let mut targets = HashMap::new();
        for (name, config) in parsed.targets {
            targets.insert(name, config);
        }

        Ok(Config {
            targets,
            log_dir: parsed.log_dir.unwrap_or_else(|| "/var/log/orion-scheduler".to_string()),
        })
    }

    /// Get configuration for a specific target
    pub fn get(&self, target: &str) -> Option<&TargetConfig> {
        self.targets.get(target)
    }

    /// Get all available target names
    pub fn target_names(&self) -> Vec<&String> {
        self.targets.keys().collect()
    }

    /// Get the log directory path
    pub fn log_dir(&self) -> &str {
        &self.log_dir
    }
}

/// Internal structure for parsing the JSON config file
#[derive(Debug, Deserialize)]
struct ConfigFile {
    #[serde(default)]
    targets: HashMap<String, TargetConfig>,
    #[serde(default)]
    log_dir: Option<String>,
}

/// Global configuration state
pub type SharedConfig = Arc<RwLock<Config>>;
