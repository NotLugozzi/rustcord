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
}

#[derive(Debug, Clone)]
pub enum Span {
    Text { text: String, style: Style },
    Link { label: String, url: String },
    CustomEmoji { name: String, id: String, animated: bool },
    UnicodeEmoji { ch: char },
    Mention { user_id: String },
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
        if let Some((user_id, consumed)) = try_parse_mention(&chars[i..]) {
            flush!();
            spans.push(Span::Mention { user_id });
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
        if chars[i..].starts_with(&['*', '*']) || chars[i..].starts_with(&['_', '_']) {
            let marker = chars[i];
            if let Some(end) = find_matching_pair(&chars, i + 2, marker) {
                flush!();
                let inner: String = chars[i + 2..end].iter().collect();
                render_nested(&inner, Style { bold: true, ..style }, &mut spans);
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
                render_nested(&inner, Style { underline: true, ..style }, &mut spans);
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

fn try_parse_mention(chars: &[char]) -> Option<(String, usize)> {
    let mut j = 0;
    if chars.first() != Some(&'<') {
        return None;
    }
    j += 1;
    if chars.get(j) != Some(&'@') {
        return None;
    }
    j += 1;
    if chars.get(j) == Some(&'!') {
        j += 1;
    }
    let id_start = j;
    while chars.get(j).is_some_and(|c| c.is_ascii_digit()) {
        j += 1;
    }
    if j == id_start || chars.get(j) != Some(&'>') {
        return None;
    }
    let user_id: String = chars[id_start..j].iter().collect();
    Some((user_id, j + 1))
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
}
