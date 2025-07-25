use crate::lib::config_loader::{load_config, Config};
use crate::lib::parallelization::{
    create_batch_ranges, get_radius_range_stats, process_all_batches_parallel, BatchStats,
};
use crate::lib::shutdown_handler::{
    get_current_chunk_index, is_shutdown_requested, setup_shutdown_handler,
    update_current_chunk_index,
};
use crate::lib::zarr_storage::ZarrBlockStorage;
use crate::tests::generation_tests;
use pumpkin_world::generation::chunk_noise::CHUNK_DIM;
use pumpkin_world::{dimension::Dimension, ProtoChunk};
use std::env;
use std::sync::Arc;
use std::time::Instant;

mod lib {
    pub mod config_loader;
    pub mod height_map;
    pub mod parallelization;
    pub mod shutdown_handler;
    pub mod zarr_storage;
}

mod tests {
    pub mod generation_tests;
}

#[tokio::main]
pub async fn main() {
    setup_shutdown_handler().await;
    handle_command_line_args().await;
}

async fn handle_command_line_args() {
    let args: Vec<String> = env::args().collect();

    match args.get(1).map(|s| s.as_str()) {
        Some("accuracy_test") => {
            println!("🧪 Running Accuracy Test");
            generation_tests::accuracy_test();
        }
        Some("parallel_test") => {
            println!("🧪 Running Parallel Test");
            generation_tests::parallel_test().await;
        }
        Some("storage_test") => {
            println!("🧪 Running Storage Test");
            generation_tests::parallel_test_with_storage().await;
        }
        Some("radius_test") => {
            println!("🧪 Running Radius Test");
            generation_tests::radius_test_with_storage().await;
        }
        Some("test") => {
            println!("🧪 Running All Tests");
            println!();

            println!("--- Accuracy Test ---");
            generation_tests::accuracy_test();
            println!();

            println!("--- Parallel Test ---");
            generation_tests::parallel_test().await;
            println!();

            println!("--- Storage Test ---");
            generation_tests::parallel_test_with_storage().await;
            println!();

            println!("--- Radius Test ---");
            generation_tests::radius_test_with_storage().await;
        }
        Some("help") | Some("--help") | Some("-h") => {
            print_help();
        }
        Some(unknown) => {
            println!("❌ Unknown command: {}", unknown);
            println!();
            print_help();
        }
        None => {
            // Default: run main generation
            ekko().await;
        }
    }
}

pub async fn ekko() {
    let config = load_config();
    let dimension = Dimension::Overworld;

    print_startup_info(&config);

    let storage = initialize_storage(&config).await;

    let start_index = config.chunk_radius_start_index;
    update_current_chunk_index(start_index);

    print_radius_stats(&config);

    let total_chunks = calculate_total_chunks(&config);
    print_generation_info(&config, start_index, total_chunks);

    let batch_ranges = create_batch_ranges(start_index, total_chunks, config.chunk_batch_size);

    let total_start_time = Instant::now();

    let chunk_callback = create_chunk_callback(storage.clone());

    let batch_callback = create_batch_callback(storage.clone());
    println!(
        "⚡ Building... and Processing {} Batches:",
        batch_ranges.len()
    );

    let result = process_all_batches_parallel(
        batch_ranges,
        &config,
        dimension,
        total_chunks,
        batch_callback,
        chunk_callback,
    );

    let total_elapsed = total_start_time.elapsed();

    print_results(result, start_index, total_elapsed);

    if let Some(storage_ref) = &storage {
        flush_storage(storage_ref).await;
        println!("💾 Final flush completed");
    }
}

// Storage utils
async fn initialize_storage(config: &Config) -> Option<Arc<ZarrBlockStorage>> {
    if !config.zarr_enabled {
        return None;
    }

    let generation_metadata = Some(crate::lib::zarr_storage::GenerationMetadata {
        seed: config.seed,
        center_x: config.chunk_radius_center_x,
        center_z: config.chunk_radius_center_z,
        radius_start: config.chunk_radius_start,
        radius_end: config.chunk_radius_end,
        circular: config.chunk_radius_circular,
    });

    let storage_arc = match ZarrBlockStorage::new(
        &config.zarr_storage_location,
        config.zarr_chunk_region_size,
        Some(config.get_zarr_compression()),
        generation_metadata,
    )
    .await
    {
        Ok(storage) => {
            println!("💾 Zarr Storage Initialized");
            println!("   • Location: {}", config.zarr_storage_location);
            println!(
                "   • Zarr region size: {} chunks",
                config.zarr_chunk_region_size
            );
            println!("   • Compression: {}", config.zarr_compression);
            println!();
            Arc::new(storage)
        }
        Err(e) => {
            println!();
            println!("❌ Zarr Storage Initialization Failed");
            println!("   • Error: {}", e);
            println!();
            std::process::exit(1);
        }
    };

    Some(storage_arc)
}

// Callbacks
fn create_chunk_callback(
    storage: Option<Arc<ZarrBlockStorage>>,
) -> impl Fn(&ProtoChunk, i32, i32) + Clone {
    #[derive(Clone)]
    struct ChunkCallback {
        storage: Option<Arc<ZarrBlockStorage>>,
        runtime: tokio::runtime::Handle,
    }

    impl ChunkCallback {
        fn call(&self, proto: &ProtoChunk, chunk_x: i32, chunk_z: i32) {
            if let Some(storage_ref) = &self.storage {
                let storage_ref = Arc::clone(storage_ref);
                let height_map: Vec<i64> = proto
                    .flat_surface_height_map
                    .iter()
                    .map(|&h| h as i64)
                    .collect();

                self.runtime.spawn(async move {
                    if let Err(e) = storage_ref.store_chunk(chunk_x, chunk_z, height_map).await {
                        eprintln!("❌ Failed to store chunk: {}", e);
                    }
                });
            }
        }
    }

    let callback = ChunkCallback {
        storage,
        runtime: tokio::runtime::Handle::current(),
    };

    move |proto: &ProtoChunk, chunk_x: i32, chunk_z: i32| {
        callback.call(proto, chunk_x, chunk_z);
    }
}

