#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Link {
    Guild { guild_id: String },
    Channel { guild_id: Option<String>, channel_id: String, message_id: Option<String> },
    Invite { code: String },
}

const WEB_HOSTS: [&str; 5] = ["discord.com", "ptb.discord.com", "canary.discord.com", "discordapp.com", "www.discord.com"];
const INVITE_HOSTS: [&str; 2] = ["discord.gg", "www.discord.gg"];

fn snowflake(part: &str) -> Option<String> {
    (!part.is_empty() && part.len() <= 20 && part.bytes().all(|byte| byte.is_ascii_digit())).then(|| part.to_string())
}

fn invite_code(part: &str) -> Option<String> {
    (!part.is_empty() && part.len() <= 64 && part.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')).then(|| part.to_string())
}

fn route(segments: &[&str]) -> Option<Link> {
    match segments {
        ["channels", "@me", channel] => Some(Link::Channel { guild_id: None, channel_id: snowflake(channel)?, message_id: None }),
        ["channels", "@me", channel, message] => Some(Link::Channel { guild_id: None, channel_id: snowflake(channel)?, message_id: Some(snowflake(message)?) }),
        ["channels", guild] => Some(Link::Guild { guild_id: snowflake(guild)? }),
        ["channels", guild, channel] => Some(Link::Channel { guild_id: Some(snowflake(guild)?), channel_id: snowflake(channel)?, message_id: None }),
        ["channels", guild, channel, message] => Some(Link::Channel {
            guild_id: Some(snowflake(guild)?),
            channel_id: snowflake(channel)?,
            message_id: Some(snowflake(message)?),
        }),
        ["invite", code] => Some(Link::Invite { code: invite_code(code)? }),
        _ => None,
    }
}

pub fn parse(link: &str) -> Option<Link> {
    let link = link.trim();
    let without_query = link.split(['?', '#']).next().unwrap_or(link);
    let lower = without_query.to_ascii_lowercase();
    let (rest, from_app) = if lower.starts_with("discord:") {
        (&without_query["discord:".len()..], true)
    } else if let Some(rest) = ["https://", "http://"].iter().find_map(|scheme| lower.starts_with(scheme).then(|| &without_query[scheme.len()..])) {
        (rest, false)
    } else {
        return None;
    };
    let mut segments: Vec<&str> = rest.split('/').filter(|segment| !segment.is_empty()).collect();
    if from_app {
        if segments.first() == Some(&"-") {
            segments.remove(0);
        }
    } else {
        let host = segments.first()?.to_ascii_lowercase();
        if INVITE_HOSTS.contains(&host.as_str()) {
            return match &segments[1..] {
                [code] => Some(Link::Invite { code: invite_code(code)? }),
                _ => None,
            };
        }
        if !WEB_HOSTS.contains(&host.as_str()) {
            return None;
        }
        segments.remove(0);
    }
    route(&segments)
}

pub fn from_args(args: impl IntoIterator<Item = String>) -> Option<String> {
    args.into_iter().skip(1).find(|arg| !arg.starts_with("--") && parse(arg).is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn channel(guild: Option<&str>, channel: &str, message: Option<&str>) -> Option<Link> {
        Some(Link::Channel { guild_id: guild.map(str::to_string), channel_id: channel.into(), message_id: message.map(str::to_string) })
    }

    #[test]
    fn app_links_and_web_links_reach_the_same_places() {
        for link in ["discord://-/channels/1/2/3", "discord:///channels/1/2/3", "https://discord.com/channels/1/2/3", "https://canary.discord.com/channels/1/2/3?x=1"] {
            assert_eq!(parse(link), channel(Some("1"), "2", Some("3")), "{link}");
        }
        assert_eq!(parse("discord://-/channels/@me/42"), channel(None, "42", None));
        assert_eq!(parse("https://discordapp.com/channels/@me/42/43"), channel(None, "42", Some("43")));
        assert_eq!(parse("discord://-/channels/7"), Some(Link::Guild { guild_id: "7".into() }));
        for link in ["discord://-/invite/rust-lang", "https://discord.gg/rust-lang", "https://discord.com/invite/rust-lang", "DISCORD://-/invite/rust-lang"] {
            assert_eq!(parse(link), Some(Link::Invite { code: "rust-lang".into() }), "{link}");
        }
    }

    #[test]
    fn anything_else_is_ignored() {
        for link in [
            "",
            "discord://-/",
            "discord://-/channels/abc/2",
            "discord://-/users/5",
            "https://example.com/channels/1/2",
            "https://discord.gg/a/b",
            "https://discord.gg/bad%20code",
            "file:///channels/1/2",
            "--demo",
        ] {
            assert_eq!(parse(link), None, "{link}");
        }
    }

    #[test]
    fn the_first_link_argument_is_picked_up() {
        let args = ["rustcord", "--demo", "discord://-/channels/1/2", "https://discord.gg/x"].map(String::from);
        assert_eq!(from_args(args).as_deref(), Some("discord://-/channels/1/2"));
        assert_eq!(from_args(["rustcord", "--demo"].map(String::from)), None);
    }
}
