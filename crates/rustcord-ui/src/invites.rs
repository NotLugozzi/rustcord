use rustcord_client::models::Invite;
use std::time::{Duration, Instant};

pub const MAX_CARDS_PER_MESSAGE: usize = 3;
pub const CACHE_TTL: Duration = Duration::from_secs(10 * 60);

const PREFIXES: [&str; 3] = ["discord.gg/", "discord.com/invite/", "discordapp.com/invite/"];

fn is_code_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '-'
}

fn strip_code(content: &str) -> String {
    let mut out = String::with_capacity(content.len());
    let mut rest = content;
    while let Some(tick) = rest.find('`') {
        out.push_str(&rest[..tick]);
        let after = &rest[tick..];
        let fence = if after.starts_with("```") { "```" } else { "`" };
        match after[fence.len()..].find(fence) {
            Some(close) => {
                out.push(' ');
                rest = &after[fence.len() + close + fence.len()..];
            }
            None => {
                out.push_str(after);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

fn code_after_prefix(text: &str, at: usize, prefix: &str) -> Option<String> {
    let boundary_ok = text[..at].chars().next_back().is_none_or(|c| !(c.is_ascii_alphanumeric() || c == '.' || c == '-'));
    if !boundary_ok {
        return None;
    }
    let code: String = text[at + prefix.len()..].chars().take_while(|c| is_code_char(*c)).collect();
    (!code.is_empty()).then_some(code)
}

pub fn invite_codes(content: &str) -> Vec<String> {
    if !content.to_ascii_lowercase().contains("discord") {
        return Vec::new();
    }
    let text = strip_code(content);
    let lower = text.to_ascii_lowercase();
    let mut found: Vec<(usize, String)> = Vec::new();
    for prefix in PREFIXES {
        for (at, _) in lower.match_indices(prefix) {
            if let Some(code) = code_after_prefix(&text, at, prefix) {
                found.push((at, code));
            }
        }
    }
    found.sort_by_key(|(at, _)| *at);
    let mut codes: Vec<String> = Vec::new();
    for (_, code) in found {
        if !codes.contains(&code) {
            codes.push(code);
        }
        if codes.len() == MAX_CARDS_PER_MESSAGE {
            break;
        }
    }
    codes
}

pub fn is_invite_url(url: &str) -> bool {
    let bare = url.strip_prefix("https://").or_else(|| url.strip_prefix("http://")).unwrap_or(url);
    let bare = bare.strip_prefix("www.").unwrap_or(bare);
    PREFIXES.iter().any(|p| bare.len() > p.len() && bare[..p.len()].eq_ignore_ascii_case(p))
}

pub struct ResolvedInvite {
    pub invite: Invite,
    pub icon_url: Option<String>,
    pub initials: String,
    pub counts: String,
    pub channel_name: String,
}

impl ResolvedInvite {
    pub fn new(invite: Invite) -> Self {
        let guild = invite.guild.as_ref();
        let icon_url = guild.and_then(|g| rustcord_client::cdn::guild_icon_url(&g.to_guild(), 64));
        let initials = guild.map(|g| crate::switcher::initials(&g.name)).unwrap_or_default();
        let counts = match (invite.approximate_presence_count, invite.approximate_member_count) {
            (Some(online), Some(members)) => format!("{online} Online   {members} Members"),
            (None, Some(members)) => format!("{members} Members"),
            _ => String::new(),
        };
        let channel_name = invite.channel.as_ref().and_then(|c| c.name.clone()).unwrap_or_else(|| "Voice channel".into());
        Self { invite, icon_url, initials, counts, channel_name }
    }
}

pub enum InviteState {
    Loading,
    Ready(Box<ResolvedInvite>),
    Invalid,
    Unavailable,
}

pub struct InviteEntry {
    pub state: InviteState,
    pub updated: Instant,
}

impl InviteEntry {
    pub fn needs_fetch(&self, now: Instant) -> bool {
        !matches!(self.state, InviteState::Loading) && now.duration_since(self.updated) >= CACHE_TTL
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_every_invite_form() {
        assert_eq!(invite_codes("join https://discord.gg/H2gNKzSYe now"), ["H2gNKzSYe"]);
        assert_eq!(invite_codes("discord.com/invite/abc-123"), ["abc-123"]);
        assert_eq!(invite_codes("http://discordapp.com/invite/Xy9"), ["Xy9"]);
        assert_eq!(invite_codes("<https://DISCORD.GG/Case>"), ["Case"]);
    }

    #[test]
    fn ignores_lookalike_hosts() {
        assert!(invite_codes("notdiscord.gg/abc").is_empty());
        assert!(invite_codes("discord.gg/").is_empty());
    }

    #[test]
    fn skips_code_spans_and_blocks() {
        assert!(invite_codes("`discord.gg/inline`").is_empty());
        assert!(invite_codes("```\ndiscord.gg/block\n```").is_empty());
        assert_eq!(invite_codes("`x` discord.gg/real ```discord.gg/no```"), ["real"]);
    }

    #[test]
    fn caps_at_three_and_dedupes() {
        let text = "discord.gg/a discord.gg/a discord.gg/b discord.gg/c discord.gg/d";
        assert_eq!(invite_codes(text), ["a", "b", "c"]);
    }

    #[test]
    fn recognizes_invite_urls() {
        assert!(is_invite_url("https://discord.gg/H2gNKzSYe"));
        assert!(is_invite_url("https://discord.com/invite/x"));
        assert!(!is_invite_url("https://discord.com/channels/1/2"));
    }

    #[test]
    fn cached_entries_expire_including_failures() {
        let now = Instant::now();
        let fresh = InviteEntry { state: InviteState::Invalid, updated: now };
        assert!(!fresh.needs_fetch(now));
        assert!(fresh.needs_fetch(now + CACHE_TTL));
        let loading = InviteEntry { state: InviteState::Loading, updated: now };
        assert!(!loading.needs_fetch(now + CACHE_TTL * 2));
    }
}
