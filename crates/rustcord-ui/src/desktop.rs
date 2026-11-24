use std::process::Command;

pub const CAN_HANDLE_LINKS: bool = cfg!(any(target_os = "linux", target_os = "windows"));

#[cfg(target_os = "linux")]
const HANDLER_FILE: &str = "rustcord-links.desktop";

fn run(program: &str, args: &[&str]) -> Result<String, String> {
    let output = Command::new(program).args(args).output().map_err(|error| format!("couldn't run {program}: {error}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        Err(format!("{program} failed: {}", String::from_utf8_lossy(&output.stderr).trim()))
    }
}

#[cfg(target_os = "linux")]
fn desktop_exec(path: &std::path::Path) -> String {
    let escaped: String = path
        .to_string_lossy()
        .chars()
        .flat_map(|c| match c {
            '"' | '`' | '$' | '\\' => vec!['\\', c],
            other => vec![other],
        })
        .collect();
    format!("\"{escaped}\" %u")
}

#[cfg(target_os = "linux")]
pub fn link_handler() -> Option<String> {
    run("xdg-mime", &["query", "default", "x-scheme-handler/discord"]).ok().filter(|handler| !handler.is_empty())
}

#[cfg(target_os = "linux")]
pub fn handles_links() -> bool {
    link_handler().as_deref() == Some(HANDLER_FILE)
}

#[cfg(target_os = "linux")]
pub fn register_link_handler() -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|error| format!("couldn't find rustcord's own path: {error}"))?;
    let dir = directories::BaseDirs::new().ok_or("there's no home folder")?.data_dir().join("applications");
    std::fs::create_dir_all(&dir).map_err(|error| format!("couldn't create {}: {error}", dir.display()))?;
    let entry = format!(
        "[Desktop Entry]\nType=Application\nName=RustCord\nComment=Open Discord links in RustCord\nExec={}\nTerminal=false\nNoDisplay=true\nMimeType=x-scheme-handler/discord;\n",
        desktop_exec(&exe)
    );
    let path = dir.join(HANDLER_FILE);
    std::fs::write(&path, entry).map_err(|error| format!("couldn't write {}: {error}", path.display()))?;
    let _ = run("update-desktop-database", &[&dir.to_string_lossy()]);
    run("xdg-mime", &["default", HANDLER_FILE, "x-scheme-handler/discord"]).map(|_| ())
}

#[cfg(target_os = "windows")]
const COMMAND_KEY: &str = r"HKCU\Software\Classes\discord\shell\open\command";

#[cfg(target_os = "windows")]
fn exe_command() -> Result<String, String> {
    let exe = std::env::current_exe().map_err(|error| format!("couldn't find rustcord's own path: {error}"))?;
    Ok(format!("\"{}\" \"%1\"", exe.display()))
}

#[cfg(target_os = "windows")]
pub fn link_handler() -> Option<String> {
    let output = run("reg", &["query", COMMAND_KEY, "/ve"]).ok()?;
    output.lines().find_map(|line| line.split_once("REG_SZ").map(|(_, value)| value.trim().to_string()))
}

#[cfg(target_os = "windows")]
pub fn handles_links() -> bool {
    exe_command().is_ok_and(|command| link_handler().as_deref() == Some(command.as_str()))
}

#[cfg(target_os = "windows")]
pub fn register_link_handler() -> Result<(), String> {
    let command = exe_command()?;
    run("reg", &["add", r"HKCU\Software\Classes\discord", "/ve", "/d", "URL:Discord Protocol", "/f"])?;
    run("reg", &["add", r"HKCU\Software\Classes\discord", "/v", "URL Protocol", "/d", "", "/f"])?;
    run("reg", &["add", COMMAND_KEY, "/ve", "/d", &command, "/f"]).map(|_| ())
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
pub fn link_handler() -> Option<String> {
    None
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
pub fn handles_links() -> bool {
    false
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
pub fn register_link_handler() -> Result<(), String> {
    Err("registering a link handler needs an app bundle on macOS".into())
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    #[test]
    fn the_desktop_entry_quotes_awkward_paths() {
        assert_eq!(desktop_exec(std::path::Path::new("/usr/bin/rustcord")), "\"/usr/bin/rustcord\" %u");
        assert_eq!(desktop_exec(std::path::Path::new("/home/a b/$x\"y")), "\"/home/a b/\\$x\\\"y\" %u");
    }
}
