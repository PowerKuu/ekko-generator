use std::panic::{RefUnwindSafe, UnwindSafe};
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio_postgres::{Client, Error as PgError, NoTls};

#[derive(Clone)]
pub struct BlockStorage {
    client: Arc<Client>,
    chunk_queue: Arc<Mutex<Vec<(i32, i32, Vec<i64>)>>>,
    batch_size: usize,
}

// Implement UnwindSafe and RefUnwindSafe (these are safe traits)
impl UnwindSafe for BlockStorage {}
impl RefUnwindSafe for BlockStorage {}

impl BlockStorage {
    /// Create a new BlockStorage instance with batching
    pub async fn new(database_url: &str, batch_size: usize) -> Result<Self, PgError> {
        let (client, connection) = tokio_postgres::connect(database_url, NoTls).await?;

        // Spawn the connection task to handle the connection in the background
        tokio::spawn(async move {
            if let Err(e) = connection.await {
                eprintln!("Database connection error: {}", e);
            }
        });

        Ok(Self {
            client: Arc::new(client),
            chunk_queue: Arc::new(Mutex::new(Vec::new())),
            batch_size,
        })
    }

    /// Queue a chunk for batched storage
    pub async fn queue_chunk(
        &self,
        chunk_x: i32,
        chunk_z: i32,
        height_map: Vec<i64>,
    ) -> Result<bool, PgError> {
        let mut queue = self.chunk_queue.lock().await;
        queue.push((chunk_x, chunk_z, height_map));

        // Check if we've reached the batch size
        if queue.len() >= self.batch_size {
            // Take all queued chunks and store them
            let chunks_to_store = queue.drain(..).collect::<Vec<_>>();
            drop(queue); // Release the lock before the async operation

            self.store_chunks_unnest(&chunks_to_store).await?;
            Ok(true) // Indicates that chunks were stored
        } else {
            Ok(false) // Indicates that chunks are still queued
        }
    }

    /// Flush all queued chunks to storage regardless of batch size
    pub async fn flush_queue(&self) -> Result<usize, PgError> {
        let mut queue = self.chunk_queue.lock().await;
        if queue.is_empty() {
            return Ok(0);
        }

        let chunks_to_store = queue.drain(..).collect::<Vec<_>>();
        let count = chunks_to_store.len();
        drop(queue); // Release the lock before the async operation

        self.store_chunks_unnest(&chunks_to_store).await?;
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

    /// Ultra-fast storage using UNNEST (single query for any amount of data)
    pub async fn store_chunks_unnest(
        &self,
        chunks: &[(i32, i32, Vec<i64>)],
    ) -> Result<(), PgError> {
        if chunks.is_empty() {
            return Ok(());
        }

        // Collect all data into arrays
        let mut world_xs = Vec::new();
        let mut world_zs = Vec::new();
        let mut world_ys = Vec::new();

        for (chunk_x, chunk_z, height_map) in chunks {
            for local_x in 0..16 {
                for local_z in 0..16 {
                    let index = (local_x * 16 + local_z) as usize;
                    let world_x = chunk_x * 16 + local_x;
                    let world_z = chunk_z * 16 + local_z;
                    let world_y = height_map[index] as i16; // Use i16 for SMALLINT

                    world_xs.push(world_x);
                    world_zs.push(world_z);
                    world_ys.push(world_y);
                }
            }
        }

        // Single query using UNNEST - handles unlimited data in one shot
        self.client
            .execute(
                "INSERT INTO blocks (world_x, world_z, world_y) 
             SELECT * FROM UNNEST($1::int[], $2::int[], $3::smallint[])",
                &[&world_xs, &world_zs, &world_ys],
            )
            .await?;

        Ok(())
    }

    /// Create table with NO indexes initially
    pub async fn create_raw_table(&self) -> Result<(), PgError> {
        self.client
            .execute(
                "CREATE TABLE IF NOT EXISTS blocks (
                    world_x INTEGER,
                    world_z INTEGER,
                    world_y SMALLINT
                );",
                &[],
            )
            .await?;

        Ok(())
    }

    /// Store a single chunk immediately (bypasses queue)
    pub async fn store_chunk_immediate(
        &self,
        chunk_x: i32,
        chunk_z: i32,
        height_map: &[i64],
    ) -> Result<(), PgError> {
        self.store_chunks_unnest(&[(chunk_x, chunk_z, height_map.to_vec())])
            .await
    }

    /// Store a single chunk (legacy method for compatibility)
    pub async fn store_chunk(
        &self,
        chunk_x: i32,
        chunk_z: i32,
        height_map: &[i64],
    ) -> Result<(), PgError> {
        self.queue_chunk(chunk_x, chunk_z, height_map.to_vec())
            .await?;
        Ok(())
    }

    /// Close the connection gracefully (flushes queue first)
    pub async fn close(self) -> Result<(), PgError> {
        // Flush any remaining chunks before closing
        self.flush_queue().await?;
        // The connection will be closed when the client is dropped
        Ok(())
    }

    /// Get statistics about stored data
    pub async fn get_stats(&self) -> Result<(i64, i32), PgError> {
        let row = self
            .client
            .query_one("SELECT COUNT(*) as total_blocks FROM blocks", &[])
            .await?;

        let total_blocks: i64 = row.get(0);
        let total_chunks = (total_blocks / 256) as i32; // 16x16 = 256 blocks per chunk

        Ok((total_blocks, total_chunks))
    }
}
