use anyhow::Result;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Entry for a custom image
#[derive(Debug, Clone, Deserialize)]
pub struct CustomImageEntry {
    /// Path to the qcow2 image file
    pub path: String,
    /// Optional description
    #[serde(default)]
    pub description: Option<String>,
    /// Disk size in GB (optional)
    #[serde(default)]
    pub disk_gb: Option<u32>,
}

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
    /// Map from image name to custom image entry
    custom_images: HashMap<String, CustomImageEntry>,
    /// Global default image name (if set, all targets use this custom image)
    default_image: Option<String>,
    /// Directory to save Orion logs
    log_dir: String,
}

impl Config {
    /// Create a new Config with the given log directory and empty targets/images
    #[cfg(test)]
    pub fn new(log_dir: String) -> Self {
        Self {
            targets: Default::default(),
            custom_images: Default::default(),
            default_image: None,
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

        let custom_images = parsed.custom_images.unwrap_or_default();

        Ok(Config {
            targets,
            custom_images,
            default_image: parsed.default_image,
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

    /// Get the path for a custom image by name
    pub fn get_image_path(&self, image_name: &str) -> Option<String> {
        self.custom_images.get(image_name).map(|e| e.path.clone())
    }

    /// Get the disk size (GB) for a custom image by name
    pub fn get_image_disk(&self, image_name: &str) -> Option<u32> {
        self.custom_images.get(image_name).and_then(|e| e.disk_gb)
    }

    /// Get the global default image name
    pub fn default_image(&self) -> Option<&String> {
        self.default_image.as_ref()
    }
}

/// Internal structure for parsing the JSON config file
#[derive(Debug, Deserialize)]
struct ConfigFile {
    #[serde(default)]
    targets: HashMap<String, TargetConfig>,
    #[serde(default)]
    custom_images: Option<HashMap<String, CustomImageEntry>>,
    #[serde(default)]
    default_image: Option<String>,
    #[serde(default)]
    log_dir: Option<String>,
}

/// Global configuration state
pub type SharedConfig = Arc<RwLock<Config>>;
