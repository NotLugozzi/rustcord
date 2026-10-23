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
        if let Some((label, url, consumed)) = try_parse_link(&chars[i..]) {
            flush!();
            spans.push(Span::Link { label, url });
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
