use crate::lib::config_loader::Config;
use crate::lib::height_map::{
    create_generation_settings, generate_surface_heights, get_chunk_proto, GenerationSettings,
};
use pumpkin_util::math::vector2::Vector2;
use pumpkin_world::generation::chunk_noise::CHUNK_DIM;
use pumpkin_world::ProtoChunk;
use pumpkin_world::{dimension::Dimension, generation::Seed};
use rayon::prelude::*;
use std::sync::Arc;
use std::time::Instant;

// Import shutdown functions from shutdown_handler module
use crate::lib::shutdown_handler::is_shutdown_requested;

/// Statistics for batch processing
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct BatchStats {
    pub batch_index: usize,
    pub total_batches: usize,
    pub chunks_in_batch: usize,
    pub batch_duration: std::time::Duration,
    pub chunks_per_sec: f64,
    pub ms_per_chunk: f64,
    pub progress_percent: f64,
    pub chunks_completed: usize,
    pub total_chunks: usize,
}

/// Overall processing statistics
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct ProcessingStats {
    pub total_duration: std::time::Duration,
    pub total_chunks_processed: usize,
    pub overall_chunks_per_sec: f64,
    pub average_ms_per_chunk: f64,
}

/// Generate all chunk coordinates for a range of radii (circular or square)
pub fn generate_radius_range_chunk_coords(
    center_x: i32,
    center_z: i32,
    radius_start: i32,
    radius_end: i32,
    circular: bool,
) -> Vec<(i32, i32)> {
    let mut all_coords = Vec::new();
    
    if circular {
        // Much faster: generate all coords in end radius, exclude those in start radius
        let radius_end_squared = radius_end * radius_end;
        let radius_start_squared = if radius_start > 0 { (radius_start - 1) * (radius_start - 1) } else { -1 };
        
        for x in -radius_end..=radius_end {
            for z in -radius_end..=radius_end {
                let distance_squared = x * x + z * z;
                if distance_squared <= radius_end_squared && distance_squared > radius_start_squared {
                    all_coords.push((center_x + x, center_z + z));
                }
            }
        }
    } else {
        // Square: generate all coords in end square, exclude those in start square
        for x in -radius_end..=radius_end {
            for z in -radius_end..=radius_end {
                let in_start_square = if radius_start > 0 {
                    let start_r = radius_start - 1;
                    x >= -start_r && x <= start_r && z >= -start_r && z <= start_r
                } else {
                    false
                };
                
                if !in_start_square {
                    all_coords.push((center_x + x, center_z + z));
                }
            }
        }
    }
    
    // Sort by distance from center for efficient processing
    all_coords.sort_by(|a, b| {
        let dist_a = (a.0 - center_x) * (a.0 - center_x) + (a.1 - center_z) * (a.1 - center_z);
        let dist_b = (b.0 - center_x) * (b.0 - center_x) + (b.1 - center_z) * (b.1 - center_z);
        dist_a.cmp(&dist_b)
    });
    
    all_coords
}

pub fn length_of_radius_range_chunk_coords(
    radius_start: i32,
    radius_end: i32,
    circular: bool,
) -> usize {
    if circular {
        // Much faster: calculate total in end radius, subtract total in start radius
        let mut total_in_end = 0;
        let radius_squared = radius_end * radius_end;
        for x in -radius_end..=radius_end {
            for z in -radius_end..=radius_end {
                let distance_squared = x * x + z * z;
                if distance_squared <= radius_squared {
                    total_in_end += 1;
                }
            }
        }
        
        let total_in_start = if radius_start > 0 {
            let mut count = 0;
            let start_radius_squared = (radius_start - 1) * (radius_start - 1);
            for x in -(radius_start - 1)..=(radius_start - 1) {
                for z in -(radius_start - 1)..=(radius_start - 1) {
                    let distance_squared = x * x + z * z;
                    if distance_squared <= start_radius_squared {
                        count += 1;
                    }
                }
            }
            count
        } else {
            0
        };
        
        total_in_end - total_in_start
    } else {
        // For square: this IS 100% accurate
        let outer = (2 * radius_end + 1) * (2 * radius_end + 1);
        let inner = if radius_start > 0 {
            let r = radius_start - 1;
            (2 * r + 1) * (2 * r + 1)
        } else { 0 };
        (outer - inner) as usize
    }
}

/// Get radius range statistics without printing
pub fn get_radius_range_stats(
    radius_start: i32,
    radius_end: i32,
    circular: bool,
) -> (usize, f64, i32) {
    let chunk_count = length_of_radius_range_chunk_coords(
        radius_start,
        radius_end,
        circular
    );
    let area_km2 = (chunk_count as f64 * (CHUNK_DIM as usize * CHUNK_DIM as usize) as f64) / 1_000_000.0;
    let diameter = radius_end * 2 + 1;
    (chunk_count, area_km2, diameter)
}

