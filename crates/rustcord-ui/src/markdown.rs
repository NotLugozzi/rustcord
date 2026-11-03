#[derive(Debug, Clone, Copy, Default)]
pub struct Style {
    pub bold: bool,
    pub italic: bool,
    pub strikethrough: bool,
    pub underline: bool,
    pub spoiler: bool,
    pub code: bool,
}

#[derive(Debug, Clone)]
pub enum Block {
    Paragraph(Vec<Span>),
    CodeBlock { language: Option<String>, code: String },
    Blockquote(Vec<Span>),
    Heading { level: u8, spans: Vec<Span> },
    Subtext(Vec<Span>),
}

#[derive(Debug, Clone)]
pub enum Span {
    Text { text: String, style: Style },
    Link { label: String, url: String },
    CustomEmoji { name: String, id: String, animated: bool },
    UnicodeEmoji { ch: char },
    Mention { user_id: String },
    ChannelMention { channel_id: String },
    RoleMention { role_id: String },
    EveryoneMention { here: bool },
    Timestamp { unix: i64, style: TimestampStyle },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimestampStyle {
    ShortTime,
    LongTime,
    ShortDate,
    LongDate,
    ShortDateTime,
    LongDateTime,
    Relative,
}

impl TimestampStyle {
    fn from_flag(flag: Option<char>) -> Option<Self> {
        Some(match flag {
            Some('t') => Self::ShortTime,
            Some('T') => Self::LongTime,
            Some('d') => Self::ShortDate,
            Some('D') => Self::LongDate,
            None | Some('f') => Self::ShortDateTime,
            Some('F') => Self::LongDateTime,
            Some('R') => Self::Relative,
            Some(_) => return None,
        })
    }
}

pub fn parse(input: &str) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut lines = input.lines().peekable();

    while let Some(line) = lines.next() {
        if let Some(rest) = line.strip_prefix("```") {
            let language = {
                let lang = rest.trim();
                if lang.is_empty() { None } else { Some(lang.to_string()) }
            };
            let mut code_lines = Vec::new();
            for code_line in lines.by_ref() {
                if code_line.trim_start().starts_with("```") {
                    break;
                }
                code_lines.push(code_line);
            }
            blocks.push(Block::CodeBlock { language, code: code_lines.join("\n") });
            continue;
        }

        if let Some(rest) = line.strip_prefix('>') {
            let text = rest.strip_prefix(' ').unwrap_or(rest);
            blocks.push(Block::Blockquote(parse_spans(text)));
            continue;
        }

        if let Some(rest) = subtext_body(line) {
            blocks.push(Block::Subtext(parse_spans(rest)));
            continue;
        }

        if let Some(level) = heading_level(line) {
            let text = line[level as usize..].trim_start();
            blocks.push(Block::Heading { level, spans: parse_spans(text) });
            continue;
        }

        if line.trim().is_empty() {
            continue;
        }

        blocks.push(Block::Paragraph(parse_spans(line)));
    }

    blocks
}

fn subtext_body(line: &str) -> Option<&str> {
    let indent = line.len() - line.trim_start_matches(' ').len();
    if indent > 3 {
        return None;
    }
    let rest = line[indent..].strip_prefix("-#")?;
    let body = rest.strip_prefix(' ')?;
    let trimmed = body.trim_start();
    if trimmed.is_empty() {
        return None;
    }
    Some(trimmed)
}

fn heading_level(line: &str) -> Option<u8> {
    let hashes = line.chars().take_while(|c| *c == '#').count();
    if hashes == 0 || hashes > 3 {
        return None;
    }
    if line.as_bytes().get(hashes) != Some(&b' ') {
        return None;
    }
    Some(hashes as u8)
}

