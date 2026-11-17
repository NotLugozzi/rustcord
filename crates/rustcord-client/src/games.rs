use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

const LIST_URL: &str = "https://cdn.discordapp.com/detectables/games-v1.json";
const LIST_FILE: &str = "discord-games.json";
const ETAG_FILE: &str = "discord-games.etag";
pub const LIST_MAX_AGE: Duration = Duration::from_secs(24 * 60 * 60);
const MISSED_SCANS_BEFORE_CLEAR: u32 = 2;

#[derive(Deserialize)]
struct RawGame {
    #[serde(default)]
    id: Option<String>,
    name: String,
    #[serde(default)]
    executables: Vec<RawExecutable>,
}

#[derive(Deserialize)]
struct RawExecutable {
    os: String,
    name: String,
    #[serde(default)]
    is_launcher: bool,
    #[serde(default)]
    arguments: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Game {
    pub id: Option<String>,
    pub name: String,
}

struct Rule {
    path: String,
    arguments: Option<String>,
    game: usize,
}

#[derive(Default)]
pub struct GameList {
    games: Vec<Game>,
    by_file: HashMap<String, Vec<Rule>>,
}

pub fn native_os() -> &'static str {
    if cfg!(target_os = "windows") {
        "win32"
    } else if cfg!(target_os = "macos") {
        "darwin"
    } else {
        "linux"
    }
}

fn normalize(path: &str) -> String {
    let path = path.replace('\\', "/").to_lowercase();
    let bytes = path.as_bytes();
    if bytes.len() > 2 && bytes[1] == b':' && bytes[2] == b'/' {
        path[2..].to_string()
    } else {
        path
    }
}

fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

impl GameList {
    pub fn parse(bytes: &[u8]) -> Option<Self> {
        let raw: Vec<Value> = serde_json::from_slice(bytes).ok()?;
        let mut list = GameList::default();
        let wanted = [native_os(), "win32"];
        for entry in raw {
            let Ok(game) = serde_json::from_value::<RawGame>(entry) else { continue };
            let rules: Vec<RawExecutable> = game.executables.into_iter().filter(|e| wanted.contains(&e.os.as_str()) && !e.is_launcher).collect();
            if rules.is_empty() {
                continue;
            }
            let index = list.games.len();
            list.games.push(Game { id: game.id, name: game.name });
            for rule in rules {
                let path = normalize(rule.name.trim_start_matches('>'));
                let key = file_name(&path).to_string();
                if key.is_empty() {
                    continue;
                }
                list.by_file.entry(key).or_default().push(Rule { path, arguments: rule.arguments.map(|a| a.to_lowercase()), game: index });
            }
        }
        Some(list)
    }

    pub fn len(&self) -> usize {
        self.games.len()
    }

    pub fn is_empty(&self) -> bool {
        self.games.is_empty()
    }

    pub fn match_process(&self, path: &str, command_line: &str) -> Option<&Game> {
        let path = normalize(path);
        let rules = self.by_file.get(file_name(&path))?;
        let command_line = command_line.to_lowercase();
        rules
            .iter()
            .filter(|rule| path == rule.path || path.ends_with(&format!("/{}", rule.path)))
            .find(|rule| rule.arguments.as_deref().is_none_or(|args| command_line.contains(args)))
            .map(|rule| &self.games[rule.game])
    }
}

pub fn activity(game: &Game, started_ms: i64) -> Value {
    let mut activity = json!({ "type": 0, "name": game.name, "timestamps": { "start": started_ms } });
    if let Some(id) = &game.id {
        activity["application_id"] = json!(id);
    }
    activity
}

fn is_wine_host(path: &str) -> bool {
    let name = file_name(path);
    name.starts_with("wine") || name.contains("preloader") || name == "proton"
}

pub fn candidate_paths(exe: Option<&str>, command: &[String]) -> Vec<String> {
    let mut paths = Vec::new();
    if let Some(exe) = exe.filter(|exe| !exe.is_empty()) {
        if !is_wine_host(&normalize(exe)) {
            paths.push(exe.to_string());
        }
    }
    let windows_program = command.iter().find(|arg| arg.to_lowercase().ends_with(".exe"));
    if let Some(program) = windows_program {
        if exe.is_none_or(|exe| is_wine_host(&normalize(exe))) || paths.is_empty() {
            paths.push(program.clone());
        }
    }
    paths
}

