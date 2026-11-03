#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Channel { guild_id: String, channel_id: String },
    Dm { channel_id: String },
    Guild { guild_id: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Icon {
    Image(String),
    Initials(String),
    Channel,
}

#[derive(Debug, Clone)]
pub struct Entry {
    pub label: String,
    pub context: Option<String>,
    pub icon: Icon,
    pub target: Target,
}

pub fn initials(name: &str) -> String {
    name.split_whitespace()
        .filter_map(|word| word.chars().next())
        .take(3)
        .flat_map(char::to_uppercase)
        .collect()
}

#[derive(Debug, Clone)]
pub struct Match {
    pub entry: Entry,
    pub score: i32,
    pub hits: Vec<usize>,
}

pub fn score(haystack: &str, needle: &str) -> Option<(i32, Vec<usize>)> {
    if needle.is_empty() {
        return Some((0, Vec::new()));
    }
    let hay: Vec<char> = haystack.chars().flat_map(char::to_lowercase).collect();
    let pat: Vec<char> = needle.chars().flat_map(char::to_lowercase).collect();

    let mut hits = Vec::with_capacity(pat.len());
    let mut total = 0i32;
    let mut hay_idx = 0usize;
    let mut previous_hit: Option<usize> = None;

    for want in &pat {
        let found = hay[hay_idx..].iter().position(|c| c == want)? + hay_idx;
        let mut points = 1;
        let consecutive = previous_hit == Some(found.wrapping_sub(1));
        if consecutive {
            points += 12;
        }
        if found == 0 {
            points += 10;
        } else if !hay[found - 1].is_alphanumeric() && !consecutive {
            points += 8;
        }
        total += points;
        hits.push(found);
        previous_hit = Some(found);
        hay_idx = found + 1;
    }

    total -= hits[0] as i32;
    total -= (hay.len() as i32 - pat.len() as i32).max(0) / 8;
    Some((total, hits))
}

pub fn rank(entries: &[Entry], query: &str, limit: usize) -> Vec<Match> {
    let mut scored: Vec<Match> = entries
        .iter()
        .filter_map(|entry| {
            let (score, hits) = score(&entry.label, query)?;
            Some(Match { entry: entry.clone(), score, hits })
        })
        .collect();
    scored.sort_by(|a, b| b.score.cmp(&a.score).then_with(|| a.entry.label.len().cmp(&b.entry.label.len())));
    scored.truncate(limit);
    scored
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(label: &str) -> Entry {
        Entry {
            label: label.into(),
            context: None,
            icon: Icon::Initials(initials(label)),
            target: Target::Guild { guild_id: label.into() },
        }
    }

    #[test]
    fn initials_take_the_first_letter_of_each_word() {
        assert_eq!(initials("Hyper Nova"), "HN");
        assert_eq!(initials("the rust mob"), "TRM");
    }

    #[test]
    fn initials_cap_at_three_letters() {
        assert_eq!(initials("a b c d e"), "ABC");
    }

    #[test]
    fn initials_of_a_single_word_are_one_letter() {
        assert_eq!(initials("general"), "G");
    }

    #[test]
    fn initials_of_a_nameless_guild_are_empty_rather_than_panicking() {
        assert_eq!(initials("   "), "");
    }

    #[test]
    fn initials_handle_a_multibyte_first_character() {
        assert_eq!(initials("\u{e9}quipe rouge"), "\u{c9}R");
    }

    fn best(labels: &[&str], query: &str) -> String {
        let entries: Vec<Entry> = labels.iter().map(|l| entry(l)).collect();
        rank(&entries, query, 10).first().expect("a match").entry.label.clone()
    }

    #[test]
    fn a_subsequence_matches_and_a_non_subsequence_does_not() {
        assert!(score("general", "gen").is_some());
        assert!(score("general", "grl").is_some());
        assert!(score("general", "xyz").is_none());
        assert!(score("gen", "general").is_none());
    }

    #[test]
    fn matching_is_case_insensitive() {
        assert!(score("General", "gen").is_some());
        assert!(score("general", "GEN").is_some());
    }

    #[test]
    fn an_empty_query_matches_everything() {
        assert_eq!(score("anything", "").map(|(s, _)| s), Some(0));
    }

    #[test]
    fn a_prefix_beats_a_match_in_the_middle() {
        assert_eq!(best(&["off-topic", "testing"], "te"), "testing");
    }

    #[test]
    fn a_word_boundary_beats_a_mid_word_match() {
        assert_eq!(best(&["announcements", "bot-commands"], "co"), "bot-commands");
    }

    #[test]
    fn consecutive_characters_beat_scattered_ones() {
        assert_eq!(best(&["general", "goblin-enclave"], "gen"), "general");
    }

    #[test]
    fn a_shorter_label_wins_an_otherwise_equal_match() {
        assert_eq!(best(&["dev-general-chat", "dev"], "dev"), "dev");
    }

    #[test]
    fn hits_point_at_the_matched_characters() {
        let (_, hits) = score("general", "grl").expect("a match");
        assert_eq!(hits, vec![0, 4, 6]);
        let chars: Vec<char> = "general".chars().collect();
        assert_eq!(hits.iter().map(|i| chars[*i]).collect::<String>(), "grl");
    }

    #[test]
    fn ranking_respects_the_limit() {
        let entries: Vec<Entry> = (0..50).map(|i| entry(&format!("channel-{i}"))).collect();
        assert_eq!(rank(&entries, "channel", 8).len(), 8);
    }

    #[test]
    fn non_ascii_labels_do_not_panic_and_still_match() {
        assert!(score("\u{e9}quipe", "eq").is_none(), "accents are distinct characters");
        assert!(score("\u{e9}quipe", "\u{e9}q").is_some());
        assert!(score("\u{1F525}-hot-takes", "hot").is_some());
    }
}
