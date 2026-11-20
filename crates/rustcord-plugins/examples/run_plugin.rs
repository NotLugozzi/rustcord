use rustcord_plugins::PluginManager;
use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let plugin_dir = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("plugins/hello-world"));
    let storage_dir = std::env::args()
        .nth(2)
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("rustcord-luna-example-storage"));
    let mut manager = PluginManager::new(storage_dir);
    let plugin_id = manager.load(&plugin_dir)?;
    manager.enable(&plugin_id)?;
    while manager.state(&plugin_id)? == rustcord_plugins::PluginState::Enabled {
        for (failed_id, error) in manager.poll()? {
            return Err(
                std::io::Error::other(format!("plugin {failed_id} failed: {error}")).into(),
            );
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    for log in manager.take_logs() {
        println!("[{}] {}", log.plugin_id, log.message);
    }
    manager.disable(&plugin_id)?;
    manager.unload(&plugin_id)?;
    Ok(())
}
