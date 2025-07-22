use crate::lib::config_loader::{Config, load_config};
use crate::lib::parallelization::{
    BatchStats, calculate_radius_for_chunks, create_batch_ranges, get_radius_stats,
    process_all_batches,
};
use crate::lib::shutdown_handler::{
    get_current_chunk_index, is_shutdown_requested, setup_shutdown_handler,
    update_current_chunk_index,
};
use crate::lib::storage::BlockStorage;
use crate::tests::generation_tests::accuracy_test;
use pumpkin_world::{ProtoChunk, dimension::Dimension};
use std::sync::Arc;
use std::time::Instant;

mod lib {
    pub mod config_loader;
    pub mod height_map;
    pub mod parallelization;
    pub mod shutdown_handler;
    pub mod storage;
}

mod tests {
    pub mod generation_tests;
}

#[tokio::main]
pub async fn main() {
    // Set up Ctrl+C handler
    accuracy_test();
}

pub async fn ekko() {
    let config = load_config();
    let dimension = Dimension::Overworld;

    print_startup_info(&config);
    let storage = initialize_storage(&config).await;

    let start_index = config.chunks_start_index;
    update_current_chunk_index(start_index);
    
    let total_chunks = calculate_total_chunks(&config);
    print_generation_info(&config, start_index, total_chunks);

    let batch_ranges = create_batch_ranges(start_index, total_chunks, config.chunk_batch_size);
    println!("Processing {} batches...", batch_ranges.len());
    let total_start_time = Instant::now();

    let chunk_callback = create_chunk_callback(storage.clone());

    let progress_callback = create_progress_callback(total_start_time);

    let result = process_all_batches(
        batch_ranges,
        &config,
        dimension,
        total_chunks,
        progress_callback,
        chunk_callback,
    );

    let total_elapsed = total_start_time.elapsed();
    flush_storage(&storage).await;

    print_results(result, start_index, total_elapsed);

    close_storage(&storage).await;
}


// Storage utils
async fn initialize_storage(config: &Config) -> Option<Arc<BlockStorage>> {
    if !config.database_enabled {
        println!("📄 Database storage disabled - chunks will not be saved");
        return None;
    }

    let storage_arc = match BlockStorage::new(&config.database_url, config.database_storage_batch_size).await {
        Ok(storage) => {
            println!(
                "✅ Connected to database with storage batch size: {}",
                config.database_storage_batch_size
            );
            Arc::new(storage)
        }
        Err(e) => {
            eprintln!("❌ Failed to connect to database: {}", e);
            std::process::exit(1);
        }
    };

    if let Err(e) = storage_arc.create_raw_table().await {
        eprintln!("❌ Failed to create table: {}", e);
        std::process::exit(1);
    }

    Some(storage_arc)
}

async fn flush_storage(storage: &Option<Arc<BlockStorage>>) {
    if let Some(storage) = storage {
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
    }
}

async fn close_storage(storage: &Option<Arc<BlockStorage>>) {
    if let Some(storage) = storage {
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
        println!("🔌 Database connections will be closed when all references are dropped");
    }
}


// Callbacks
fn create_chunk_callback(storage: Option<Arc<BlockStorage>>) -> impl Fn(&ProtoChunk, i32, i32) + Clone {
    #[derive(Clone)]
    struct ChunkCallback {
        storage: Option<Arc<BlockStorage>>,
        runtime: tokio::runtime::Handle,
    }

    impl ChunkCallback {
        fn call(&self, proto: &ProtoChunk, chunk_x: i32, chunk_z: i32) {
            if let Some(storage_ref) = &self.storage {
                let storage_ref = Arc::clone(storage_ref);
                let height_map = proto.flat_surface_height_map.to_vec();

                self.runtime.spawn(async move {
                    if let Err(e) = storage_ref.queue_chunk(chunk_x, chunk_z, height_map).await {
                        eprintln!("❌ Failed to queue chunk: {}", e);
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

fn create_progress_callback(total_start_time: Instant) -> impl Fn(BatchStats) {
    move |stats: BatchStats| {
        update_current_chunk_index(stats.chunks_completed);

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
    }
}


// Prints
fn print_generation_info(config: &Config, start_index: usize, total_chunks: usize) {
    println!(
        "Generating {} chunks in batches of {} around center ({}, {})",
        total_chunks - start_index,
        config.chunk_batch_size,
        config.center_x,
        config.center_z
    );
}

fn print_results(result: Result<crate::lib::parallelization::ProcessingStats, usize>, start_index: usize, total_elapsed: std::time::Duration) {
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
}

fn print_startup_info(config: &Config) {
    println!("Loaded configuration: {:?}", config);
    println!("🎯 Press Ctrl+C at any time to gracefully stop and save progress");
}


// Misc
fn calculate_total_chunks(config: &Config) -> usize {
    if config.use_radius_generation {
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
    }
}