fn create_batch_callback(storage: Option<Arc<ZarrBlockStorage>>) -> impl Fn(BatchStats) {
    move |stats: BatchStats| {
        update_current_chunk_index(stats.chunks_completed);

        // Flush storage at end of each batch
        if let Some(storage_ref) = &storage {
            let storage_ref = Arc::clone(storage_ref);
            tokio::runtime::Handle::current().spawn(async move {
                flush_storage(&storage_ref).await;
            });
        }

        println!(
            "   Batch {}/{} • {:.1}% complete • {} chunks • {:.2}s • {:.0} chunks/sec",
            stats.batch_index,
            stats.total_batches,
            stats.progress_percent,
            stats.chunks_in_batch,
            stats.batch_duration.as_secs_f64(),
            stats.chunks_per_sec
        );
    }
}

// Prints
fn print_generation_info(_config: &Config, start_index: usize, total_chunks: usize) {
    println!("🚀 Starting Generation");
    println!(
        "   • Processing: {} chunks ({} blocks)",
        total_chunks - start_index,
        (total_chunks - start_index) * (CHUNK_DIM as usize * CHUNK_DIM as usize)
    );
    println!();
}

fn print_results(
    result: Result<crate::lib::parallelization::ProcessingStats, usize>,
    start_index: usize,
    total_elapsed: std::time::Duration,
) {
    match result {
        Ok(stats) => {
            if is_shutdown_requested() {
                let current_index = get_current_chunk_index();
                println!();
                println!("🛑 Generation Stopped");
                println!("   • Processed: {} chunks", current_index - start_index);
                println!(
                    "   • To resume: Set radius_start_chunk_index to {}",
                    current_index
                );
            } else {
                println!();
                println!("✅ Generation Complete!");
                println!("   • Total time: {:.2}s", total_elapsed.as_secs_f64());
                println!(
                    "   • Average per chunk: {:.2}ms",
                    stats.average_ms_per_chunk
                );
                println!(
                    "   • Processing rate: {:.1} chunks/sec",
                    stats.overall_chunks_per_sec
                );
            }
            println!();
        }
        Err(failed_at_index) => {
            let chunks_completed = failed_at_index - start_index;
            println!();
            println!("❌ Generation Failed");
            println!("   • Failed at index: {}", failed_at_index);
            println!("   • Completed: {} chunks", chunks_completed);
            println!("   • Time elapsed: {:.2}s", total_elapsed.as_secs_f64());
            if chunks_completed > 0 {
                println!(
                    "   • Average per chunk: {:.2}ms",
                    total_elapsed.as_millis() as f64 / chunks_completed as f64
                );
            }
            println!(
                "   • To resume: Set radius_start_chunk_index to {}",
                failed_at_index
            );
            println!();
        }
    }
}

fn print_help() {
    println!("🎯 Ekko Generator - Minecraft Chunk Generation Tool");
    println!();
    println!("Usage:");
    println!("  cargo run                    Run chunk generation (default)");
    println!("  cargo run accuracy_test     Test height map accuracy");
    println!("  cargo run parallel_test     Test parallel processing");
    println!("  cargo run storage_test      Test zarr storage");
    println!("  cargo run radius_test       Test radius generation");
    println!("  cargo run test              Run all tests");
    println!("  cargo run help              Show this help message");
    println!();
}

fn print_startup_info(config: &Config) {
    println!("🎯 Ekko Generator - Minecraft Chunk Generation Tool");
    println!();
    println!("🛠️  Configuration:");
    let shape = if config.chunk_radius_circular {
        "circular"
    } else {
        "square"
    };
    println!(
        "   • Mode: {} radius {} to {}",
        shape, config.chunk_radius_start, config.chunk_radius_end
    );
    println!(
        "   • Center: ({}, {})",
        config.chunk_radius_center_x, config.chunk_radius_center_z
    );
    println!("   • Start index: {}", config.chunk_radius_start_index);
    println!("   • Batch size: {} chunks", config.chunk_batch_size);
    println!();
    println!("💡 Press Ctrl+C at any time to gracefully stop");
    println!();
}

fn print_radius_stats(config: &Config) {
    let (total_chunks, area_km2, diameter) = get_radius_range_stats(
        config.chunk_radius_start,
        config.chunk_radius_end,
        config.chunk_radius_circular,
    );

    let shape = if config.chunk_radius_circular {
        "Circular"
    } else {
        "Square"
    };
    println!(
        "📊 {} Radius Range {} to {} Generation Stats:",
        shape, config.chunk_radius_start, config.chunk_radius_end
    );
    println!("   • Total chunks: {}", total_chunks);
    println!("   • Area coverage: {:.2} km²", area_km2);
    println!(
        "   • Final diameter: {} chunks ({} blocks)",
        diameter,
        diameter * CHUNK_DIM as i32
    );
    println!();
}

// Misc
fn calculate_total_chunks(config: &Config) -> usize {
    let (total_chunks, _, _) = get_radius_range_stats(
        config.chunk_radius_start,
        config.chunk_radius_end,
        config.chunk_radius_circular,
    );

    total_chunks
}

async fn flush_storage(storage: &Arc<ZarrBlockStorage>) {
    if let Err(e) = storage.flush_chunks().await {
        eprintln!("❌ Failed to flush chunks: {}", e);
    }
}
