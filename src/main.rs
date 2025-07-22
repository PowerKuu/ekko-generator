use crate::lib::config_loader::{load_config, Config};
use crate::lib::parallelization::{
    calculate_radius_for_chunks, create_batch_ranges, get_radius_stats, process_all_batches,
    BatchStats,
};
use crate::lib::shutdown_handler::{
    get_current_chunk_index, is_shutdown_requested, setup_shutdown_handler,
    update_current_chunk_index,
};
use crate::lib::storage::BlockStorage;
use crate::tests::generation_tests;
use pumpkin_world::{dimension::Dimension, ProtoChunk};
use std::env;
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

    let start_index = config.chunk_start_index;
    update_current_chunk_index(start_index);

    let total_chunks = calculate_total_chunks(&config);
    print_generation_info(&config, start_index, total_chunks);

    let batch_ranges = create_batch_ranges(start_index, total_chunks, config.chunk_batch_size);

    println!("⚡ Processing {} batches:", batch_ranges.len());
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
        return None;
    }

    let storage_arc =
        match BlockStorage::new(&config.database_url, config.database_storage_batch_size).await {
            Ok(storage) => {
                println!("💾 Database Connected");
                println!(
                    "   • Batch size: {} chunks",
                    config.database_storage_batch_size
                );
                println!();
                Arc::new(storage)
            }
            Err(e) => {
                println!();
                println!("❌ Database Connection Failed");
                println!("   • Error: {}", e);
                println!();
                std::process::exit(1);
            }
        };

    if let Err(e) = storage_arc.create_raw_table().await {
        println!("❌ Database Table Creation Failed");
        println!("   • Error: {}", e);
        println!();
        std::process::exit(1);
    }

    Some(storage_arc)
}

async fn flush_storage(storage: &Option<Arc<BlockStorage>>) {
    if let Some(storage) = storage {
        match storage.flush_queue().await {
            Ok(flushed_count) => {
                if flushed_count > 0 {
                    println!("💾 Saved {} chunks to database", flushed_count);
                }
            }
            Err(e) => {
                println!("❌ Failed to save chunks: {}", e);
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
fn create_chunk_callback(
    storage: Option<Arc<BlockStorage>>,
) -> impl Fn(&ProtoChunk, i32, i32) + Clone {
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
fn print_generation_info(config: &Config, start_index: usize, total_chunks: usize) {
    println!("🚀 Starting Generation");
    println!("   • Processing: {} chunks", total_chunks - start_index);
    println!("   • Batch size: {} chunks", config.chunk_batch_size);
    println!(
        "   • Center point: ({}, {})",
        config.center_x, config.center_z
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
                    "   • To resume: Set chunks_start_index to {}",
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
                "   • To resume: Set chunks_start_index to {}",
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
    println!("  cargo run storage_test      Test database storage");
    println!("  cargo run radius_test       Test radius generation");
    println!("  cargo run test              Run all tests");
    println!("  cargo run help              Show this help message");
    println!();
}

fn print_startup_info(config: &Config) {
    println!("🎯 Ekko Generator");
    println!();
    println!("Configuration:");
    println!("  • Batch size: {} chunks", config.chunk_batch_size);
    println!("  • Start index: {}", config.chunk_start_index);
    println!("  • Center: ({}, {})", config.center_x, config.center_z);
    if config.use_radius_generation {
        println!(
            "  • Mode: Radius generation ({})",
            config.radius.map_or("auto".to_string(), |r| r.to_string())
        );
    } else {
        println!(
            "  • Mode: Grid generation ({} chunks)",
            config.chunks_to_load
        );
    }
    println!(
        "  • Database: {}",
        if config.database_enabled {
            "enabled"
        } else {
            "disabled"
        }
    );
    println!();
    println!("💡 Press Ctrl+C at any time to gracefully stop and save progress");
    println!();
}

// Misc
fn calculate_total_chunks(config: &Config) -> usize {
    if config.use_radius_generation {
        let radius = config
            .radius
            .unwrap_or_else(|| calculate_radius_for_chunks(config.chunks_to_load));

        let (chunk_count, area_km2, diameter) = get_radius_stats(radius);
        println!("📊 Radius {} Generation Stats:", radius);
        println!("   • Total chunks: {}", chunk_count);
        println!("   • Area coverage: {:.2} km²", area_km2);
        println!(
            "   • Diameter: {} chunks ({} blocks)",
            diameter,
            diameter * 16
        );
        println!();

        chunk_count
    } else {
        config.chunks_to_load
    }
}
