use pumpkin_data::noise_router::{
    END_BASE_NOISE_ROUTER, NETHER_BASE_NOISE_ROUTER, OVERWORLD_BASE_NOISE_ROUTER,
};
use pumpkin_util::math::{vector2::Vector2, vector3::Vector3};
use pumpkin_world::{
    dimension::Dimension,
    generation::{
        chunk_noise::CHUNK_DIM, positions::chunk_pos::{start_block_x, start_block_z}, settings::gen_settings_from_dimension, Seed
    },
    ProtoChunk, ProtoNoiseRouters,
};

// Configuration constants - change these as needed
const TERRAIN_MIN_Y: i32 = 63; // Water level in Minecraft
const TERRAIN_MAX_Y: i32 = 200; // Reasonable mountain height
const ADAPTIVE_SEARCH_RANGE: i32 = 5; // +/- blocks around estimated surface
const INITIAL_SURFACE_ESTIMATE: i32 = 75; // Common surface height
const INITIAL_WIDE_SEARCH_RANGE: i32 = 40; // Wider range for first few samples
const SAMPLES_FOR_CONVERGENCE: usize = 16; // How many samples before trusting estimate

pub struct SurfaceHeightGenerator {
    terrain_min_y: i32,
    terrain_max_y: i32,
    adaptive_search_range: i32,
    initial_surface_estimate: i32,
    initial_wide_search_range: i32,
    samples_for_convergence: usize,
}

impl SurfaceHeightGenerator {
    pub fn new() -> Self {
        Self {
            terrain_min_y: TERRAIN_MIN_Y,
            terrain_max_y: TERRAIN_MAX_Y,
            adaptive_search_range: ADAPTIVE_SEARCH_RANGE,
            initial_surface_estimate: INITIAL_SURFACE_ESTIMATE,
            initial_wide_search_range: INITIAL_WIDE_SEARCH_RANGE,
            samples_for_convergence: SAMPLES_FOR_CONVERGENCE,
        }
    }

    /// Main entry point: Generate surface heights for the chunk
    pub fn generate(&self, proto: &mut ProtoChunk) {
        let params = self.calculate_generation_params(proto);
        let mut state = GenerationState::new(self.initial_surface_estimate);

        proto.noise_sampler.sample_start_density();

        // Process each column of cells along the X axis
        for cell_x in 0..params.horizontal_cells {
            proto.noise_sampler.sample_end_density(cell_x as u8);

            if self.process_x_cells(proto, &params, &mut state, cell_x) {
                break; // All surfaces found
            }

            proto.noise_sampler.swap_buffers();
        }
    }

    /// Extract all generation parameters from the chunk
    fn calculate_generation_params(&self, proto: &ProtoChunk) -> GenerationParams {
        let horizontal_cell_block_count = proto.noise_sampler.horizontal_cell_block_count();
        let vertical_cell_block_count = proto.noise_sampler.vertical_cell_block_count();
        let horizontal_cells = CHUNK_DIM / horizontal_cell_block_count;

        let min_y = proto.noise_sampler.min_y();
        let minimum_cell_y = min_y / vertical_cell_block_count as i8;
        let cell_height = proto.noise_sampler.height() / vertical_cell_block_count as u16;

        let start_cell =
            ((self.terrain_min_y - min_y as i32) / vertical_cell_block_count as i32).max(0) as u16;
        let end_cell = ((self.terrain_max_y - min_y as i32) / vertical_cell_block_count as i32)
            .min(cell_height as i32 - 1) as u16;

        let start_block_x = start_block_x(&proto.chunk_pos);
        let start_block_z = start_block_z(&proto.chunk_pos);
        let start_cell_x = start_block_x / horizontal_cell_block_count as i32;
        let start_cell_z = start_block_z / horizontal_cell_block_count as i32;

        GenerationParams {
            horizontal_cell_block_count,
            vertical_cell_block_count,
            horizontal_cells: horizontal_cells.into(),
            min_y,
            minimum_cell_y,
            start_cell,
            end_cell,
            start_block_x,
            start_block_z,
            start_cell_x,
            start_cell_z,
        }
    }