#[derive(Debug, Clone, PartialEq)]
pub struct Running {
    pub game: Game,
    pub started_ms: i64,
}

pub struct Scanner {
    system: sysinfo::System,
    list: GameList,
    me: Option<sysinfo::Uid>,
    current: Option<(sysinfo::Pid, Running)>,
    missed: u32,
}

impl Scanner {
    pub fn new(list: GameList) -> Self {
        Self { system: sysinfo::System::new(), list, me: None, current: None, missed: 0 }
    }

    fn refresh(&mut self) {
        use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, UpdateKind};
        let kind = ProcessRefreshKind::nothing()
            .without_tasks()
            .with_exe(UpdateKind::OnlyIfNotSet)
            .with_cmd(UpdateKind::OnlyIfNotSet)
            .with_user(UpdateKind::OnlyIfNotSet);
        self.system.refresh_processes_specifics(ProcessesToUpdate::All, true, kind);
        if self.me.is_none() {
            self.me = sysinfo::get_current_pid().ok().and_then(|pid| self.system.process(pid)).and_then(|p| p.user_id()).cloned();
        }
    }

    pub fn scan(&mut self) -> Option<Running> {
        self.refresh();
        if let Some((pid, _)) = &self.current {
            if self.system.process(*pid).is_some() {
                self.missed = 0;
                return self.current.as_ref().map(|(_, running)| running.clone());
            }
            self.missed += 1;
        }
        let mut found: Option<(sysinfo::Pid, Running)> = None;
        for (pid, process) in self.system.processes() {
            if self.me.is_some() && process.user_id() != self.me.as_ref() {
                continue;
            }
            let exe = process.exe().map(|p| p.to_string_lossy().into_owned());
            let command: Vec<String> = process.cmd().iter().map(|a| a.to_string_lossy().into_owned()).collect();
            let joined = command.join(" ");
            for path in candidate_paths(exe.as_deref(), &command) {
                if let Some(game) = self.list.match_process(&path, &joined) {
                    let started_ms = (process.start_time() as i64) * 1000;
                    if found.as_ref().is_none_or(|(_, best)| started_ms > best.started_ms) {
                        found = Some((*pid, Running { game: game.clone(), started_ms }));
                    }
                    break;
                }
            }
        }
        match found {
            Some(next) => {
                self.missed = 0;
                self.current = Some(next);
            }
            None if self.missed >= MISSED_SCANS_BEFORE_CLEAR || self.current.is_none() => {
                self.current = None;
            }
            None => {}
        }
        self.current.as_ref().map(|(_, running)| running.clone())
    }
}

fn list_paths(dir: &Path) -> (PathBuf, PathBuf) {
    (dir.join(LIST_FILE), dir.join(ETAG_FILE))
}

pub fn cached_list(dir: &Path) -> Option<(Vec<u8>, std::time::SystemTime)> {
    let (list, _) = list_paths(dir);
    let modified = std::fs::metadata(&list).and_then(|m| m.modified()).ok()?;
    Some((std::fs::read(&list).ok()?, modified))
}

pub async fn refresh_list(dir: &Path) -> Result<Option<Vec<u8>>, String> {
    let (list_path, etag_path) = list_paths(dir);
    let client = reqwest::Client::new();
    let mut request = client.get(LIST_URL);
    if list_path.exists() {
        if let Ok(etag) = std::fs::read_to_string(&etag_path) {
            request = request.header("If-None-Match", etag.trim());
        }
    }
    let response = request.send().await.map_err(|e| e.to_string())?;
    if response.status() == reqwest::StatusCode::NOT_MODIFIED {
        let _ = filetime_touch(&list_path);
        return Ok(None);
    }
    if !response.status().is_success() {
        return Err(format!("the game list answered {}", response.status()));
    }
    let etag = response.headers().get("etag").and_then(|v| v.to_str().ok()).map(str::to_string);
    let bytes = response.bytes().await.map_err(|e| e.to_string())?.to_vec();
    if GameList::parse(&bytes).is_none_or(|list| list.is_empty()) {
        return Err("the game list couldn't be read".into());
    }
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    std::fs::write(&list_path, &bytes).map_err(|e| e.to_string())?;
    if let Some(etag) = etag {
        let _ = std::fs::write(&etag_path, etag);
    }
    Ok(Some(bytes))
}

