//! Generic configuration I/O utilities
//!
//! Provides generic YAML configuration loading and saving that works
//! with any serializable configuration type.

use anyhow::{Context, Result};
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::path::Path;

/// Load configuration from a YAML file
///
/// If the file doesn't exist, returns default config.
/// If the file exists but is invalid, logs a warning and returns default config.
///
/// # Type Parameters
/// * `T` - Configuration type that implements `DeserializeOwned` and `Default`
///
/// # Arguments
/// * `path` - Path to the YAML configuration file
///
/// # Example
///
/// ```ignore
/// let config: PlayerConfig = load_config(&Path::new("config.yaml"));
/// ```
pub fn load_config<T>(path: &Path) -> T
where
    T: DeserializeOwned + Default,
{
    log::info!("load_config: Loading from {:?}", path);

    if !path.exists() {
        log::info!("load_config: Config file doesn't exist, using defaults");
        return T::default();
    }

    match std::fs::read_to_string(path) {
        Ok(contents) => match serde_yaml::from_str::<T>(&contents) {
            Ok(config) => {
                log::info!("load_config: Successfully loaded config from {:?}", path);
                config
            }
            Err(e) => {
                log::warn!("load_config: Failed to parse config: {}, using defaults", e);
                T::default()
            }
        },
        Err(e) => {
            log::warn!(
                "load_config: Failed to read config file: {}, using defaults",
                e
            );
            T::default()
        }
    }
}

/// Save configuration to a YAML file
///
/// Creates parent directories if they don't exist.
///
/// # Type Parameters
/// * `T` - Configuration type that implements `Serialize`
///
/// # Arguments
/// * `config` - Configuration to save
/// * `path` - Path to the YAML configuration file
///
/// # Example
///
/// ```ignore
/// save_config(&config, &Path::new("config.yaml"))?;
/// ```
pub fn save_config<T>(config: &T, path: &Path) -> Result<()>
where
    T: Serialize,
{
    log::info!("save_config: Saving to {:?}", path);

    // Ensure parent directory exists
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create config directory: {:?}", parent))?;
    }

    // Serialize to YAML
    let yaml = serde_yaml::to_string(config).context("Failed to serialize config to YAML")?;

    // Write to file
    std::fs::write(path, yaml)
        .with_context(|| format!("Failed to write config file: {:?}", path))?;

    log::info!("save_config: Config saved successfully");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
    struct TestConfig {
        value: i32,
        name: String,
    }

    #[test]
    fn test_load_nonexistent_returns_default() {
        let config: TestConfig = load_config(Path::new("/nonexistent/path/config.yaml"));
        assert_eq!(config, TestConfig::default());
    }

    #[test]
    fn test_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test-config.yaml");

        let config = TestConfig {
            value: 42,
            name: "test".to_string(),
        };

        save_config(&config, &path).unwrap();
        let loaded: TestConfig = load_config(&path);

        assert_eq!(loaded.value, 42);
        assert_eq!(loaded.name, "test");
    }
}