    fn calculate_cell_bounds(
        &self,
        params: &GenerationParams,
        state: &GenerationState,
        cell_x: u16,
        cell_z: u16,
    ) -> CellBounds {
        let search_range = if state.surface_samples.len() < self.samples_for_convergence {
            self.initial_wide_search_range
        } else {
            self.adaptive_search_range
        };

        let adaptive_start = ((state.estimated_surface_y - search_range - params.min_y as i32)
            / params.vertical_cell_block_count as i32)
            .max(params.start_cell as i32) as u16;
        let adaptive_end = ((state.estimated_surface_y + search_range - params.min_y as i32)
            / params.vertical_cell_block_count as i32)
            .min(params.end_cell as i32) as u16;

        let cell_x_offset = cell_x as i32 * params.horizontal_cell_block_count as i32;
        let cell_z_offset = cell_z as i32 * params.horizontal_cell_block_count as i32;
        let cell_start_x = params.start_block_x + cell_x_offset;
        let cell_start_z = params.start_block_z + cell_z_offset;
        let sample_start_x =
            (params.start_cell_x + cell_x as i32) * params.horizontal_cell_block_count as i32;
        let sample_start_z =
            (params.start_cell_z + cell_z as i32) * params.horizontal_cell_block_count as i32;

        CellBounds {
            adaptive_start,
            adaptive_end,
            cell_start_x,
            cell_start_z,
            sample_start_x,
            sample_start_z,
        }
    }

    /// Process all cells along the Z axis for a given X position
    fn process_x_cells(
        &self,
        proto: &mut ProtoChunk,
        params: &GenerationParams,
        state: &mut GenerationState,
        cell_x: u16,
    ) -> bool {
        for cell_z in 0..params.horizontal_cells {
            if self.process_xz_cell(proto, params, state, cell_x, cell_z) {
                return true; // All columns completed
            }
        }
        false
    }

    /// Process a single cell at (X,Z) position, working through Y layers
    fn process_xz_cell(
        &self,
        proto: &mut ProtoChunk,
        params: &GenerationParams,
        state: &mut GenerationState,
        cell_x: u16,
        cell_z: u16,
    ) -> bool {
        let cell_bounds = self.calculate_cell_bounds(params, state, cell_x, cell_z);

        // Search from top to bottom (reverse Y order)
        for cell_y in (cell_bounds.adaptive_start..=cell_bounds.adaptive_end).rev() {
            if state.columns_completed >= (CHUNK_DIM as usize * CHUNK_DIM as usize) {
                return true;
            }

            proto
                .noise_sampler
                .on_sampled_cell_corners(cell_x as u8, cell_y, cell_z as u8);

            if self.process_y_layers_in_cell(proto, params, state, &cell_bounds, cell_y) {
                return true;
            }
        }
        false
    }

    /// Process all block layers within a cell at a specific Y level
    fn process_y_layers_in_cell(
        &self,
        proto: &mut ProtoChunk,
        params: &GenerationParams,
        state: &mut GenerationState,
        cell_bounds: &CellBounds,
        cell_y: u16,
    ) -> bool {
        let sample_start_y = (params.minimum_cell_y as i32 + cell_y as i32)
            * params.vertical_cell_block_count as i32;
        let inv_horizontal = 1.0 / params.horizontal_cell_block_count as f64;
        let inv_vertical = 1.0 / params.vertical_cell_block_count as f64;

        // Process each block layer in this cell (top to bottom)
        for local_y in (0..params.vertical_cell_block_count).rev() {
            let block_y = sample_start_y + local_y as i32;
            let delta_y = local_y as f64 * inv_vertical;
            proto.noise_sampler.interpolate_y(delta_y);

            if self.process_blocks_at_height(
                proto,
                params,
                state,
                cell_bounds,
                block_y,
                local_y as i32,
                sample_start_y,
                inv_horizontal,
            ) {
                return true;
            }
        }
        false
    }

