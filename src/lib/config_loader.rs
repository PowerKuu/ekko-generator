use crate::lib::zarr_storage::CompressionType;
use serde::Deserialize;
use std::fs;

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    // World generation
    pub seed: u64,

    // Processing configuration
    pub chunk_radius_start_index: usize,
    pub chunk_radius_center_x: i32,
    pub chunk_radius_center_z: i32,
    pub chunk_radius_start: i32,
    pub chunk_radius_end: i32,
    pub chunk_radius_circular: bool,

    // Zarr storage configuration
    pub zarr_enabled: bool,
    pub zarr_storage_location: String,
    pub zarr_chunk_region_size: usize,
    pub chunk_batch_size: usize,
    pub zarr_compression: String,
    pub zarr_compression_level: i32,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            seed: 8221611027149008269,
            chunk_radius_start_index: 0,
            chunk_radius_center_x: 0,
            chunk_radius_center_z: 0,
            chunk_radius_start: 0,
            chunk_radius_end: 20,
            chunk_radius_circular: true,
            chunk_batch_size: 1000,

            zarr_enabled: true,
            zarr_storage_location: "./.zarr_data_test".to_string(), // Dont change its for testing
            zarr_chunk_region_size: 64,
            zarr_compression: "zstd".to_string(),
            zarr_compression_level: 3,
        }
    }
}

impl Config {
    /// Convert config compression string to zarr CompressionType
    pub fn get_zarr_compression(&self) -> CompressionType {
        match self.zarr_compression.to_lowercase().as_str() {
            "none" => CompressionType::None,
            "gzip" => CompressionType::Gzip {
                level: self.zarr_compression_level as u32,
            },
            "zstd" => CompressionType::Zstd {
                level: self.zarr_compression_level,
            },
            _ => {
                eprintln!(
                    "Unknown compression type '{}', defaulting to zstd",
                    self.zarr_compression
                );
                CompressionType::Zstd {
                    level: self.zarr_compression_level,
                }
            }
        }
    }
}

pub fn load_config() -> Config {
    let config_str = fs::read_to_string("config.toml").expect("Failed to read config file");
    toml::from_str(&config_str).expect("Failed to parse config")
}
