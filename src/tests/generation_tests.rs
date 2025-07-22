use crate::lib::config_loader::Config;
use crate::lib::height_map::{create_generation_settings, generate_surface_heights, get_chunk_proto};
use crate::lib::parallelization::{process_chunks_simple, generate_radius_coords};
use crate::lib::storage::BlockStorage;
use pumpkin_util::math::vector2::Vector2;
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

    let expected_flat_surface_height_map = [65, 67, 67, 67, 67, 66, 66, 66, 66, 66, 66, 66, 65, 65, 65, 65, 65, 67, 67, 67, 67, 66, 66, 66, 66, 66, 66, 66, 66, 66, 65, 65, 66, 67, 67, 67, 67, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 65, 67, 67, 67, 67, 67, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 65, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66, 65, 65, 65, 65, 65, 66, 66, 66, 66, 66, 66, 66, 65, 65, 65, 65, 65, 65, 65, 65, 65, 66, 66, 66, 66, 66, 65, 65, 65, 64, 64, 64, 64, 64, 64, 64, 64, 66, 66, 66, 66, 66, 65, 65, 64, 64, 64, 64, 63, 63, 64, 64, 64, 66, 66, 65, 65, 65, 65, 64, 64, 63, 63, 63, 63, 63, 63, 63, 63, 66, 65, 65, 65, 64, 64, 64, 63, 63, 63, 63, 63, 63, 63, 63, 63, 66, 65, 65, 64, 64, 63, 63, 63, 63, 63, 63, 63, 63, 63, 63, 63];

    // Top Blocks 16 x 16 chunk display x, and z and get height. Every 16 is a new z
    for x in 0..16 {
        for z in 0..16 {
            let index = (x * 16 + z) as usize;
            let y = proto.flat_surface_height_map[index] as i32;
            let world_x = chunk_x * 16 + x;
            let world_z = chunk_z * 16 + z;
            let world_y = y;
            println!(
                "setblock {} {} {} minecraft:diamond_block",
                world_x, world_y, world_z
            );
        }
    }

    // Check if the generated height map matches the expected one
    if proto.flat_surface_height_map != Box::new(expected_flat_surface_height_map) {
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

    // Create storage for testing
    let storage = Arc::new(
        BlockStorage::new(&config.database_url, config.database_storage_batch_size)
            .await
            .expect("Failed to create storage"),
    );
    storage
        .create_raw_table()
        .await
        .expect("Failed to create table");

    let storage_clone = Arc::clone(&storage);

    process_chunks_simple(
        vec![(chunk_x, chunk_z); times],
        &config,
        dimension,
        move |proto, chunk_x, chunk_z| {
            let storage_ref = Arc::clone(&storage_clone);
            let height_map_owned = proto.flat_surface_height_map.to_vec();

            tokio::spawn(async move {
                if let Err(e) = storage_ref
                    .queue_chunk(chunk_x, chunk_z, height_map_owned)
                    .await
                {
                    eprintln!("Failed to queue chunk: {}", e);
                }
            });
        },
    );

    // Flush remaining chunks
    if let Err(e) = storage.flush_queue().await {
        eprintln!("Failed to flush queue: {}", e);
    }

    println!("🔌 Database connection will be closed when storage is dropped");

    let duration = start_time.elapsed();
    println!("Parallel test completed in: {:?}", duration);
}


pub async fn radius_test_with_storage() {
    let config = Config::default();
    let dimension = Dimension::Overworld;

    let chunk_coords = generate_radius_coords(
        config.center_x,
        config.center_z,
        config.radius.unwrap_or(25),
    );

    println!(
        "Testing radius generation with {} chunks in radius {}",
        chunk_coords.len(),
        config.radius.unwrap_or(25)
    );
    let start_time = Instant::now();

    // Create storage for testing
    let storage = Arc::new(
        BlockStorage::new(&config.database_url, config.database_storage_batch_size)
            .await
            .expect("Failed to create storage"),
    );
    storage
        .create_raw_table()
        .await
        .expect("Failed to create table");

    let storage_clone = Arc::clone(&storage);

    process_chunks_simple(
        chunk_coords,
        &config,
        dimension,
        move |proto, chunk_x, chunk_z| {
            let storage_ref = Arc::clone(&storage_clone);
            let height_map = proto.flat_surface_height_map.clone();

            tokio::spawn(async move {
                if let Err(e) = storage_ref
                    .queue_chunk(chunk_x, chunk_z, height_map.to_vec())
                    .await
                {
                    eprintln!("Failed to queue chunk: {}", e);
                }
            });
        },
    );

    // Flush remaining chunks
    if let Err(e) = storage.flush_queue().await {
        eprintln!("Failed to flush queue: {}", e);
    }

    println!("🔌 Database connection will be closed when storage is dropped");

    let duration = start_time.elapsed();
    println!("Radius test completed in: {:?}", duration);
}