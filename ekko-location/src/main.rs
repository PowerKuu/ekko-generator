use crate::lib::config_loader::load_config;

mod lib {
    pub mod config_loader;
}

#[tokio::main]
pub async fn main() {
    let config = load_config();

    println!("{}", config.zarr_storage_location)
}