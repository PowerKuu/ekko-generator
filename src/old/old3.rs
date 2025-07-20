use std::{time::Instant};
use pumpkin_data::noise_router::{END_BASE_NOISE_ROUTER, NETHER_BASE_NOISE_ROUTER, OVERWORLD_BASE_NOISE_ROUTER};
use pumpkin_util::math::{vector2::Vector2, vector3::Vector3};
use pumpkin_world::{
    dimension::Dimension, generation::{chunk_noise::CHUNK_DIM, positions::chunk_pos::{start_block_x, start_block_z}, settings::gen_settings_from_dimension, Seed}, world::BlockAccessor, ProtoChunk, ProtoNoiseRouters
};
use rayon::prelude::*;

pub fn main() {
    println!("Terrain generation tool started.");

    let chunk_x = -1;
    let chunk_z = -2;
    let times = 10000;
    let seed = Seed(8221611027149008269);
    let dimension = Dimension::Overworld;

    let generation_settings = create_generation_settings(seed, &Dimension::Overworld);
    let now = Instant::now();

    // Run this in parallel using Rayon
    (0..times).into_par_iter().for_each(|_| {
        let at = Vector2::new(chunk_x, chunk_z);
        let mut proto = get_chunk_proto(&generation_settings, at, &dimension);
        v11_coordinate_optimization(&mut proto);
        
    });

    let elapsed = now.elapsed();

    let at = Vector2::new(chunk_x, chunk_z);
    let mut test_proto = get_chunk_proto(&generation_settings, at, &dimension);
    v11_coordinate_optimization(&mut test_proto);

    // Display results
    for x in 0..16 {
        for z in 0..16 {
            let index = (x * 16 + z) as usize;
            let y = test_proto.flat_surface_height_map[index] as i32;
            let world_x = chunk_x * 16 + x;
            let world_z = chunk_z * 16 + z;
            println!("setblock {} {} {} minecraft:diamond_block", world_x, y, world_z);
        }
    }  

    println!(
        "Generated {} chunks in {:?} ({} micro per chunk)",
        times, elapsed, elapsed.as_micros() / times as u128
    );
}

// v11: Small coordinate optimization
pub fn v11_coordinate_optimization(proto: &mut ProtoChunk) {
    let horizontal_cell_block_count = proto.noise_sampler.horizontal_cell_block_count();
    let vertical_cell_block_count = proto.noise_sampler.vertical_cell_block_count();
    let horizontal_cells = CHUNK_DIM / horizontal_cell_block_count;

    let min_y = proto.noise_sampler.min_y();
    let minimum_cell_y = min_y / vertical_cell_block_count as i8;
    let cell_height = proto.noise_sampler.height() / vertical_cell_block_count as u16;

    let terrain_min_y = 63;  
    let terrain_max_y = 100;   
    
    let start_cell = ((terrain_min_y - min_y as i32) / vertical_cell_block_count as i32).max(0) as u16;
    let end_cell = ((terrain_max_y - min_y as i32) / vertical_cell_block_count as i32).min(cell_height as i32 - 1) as u16;

    let start_block_x = start_block_x(&proto.chunk_pos);
    let start_block_z = start_block_z(&proto.chunk_pos);
    let start_cell_x = start_block_x / horizontal_cell_block_count as i32;
    let start_cell_z = start_block_z / horizontal_cell_block_count as i32;
    
    let horizontal_cell_block_count_i32 = horizontal_cell_block_count as i32;
    let vertical_cell_block_count_i32 = vertical_cell_block_count as i32;
    let inv_horizontal_cell_block_count = 1.0 / horizontal_cell_block_count as f64;
    let inv_vertical_cell_block_count = 1.0 / vertical_cell_block_count as f64;
    let minimum_cell_y_i32 = minimum_cell_y as i32;

    let mut surface_found = [[false; 16]; 16];
    let mut columns_completed = 0;
    const TOTAL_COLUMNS: usize = 256;

    let mut estimated_surface_y = 75;

    proto.noise_sampler.sample_start_density();
    for cell_x in 0..horizontal_cells {
        proto.noise_sampler.sample_end_density(cell_x);
        
        let cell_x_i32 = cell_x as i32;
        let cell_x_offset = cell_x_i32 * horizontal_cell_block_count_i32;
        let cell_start_x = start_block_x + cell_x_offset;
        let sample_start_x = (start_cell_x + cell_x_i32) * horizontal_cell_block_count_i32;

        for cell_z in 0..horizontal_cells {
            let cell_z_i32 = cell_z as i32;
            let cell_z_offset = cell_z_i32 * horizontal_cell_block_count_i32;
            let cell_start_z = start_block_z + cell_z_offset;
            let sample_start_z = (start_cell_z + cell_z_i32) * horizontal_cell_block_count_i32;
            
            let adaptive_start = ((estimated_surface_y - 5 - min_y as i32) / vertical_cell_block_count as i32).max(start_cell as i32) as u16;
            let adaptive_end = ((estimated_surface_y + 5 - min_y as i32) / vertical_cell_block_count as i32).min(end_cell as i32) as u16;
            
            for cell_y in (adaptive_start..=adaptive_end).rev() {
                if columns_completed >= TOTAL_COLUMNS { break; }

                proto.noise_sampler.on_sampled_cell_corners(cell_x, cell_y, cell_z);
                
                let cell_y_i32 = cell_y as i32;
                let sample_start_y = (minimum_cell_y_i32 + cell_y_i32) * vertical_cell_block_count_i32;

                for local_y in (0..vertical_cell_block_count).rev() {
                    let local_y_i32 = local_y as i32;
                    let block_y = sample_start_y + local_y_i32;
                    let delta_y = local_y as f64 * inv_vertical_cell_block_count;
                    proto.noise_sampler.interpolate_y(delta_y);

                    // OPTIMIZATION: Pre-calculate chunk coordinates outside inner loops
                    for local_x in 0..horizontal_cell_block_count {
                        let local_x_i32 = local_x as i32;
                        let block_x = cell_start_x + local_x_i32;
                        let chunk_local_x = (local_x_i32 + cell_x_offset) as usize;

                        for local_z in 0..horizontal_cell_block_count {
                            let local_z_i32 = local_z as i32;
                            let block_z = cell_start_z + local_z_i32;
                            let chunk_local_z = (local_z_i32 + cell_z_offset) as usize;

                            if surface_found[chunk_local_x][chunk_local_z] { continue; }

                            let delta_x = local_x as f64 * inv_horizontal_cell_block_count;
                            let delta_z = local_z as f64 * inv_horizontal_cell_block_count;
                            proto.noise_sampler.interpolate_x(delta_x);
                            proto.noise_sampler.interpolate_z(delta_z);

                            let block_state = proto.noise_sampler.sample_block_state(
                                Vector3::new(sample_start_x, sample_start_y, sample_start_z),
                                Vector3::new(local_x_i32, local_y_i32, local_z_i32),
                                &mut proto.surface_height_estimate_sampler,
                            ).unwrap_or(proto.default_block);

                            proto.set_block_state(&Vector3::new(block_x, block_y, block_z), block_state);

                            if !block_state.is_air() {
                                let index = (chunk_local_x * 16 + chunk_local_z) as usize;
                                proto.flat_surface_height_map[index] = block_y as i64;
                                surface_found[chunk_local_x][chunk_local_z] = true;
                                columns_completed += 1;
                                estimated_surface_y = (estimated_surface_y + block_y) / 2;
                            }
                        }
                    }
                }
            }
            if columns_completed >= TOTAL_COLUMNS { break; }
        }
        proto.noise_sampler.swap_buffers();
        if columns_completed >= TOTAL_COLUMNS { break; }
    }
}