pub fn parse_spans(text: &str) -> Vec<Span> {
    let chars: Vec<char> = text.chars().collect();
    let mut spans = Vec::new();
    let mut buf = String::new();
    let style = Style::default();
    let mut i = 0;

    macro_rules! flush {
        () => {
            if !buf.is_empty() {
                spans.push(Span::Text { text: std::mem::take(&mut buf), style });
            }
        };
    }

    while i < chars.len() {
        if let Some((name, id, animated, consumed)) = try_parse_custom_emoji(&chars[i..]) {
            flush!();
            spans.push(Span::CustomEmoji { name, id, animated });
            i += consumed;
            continue;
        }
        if let Some((span, consumed)) = try_parse_tag(&chars[i..]) {
            flush!();
            spans.push(span);
            i += consumed;
            continue;
        }
        if let Some((span, consumed)) = try_parse_everyone(&chars[i..]) {
            flush!();
            spans.push(span);
            i += consumed;
            continue;
        }
        if crate::emoji::is_emoji(chars[i]) {
            flush!();
            spans.push(Span::UnicodeEmoji { ch: chars[i] });
            i += 1;
            continue;
        }
        if let Some((label, url, consumed)) = try_parse_link(&chars[i..]) {
            flush!();
            spans.push(Span::Link { label, url });
            i += consumed;
            continue;
        }
        if let Some((url, consumed)) = try_parse_bare_url(&chars[i..]) {
            flush!();
            spans.push(Span::Link { label: url.clone(), url });
            i += consumed;
            continue;
        }
        if chars[i] == '`' {
            if let Some(end) = find_matching(&chars, i + 1, '`') {
                flush!();
                let code_text: String = chars[i + 1..end].iter().collect();
                spans.push(Span::Text { text: code_text, style: Style { code: true, ..Default::default() } });
                i = end + 1;
                continue;
            }
        }
        if chars[i..].starts_with(&['*', '*']) {
            if let Some(end) = find_matching_pair(&chars, i + 2, '*') {
                flush!();
                let inner: String = chars[i + 2..end].iter().collect();
                render_nested(&inner, Style { bold: true, ..style }, &mut spans);
                i = end + 2;
                continue;
            }
        }
        if chars[i..].starts_with(&['_', '_']) {
            if let Some(end) = find_matching_pair(&chars, i + 2, '_') {
                flush!();
                let inner: String = chars[i + 2..end].iter().collect();
                render_nested(&inner, Style { underline: true, ..style }, &mut spans);
                i = end + 2;
                continue;
            }
        }
        if chars[i..].starts_with(&['~', '~']) {
            if let Some(end) = find_matching_pair(&chars, i + 2, '~') {
                flush!();
                let inner: String = chars[i + 2..end].iter().collect();
                render_nested(&inner, Style { strikethrough: true, ..style }, &mut spans);
                i = end + 2;
                continue;
            }
        }
        if chars[i..].starts_with(&['|', '|']) {
            if let Some(end) = find_matching_pair(&chars, i + 2, '|') {
                flush!();
                let inner: String = chars[i + 2..end].iter().collect();
                render_nested(&inner, Style { spoiler: true, ..style }, &mut spans);
                i = end + 2;
                continue;
            }
        }
        if chars[i] == '_' {
            if let Some(end) = find_matching(&chars, i + 1, '_') {
                flush!();
                let inner: String = chars[i + 1..end].iter().collect();
                render_nested(&inner, Style { italic: true, ..style }, &mut spans);
                i = end + 1;
                continue;
            }
        }
        if chars[i] == '*' {
            if let Some(end) = find_matching(&chars, i + 1, '*') {
                flush!();
                let inner: String = chars[i + 1..end].iter().collect();
                render_nested(&inner, Style { italic: true, ..style }, &mut spans);
                i = end + 1;
                continue;
            }
        }

        buf.push(chars[i]);
        i += 1;
    }
    flush!();
    spans
}

fn render_nested(inner: &str, style: Style, spans: &mut Vec<Span>) {
    if inner.is_empty() {
        return;
    }
    let nested = parse_spans(inner);
    if nested.len() == 1 {
        if let Span::Text { text, style: nested_style } = &nested[0] {
            spans.push(Span::Text {
                text: text.clone(),
                style: merge_style(style, *nested_style),
            });
            return;
        }
    }
    for span in nested {
        match span {
            Span::Text { text, style: nested_style } => {
                spans.push(Span::Text { text, style: merge_style(style, nested_style) });
            }
            other => spans.push(other),
        }
    }
}