/// Process a single batch of chunks in parallel with shutdown checking and save callback
pub fn process_batch_parallel<F>(
    chunk_coords: &[(i32, i32)],
    generation_settings: Arc<GenerationSettings>,
    chunk_callback: F,
) -> bool
where
    F: Fn(&ProtoChunk, i32, i32) + Send + Sync,
{
    use std::sync::atomic::{AtomicBool, Ordering};
    let early_exit = AtomicBool::new(false);

    chunk_coords.par_iter().for_each(|(chunk_x, chunk_z)| {
        // Check for shutdown at the start of each chunk
        if is_shutdown_requested() || early_exit.load(Ordering::Relaxed) {
            early_exit.store(true, Ordering::Relaxed);
            return;
        }

        let at = Vector2::new(*chunk_x, *chunk_z);
        let mut proto = get_chunk_proto(&generation_settings, at);
        generate_surface_heights(&mut proto);

        // Call the save callback
        chunk_callback(&proto, *chunk_x, *chunk_z);
    });

    // Return true if we completed without shutdown
    !early_exit.load(Ordering::Relaxed) && !is_shutdown_requested()
}

/// Process all batches with parallel execution, error recovery, shutdown handling, and save callback
pub fn process_all_batches_parallel<F, S>(
    batch_ranges: Vec<(usize, usize)>,
    config: &Config,
    dimension: Dimension,
    total_chunks: usize,
    mut batch_callback: F,
    chunk_callback: S,
) -> Result<ProcessingStats, usize>
where
    F: FnMut(BatchStats),
    S: Fn(&ProtoChunk, i32, i32) + Send + Sync + Clone + std::panic::RefUnwindSafe,
{
    let generation_settings = Arc::new(create_generation_settings(Seed(config.seed), &dimension));
    let mut total_chunks_processed = 0;
    let overall_start_time = Instant::now();

    // Generate all coordinates once to avoid regenerating for each batch
    let all_coords = generate_radius_range_chunk_coords(
        config.chunk_radius_center_x,
        config.chunk_radius_center_z,
        config.chunk_radius_start,
        config.chunk_radius_end,
        config.chunk_radius_circular,
    );

    for (batch_index, (start_index, batch_size)) in batch_ranges.iter().enumerate() {
        // Check for shutdown before starting each batch
        if is_shutdown_requested() {
            println!(
                "🛑 Shutdown requested, stopping at batch {}/{}",
                batch_index + 1,
                batch_ranges.len()
            );
            break;
        }

        // Get batch coordinates by slicing the pre-generated list
        let batch_coords = if *start_index < all_coords.len() {
            let end_index = (*start_index + *batch_size).min(all_coords.len());
            &all_coords[*start_index..end_index]
        } else {
            &[]
        };

        // Process batch with panic recovery and shutdown checking
        let batch_start_time = Instant::now();
        let batch_result = std::panic::catch_unwind(|| {
            process_batch_parallel(
                &batch_coords,
                Arc::clone(&generation_settings),
                chunk_callback.clone(),
            )
        });
        let batch_elapsed = batch_start_time.elapsed();

        match batch_result {
            Ok(completed_successfully) => {
                if !completed_successfully {
                    // Batch was interrupted by shutdown
                    println!("🛑 Batch interrupted by shutdown signal");
                    break;
                }
            }
            Err(_) => {
                println!("❌ Batch failed due to panic");
                return Err(*start_index);
            }
        }

        let chunks_processed = batch_coords.len();
        total_chunks_processed += chunks_processed;
        let progress_percent = (total_chunks_processed as f64 / total_chunks as f64) * 100.0;

        // Calculate stats - prevent division by zero
        let chunks_per_sec = if batch_elapsed.as_secs_f64() > 0.0 {
            chunks_processed as f64 / batch_elapsed.as_secs_f64()
        } else {
            0.0
        };
        let time_per_chunk = if chunks_processed > 0 {
            batch_elapsed.as_millis() as f64 / chunks_processed as f64
        } else {
            0.0
        };

        let stats = BatchStats {
            batch_index: batch_index + 1,
            total_batches: batch_ranges.len(),
            chunks_in_batch: chunks_processed,
            batch_duration: batch_elapsed,
            chunks_per_sec,
            ms_per_chunk: time_per_chunk,
            progress_percent,
            chunks_completed: total_chunks_processed,
            total_chunks,
        };

        batch_callback(stats);
    }

    let total_elapsed = overall_start_time.elapsed();
    let duration_secs = total_elapsed.as_secs_f64().max(0.001);
    let overall_chunks_per_sec = total_chunks_processed as f64 / duration_secs;
    let average_ms_per_chunk =
        total_elapsed.as_millis().max(1) as f64 / total_chunks_processed.max(1) as f64;

    Ok(ProcessingStats {
        total_duration: total_elapsed,
        total_chunks_processed,
        overall_chunks_per_sec,
        average_ms_per_chunk,
    })
}

/// Create batch ranges for processing
pub fn create_batch_ranges(
    start_index: usize,
    total_chunks: usize,
    batch_size: usize,
) -> Vec<(usize, usize)> {
    (start_index..total_chunks)
        .step_by(batch_size)
        .map(|batch_start| {
            let batch_end = (batch_start + batch_size).min(total_chunks);
            (batch_start, batch_end - batch_start)
        })
        .collect()
}

/// Simple chunk processing without batching (for small numbers)
pub fn process_chunks_simple<F>(
    chunk_coords: Vec<(i32, i32)>,
    config: &Config,
    dimension: Dimension,
    chunk_callback: F,
) where
    F: Fn(&ProtoChunk, i32, i32) + Send + Sync,
{
    let generation_settings = Arc::new(create_generation_settings(Seed(config.seed), &dimension));
    process_batch_parallel(&chunk_coords, generation_settings, chunk_callback);
}