fn filetime_touch(path: &Path) -> std::io::Result<()> {
    let file = std::fs::OpenOptions::new().append(true).open(path)?;
    file.set_modified(std::time::SystemTime::now())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list() -> GameList {
        GameList::parse(
            json!([
                { "id": "1", "name": "Celeste", "executables": [ { "os": "linux", "name": "celeste/celeste.bin.x86_64" } ] },
                { "id": "2", "name": "Elden Ring", "executables": [ { "os": "win32", "name": "eldenring.exe" }, { "os": "win32", "name": "launcher.exe", "is_launcher": true } ] },
                { "id": "3", "name": "Minecraft", "executables": [ { "os": "linux", "name": "java", "arguments": "net.minecraft" } ] },
                { "name": "Mac Only", "executables": [ { "os": "darwin", "name": "maconly" } ] },
                { "id": "bad" }
            ])
            .to_string()
            .as_bytes(),
        )
        .unwrap()
    }

    #[test]
    fn the_list_keeps_this_systems_and_windows_executables_but_not_launchers() {
        let list = list();
        let expected = match native_os() {
            "linux" => 3,
            "darwin" => 2,
            _ => 1,
        };
        assert_eq!(list.len(), expected);
        assert!(list.match_process("/x/launcher.exe", "").is_none());
    }

    #[test]
    fn a_native_game_matches_by_its_path_suffix_only_at_a_folder_boundary() {
        let list = list();
        if native_os() != "linux" {
            return;
        }
        assert_eq!(list.match_process("/home/me/games/Celeste/Celeste.bin.x86_64", "").map(|g| g.name.as_str()), Some("Celeste"));
        assert!(list.match_process("/home/me/games/notceleste/celeste.bin.x86_64", "").is_none());
    }

    #[test]
    fn a_shared_runtime_needs_its_arguments() {
        let list = list();
        if native_os() != "linux" {
            return;
        }
        assert_eq!(list.match_process("/usr/bin/java", "java -cp x net.minecraft.client.Main").map(|g| g.name.as_str()), Some("Minecraft"));
        assert!(list.match_process("/usr/bin/java", "java -jar other.jar").is_none());
    }

    #[test]
    fn wine_and_proton_games_match_by_the_windows_program() {
        let list = list();
        let command = vec!["Z:\\home\\me\\.steam\\common\\ELDEN RING\\Game\\eldenring.exe".to_string()];
        let paths = candidate_paths(Some("/usr/bin/wine64-preloader"), &command);
        assert_eq!(paths.len(), 1);
        assert_eq!(list.match_process(&paths[0], "").map(|g| g.name.as_str()), Some("Elden Ring"));
        assert_eq!(candidate_paths(Some("/usr/bin/firefox"), &["firefox".into()]), ["/usr/bin/firefox"]);
    }

    #[test]
    fn a_games_activity_names_it_and_when_it_started() {
        let game = Game { id: Some("2".into()), name: "Elden Ring".into() };
        assert_eq!(activity(&game, 5000), json!({ "type": 0, "name": "Elden Ring", "application_id": "2", "timestamps": { "start": 5000 } }));
        let unknown = Game { id: None, name: "Custom".into() };
        assert!(activity(&unknown, 1).get("application_id").is_none());
    }

    #[test]
    fn windows_paths_drop_their_drive_and_case() {
        assert_eq!(normalize("C:\\Games\\Foo.EXE"), "/games/foo.exe");
    }

    #[tokio::test]
    #[ignore]
    async fn scans_this_machine_against_discords_real_list() {
        let dir = std::env::temp_dir().join(format!("rustcord-games-{}", std::process::id()));
        let bytes = refresh_list(&dir).await.expect("download").expect("a fresh list");
        let list = GameList::parse(&bytes).expect("parse");
        let started = std::time::Instant::now();
        let mut scanner = Scanner::new(list);
        let found = scanner.scan();
        eprintln!("games for this system: {}, first scan took {:?}, detected: {:?}", scanner.list.len(), started.elapsed(), found.map(|r| r.game.name));
        assert!(refresh_list(&dir).await.expect("revalidate").is_none(), "an unchanged list answers 304");
        let _ = std::fs::remove_dir_all(dir);
    }
}

