use serde::Deserialize;
use std::fs;

#[allow(dead_code)]
#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    // Processing configuration
    pub chunks_start_index: usize,
    pub chunk_batch_size: usize,
    pub chunks_to_load: usize,  // Only used if use_radius_generation is false
    
    // Positioning configuration
    pub center_x: i32,
    pub center_z: i32,
    
    // Generation mode
    pub use_radius_generation: bool,
    pub radius: Option<i32>,  // If None, calculates from chunks_to_load
    
    // World generation
    pub seed: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            chunks_start_index: 0,
            chunk_batch_size: 1000,
            chunks_to_load: 10000,
            center_x: 0,
            center_z: 0,
            use_radius_generation: true,
            radius: Some(25),
            seed: 8221611027149008269,
        }
    }
}

pub fn load_config() -> Config {
    let config_str = fs::read_to_string("config.toml").expect("Failed to read config file");
    toml::from_str(&config_str).expect("Failed to parse config")
}