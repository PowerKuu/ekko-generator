use crate::config_loader::Config;
use crate::height_map::{create_generation_settings, generate_surface_heights, get_chunk_proto};
use pumpkin_util::math::vector2::Vector2;
use pumpkin_world::ProtoChunk;
use pumpkin_world::{dimension::Dimension, generation::Seed};
use rayon::prelude::*;
use std::sync::Arc;
use std::time::Instant;

// Import shutdown functions from main module
use crate::{is_shutdown_requested, update_current_chunk_index};

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

/// Count all chunks in a square area with the given chunk radius (centered at 0,0)
pub fn calculate_chunks_for_chunk_radius(radius: i32) -> usize {
    let side = 2 * radius + 1;
    (side * side) as usize
}

/// Given a block radius, return the number of chunks in a square area
pub fn calculate_chunks_for_block_radius(radius: i32) -> usize {
    let chunk_radius = (radius as f32 / 16.0).ceil() as i32;
    calculate_chunks_for_chunk_radius(chunk_radius)
}

/// Calculate the radius needed for a specific number of chunks
pub fn calculate_radius_for_chunks(target_chunks: usize) -> i32 {
    let mut radius = 0;

    while calculate_chunks_for_chunk_radius(radius) < target_chunks {
        radius += 1;
    }

    radius
}

/// Generate all chunk coordinates within a radius from center point
pub fn generate_radius_coords(center_x: i32, center_z: i32, radius: i32) -> Vec<(i32, i32)> {
    let mut coords = Vec::new();
    let radius_squared = radius * radius;

    for x in -radius..=radius {
        for z in -radius..=radius {
            let distance_squared = x * x + z * z;
            if distance_squared <= radius_squared {
                coords.push((center_x + x, center_z + z));
            }
        }
    }

    // Sort by distance from center for more efficient processing
    coords.sort_by(|a, b| {
        let dist_a = (a.0 - center_x) * (a.0 - center_x) + (a.1 - center_z) * (a.1 - center_z);
        let dist_b = (b.0 - center_x) * (b.0 - center_x) + (b.1 - center_z) * (b.1 - center_z);
        dist_a.cmp(&dist_b)
    });

    coords
}

/// Generate chunk coordinates for a batch (helper for large scale processing)
pub fn generate_batch_coords(
    start_index: usize,
    batch_size: usize,
    total_chunks: usize,
) -> Vec<(i32, i32)> {
    // Calculate grid size based on total chunks (make it square)
    let grid_size = (total_chunks as f64).sqrt().ceil() as usize;
    let half_grid = grid_size as i32 / 2;

    (start_index..start_index + batch_size)
        .map(|i| {
            let x = (i % grid_size) as i32 - half_grid;
            let z = (i / grid_size) as i32 - half_grid;
            (x, z)
        })
        .collect()
}

/// Generate batch coordinates from radius-based generation
pub fn generate_radius_batch_coords(
    start_index: usize,
    batch_size: usize,
    center_x: i32,
    center_z: i32,
    radius: i32,
) -> Vec<(i32, i32)> {
    let all_coords = generate_radius_coords(center_x, center_z, radius);

    let end_index = (start_index + batch_size).min(all_coords.len());

    if start_index < all_coords.len() {
        all_coords[start_index..end_index].to_vec()
    } else {
        Vec::new()
    }
}

/// Get radius statistics without printing
pub fn get_radius_stats(radius: i32) -> (usize, f64, i32) {
    let chunk_count = calculate_chunks_for_chunk_radius(radius);
    let area_km2 = (chunk_count * 16 * 16) as f64 / 1_000_000.0;
    let diameter = radius * 2 + 1;

    (chunk_count, area_km2, diameter)
}

/// Process a single batch of chunks in parallel with shutdown checking and save callback
pub fn process_batch_parallel<F>(
    chunk_coords: &[(i32, i32)],
    generation_settings: Arc<crate::height_map::GenerationSettings>,
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
pub fn process_all_batches<F, S>(
    batch_ranges: Vec<(usize, usize)>,
    config: &Config,
    dimension: Dimension,
    total_chunks: usize,
    mut progress_callback: F,
    chunk_callback: S,
) -> Result<ProcessingStats, usize>
where
    F: FnMut(BatchStats),
    S: Fn(&ProtoChunk, i32, i32) + Send + Sync + Clone + std::panic::RefUnwindSafe,
{
    let generation_settings = Arc::new(create_generation_settings(Seed(config.seed), &dimension));
    let mut total_chunks_processed = 0;
    let overall_start_time = Instant::now();

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

        let batch_coords = if config.use_radius_generation {
            let actual_radius = config
                .radius
                .unwrap_or_else(|| calculate_radius_for_chunks(total_chunks));
            generate_radius_batch_coords(
                *start_index,
                *batch_size,
                config.center_x,
                config.center_z,
                actual_radius,
            )
        } else {
            generate_batch_coords(*start_index, *batch_size, total_chunks)
        };

        let batch_start_time = Instant::now();

        // Process batch with panic recovery and shutdown checking
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

                total_chunks_processed += *batch_size;
                let progress_percent =
                    ((*start_index + *batch_size) as f64 / total_chunks as f64) * 100.0;
                let chunks_per_sec = *batch_size as f64 / batch_elapsed.as_secs_f64();
                let time_per_chunk = batch_elapsed.as_millis() as f64 / *batch_size as f64;

                let stats = BatchStats {
                    batch_index: batch_index + 1,
                    total_batches: batch_ranges.len(),
                    chunks_in_batch: *batch_size,
                    batch_duration: batch_elapsed,
                    chunks_per_sec,
                    ms_per_chunk: time_per_chunk,
                    progress_percent,
                    chunks_completed: *start_index + *batch_size,
                    total_chunks,
                };

                progress_callback(stats);
            }
            Err(_) => {
                println!("❌ Batch failed due to panic");
                return Err(*start_index);
            }
        }
    }

    let total_elapsed = overall_start_time.elapsed();
    let overall_chunks_per_sec = total_chunks_processed as f64 / total_elapsed.as_secs_f64();
    let average_ms_per_chunk = total_elapsed.as_millis() as f64 / total_chunks_processed as f64;

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_radius_calculations() {
        assert_eq!(calculate_chunks_for_chunk_radius(0), 1);
        assert_eq!(calculate_chunks_for_chunk_radius(1), 5);

        for chunks in [1, 5, 13, 25, 49, 100] {
            let radius = calculate_radius_for_chunks(chunks);
            assert!(calculate_chunks_for_chunk_radius(radius) >= chunks);
            if radius > 0 {
                assert!(calculate_chunks_for_chunk_radius(radius - 1) < chunks);
            }
        }
    }

    #[test]
    fn test_batch_ranges() {
        let ranges = create_batch_ranges(0, 1000, 100);
        assert_eq!(ranges.len(), 10);
        assert_eq!(ranges[0], (0, 100));
        assert_eq!(ranges[9], (900, 100));

        let ranges = create_batch_ranges(0, 950, 100);
        assert_eq!(ranges.len(), 10);
        assert_eq!(ranges[9], (900, 50));
    }
}
