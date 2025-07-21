use crate::config_loader::{Config, load_config};
use crate::height_map::{create_generation_settings, generate_surface_heights, get_chunk_proto};
use crate::parallelization::{
    BatchStats, calculate_radius_for_chunks, create_batch_ranges, get_radius_stats,
    process_all_batches, process_chunks_simple,
};
use crate::shutdown_handler::{
    get_current_chunk_index, is_shutdown_requested, setup_shutdown_handler,
    update_current_chunk_index,
};
use crate::storage::BlockStorage;
use pumpkin_util::math::vector2::Vector2;
use pumpkin_world::{ProtoChunk, dimension::Dimension, generation::Seed};
use std::sync::Arc;
use std::time::Instant; // Import your BlockStorage

mod config_loader;
mod height_map;
mod parallelization;
mod shutdown_handler;
mod storage;

#[tokio::main]
pub async fn main() {
    // Set up Ctrl+C handler
    setup_shutdown_handler().await;

    // Run the main logic
    run().await;
}

pub async fn run() {
    let config = load_config();
    let dimension = Dimension::Overworld;

    println!("Loaded configuration: {:?}", config);
    println!("🎯 Press Ctrl+C at any time to gracefully stop and save progress");

    let storage =
        match BlockStorage::new(&config.database_url, config.database_storage_batch_size).await {
            Ok(storage) => {
                println!(
                    "✅ Connected to database with storage batch size: {}",
                    config.database_storage_batch_size
                );
                Arc::new(storage)
            }
            Err(e) => {
                eprintln!("❌ Failed to connect to database: {}", e);
                return;
            }
        };

    // Create the table if it doesn't exist
    if let Err(e) = storage.create_raw_table().await {
        eprintln!("❌ Failed to create table: {}", e);
        return;
    }

    let start_index = config.chunks_start_index;
    let center_x = config.center_x;
    let center_z = config.center_z;

    // Initialize the current chunk index
    update_current_chunk_index(start_index);

    // Determine total chunks based on generation mode
    let total_chunks = if config.use_radius_generation {
        let radius = config
            .radius
            .unwrap_or_else(|| calculate_radius_for_chunks(config.chunks_to_load));

        let (chunk_count, area_km2, diameter) = get_radius_stats(radius);
        println!("Radius {} stats:", radius);
        println!("  - Total chunks: {}", chunk_count);
        println!("  - Area: {:.2} km²", area_km2);
        println!(
            "  - Diameter: {} chunks ({} blocks)",
            diameter,
            diameter * 16
        );

        chunk_count
    } else {
        config.chunks_to_load
    };

    println!(
        "Generating {} chunks in batches of {} around center ({}, {})",
        total_chunks - start_index,
        config.chunk_batch_size,
        center_x,
        center_z
    );

    // Create batch ranges
    let batch_ranges = create_batch_ranges(start_index, total_chunks, config.chunk_batch_size);

    println!("Processing {} batches...", batch_ranges.len());

    let total_start_time = Instant::now();

    // Clone storage for use in the closure
    let storage_for_callback = Arc::clone(&storage);

    // Define your save callback function with storage
    // If you don't want any buffering complexity, go back to Solution 1:
    let chunk_callback = {
        let storage_clone = Arc::clone(&storage_for_callback);
        let runtime = tokio::runtime::Handle::current();

        move |proto: &ProtoChunk, chunk_x: i32, chunk_z: i32| {
            let storage_ref = Arc::clone(&storage_clone);
            let height_map = proto.flat_surface_height_map.to_vec();

            runtime.spawn(async move {
                if let Err(e) = storage_ref.queue_chunk(chunk_x, chunk_z, height_map).await {
                    eprintln!("❌ Failed to queue chunk: {}", e);
                }
            });
        }
    };

    // Define progress callback function
    let progress_callback = |stats: BatchStats| {
        // Update current index for shutdown handler
        update_current_chunk_index(stats.chunks_completed);

        // Progress callback - this is where all the logging happens
        let overall_elapsed = total_start_time.elapsed();
        let overall_chunks_per_sec = stats.chunks_completed as f64 / overall_elapsed.as_secs_f64();

        println!(
            "Batch {}/{} completed - Progress: {:.2}% ({}/{}) | Batch time: {:?} | {:.1} chunks/sec | {:.2}ms/chunk | Overall: {:.1} chunks/sec",
            stats.batch_index,
            stats.total_batches,
            stats.progress_percent,
            stats.chunks_completed,
            stats.total_chunks,
            stats.batch_duration,
            stats.chunks_per_sec,
            stats.ms_per_chunk,
            overall_chunks_per_sec
        );
    };

    // Process all batches with progress callback, shutdown handling, and save callback
    let result = process_all_batches(
        batch_ranges,
        &config,
        dimension,
        total_chunks,
        progress_callback,
        chunk_callback,
    );

    let total_elapsed = total_start_time.elapsed();

    // Flush any remaining chunks before finishing
    match storage.flush_queue().await {
        Ok(flushed_count) => {
            if flushed_count > 0 {
                println!("💾 Flushed {} remaining chunks to database", flushed_count);
            }
        }
        Err(e) => {
            eprintln!("❌ Failed to flush remaining chunks: {}", e);
        }
    }

    match result {
        Ok(stats) => {
            if is_shutdown_requested() {
                let current_index = get_current_chunk_index();
                println!("🛑 Generation stopped by user request");
                println!(
                    "📊 Processed {} chunks before stopping",
                    current_index - start_index
                );
                println!(
                    "💾 To resume, update your config chunks_start_index to: {}",
                    current_index
                );
            } else {
                println!("✅ Generation completed successfully!");
                println!("Total time: {:?}", total_elapsed);
                println!(
                    "Average time per chunk: {:.2}ms",
                    stats.average_ms_per_chunk
                );
                println!(
                    "Overall processing rate: {:.1} chunks/sec",
                    stats.overall_chunks_per_sec
                );
            }
        }
        Err(failed_at_index) => {
            let chunks_completed = failed_at_index - start_index;
            println!("❌ Generation failed at chunk index: {}", failed_at_index);
            println!("Time elapsed before failure: {:?}", total_elapsed);
            if chunks_completed > 0 {
                println!(
                    "Average time per chunk: {:.2}ms",
                    total_elapsed.as_millis() as f64 / chunks_completed as f64
                );
            }
            println!(
                "💾 To resume, update your config chunks_start_index to: {}",
                failed_at_index
            );
        }
    }

    // Close the storage connection gracefully
    // First flush any remaining chunks
    match storage.flush_queue().await {
        Ok(flushed_count) => {
            if flushed_count > 0 {
                println!("💾 Flushed {} remaining chunks to database", flushed_count);
            }
        }
        Err(e) => {
            eprintln!("❌ Failed to flush remaining chunks: {}", e);
        }
    }

    // We can't move out of Arc if there are still references, so we'll just let it drop
    println!("🔌 Database connections will be closed when all references are dropped");
}

// =============================================================================
// TEST FUNCTIONS (Updated to use BlockStorage)
// =============================================================================

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
        move |proto, chunk_x, chunk_z| {},
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
            let height_map_owned = proto.flat_surface_height_map.to_vec(); // Convert to owned Vec

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

    let chunk_coords = crate::parallelization::generate_radius_coords(
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
