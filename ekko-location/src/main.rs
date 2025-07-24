use crate::lib::config_loader::load_config;
use candle_core::{Device, Tensor};
use zarrs::array::{Array, ArrayBytes};
use zarrs_filesystem::FilesystemStore;
use std::{path::Path, sync::Arc};

mod lib {
    pub mod config_loader;
}

// Simple function to get bytes from zarr data
fn get_bytes_from_zarr_data(data: ArrayBytes) -> Vec<u8> {
    match data.into_owned() {
        ArrayBytes::Variable(raw_bytes, _offsets) => raw_bytes.into_owned(),
        ArrayBytes::Fixed(raw_bytes) => raw_bytes.into_owned(),
    }
}

// Convert bytes to height data (i64 -> f32)
fn convert_bytes_to_heights(bytes: Vec<u8>) -> Vec<f32> {
    let i64_slice = bytemuck::cast_slice::<u8, i64>(&bytes);
    i64_slice.iter().map(|&height| height as f32).collect()
}

// Calculate how much memory a chunk will use
fn calculate_chunk_size_mb(height_count: usize) -> usize {
    // Each height: i64 (8 bytes) + f32 (4 bytes) = 12 bytes during processing
    (height_count * 12) / (1024 * 1024)
}

// Get the size of one chunk
fn get_chunk_dimensions(array: &Array<FilesystemStore>, chunk_coords: &[u64]) -> (usize, Vec<usize>) {
    let chunk_shape_result = array.chunk_grid().chunk_shape(chunk_coords, array.shape());
    
    match chunk_shape_result {
        Ok(Some(chunk_shape)) => {
            let dims = chunk_shape.to_vec();
            let dim_sizes: Vec<usize> = dims.iter().map(|x| x.get() as usize).collect();
            let total_size = dim_sizes.iter().product();
            (total_size, dim_sizes)
        },
        _ => (256 * 256, vec![256, 256]) // Default fallback
    }
}

// Find all chunk coordinates in the zarr file
fn find_all_chunks(array: &Array<FilesystemStore>) -> Vec<Vec<u64>> {
    let shape = array.shape();
    let chunk_grid = array.chunk_grid();
    
    let grid_shape = match chunk_grid.grid_shape(shape) {
        Ok(Some(grid)) => grid,
        _ => return Vec::new(),
    };
    
    let mut all_chunks = Vec::new();
    generate_chunk_coords(&mut Vec::new(), &grid_shape, &mut all_chunks);
    all_chunks
}

// Helper to generate all possible chunk coordinates
fn generate_chunk_coords(current: &mut Vec<u64>, remaining: &[u64], all_chunks: &mut Vec<Vec<u64>>) {
    if remaining.is_empty() {
        all_chunks.push(current.clone());
        return;
    }
    
    for i in 0..remaining[0] {
        current.push(i);
        generate_chunk_coords(current, &remaining[1..], all_chunks);
        current.pop();
    }
}

// Load one chunk and put it on GPU
fn load_chunk_to_gpu(array: &Array<FilesystemStore>, chunk_coords: &[u64], gpu: &Device) -> Result<Tensor, Box<dyn std::error::Error>> {
    // 1. Load the chunk data
    let data = array.retrieve_chunk(chunk_coords)?;
    
    // 2. Convert to bytes
    let bytes = get_bytes_from_zarr_data(data);
    
    // 3. Convert to height data
    let heights = convert_bytes_to_heights(bytes);
    
    // 4. Get chunk dimensions
    let (_size, dimensions) = get_chunk_dimensions(array, chunk_coords);
    
    // 5. Create GPU tensor
    let tensor = Tensor::from_vec(heights, dimensions.as_slice(), gpu)?;
    
    Ok(tensor)
}

// Group chunks into batches that fit in GPU memory
fn group_chunks_into_batches(array: &Array<FilesystemStore>, all_chunks: Vec<Vec<u64>>, max_memory_mb: usize) -> Vec<Vec<Vec<u64>>> {
    let mut batches = Vec::new();
    let mut current_batch = Vec::new();
    let mut current_memory = 0;
    
    for chunk_coords in all_chunks {
        let (chunk_size, _dims) = get_chunk_dimensions(array, &chunk_coords);
        let chunk_memory = calculate_chunk_size_mb(chunk_size);
        
        // If adding this chunk would exceed memory limit, start new batch
        if current_memory + chunk_memory > max_memory_mb && !current_batch.is_empty() {
            batches.push(current_batch);
            current_batch = Vec::new();
            current_memory = 0;
        }
        
        current_batch.push(chunk_coords);
        current_memory += chunk_memory;
    }
    
    // Add the last batch if it has chunks
    if !current_batch.is_empty() {
        batches.push(current_batch);
    }
    
    batches
}