fn merge_style(outer: Style, inner: Style) -> Style {
    Style {
        bold: outer.bold || inner.bold,
        italic: outer.italic || inner.italic,
        strikethrough: outer.strikethrough || inner.strikethrough,
        underline: outer.underline || inner.underline,
        spoiler: outer.spoiler || inner.spoiler,
        code: outer.code || inner.code,
    }
}

fn find_matching(chars: &[char], start: usize, marker: char) -> Option<usize> {
    (start..chars.len()).find(|&j| chars[j] == marker)
}

fn find_matching_pair(chars: &[char], start: usize, marker: char) -> Option<usize> {
    let mut j = start;
    while j + 1 < chars.len() {
        if chars[j] == marker && chars[j + 1] == marker {
            return Some(j);
        }
        j += 1;
    }
    None
}

fn try_parse_tag(chars: &[char]) -> Option<(Span, usize)> {
    if chars.first() != Some(&'<') {
        return None;
    }
    let mut j = 1;
    let span_kind = match chars.get(j)? {
        '#' => {
            j += 1;
            TagKind::Channel
        }
        '@' => {
            j += 1;
            match chars.get(j) {
                Some('&') => {
                    j += 1;
                    TagKind::Role
                }
                Some('!') => {
                    j += 1;
                    TagKind::User
                }
                _ => TagKind::User,
            }
        }
        't' if chars.get(j + 1) == Some(&':') => {
            j += 2;
            TagKind::Timestamp
        }
        _ => return None,
    };

    let digits_start = j;
    if span_kind == TagKind::Timestamp && chars.get(j) == Some(&'-') {
        j += 1;
    }
    let num_start = j;
    while chars.get(j).is_some_and(|c| c.is_ascii_digit()) {
        j += 1;
    }
    if j == num_start {
        return None;
    }
    let digits: String = chars[digits_start..j].iter().collect();

    if span_kind == TagKind::Timestamp {
        let flag = match chars.get(j) {
            Some('>') => None,
            Some(':') => {
                let f = *chars.get(j + 1)?;
                j += 2;
                Some(f)
            }
            _ => return None,
        };
        if chars.get(j) != Some(&'>') {
            return None;
        }
        let unix = digits.parse::<i64>().ok()?;
        let style = TimestampStyle::from_flag(flag)?;
        return Some((Span::Timestamp { unix, style }, j + 1));
    }

    if chars.get(j) != Some(&'>') {
        return None;
    }
    let span = match span_kind {
        TagKind::User => Span::Mention { user_id: digits },
        TagKind::Channel => Span::ChannelMention { channel_id: digits },
        TagKind::Role => Span::RoleMention { role_id: digits },
        TagKind::Timestamp => unreachable!("handled above"),
    };
    Some((span, j + 1))
}

#[derive(PartialEq, Eq)]
enum TagKind {
    User,
    Channel,
    Role,
    Timestamp,
}

fn try_parse_everyone(chars: &[char]) -> Option<(Span, usize)> {
    if chars.first() != Some(&'@') {
        return None;
    }
    for (word, here) in [("everyone", false), ("here", true)] {
        let len = word.len() + 1;
        if chars.len() >= len && chars[1..len].iter().copied().eq(word.chars()) {
            if chars.get(len).is_some_and(|c| c.is_alphanumeric() || *c == '_') {
                continue;
            }
            return Some((Span::EveryoneMention { here }, len));
        }
    }
    None
}

