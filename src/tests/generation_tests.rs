use crate::lib::config_loader::Config;

use crate::lib::height_map::{
    create_generation_settings, generate_surface_heights, get_chunk_proto,
};
use crate::lib::parallelization::process_chunks_simple;
use crate::lib::zarr_storage::ZarrBlockStorage;
use pumpkin_util::math::vector2::Vector2;
use pumpkin_world::generation::chunk_noise::CHUNK_DIM;
use pumpkin_world::{dimension::Dimension, generation::Seed};
use std::sync::Arc;
use std::time::Instant;

pub fn accuracy_test() {
    let chunk_x = -2;
    let chunk_z = 1;
    let config = Config::default();
    let seed = Seed(config.seed);
    let dimension = Dimension::Overworld;
    let generation_settings = create_generation_settings(seed, &dimension);
    let at = Vector2::new(chunk_x, chunk_z);
    let mut proto = get_chunk_proto(&generation_settings, at);
    let start_time = Instant::now();
    generate_surface_heights(&mut proto);
    let duration = start_time.elapsed();

    let expected_flat_surface_height_map = [
        65, 67, 67, 67, 67, 66, 66, 66, 66, 66, 66, 66, 65, 65, 65, 65, 65, 67, 67, 67, 67, 66, 66,
        66, 66, 66, 66, 66, 66, 66, 65, 65, 66, 67, 67, 67, 67, 66, 66, 66, 66, 66, 66, 66, 66, 66,
        66, 65, 67, 67, 67, 67, 67, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 65, 66, 66, 66, 66, 66,
        66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66,
        66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66,
        66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66,
        66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 65, 65, 65, 65, 65, 66,
        66, 66, 66, 66, 66, 66, 65, 65, 65, 65, 65, 65, 65, 65, 65, 66, 66, 66, 66, 66, 65, 65, 65,
        64, 64, 64, 64, 64, 64, 64, 64, 66, 66, 66, 66, 66, 65, 65, 64, 64, 64, 64, 63, 63, 64, 64,
        64, 66, 66, 65, 65, 65, 65, 64, 64, 63, 63, 63, 63, 63, 63, 63, 63, 66, 65, 65, 65, 64, 64,
        64, 63, 63, 63, 63, 63, 63, 63, 63, 63, 66, 65, 65, 64, 64, 63, 63, 63, 63, 63, 63, 63, 63,
        63, 63, 63,
    ];

    for x in 0..CHUNK_DIM {
        for z in 0..CHUNK_DIM {
            let index = (x * CHUNK_DIM + z) as usize;
            let y = proto.flat_surface_height_map[index] as i32;
            let world_x = chunk_x * CHUNK_DIM as i32 + x as i32;
            let world_z = chunk_z * CHUNK_DIM as i32 + z as i32;
            let world_y = y;
            println!(
                "setblock {} {} {} minecraft:diamond_block",
                world_x, world_y, world_z
            );
        }
    }

    // Check if the generated height map matches the expected one
    if proto.flat_surface_height_map != expected_flat_surface_height_map.into() {
        eprintln!("❌ Height map does not match expected values!");
    } else {
        println!("✅ Height map matches expected values!");
    }

    println!("Surface height generation took: {:?}", duration);
}

pub async fn parallel_test() {
    let chunk_x = -1;
    let chunk_z = -14;
    let times = 10000;
    let config = Config::default();
    let dimension = Dimension::Overworld;
    let start_time = Instant::now();
    println!("Starting parallel test for {} chunks...", times);

    process_chunks_simple(
        vec![(chunk_x, chunk_z); times],
        &config,
        dimension,
        move |_proto, _chunk_x, _chunk_z| {},
    );

    let duration = start_time.elapsed();
    println!("Parallel test completed in: {:?}", duration);
}

pub async fn parallel_test_with_storage() {
    let chunk_x = -1;
    let chunk_z = -14;
    let times = 1000;
    let config = Config::default();
    let dimension = Dimension::Overworld;
    let start_time = Instant::now();
    println!("Starting parallel test for {} chunks...", times);

    // Create zarr storage for testing
    let storage = Arc::new(
        ZarrBlockStorage::new(
            &config.zarr_storage_location,
            config.zarr_chunk_region_size,
            Some(config.get_zarr_compression()),
            None, // No metadata for tests
        )
        .await
        .expect("Failed to create zarr storage"),
    );

    let storage_clone = Arc::clone(&storage);

    process_chunks_simple(
        vec![(chunk_x, chunk_z); times],
        &config,
        dimension,
        move |proto, chunk_x, chunk_z| {
            let storage_ref = Arc::clone(&storage_clone);
            let height_map: Vec<i64> = proto.flat_surface_height_map.iter().map(|&h| h as i64).collect();

            tokio::spawn(async move {
                if let Err(e) = storage_ref
                    .store_chunk(chunk_x, chunk_z, height_map)
                    .await
                {
                    eprintln!("Failed to store chunk: {}", e);
                }
            });
        },
    );

    // Flush remaining chunks
    if let Err(e) = storage.flush_chunks().await {
        eprintln!("Failed to flush chunks: {}", e);
    }

    println!("🔌 Zarr storage flushed and closed");

    let duration = start_time.elapsed();
    println!("Parallel test completed in: {:?}", duration);
}

pub async fn radius_test_with_storage() {
    let config = Config::default();
    let dimension = Dimension::Overworld;

    use crate::lib::parallelization::generate_radius_range_chunk_coords;
    
    let chunk_coords = generate_radius_range_chunk_coords(
        config.chunk_radius_center_x,
        config.chunk_radius_center_z,
        config.chunk_radius_start,
        config.chunk_radius_end,
        config.chunk_radius_circular,
    );

    let shape = if config.chunk_radius_circular { "circular" } else { "square" };
    println!(
        "Testing {} radius range generation with {} chunks (radius {} to {})",
        shape,
        chunk_coords.len(),
        config.chunk_radius_start,
        config.chunk_radius_end
    );
    let start_time = Instant::now();

    // Create zarr storage for testing
    let storage = Arc::new(
        ZarrBlockStorage::new(
            &config.zarr_storage_location,
            config.zarr_chunk_region_size,
            Some(config.get_zarr_compression()),
            None, // No metadata for tests
        )
        .await
        .expect("Failed to create zarr storage"),
    );

    let storage_clone = Arc::clone(&storage);

    process_chunks_simple(
        chunk_coords,
        &config,
        dimension,
        move |proto, chunk_x, chunk_z| {
            let storage_ref = Arc::clone(&storage_clone);
            let height_map: Vec<i64> = proto.flat_surface_height_map.iter().map(|&h| h as i64).collect();

            tokio::spawn(async move {
                if let Err(e) = storage_ref
                    .store_chunk(chunk_x, chunk_z, height_map)
                    .await
                {
                    eprintln!("Failed to store chunk: {}", e);
                }
            });
        },
    );

    // Flush remaining chunks
    if let Err(e) = storage.flush_chunks().await {
        eprintln!("Failed to flush chunks: {}", e);
    }

    println!("🔌 Zarr storage flushed and closed");

    let duration = start_time.elapsed();
    println!("Radius test completed in: {:?}", duration);
}
