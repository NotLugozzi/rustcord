use rustcord_plugins::{ApplicationEvent, PluginManager};
use std::path::PathBuf;
use std::time::Duration;

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
    manager.dispatch(ApplicationEvent::Started);
    std::thread::sleep(Duration::from_millis(100));
    manager.dispatch(ApplicationEvent::Shutdown);
    let failures = manager.shutdown(Duration::from_secs(2));
    for log in manager.take_logs() {
        println!("[{}] {}", log.plugin_id(), log.message());
    }
    for (failed_id, error) in &failures {
        eprintln!(
            "plugin {failed_id} failed ({}): {error}",
            error.category().label()
        );
    }
    manager.unload(&plugin_id)?;
    if failures.is_empty() {
        Ok(())
    } else {
        Err(std::io::Error::other("a plugin reported errors").into())
    }
}