fn try_parse_custom_emoji(chars: &[char]) -> Option<(String, String, bool, usize)> {
    let mut j = 0;
    if chars.first() != Some(&'<') {
        return None;
    }
    j += 1;
    let animated = chars.get(j) == Some(&'a');
    if animated {
        j += 1;
    }
    if chars.get(j) != Some(&':') {
        return None;
    }
    j += 1;
    let name_start = j;
    while chars.get(j).is_some_and(|c| *c != ':') {
        j += 1;
    }
    if j == name_start || chars.get(j) != Some(&':') {
        return None;
    }
    let name: String = chars[name_start..j].iter().collect();
    j += 1;
    let id_start = j;
    while chars.get(j).is_some_and(|c| c.is_ascii_digit()) {
        j += 1;
    }
    if j == id_start || chars.get(j) != Some(&'>') {
        return None;
    }
    let id: String = chars[id_start..j].iter().collect();
    Some((name, id, animated, j + 1))
}

fn try_parse_link(chars: &[char]) -> Option<(String, String, usize)> {
    if chars.first() != Some(&'[') {
        return None;
    }
    let close_bracket = (1..chars.len()).find(|&j| chars[j] == ']')?;
    if chars.get(close_bracket + 1) != Some(&'(') {
        return None;
    }
    let close_paren = (close_bracket + 2..chars.len()).find(|&j| chars[j] == ')')?;
    let label: String = chars[1..close_bracket].iter().collect();
    let url: String = chars[close_bracket + 2..close_paren].iter().collect();
    Some((label, url, close_paren + 1))
}

const URL_SCHEMES: [&[char]; 2] = [&['h', 't', 't', 'p', ':', '/', '/'], &['h', 't', 't', 'p', 's', ':', '/', '/']];

