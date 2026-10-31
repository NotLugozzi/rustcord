pub const NAME: &str = "rustcord";

pub const DESCRIPTION: &str =
    "A native Discord client written in Rust. Talks to Discord's real gateway and voice servers \
     with an imgui/wgpu interface, including text chat, voice calls with end-to-end encryption, \
     and inline media.";

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub fn commit() -> Option<&'static str> {
    non_empty(option_env!("RUSTCORD_COMMIT").unwrap_or(""))
}

pub fn tag() -> Option<&'static str> {
    non_empty(option_env!("RUSTCORD_TAG").unwrap_or(""))
}

pub fn commit_date() -> Option<&'static str> {
    non_empty(option_env!("RUSTCORD_COMMIT_DATE").unwrap_or(""))
}

pub fn version_line() -> String {
    let mut line = match tag() {
        Some(tag) => tag.to_string(),
        None => format!("v{VERSION}"),
    };
    if let Some(commit) = commit() {
        line.push_str(&format!(" ({commit})"));
    }
    line
}

fn non_empty(value: &'static str) -> Option<&'static str> {
    if value.is_empty() { None } else { Some(value) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_untagged_build_falls_back_to_the_crate_version() {
        let line = version_line();
        assert!(tag().is_some() || line.starts_with(&format!("v{VERSION}")), "got {line}");
    }

    #[test]
    fn the_version_line_includes_the_commit_when_git_is_available() {
        match commit() {
            Some(commit) => assert!(version_line().contains(commit)),
            None => assert!(!version_line().contains('(')),
        }
    }

    #[test]
    fn the_build_script_captured_real_git_metadata() {
        let in_git_repo = std::process::Command::new("git")
            .args(["rev-parse", "--git-dir"])
            .output()
            .is_ok_and(|o| o.status.success());
        if !in_git_repo {
            return;
        }
        let commit = commit().expect("build script should have captured a commit hash");
        assert!(commit.len() >= 7, "short hash looks wrong: {commit}");
        assert!(commit.chars().all(|c| c.is_ascii_hexdigit()), "not a hash: {commit}");
        let date = commit_date().expect("build script should have captured a commit date");
        assert_eq!(date.len(), 10, "expected YYYY-MM-DD, got {date}");
    }

    #[test]
    fn empty_build_env_vars_read_as_absent() {
        assert_eq!(non_empty(""), None);
        assert_eq!(non_empty("abc123"), Some("abc123"));
    }
}
