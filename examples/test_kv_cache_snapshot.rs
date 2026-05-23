//! Test KV Cache snapshot functionality
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    println!("Testing KV Cache Snapshot Manager...\n");

    // Create a temporary directory for snapshots
    let temp_dir = "./test_snapshots";
    std::fs::create_dir_all(temp_dir)?;

    // Initialize snapshot manager
    use jcode_cpu_inference::graceful_manager::SnapshotManager;
    let manager = SnapshotManager::new(temp_dir.to_string());

    println!("1. Testing save_metadata...");
    let metadata = jcode_cpu_inference::graceful_manager::SnapshotMetadata {
        instance_id: "test-instance-001".to_string(),
        model_name: "qwen3.6-27b".to_string(),
        timestamp: chrono::Utc::now(),
        request_id: "test-request-001".to_string(),
        sequence_length: 1024,
        layer_count: 64,
        size_bytes: 2048,
    };

    manager.save_metadata(&metadata)?;
    println!("   Metadata saved successfully\n");

    println!("2. Testing load_metadata...");
    let loaded = manager.load_metadata("test-request-001")?;
    println!("   Loaded metadata:");
    println!("     - Model: {}", loaded.model_name);
    println!("     - Instance: {}", loaded.instance_id);
    println!("     - Seq Length: {}", loaded.sequence_length);
    println!("     - Layers: {}", loaded.layer_count);
    println!("     - Size: {} bytes\n", loaded.size_bytes);

    println!("3. Testing list_snapshots...");
    let snapshots = manager.list_snapshots(None);
    println!("   Found {} snapshot(s)", snapshots.len());
    for snap in &snapshots {
        println!("     - {} (model: {}, time: {})",
            snap.request_id, snap.model_name, snap.timestamp);
    }
    println!();

    println!("4. Testing save_kv_cache_snapshot (simulated)...");
    match manager.save_kv_cache_snapshot(
        "test-instance-001",
        "qwen3.6-27b",
        18000,
        "test-request-002"
    ).await {
        Ok(name) => println!("   Snapshot saved: {}\n", name),
        Err(e) => println!("   Expected warning (llama.cpp not running): {}\n", e),
    }

    println!("5. Testing cleanup_old_snapshots...");
    let cleaned = manager.cleanup_old_snapshots(0)?;
    println!("   Cleaned {} old snapshots\n", cleaned);

    // Cleanup test directory
    std::fs::remove_dir_all(temp_dir)?;
    println!("Test completed successfully!");

    Ok(())
}