    /// Process all individual blocks at a specific height (Y level)
    fn process_blocks_at_height(
        &self,
        proto: &mut ProtoChunk,
        params: &GenerationParams,
        state: &mut GenerationState,
        cell_bounds: &CellBounds,
        block_y: i32,
        local_y: i32,
        sample_start_y: i32,
        inv_horizontal: f64,
    ) -> bool {
        // Loop through all blocks in this horizontal layer
        for local_x in 0..params.horizontal_cell_block_count {
            let block_x = cell_bounds.cell_start_x + local_x as i32;
            let chunk_local_x = (block_x - params.start_block_x) as usize;

            for local_z in 0..params.horizontal_cell_block_count {
                let block_z = cell_bounds.cell_start_z + local_z as i32;
                let chunk_local_z = (block_z - params.start_block_z) as usize;

                // Skip if we already found surface for this column
                if state.surface_found[chunk_local_x][chunk_local_z] {
                    continue;
                }

                // Sample this specific block
                let delta_x = local_x as f64 * inv_horizontal;
                let delta_z = local_z as f64 * inv_horizontal;
                proto.noise_sampler.interpolate_x(delta_x);
                proto.noise_sampler.interpolate_z(delta_z);

                let block_state = proto
                    .noise_sampler
                    .sample_block_state(
                        Vector3::new(
                            cell_bounds.sample_start_x,
                            sample_start_y,
                            cell_bounds.sample_start_z,
                        ),
                        Vector3::new(local_x as i32, local_y, local_z as i32),
                        &mut proto.surface_height_estimate_sampler,
                    )
                    .unwrap_or(proto.default_block);

                proto.set_block_state(&Vector3::new(block_x, block_y, block_z), block_state);

                // If we found a solid block, mark it as the surface height
                if !block_state.is_air() {
                    self.handle_surface_found(proto, state, chunk_local_x, chunk_local_z, block_y);

                    if state.columns_completed >= (CHUNK_DIM as usize * CHUNK_DIM as usize) {
                        return true;
                    }
                }
            }
        }
        false
    }

    fn handle_surface_found(
        &self,
        proto: &mut ProtoChunk,
        state: &mut GenerationState,
        chunk_local_x: usize,
        chunk_local_z: usize,
        block_y: i32,
    ) {
        let index = chunk_local_x * CHUNK_DIM as usize + chunk_local_z;
        proto.flat_surface_height_map[index] = block_y as i64;
        state.surface_found[chunk_local_x][chunk_local_z] = true;
        state.columns_completed += 1;
        state.surface_samples.push(block_y);
        state.update_surface_estimate(self.samples_for_convergence);
    }
}

struct GenerationParams {
    horizontal_cell_block_count: u8,
    vertical_cell_block_count: u8,
    horizontal_cells: u16,
    min_y: i8,
    minimum_cell_y: i8,
    start_cell: u16,
    end_cell: u16,
    start_block_x: i32,
    start_block_z: i32,
    start_cell_x: i32,
    start_cell_z: i32,
}

struct GenerationState {
    surface_found: [[bool; CHUNK_DIM as usize]; CHUNK_DIM as usize],
    columns_completed: usize,
    surface_samples: Vec<i32>,
    estimated_surface_y: i32,
}

impl GenerationState {
    fn new(initial_estimate: i32) -> Self {
        Self {
            surface_found: [[false; CHUNK_DIM as usize]; CHUNK_DIM as usize],
            columns_completed: 0,
            surface_samples: Vec::new(),
            estimated_surface_y: initial_estimate,
        }
    }

    fn update_surface_estimate(&mut self, samples_for_convergence: usize) {
        if self.surface_samples.len() <= samples_for_convergence {
            self.estimated_surface_y =
                self.surface_samples.iter().sum::<i32>() / self.surface_samples.len() as i32;
        } else {
            let weight = 0.1;
            let latest_sample = *self.surface_samples.last().unwrap();
            self.estimated_surface_y = ((1.0 - weight) * self.estimated_surface_y as f32
                + weight * latest_sample as f32) as i32;
        }
    }
}

struct CellBounds {
    adaptive_start: u16,
    adaptive_end: u16,
    cell_start_x: i32,
    cell_start_z: i32,
    sample_start_x: i32,
    sample_start_z: i32,
}

// Legacy function to maintain compatibility
pub fn generate_surface_heights(proto: &mut ProtoChunk) {
    let generator = SurfaceHeightGenerator::new();
    generator.generate(proto);
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
) -> ProtoChunk<'a> {
    ProtoChunk::new(
        at,
        &generation_settings.base_router,
        &generation_settings.random_config,
        generation_settings.generation_settings,
    )
}
