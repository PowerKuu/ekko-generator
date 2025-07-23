use serde::Deserialize;
use std::fs;

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub zarr_storage_location: String
}

impl Default for Config {
    fn default() -> Self {
        Self {
            zarr_storage_location: "./.zarr_data_test".to_string(), // Dont change its for testing
        }
    }
}

pub fn load_config() -> Config {
    let config_str = fs::read_to_string("config.toml").expect("Failed to read config file");
    toml::from_str(&config_str).expect("Failed to parse config")
}
