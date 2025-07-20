use sqlx::{PgPool, Row};
use std::sync::Arc;
use tokio::sync::Mutex;
use std::panic::{RefUnwindSafe, UnwindSafe};

#[derive(Clone)]
pub struct BlockStorage {
    pool: PgPool,
    chunk_queue: Arc<Mutex<Vec<(i32, i32, Vec<i64>)>>>,
    batch_size: usize,
}

// Implement UnwindSafe and RefUnwindSafe (these are safe traits)
impl UnwindSafe for BlockStorage {}
impl RefUnwindSafe for BlockStorage {}

impl BlockStorage {
    /// Create a new BlockStorage instance with batching
    pub async fn new(database_url: &str, batch_size: usize) -> Result<Self, sqlx::Error> {
        let pool = PgPool::connect(database_url).await?;
        Ok(Self { 
            pool,
            chunk_queue: Arc::new(Mutex::new(Vec::new())),
            batch_size,
        })
    }
    
    /// Create a BlockStorage from an existing pool with batching
    pub fn from_pool(pool: PgPool, batch_size: usize) -> Self {
        Self { 
            pool,
            chunk_queue: Arc::new(Mutex::new(Vec::new())),
            batch_size,
        }
    }
    
    /// Get a reference to the underlying pool (if needed)
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }
    
    /// Queue a chunk for batched storage
    pub async fn queue_chunk(&self, chunk_x: i32, chunk_z: i32, height_map: Vec<i64>) -> Result<bool, sqlx::Error> {
        let mut queue = self.chunk_queue.lock().await;
        queue.push((chunk_x, chunk_z, height_map));
        
        // Check if we've reached the batch size
        if queue.len() >= self.batch_size {
            // Take all queued chunks and store them
            let chunks_to_store = queue.drain(..).collect::<Vec<_>>();
            drop(queue); // Release the lock before the async operation
            
            self.store_chunks_raw(&chunks_to_store).await?;
            Ok(true) // Indicates that chunks were stored
        } else {
            Ok(false) // Indicates that chunks are still queued
        }
    }
    
    /// Flush all queued chunks to storage regardless of batch size
    pub async fn flush_queue(&self) -> Result<usize, sqlx::Error> {
        let mut queue = self.chunk_queue.lock().await;
        if queue.is_empty() {
            return Ok(0);
        }
        
        let chunks_to_store = queue.drain(..).collect::<Vec<_>>();
        let count = chunks_to_store.len();
        drop(queue); // Release the lock before the async operation
        
        self.store_chunks_raw(&chunks_to_store).await?;
        Ok(count)
    }
    
    /// Get the current queue size
    pub async fn queue_size(&self) -> usize {
        self.chunk_queue.lock().await.len()
    }
    
    /// Check if the queue is empty
    pub async fn is_queue_empty(&self) -> bool {
        self.chunk_queue.lock().await.is_empty()
    }
    
    /// Ultra-fast storage with no indexes
    pub async fn store_chunks_raw(&self, chunks: &[(i32, i32, Vec<i64>)]) -> Result<(), sqlx::Error> {
        if chunks.is_empty() {
            return Ok(());
        }
        
        // Build one massive COPY statement
        let mut copy_data = String::new();
       
        for (chunk_x, chunk_z, height_map) in chunks {
            for local_x in 0..16 {
                for local_z in 0..16 {
                    let index = (local_x * 16 + local_z) as usize;
                    let height = height_map[index] as i32;
                    let world_x = chunk_x * 16 + local_x;
                    let world_z = chunk_z * 16 + local_z;
                   
                    copy_data.push_str(&format!("{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
                        chunk_x, chunk_z, local_x, local_z, height, world_x, world_z));
                }
            }
        }
       
        // Raw COPY - no constraint checking, no indexes
        sqlx::query("COPY blocks FROM STDIN WITH (FORMAT text, DELIMITER E'\\t')")
            .execute(&self.pool)
            .await?;
       
        Ok(())
    }
    
    /// Create table with NO indexes initially
    pub async fn create_raw_table(&self) -> Result<(), sqlx::Error> {
        sqlx::query(r#"
            CREATE TABLE IF NOT EXISTS blocks (
                chunk_x INTEGER,
                chunk_z INTEGER,
                local_x INTEGER,
                local_z INTEGER,
                height INTEGER,
                world_x INTEGER,
                world_z INTEGER
            );
            -- NO INDEXES, NO CONSTRAINTS - just raw storage
        "#).execute(&self.pool).await?;
       
        Ok(())
    }
    
    /// Store a single chunk immediately (bypasses queue)
    pub async fn store_chunk_immediate(&self, chunk_x: i32, chunk_z: i32, height_map: &[i64]) -> Result<(), sqlx::Error> {
        self.store_chunks_raw(&[(chunk_x, chunk_z, height_map.to_vec())]).await
    }
    
    /// Store a single chunk (legacy method for compatibility)
    pub async fn store_chunk(&self, chunk_x: i32, chunk_z: i32, height_map: &[i64]) -> Result<(), sqlx::Error> {
        self.queue_chunk(chunk_x, chunk_z, height_map.to_vec()).await?;
        Ok(())
    }
    
    /// Close the connection pool gracefully (flushes queue first)
    pub async fn close(self) -> Result<(), sqlx::Error> {
        // Flush any remaining chunks before closing
        self.flush_queue().await?;
        self.pool.close().await;
        Ok(())
    }
}