// v10: Your current best (836 micro per chunk)
pub fn v10_skip_column_when_done(proto: &mut ProtoChunk) {
    let horizontal_cell_block_count = proto.noise_sampler.horizontal_cell_block_count();
    let vertical_cell_block_count = proto.noise_sampler.vertical_cell_block_count();
    let horizontal_cells = CHUNK_DIM / horizontal_cell_block_count;

    let min_y = proto.noise_sampler.min_y();
    let minimum_cell_y = min_y / vertical_cell_block_count as i8;
    let cell_height = proto.noise_sampler.height() / vertical_cell_block_count as u16;

    let terrain_min_y = 63;  
    let terrain_max_y = 100;   
    
    let start_cell = ((terrain_min_y - min_y as i32) / vertical_cell_block_count as i32).max(0) as u16;
    let end_cell = ((terrain_max_y - min_y as i32) / vertical_cell_block_count as i32).min(cell_height as i32 - 1) as u16;

    let start_block_x = start_block_x(&proto.chunk_pos);
    let start_block_z = start_block_z(&proto.chunk_pos);
    let start_cell_x = start_block_x / horizontal_cell_block_count as i32;
    let start_cell_z = start_block_z / horizontal_cell_block_count as i32;
    
    let horizontal_cell_block_count_i32 = horizontal_cell_block_count as i32;
    let vertical_cell_block_count_i32 = vertical_cell_block_count as i32;
    let inv_horizontal_cell_block_count = 1.0 / horizontal_cell_block_count as f64;
    let inv_vertical_cell_block_count = 1.0 / vertical_cell_block_count as f64;
    let minimum_cell_y_i32 = minimum_cell_y as i32;

    let mut surface_found = [[false; 16]; 16];
    let mut columns_completed = 0;
    const TOTAL_COLUMNS: usize = 256;

    let mut estimated_surface_y = 75;

    proto.noise_sampler.sample_start_density();
    for cell_x in 0..horizontal_cells {
        proto.noise_sampler.sample_end_density(cell_x);
        
        let cell_x_i32 = cell_x as i32;
        let cell_x_offset = cell_x_i32 * horizontal_cell_block_count_i32;
        let cell_start_x = start_block_x + cell_x_offset;
        let sample_start_x = (start_cell_x + cell_x_i32) * horizontal_cell_block_count_i32;

        for cell_z in 0..horizontal_cells {
            let cell_z_i32 = cell_z as i32;
            let cell_z_offset = cell_z_i32 * horizontal_cell_block_count_i32;
            let cell_start_z = start_block_z + cell_z_offset;
            let sample_start_z = (start_cell_z + cell_z_i32) * horizontal_cell_block_count_i32;
            
            let adaptive_start = ((estimated_surface_y - 5 - min_y as i32) / vertical_cell_block_count as i32).max(start_cell as i32) as u16;
            let adaptive_end = ((estimated_surface_y + 5 - min_y as i32) / vertical_cell_block_count as i32).min(end_cell as i32) as u16;
            
            for cell_y in (adaptive_start..=adaptive_end).rev() {
                if columns_completed >= TOTAL_COLUMNS { break; }

                proto.noise_sampler.on_sampled_cell_corners(cell_x, cell_y, cell_z);
                
                let cell_y_i32 = cell_y as i32;
                let sample_start_y = (minimum_cell_y_i32 + cell_y_i32) * vertical_cell_block_count_i32;

                for local_y in (0..vertical_cell_block_count).rev() {
                    let local_y_i32 = local_y as i32;
                    let block_y = sample_start_y + local_y_i32;
                    let delta_y = local_y as f64 * inv_vertical_cell_block_count;
                    proto.noise_sampler.interpolate_y(delta_y);

                    for local_x in 0..horizontal_cell_block_count {
                        let local_x_i32 = local_x as i32;
                        let block_x = cell_start_x + local_x_i32;
                        let chunk_local_x = (block_x - start_block_x) as usize;

                        for local_z in 0..horizontal_cell_block_count {
                            let local_z_i32 = local_z as i32;
                            let block_z = cell_start_z + local_z_i32;
                            let chunk_local_z = (block_z - start_block_z) as usize;

                            if surface_found[chunk_local_x][chunk_local_z] { continue; }

                            let delta_x = local_x as f64 * inv_horizontal_cell_block_count;
                            let delta_z = local_z as f64 * inv_horizontal_cell_block_count;
                            proto.noise_sampler.interpolate_x(delta_x);
                            proto.noise_sampler.interpolate_z(delta_z);

                            let block_state = proto.noise_sampler.sample_block_state(
                                Vector3::new(sample_start_x, sample_start_y, sample_start_z),
                                Vector3::new(local_x_i32, local_y_i32, local_z_i32),
                                &mut proto.surface_height_estimate_sampler,
                            ).unwrap_or(proto.default_block);

                            proto.set_block_state(&Vector3::new(block_x, block_y, block_z), block_state);

                            if !block_state.is_air() {
                                let index = (chunk_local_x * 16 + chunk_local_z) as usize;
                                proto.flat_surface_height_map[index] = block_y as i64;
                                surface_found[chunk_local_x][chunk_local_z] = true;
                                columns_completed += 1;
                                estimated_surface_y = (estimated_surface_y + block_y) / 2;
                            }
                        }
                    }
                }
            }
            if columns_completed >= TOTAL_COLUMNS { break; }
        }
        proto.noise_sampler.swap_buffers();
        if columns_completed >= TOTAL_COLUMNS { break; }
    }
}