fn try_parse_bare_url(chars: &[char]) -> Option<(String, usize)> {
    let suppressed = chars.first() == Some(&'<');
    let start = if suppressed { 1 } else { 0 };
    if !URL_SCHEMES.iter().any(|scheme| chars[start..].starts_with(scheme)) {
        return None;
    }

    let mut end = start;
    let mut depth = 0i32;
    while let Some(&c) = chars.get(end) {
        if c.is_whitespace() || c == '<' {
            break;
        }
        if suppressed && c == '>' {
            break;
        }
        if c == '(' {
            depth += 1;
        } else if c == ')' {
            if depth == 0 {
                break;
            }
            depth -= 1;
        }
        end += 1;
    }

    while end > start && matches!(chars[end - 1], '.' | ',' | ':' | ';' | '!' | '?' | '\'' | '"') {
        end -= 1;
    }

    if chars[start..end].iter().filter(|c| **c == '/').count() < 3 {
        return None;
    }

    let url: String = chars[start..end].iter().collect();
    let consumed = if suppressed && chars.get(end) == Some(&'>') { end + 1 } else { end };
    Some((url, consumed))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn links(text: &str) -> Vec<String> {
        parse_spans(text)
            .into_iter()
            .filter_map(|span| match span {
                Span::Link { url, .. } => Some(url),
                _ => None,
            })
            .collect()
    }

    fn text_of(text: &str) -> String {
        parse_spans(text)
            .into_iter()
            .map(|span| match span {
                Span::Text { text, .. } => text,
                Span::Link { label, .. } => label,
                _ => String::new(),
            })
            .collect()
    }

    fn blocks_of(text: &str) -> Vec<Block> {
        parse(text)
    }

    fn subtext_text(text: &str) -> Option<String> {
        blocks_of(text).into_iter().find_map(|b| match b {
            Block::Subtext(spans) => Some(
                spans
                    .into_iter()
                    .map(|s| match s {
                        Span::Text { text, .. } => text,
                        Span::Link { label, .. } => label,
                        _ => String::new(),
                    })
                    .collect(),
            ),
            _ => None,
        })
    }

    #[test]
    fn dash_hash_marks_a_line_as_subtext() {
        assert_eq!(subtext_text("-# small print").as_deref(), Some("small print"));
    }

    #[test]
    fn subtext_requires_a_space_after_the_marker() {
        assert!(subtext_text("-#nospace").is_none());
    }

    #[test]
    fn an_empty_subtext_marker_is_not_subtext() {
        assert!(subtext_text("-# ").is_none());
        assert!(subtext_text("-#").is_none());
    }

    #[test]
    fn a_list_item_starting_with_a_heading_is_not_subtext() {
        assert!(subtext_text("- # heading in a list").is_none());
    }

    #[test]
    fn subtext_still_parses_inline_spans() {
        let spans = blocks_of("-# see https://example.com/x")
            .into_iter()
            .find_map(|b| match b {
                Block::Subtext(spans) => Some(spans),
                _ => None,
            })
            .expect("subtext block");
        assert!(spans.iter().any(|s| matches!(s, Span::Link { .. })));
    }

    #[test]
    fn a_small_indent_before_the_marker_is_allowed() {
        assert_eq!(subtext_text("   -# indented").as_deref(), Some("indented"));
    }

    #[test]
    fn a_deep_indent_is_not_subtext() {
        assert!(subtext_text("     -# too far").is_none());
    }

    #[test]
    fn the_marker_only_applies_at_the_start_of_a_line() {
        assert!(subtext_text("so it's like `-# this is a tiny message`").is_none());
        assert!(subtext_text("text then -# not a marker").is_none());
    }

    #[test]
    fn only_the_marked_line_becomes_subtext() {
        let blocks = blocks_of("normal line
-# tiny line
normal again");
        assert!(matches!(blocks[0], Block::Paragraph(_)));
        assert!(matches!(blocks[1], Block::Subtext(_)));
        assert!(matches!(blocks[2], Block::Paragraph(_)));
    }

    #[test]
    fn bare_url_becomes_a_link() {
        assert_eq!(links("see https://example.com/x now"), ["https://example.com/x"]);
    }

    #[test]
    fn trailing_sentence_punctuation_is_not_part_of_the_url() {
        assert_eq!(links("go to https://example.com/page."), ["https://example.com/page"]);
        assert_eq!(text_of("go to https://example.com/page."), "go to https://example.com/page.");
    }

    #[test]
    fn wrapping_paren_is_not_swallowed_but_inner_ones_are() {
        assert_eq!(links("(https://example.com/a)"), ["https://example.com/a"]);
        assert_eq!(links("https://en.wikipedia.org/wiki/Rust_(language)"), ["https://en.wikipedia.org/wiki/Rust_(language)"]);
    }

    #[test]
    fn angle_brackets_suppress_the_embed_without_entering_the_url() {
        assert_eq!(links("<https://example.com/a>"), ["https://example.com/a"]);
        assert_eq!(text_of("<https://example.com/a>"), "https://example.com/a");
    }

    #[test]
    fn explicit_markdown_link_still_wins() {
        assert_eq!(links("[label](https://example.com/a)"), ["https://example.com/a"]);
    }

    #[test]
    fn scheme_without_a_host_is_not_a_link() {
        assert!(links("https://").is_empty());
    }

    #[test]
    fn single_underscore_is_italic_not_underline() {
        let spans = parse_spans("_hi_");
        let Span::Text { style, .. } = &spans[0] else { panic!("{spans:?}") };
        assert!(style.italic, "expected italic, got {style:?}");
        assert!(!style.underline);
    }

    #[test]
    fn double_underscore_is_underline_not_bold() {
        let spans = parse_spans("__hi__");
        let Span::Text { style, .. } = &spans[0] else { panic!("{spans:?}") };
        assert!(style.underline, "expected underline, got {style:?}");
        assert!(!style.bold);
    }

    #[test]
    fn asterisk_emphasis_is_unchanged() {
        let one = parse_spans("*hi*");
        let Span::Text { style, .. } = &one[0] else { panic!() };
        assert!(style.italic && !style.underline);
        let two = parse_spans("**hi**");
        let Span::Text { style, .. } = &two[0] else { panic!() };
        assert!(style.bold && !style.underline);
    }

    #[test]
    fn mixed_underscore_and_asterisk_nest() {
        let spans = parse_spans("__*x*__");
        let Span::Text { style, .. } = &spans[0] else { panic!("{spans:?}") };
        assert!(style.underline && style.italic);
    }

    #[test]
    fn parses_user_mention_both_forms() {
        for src in ["<@123>", "<@!123>"] {
            let spans = parse_spans(src);
            assert!(matches!(&spans[0], Span::Mention { user_id } if user_id == "123"), "{src}: {spans:?}");
        }
    }

    #[test]
    fn parses_channel_and_role_mentions() {
        let spans = parse_spans("<#456>");
        assert!(matches!(&spans[0], Span::ChannelMention { channel_id } if channel_id == "456"), "{spans:?}");
        let spans = parse_spans("<@&789>");
        assert!(matches!(&spans[0], Span::RoleMention { role_id } if role_id == "789"), "{spans:?}");
    }

    #[test]
    fn role_mention_is_not_read_as_a_user() {
        let spans = parse_spans("<@&789>");
        assert!(!matches!(&spans[0], Span::Mention { .. }), "{spans:?}");
    }

    #[test]
    fn parses_everyone_and_here() {
        assert!(matches!(&parse_spans("@everyone")[0], Span::EveryoneMention { here: false }));
        assert!(matches!(&parse_spans("@here")[0], Span::EveryoneMention { here: true }));
    }

    #[test]
    fn everyone_requires_a_word_boundary() {
        for src in ["@everyonewins", "@here2", "@everyone_x"] {
            let spans = parse_spans(src);
            assert!(
                !spans.iter().any(|s| matches!(s, Span::EveryoneMention { .. })),
                "{src} should not be a mention: {spans:?}"
            );
        }
        let spans = parse_spans("@everyone!");
        assert!(matches!(&spans[0], Span::EveryoneMention { here: false }), "{spans:?}");
    }

    #[test]
    fn parses_timestamp_styles() {
        use TimestampStyle::*;
        for (src, want) in [
            ("<t:1730000000>", ShortDateTime),
            ("<t:1730000000:f>", ShortDateTime),
            ("<t:1730000000:t>", ShortTime),
            ("<t:1730000000:T>", LongTime),
            ("<t:1730000000:d>", ShortDate),
            ("<t:1730000000:D>", LongDate),
            ("<t:1730000000:F>", LongDateTime),
            ("<t:1730000000:R>", Relative),
        ] {
            let spans = parse_spans(src);
            let Span::Timestamp { unix, style } = &spans[0] else { panic!("{src}: {spans:?}") };
            assert_eq!(*unix, 1730000000, "{src}");
            assert_eq!(*style, want, "{src}");
        }
    }

    #[test]
    fn timestamp_accepts_negative_and_rejects_bad_style() {
        let spans = parse_spans("<t:-86400:R>");
        assert!(matches!(&spans[0], Span::Timestamp { unix: -86400, .. }), "{spans:?}");
        let spans = parse_spans("<t:1730000000:Z>");
        assert!(!spans.iter().any(|s| matches!(s, Span::Timestamp { .. })), "{spans:?}");
    }

    #[test]
    fn malformed_tags_stay_literal() {
        for src in ["<@>", "<#>", "<@&>", "<@abc>", "<t:>", "<t:abc>", "<@123", "<#123"] {
            let spans = parse_spans(src);
            let is_tag = spans.iter().any(|s| {
                matches!(
                    s,
                    Span::Mention { .. } | Span::ChannelMention { .. } | Span::RoleMention { .. } | Span::Timestamp { .. }
                )
            });
            assert!(!is_tag, "{src} should stay literal: {spans:?}");
        }
    }

    #[test]
    fn custom_emoji_still_wins_over_tag_parser() {
        let spans = parse_spans("<:smile:12345>");
        assert!(matches!(&spans[0], Span::CustomEmoji { .. }), "{spans:?}");
    }

    #[test]
    fn mentions_parse_inline_with_surrounding_text() {
        let spans = parse_spans("hey <@1> look at <#2> re <@&3>");
        let kinds: Vec<&str> = spans
            .iter()
            .map(|s| match s {
                Span::Text { .. } => "text",
                Span::Mention { .. } => "user",
                Span::ChannelMention { .. } => "channel",
                Span::RoleMention { .. } => "role",
                _ => "other",
            })
            .collect();
        assert_eq!(kinds, ["text", "user", "text", "channel", "text", "role"], "{spans:?}");
    }
}
