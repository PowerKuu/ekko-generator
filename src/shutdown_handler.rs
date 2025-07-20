use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

// Global shutdown signal
static SHUTDOWN_REQUESTED: AtomicBool = AtomicBool::new(false);
static CURRENT_CHUNK_INDEX: AtomicUsize = AtomicUsize::new(0);

pub async fn setup_shutdown_handler() {
    tokio::spawn(async {
        tokio::signal::ctrl_c()
            .await
            .expect("Failed to listen for ctrl-c");
        
        println!("\n🛑 Shutdown signal received! Saving progress...");
        SHUTDOWN_REQUESTED.store(true, Ordering::SeqCst);
        
        // Give a moment for current operations to complete
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        
        let current_index = CURRENT_CHUNK_INDEX.load(Ordering::SeqCst);
        println!("📝 Current progress saved!");
        println!("💾 To resume generation, update your config chunks_start_index to: {}", current_index);
        println!("👋 Goodbye!");
        
        std::process::exit(0);
    });
}

pub fn is_shutdown_requested() -> bool {
    SHUTDOWN_REQUESTED.load(Ordering::SeqCst)
}

pub fn update_current_chunk_index(index: usize) {
    CURRENT_CHUNK_INDEX.store(index, Ordering::SeqCst);
}

pub fn get_current_chunk_index() -> usize {
    CURRENT_CHUNK_INDEX.load(Ordering::SeqCst)
}