// Helper functions
pub struct GenerationSettings<'a> {
    random_config: pumpkin_world::generation::GlobalRandomConfig,
    base_router: ProtoNoiseRouters,
    generation_settings: &'a pumpkin_world::generation::settings::GenerationSettings,
}

pub fn create_generation_settings(seed: Seed, dimension: &Dimension) -> GenerationSettings {
    let random_config = pumpkin_world::generation::GlobalRandomConfig::new(seed.0, false);
    let base_router = match dimension {
        Dimension::Overworld => OVERWORLD_BASE_NOISE_ROUTER,
        Dimension::Nether => NETHER_BASE_NOISE_ROUTER,
        Dimension::End => END_BASE_NOISE_ROUTER,
    };
    let base_router = ProtoNoiseRouters::generate(&base_router, &random_config);
    let generation_settings = gen_settings_from_dimension(&dimension);

    GenerationSettings {
        random_config,
        base_router,
        generation_settings,
    }
}

pub fn get_chunk_proto<'a>(
    generation_settings: &'a GenerationSettings<'a>,
    at: Vector2<i32>,
    dimension: &'a Dimension,
) -> ProtoChunk<'a> {
    ProtoChunk::new(
        at,
        &generation_settings.base_router,
        &generation_settings.random_config,
        generation_settings.generation_settings,
    )
}

fn start_block_x_on_proto(proto: &ProtoChunk) -> i32 {
    start_block_x(&proto.chunk_pos)
}

fn start_block_z_on_proto(proto: &ProtoChunk) -> i32 {
    start_block_z(&proto.chunk_pos)
}

fn start_cell_x_on_proto(proto: &ProtoChunk) -> i32 {
    start_block_x_on_proto(proto) / proto.noise_sampler.horizontal_cell_block_count() as i32
}

fn start_cell_z_on_proto(proto: &ProtoChunk) -> i32 {
    start_block_z_on_proto(proto) / proto.noise_sampler.horizontal_cell_block_count() as i32
}