// Simple callback type for pattern recognition
type PatternCallback = fn(&Tensor, &[u64]) -> Result<(), Box<dyn std::error::Error>>;

// Main class for processing chunks on CUDA GPU
struct ChunkCudaProcessor {
    array: Array<FilesystemStore>,
    gpu: Device,
    max_memory_mb: usize,
}

impl ChunkCudaProcessor {
    // Create new processor
    fn new(zarr_path: &str, max_memory_mb: usize) -> Result<Self, Box<dyn std::error::Error>> {
        let store = Arc::new(FilesystemStore::new(Path::new(zarr_path))?);
        let array = Array::open(store, "/")?;
        
        // Check if CUDA is available first
        let gpu = match Device::new_cuda(0) {
            Ok(device) => device,
            Err(_) => return Err("CUDA not found! This processor requires CUDA.".into()),
        };
        
        Ok(Self {
            array,
            gpu,
            max_memory_mb,
        })
    }
    
    // Get info about the zarr file
    fn get_info(&self) {
        println!("Zarr array shape: {:?}", self.array.shape());
        println!("Using GPU: {:?}", self.gpu);
        println!("Max GPU memory: {}MB", self.max_memory_mb);
    }
    
    // Count total number of chunks
    fn count_chunks(&self) -> usize {
        let all_chunks = find_all_chunks(&self.array);
        all_chunks.len()
    }
    
    // Process all chunks with a callback function
    fn process_all_chunks(&self, callback: PatternCallback) -> Result<(), Box<dyn std::error::Error>> {
        // 1. Find all chunks
        let all_chunks = find_all_chunks(&self.array);
        println!("Found {} chunks to process", all_chunks.len());
        
        // 2. Group into batches
        let batches = group_chunks_into_batches(&self.array, all_chunks, self.max_memory_mb);
        println!("Split into {} batches", batches.len());
        
        // 3. Process each batch
        for (batch_num, batch) in batches.iter().enumerate() {
            println!("Processing batch {}/{} with {} chunks", 
                     batch_num + 1, batches.len(), batch.len());
                     
            self.process_one_batch(batch, callback)?;
        }
        
        Ok(())
    }
    
    // Process one batch of chunks
    fn process_one_batch(&self, batch: &[Vec<u64>], callback: PatternCallback) -> Result<(), Box<dyn std::error::Error>> {
        for chunk_coords in batch {
            // Load chunk to GPU
            let tensor = load_chunk_to_gpu(&self.array, chunk_coords, &self.gpu)?;
            
            // Run pattern recognition callback
            callback(&tensor, chunk_coords)?;
        }
        
        Ok(())
    }
}

// Example pattern recognition function
fn simple_pattern_analysis(tensor: &Tensor, chunk_coords: &[u64]) -> Result<(), Box<dyn std::error::Error>> {
    let max_height = tensor.max(tensor.dims().len() - 1)?.max(tensor.dims().len() - 2)?;
    let min_height = tensor.min(tensor.dims().len() - 1)?.min(tensor.dims().len() - 2)?;
    
    println!("Chunk {:?}: Heights from {} to {}", 
             chunk_coords,
             min_height.to_scalar::<f32>()?, 
             max_height.to_scalar::<f32>()?);
             
    Ok(())
}

#[tokio::main]
pub async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("CUDA available? {}", tch::Cuda::is_available());
    
    let config = load_config();
    println!("Zarr file: {}", config.zarr_storage_location);
    
    // Create processor with 2GB GPU memory limit
    let processor = ChunkCudaProcessor::new(&config.zarr_storage_location, 2048)?;
    
    // Show info
    processor.get_info();
    println!("Total chunks: {}", processor.count_chunks());
    
    // Process all chunks with simple pattern analysis
    processor.process_all_chunks(simple_pattern_analysis)?;
    
    Ok(())
}