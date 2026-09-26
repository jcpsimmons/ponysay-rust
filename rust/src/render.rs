//! Native implementation of ponysay's pony and balloon file formats.
//!
//! Messages are always data: neither dollars nor backslashes in a message or
//! variable value are interpreted as pony macros, Python, or shell escapes.

use std::collections::HashMap;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

const RESET: &str = "\x1b[0m";
const MAX_DIMENSION: usize = 16_384;

#[derive(Clone, Debug)]
pub struct RenderOptions {
    /// Total balloon width, including its borders. None disables wrapping.
    pub wrap: Option<usize>,
    pub think: bool,
    pub pony_only: bool,
    /// Clip final output to this many terminal columns.
    pub width: Option<usize>,
    /// Raw contents of a .say or .think balloon file.
    pub balloon: Option<String>,
    pub compact: bool,
}

impl Default for RenderOptions {
    fn default() -> Self {
        Self {
            wrap: Some(65),
            think: false,
            pony_only: false,
            width: None,
            balloon: None,
            compact: false,
        }
    }
}

/// Render a pony template, preserving its original ANSI artwork.
pub fn render(pony: &str, message: &str, options: &RenderOptions) -> Result<String, String> {
    if options.wrap.is_some_and(|w| w > MAX_DIMENSION) {
        return Err(format!("wrap width exceeds {MAX_DIMENSION} columns"));
    }
    let pony = pony.replace("\r\n", "\n");
    let (body, trim_top, trim_bottom) = strip_metadata(&pony)?;
    let style = match options.balloon.as_deref() {
        Some(raw) => BalloonStyle::parse(raw)?,
        None => BalloonStyle::default_style(options.think),
    };
    let message = prepare_message(message, options.compact);
    let parts = parse_template(body, &style, options.pony_only)?;
    let mut lines = vec![String::new()];
    let mut line_states = vec![AnsiState::default()];
    let mut state = AnsiState::default();
    let mut source_row = 0;
    let mut overlays: Vec<(usize, usize, String)> = Vec::new();

    for part in parts {
        match part {
            Part::Text(text) => {
                for token in Tokens::new(&text) {
                    match token {
                        Token::Escape(code) => {
                            lines.last_mut().unwrap().push_str(code);
                            state.feed(code);
                        }
                        Token::Glyph("\n", _) => {
                            source_row += 1;
                            lines.push(String::new());
                            line_states.push(state.clone());
                        }
                        Token::Glyph("\t", _) => {
                            let column = visible_width(lines.last().unwrap());
                            lines
                                .last_mut()
                                .unwrap()
                                .push_str(&" ".repeat(8 - column % 8));
                        }
                        Token::Glyph(text, _) => lines.last_mut().unwrap().push_str(text),
                    }
                }
            }
            Part::Link(link) => {
                lines.last_mut().unwrap().push_str(RESET);
                lines.last_mut().unwrap().push_str(&link);
                lines.last_mut().unwrap().push_str(&state.restore());
            }
            Part::Balloon(spec) => {
                if options.pony_only {
                    continue;
                }
                let column = visible_width(lines.last().unwrap());
                let balloon = style.make(&message, &spec, column, options.wrap)?;
                let start_row = lines.len() - 1;
                let restore = state.restore();
                lines.last_mut().unwrap().push_str(RESET);
                lines.last_mut().unwrap().push_str(&balloon[0]);
                lines.last_mut().unwrap().push_str(&restore);
                if source_row == 0 {
                    // A top balloon inserts rows, rather than overwriting the art.
                    for row in balloon.into_iter().skip(1) {
                        lines.push(format!("{}{RESET}{row}{restore}", " ".repeat(column)));
                        line_states.push(state.clone());
                    }
                } else {
                    // Interior and bottom balloons replace cells on later rows.
                    for (offset, row) in balloon.into_iter().enumerate().skip(1) {
                        overlays.push((start_row + offset, column, row));
                    }
                }
            }
        }
    }

    for (row, column, balloon) in overlays {
        while lines.len() <= row {
            lines.push(String::new());
            line_states.push(state.clone());
        }
        lines[row] = overlay(&lines[row], column, &balloon, &line_states[row]);
    }

    // split('\n') includes the final empty sentinel; metadata counts real rows.
    let trailing_newline = lines.last().is_some_and(String::is_empty);
    if trailing_newline {
        lines.pop();
    }
    if options.pony_only {
        let end = lines.len().saturating_sub(trim_bottom);
        let start = trim_top.min(end);
        lines = lines[start..end].to_vec();
    }
    if let Some(width) = options.width {
        for line in &mut lines {
            *line = clip(line, width);
        }
    }
    let mut output = lines.join("\n");
    if trailing_newline && !lines.is_empty() {
        output.push('\n');
    }
    Ok(output)
}

fn strip_metadata(pony: &str) -> Result<(&str, usize, usize), String> {
    let Some(rest) = pony.strip_prefix("$$$\n") else {
        return Ok((pony, 0, 0));
    };
    let (metadata, body) = if let Some(body) = rest.strip_prefix("$$$\n") {
        ("", body)
    } else if let Some((metadata, body)) = rest.split_once("\n$$$\n") {
        (metadata, body)
    } else if let Some(metadata) = rest.strip_suffix("\n$$$") {
        (metadata, "")
    } else {
        return Err("unterminated pony metadata: expected a closing $$$ line".into());
    };
    let mut top = 0;
    let mut bottom = 0;
    for line in metadata.lines() {
        if let Some((key, value)) = line.split_once(':') {
            if matches!(key.trim(), "BALLOON TOP" | "BALLOON BOTTOM") && !value.trim().is_empty() {
                let count = value
                    .trim()
                    .parse::<usize>()
                    .map_err(|_| format!("invalid {} metadata: {}", key.trim(), value.trim()))?;
                if key.trim() == "BALLOON TOP" {
                    top = count;
                } else {
                    bottom = count;
                }
            }
        }
    }
    Ok((body, top, bottom))
}

enum Part {
    Text(String),
    Link(String),
    Balloon(BalloonSpec),
}

fn parse_template(body: &str, style: &BalloonStyle, pony_only: bool) -> Result<Vec<Part>, String> {
    let mut parts = Vec::new();
    let mut variables = HashMap::<String, String>::new();
    let mut offset = 0;
    while offset < body.len() {
        let Some(start) = body[offset..].find('$').map(|n| n + offset) else {
            parts.push(Part::Text(body[offset..].to_string()));
            break;
        };
        if start > offset {
            parts.push(Part::Text(body[offset..start].to_string()));
        }
        let end = body[start + 1..]
            .find('$')
            .map(|n| n + start + 1)
            .ok_or_else(|| {
                format!(
                    "unterminated pony macro on line {}",
                    body[..start].bytes().filter(|b| *b == b'\n').count() + 1
                )
            })?;
        let name = &body[start + 1..end];
        if let Some((key, value)) = name.split_once('=') {
            if key.is_empty() || key.contains('\n') {
                return Err("pony variable definitions need a nonempty, single-line name".into());
            }
            variables.insert(key.to_owned(), value.to_owned());
        } else if name.is_empty() {
            parts.push(Part::Text("$".into()));
        } else if let Some(value) = variables.get(name) {
            parts.push(Part::Text(value.clone()));
        } else if let Some(props) = name.strip_prefix("balloon") {
            parts.push(Part::Balloon(BalloonSpec::parse(props)?));
        } else if matches!(name, "\\" | "/" | "X") {
            if pony_only {
                parts.push(Part::Text(" ".into()));
            } else {
                parts.push(Part::Link(style.one(name).to_string()));
            }
        } else {
            return Err(format!("unknown pony macro ${name}$"));
        }
        offset = end + 1;
    }
    Ok(parts)
}

#[derive(Default)]
struct BalloonSpec {
    width: usize,
    height: usize,
    inner_left: usize,
    justify: Option<char>,
}

impl BalloonSpec {
    fn parse(raw: &str) -> Result<Self, String> {
        let dimension = |raw: &str| -> Result<usize, String> {
            if raw.is_empty() {
                return Ok(0);
            }
            let value = raw
                .parse::<usize>()
                .map_err(|_| format!("invalid balloon dimensions: {raw}"))?;
            if value > MAX_DIMENSION {
                return Err(format!("balloon dimension exceeds {MAX_DIMENSION}: {raw}"));
            }
            Ok(value)
        };
        let (width, height) = raw.split_once(',').unwrap_or((raw, ""));
        let mut spec = Self {
            height: dimension(height)?,
            ..Self::default()
        };
        if let Some((pos, justify)) = width
            .char_indices()
            .find(|(_, c)| matches!(c, 'l' | 'c' | 'r'))
        {
            spec.inner_left = dimension(&width[..pos])?;
            let right = dimension(&width[pos + 1..])?;
            spec.width = right.checked_sub(spec.inner_left).ok_or_else(|| {
                "balloon's right bound is smaller than its left bound".to_string()
            })?;
            spec.justify = Some(justify);
        } else {
            spec.width = dimension(width)?;
        }
        Ok(spec)
    }
}

struct BalloonStyle {
    fields: HashMap<String, Vec<String>>,
    min_width: usize,
}

impl BalloonStyle {
    fn default_style(think: bool) -> Self {
        // Same default geometry as upstream's Balloon.fromFile(None, ...).
        let raw = if think {
            "/:o\n\\:o\nX:o\nn:_\ns:-\nw:( \ne: )\nww:( \nee: )\nnw: _\nne:_ \nsw: -\nse:- \nnnw:_\nnne:_\nssw:-\nsse:-\nnww:( \nnee: )\nsww:( \nsee: )\n"
        } else {
            include_str!("../../balloons/cowsay.say")
        };
        Self::parse(raw).expect("bundled default balloon is valid")
    }

    fn parse(raw: &str) -> Result<Self, String> {
        let keys = [
            "\\", "/", "X", "ww", "ee", "nw", "nnw", "n", "nne", "ne", "nee", "e", "see", "se",
            "sse", "s", "ssw", "sw", "sww", "w", "nww",
        ];
        let mut fields: HashMap<String, Vec<String>> = HashMap::new();
        let mut last = None::<String>;
        for (index, line) in raw.lines().enumerate() {
            let line = line.strip_suffix('\r').unwrap_or(line);
            if line.is_empty() {
                continue;
            }
            let (key, value) = line.split_once(':').ok_or_else(|| {
                format!(
                    "invalid balloon style line {}: expected key:value",
                    index + 1
                )
            })?;
            let key = if key.is_empty() {
                last.as_deref().ok_or_else(|| {
                    format!("balloon continuation without a key on line {}", index + 1)
                })?
            } else {
                if !keys.contains(&key) {
                    return Err(format!("unknown balloon style key: {key}"));
                }
                key
            };
            fields
                .entry(key.to_owned())
                .or_default()
                .push(value.to_owned());
            last = Some(key.to_owned());
        }
        for key in keys {
            if !fields.contains_key(key) {
                return Err(format!("balloon style is missing {key}:"));
            }
        }
        for group in [
            ["nw", "nnw", "n", "nne", "ne"],
            ["sw", "ssw", "s", "sse", "se"],
        ] {
            let count = fields[group[2]].len();
            if group.iter().any(|key| fields[*key].len() != count) {
                return Err(format!(
                    "balloon {} border parts have inconsistent row counts",
                    group[2]
                ));
            }
        }
        for key in [
            "\\", "/", "X", "ww", "ee", "nee", "e", "see", "sww", "w", "nww",
        ] {
            if fields[key].len() != 1 {
                return Err(format!("balloon {key}: must have exactly one row"));
            }
        }
        let edge_width = |keys: &[&str]| {
            keys.iter()
                .flat_map(|key| fields[*key].iter())
                .map(|s| visible_width(s))
                .max()
                .unwrap_or(0)
        };
        let min_width = edge_width(&["nw", "nww", "w", "sww", "sw", "ww"])
            + edge_width(&["ne", "nee", "e", "see", "se", "ee"]);
        if min_width > MAX_DIMENSION {
            return Err("balloon style is too wide".into());
        }
        Ok(Self { fields, min_width })
    }

    fn one(&self, key: &str) -> &str {
        &self.fields[key][0]
    }

    fn make(
        &self,
        message: &str,
        spec: &BalloonSpec,
        left: usize,
        wrap: Option<usize>,
    ) -> Result<Vec<String>, String> {
        let wrap = wrap.map(|w| w.saturating_sub(self.min_width).saturating_sub(left).max(8));
        let mut messages = wrap_message(message, wrap);
        let top_height = self.fields["n"].len();
        let bottom_height = self.fields["s"].len();
        while messages.len() + top_height + bottom_height < spec.height {
            messages.push(String::new());
        }
        let message_width = messages.iter().map(|s| visible_width(s)).max().unwrap_or(0);
        let width = spec.width.max(message_width.saturating_add(self.min_width));
        if width > MAX_DIMENSION {
            return Err(format!("rendered balloon exceeds {MAX_DIMENSION} columns"));
        }
        let mut extra_left = spec.inner_left as isize;
        if let Some(justify) = spec.justify {
            let actual = message_width + self.min_width;
            if actual > spec.width {
                match justify {
                    'r' => extra_left -= (actual - spec.width) as isize,
                    'c' => extra_left -= ((actual - spec.width) / 2) as isize,
                    _ => {}
                }
                extra_left = extra_left.max(0);
                if justify != 'r' {
                    if let Some(wrap) = wrap {
                        if extra_left as usize + actual > wrap {
                            extra_left -= actual.saturating_sub(wrap) as isize;
                        }
                    }
                }
            }
        }
        let prefix = " ".repeat(extra_left.max(0) as usize);
        let mut output = Vec::with_capacity(top_height + messages.len() + bottom_height);
        self.border(&mut output, ["nw", "nnw", "n", "nne", "ne"], width, &prefix);
        let count = messages.len();
        let mut message_state = AnsiState::default();
        for (row, message) in messages.iter().enumerate() {
            let (west, east) = if count == 1 {
                ("ww", "ee")
            } else if row == 0 {
                ("nww", "nee")
            } else if row == count - 1 {
                ("sww", "see")
            } else {
                ("w", "e")
            };
            let padding = width.saturating_sub(
                visible_width(message)
                    + visible_width(self.one(west))
                    + visible_width(self.one(east)),
            );
            let begin = message_state.restore();
            for token in Tokens::new(message) {
                if let Token::Escape(code) = token {
                    message_state.feed(code);
                }
            }
            output.push(format!(
                "{prefix}{}{begin}{message}{RESET}{}{}",
                self.one(west),
                " ".repeat(padding),
                self.one(east)
            ));
        }
        self.border(&mut output, ["sw", "ssw", "s", "sse", "se"], width, &prefix);
        Ok(output)
    }

    fn border(&self, output: &mut Vec<String>, keys: [&str; 5], width: usize, prefix: &str) {
        for row in 0..self.fields[keys[2]].len() {
            let fields: Vec<&str> = keys
                .iter()
                .map(|key| self.fields[*key][row].as_str())
                .collect();
            let outer = visible_width(fields[0]) + visible_width(fields[4]);
            let inner = visible_width(fields[1]) + visible_width(fields[3]);
            if outer + inner <= width {
                output.push(format!(
                    "{prefix}{}{}{}{}{}",
                    fields[0],
                    fields[1],
                    fill_columns(fields[2], width - outer - inner),
                    fields[3],
                    fields[4]
                ));
            } else {
                output.push(format!(
                    "{prefix}{}{}{}",
                    fields[0],
                    fill_columns(fields[2], width.saturating_sub(outer)),
                    fields[4]
                ));
            }
        }
    }
}

fn fill_columns(pattern: &str, width: usize) -> String {
    let size = visible_width(pattern);
    if size == 0 {
        return " ".repeat(width);
    }
    format!(
        "{}{}",
        pattern.repeat(width / size),
        " ".repeat(width % size)
    )
}

fn prepare_message(message: &str, compact: bool) -> String {
    let mut result = String::with_capacity(message.len());
    let mut column = 0;
    for token in Tokens::new(message) {
        match token {
            Token::Escape(code) => result.push_str(code),
            Token::Glyph("\n", _) => {
                result.push('\n');
                column = 0;
            }
            Token::Glyph("\t", _) => {
                let spaces = 8 - column % 8;
                result.push_str(&" ".repeat(spaces));
                column += spaces;
            }
            Token::Glyph(text, width) => {
                result.push_str(text);
                column += width;
            }
        }
    }
    let lines: Vec<&str> = result.split('\n').collect();
    let indent = lines
        .iter()
        .map(|line| line.bytes().take_while(|b| *b == b' ').count())
        .min()
        .unwrap_or(0);
    let result = lines
        .iter()
        .map(|line| line[indent..].trim_end_matches(' '))
        .collect::<Vec<_>>()
        .join("\n");
    if !compact {
        return result;
    }
    // Preserve paragraph breaks, while folding ordinary line breaks and spaces.
    let mut paragraphs = Vec::new();
    let mut paragraph = Vec::new();
    for line in result.lines() {
        if line.trim().is_empty() {
            if !paragraph.is_empty() {
                paragraphs.push(paragraph.join(" "));
                paragraph.clear();
            }
        } else {
            paragraph.extend(line.split_whitespace());
        }
    }
    if !paragraph.is_empty() {
        paragraphs.push(paragraph.join(" "));
    }
    paragraphs.join("\n\n")
}

fn wrap_message(message: &str, wrap: Option<usize>) -> Vec<String> {
    let Some(limit) = wrap else {
        return message.split('\n').map(str::to_owned).collect();
    };
    let mut output = Vec::new();
    for source in message.split('\n') {
        let tokens: Vec<Token<'_>> = Tokens::new(source).collect();
        let indent: usize = tokens
            .iter()
            .take_while(|t| matches!(t, Token::Glyph(" ", _)))
            .count()
            .min(limit.saturating_sub(1));
        let mut row = String::new();
        let mut column = 0;
        let mut index = 0;
        let mut pending_spaces = String::new();
        while index < tokens.len() {
            if matches!(tokens[index], Token::Glyph(" ", _)) {
                pending_spaces.push(' ');
                index += 1;
                continue;
            }
            let begin = index;
            while index < tokens.len() && !matches!(tokens[index], Token::Glyph(" ", _)) {
                index += 1;
            }
            let word = &tokens[begin..index];
            let word_width: usize = word.iter().map(|t| t.width()).sum();
            if column > indent && column + pending_spaces.len() + word_width > limit {
                output.push(row);
                row = " ".repeat(indent);
                column = indent;
                pending_spaces.clear();
            }
            if column + pending_spaces.len() < limit {
                row.push_str(&pending_spaces);
                column += pending_spaces.len();
            }
            pending_spaces.clear();
            let mut position = 0;
            let mut remaining_width = word_width;
            while position < word.len() {
                let token = &word[position];
                if token.width() > 0 && column + remaining_width > limit && column > indent {
                    let room = limit.saturating_sub(column);
                    if room <= token.width() {
                        if room > 0 {
                            row.push('-');
                        }
                        output.push(row);
                        row = " ".repeat(indent);
                        column = indent;
                        continue;
                    }
                }
                // Soft hyphens are discretionary break points, not printed glyphs.
                if token.text() != "\u{00ad}" {
                    row.push_str(token.text());
                    column += token.width();
                }
                remaining_width = remaining_width.saturating_sub(token.width());
                position += 1;
            }
        }
        output.push(row.trim_end_matches(' ').to_string());
    }
    output
}

/// Terminal width of text, ignoring ANSI CSI/OSC sequences and combining marks.
pub fn visible_width(text: &str) -> usize {
    if !text.contains('\x1b') {
        return UnicodeWidthStr::width(text);
    }
    let mut plain = String::with_capacity(text.len());
    for token in Tokens::new(text) {
        if let Token::Glyph(text, _) = token {
            plain.push_str(text);
        }
    }
    UnicodeWidthStr::width(plain.as_str())
}

#[derive(Clone, Default)]
struct AnsiState {
    codes: Vec<String>,
}

impl AnsiState {
    fn feed(&mut self, code: &str) {
        if let Some(params) = code.strip_prefix("\x1b[").and_then(|p| p.strip_suffix('m')) {
            let params: Vec<&str> = params.split(';').collect();
            let mut i = 0;
            while i < params.len() {
                let p = params[i].parse::<u16>().unwrap_or(0);
                if p == 0 {
                    self.codes.clear();
                }
                if matches!(p, 38 | 48 | 58) && i + 1 < params.len() {
                    i += match params[i + 1] {
                        "2" => 4,
                        "5" => 2,
                        _ => 0,
                    };
                }
                i += 1;
            }
            self.codes.push(code.to_owned());
        }
    }
    fn restore(&self) -> String {
        format!("{RESET}{}", self.codes.concat())
    }
}

#[derive(Clone, Copy)]
enum Token<'a> {
    Escape(&'a str),
    Glyph(&'a str, usize),
}
impl<'a> Token<'a> {
    fn text(&self) -> &'a str {
        match self {
            Self::Escape(t) | Self::Glyph(t, _) => t,
        }
    }
    fn width(&self) -> usize {
        match self {
            Self::Escape(_) => 0,
            Self::Glyph(_, w) => *w,
        }
    }
}
struct Tokens<'a> {
    text: &'a str,
    offset: usize,
}
impl<'a> Tokens<'a> {
    fn new(text: &'a str) -> Self {
        Self { text, offset: 0 }
    }
}
impl<'a> Iterator for Tokens<'a> {
    type Item = Token<'a>;
    fn next(&mut self) -> Option<Self::Item> {
        if self.offset >= self.text.len() {
            return None;
        }
        let start = self.offset;
        let bytes = self.text.as_bytes();
        if bytes[start] == 0x1b {
            let mut end = start + 1;
            if end < bytes.len() {
                match bytes[end] {
                    b'[' => {
                        end += 1;
                        while end < bytes.len() {
                            let b = bytes[end];
                            end += 1;
                            if (0x40..=0x7e).contains(&b) {
                                break;
                            }
                        }
                    }
                    b']' | b'P' | b'_' | b'^' => {
                        let osc = bytes[end] == b']';
                        end += 1;
                        // Linux virtual-terminal palette escapes are not terminated.
                        if osc && bytes.get(end) == Some(&b'P') {
                            end = (end + 8).min(bytes.len());
                        } else if osc && bytes.get(end) == Some(&b'R') {
                            end += 1;
                        } else {
                            while end < bytes.len() {
                                if bytes[end] == 7 {
                                    end += 1;
                                    break;
                                }
                                if bytes[end] == 0x1b && bytes.get(end + 1) == Some(&b'\\') {
                                    end += 2;
                                    break;
                                }
                                end += 1;
                            }
                        }
                    }
                    _ => end += self.text[end..].chars().next().unwrap().len_utf8(),
                }
            }
            while end < bytes.len() && !self.text.is_char_boundary(end) {
                end += 1;
            }
            self.offset = end;
            return Some(Token::Escape(&self.text[start..end]));
        }
        let first = self.text[start..].chars().next().unwrap();
        let mut end = start + first.len_utf8();
        // Keep combining sequences, emoji ZWJ sequences, and flag pairs intact.
        if !first.is_control() {
            let mut join_next = false;
            let mut regional = (0x1f1e6..=0x1f1ff).contains(&(first as u32));
            while end < bytes.len() {
                let next = self.text[end..].chars().next().unwrap();
                if next.is_control() {
                    break;
                }
                let combining = UnicodeWidthChar::width(next) == Some(0)
                    || (0x1f3fb..=0x1f3ff).contains(&(next as u32));
                let flag = regional && (0x1f1e6..=0x1f1ff).contains(&(next as u32));
                if !combining && !join_next && !flag {
                    break;
                }
                end += next.len_utf8();
                join_next = next == '\u{200d}';
                regional = false;
            }
        }
        self.offset = end;
        let text = &self.text[start..end];
        Some(Token::Glyph(text, UnicodeWidthStr::width(text)))
    }
}

fn clip(text: &str, width: usize) -> String {
    let mut result = String::with_capacity(text.len());
    let mut column = 0;
    let mut clipped = false;
    for token in Tokens::new(text) {
        match token {
            Token::Escape(code) => result.push_str(code),
            Token::Glyph(text, size) => {
                if !clipped && column + size <= width {
                    result.push_str(text);
                } else {
                    clipped = true;
                }
                column += size;
            }
        }
    }
    result
}

fn overlay(original: &str, column: usize, replacement: &str, initial: &AnsiState) -> String {
    let end = column + visible_width(replacement);
    let mut prefix = String::new();
    let mut suffix = String::new();
    let mut state = initial.clone();
    let mut x = 0;
    for token in Tokens::new(original) {
        match token {
            Token::Escape(code) => {
                if x < column {
                    prefix.push_str(code);
                    state.feed(code);
                } else if x < end {
                    state.feed(code);
                } else {
                    suffix.push_str(code);
                }
            }
            Token::Glyph(text, width) => {
                if x + width <= column {
                    prefix.push_str(text);
                } else if x < column {
                    prefix.push_str(&" ".repeat(column - x));
                }
                if x >= end {
                    suffix.push_str(text);
                } else if x + width > end {
                    suffix.push_str(&" ".repeat(x + width - end));
                }
                x += width;
            }
        }
    }
    if x < column {
        prefix.push_str(&" ".repeat(column - x));
    }
    format!("{prefix}{RESET}{replacement}{}{suffix}", state.restore())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn plain(text: &str) -> String {
        Tokens::new(text)
            .filter_map(|t| match t {
                Token::Glyph(s, _) => Some(s),
                _ => None,
            })
            .collect()
    }
    fn unwrapped() -> RenderOptions {
        RenderOptions {
            wrap: None,
            ..RenderOptions::default()
        }
    }

    #[test]
    fn literal_messages_are_never_executed_or_expanded() {
        let message = r#"C:\new\path $balloon$ $(touch /tmp/nope) "quotes" 'single' \u1234"#;
        let result = render("$balloon$\n $\\$\nPONY\n", message, &unwrapped()).unwrap();
        assert!(plain(&result).contains(message));
        assert!(plain(&result).ends_with(" \\\nPONY\n"));
    }

    #[test]
    fn default_single_and_multiline_balloon_geometry() {
        assert_eq!(
            plain(&render("$balloon$", "hi", &unwrapped()).unwrap()),
            " ____ \n< hi >\n ---- "
        );
        assert_eq!(
            plain(&render("$balloon$", "one\ntwo", &unwrapped()).unwrap()),
            " _____ \n/ one \\\n\\ two /\n ----- "
        );
    }

    #[test]
    fn top_balloon_preserves_indent_and_source_row_order() {
        let result = plain(&render("  $balloon$\n  $\\$\npony", "hi", &unwrapped()).unwrap());
        assert_eq!(result, "   ____ \n  < hi >\n   ---- \n  \\\npony");
    }

    #[test]
    fn bottom_balloon_extends_past_end_without_losing_rows() {
        let result = plain(&render("pony\n$/$\n$balloon$\n", "hi", &unwrapped()).unwrap());
        assert_eq!(result, "pony\n/\n ____ \n< hi >\n ---- ");
    }

    #[test]
    fn embedded_balloon_overwrites_cells_and_preserves_surrounding_art() {
        let result = plain(
            &render(
                "pony\nab$balloon$TAIL\n01xxxxxxxxTAIL\n23xxxxxxxxTAIL\nend",
                "hi",
                &unwrapped(),
            )
            .unwrap(),
        );
        assert_eq!(
            result,
            "pony\nab ____ TAIL\n01< hi >xxTAIL\n23 ---- xxTAIL\nend"
        );
    }

    #[test]
    fn variables_dollars_links_and_think_are_supported() {
        let result = plain(
            &render(
                "$v=C:\\literal\\n$ $v$ $$ $\\$ $/$ $X$",
                "",
                &RenderOptions {
                    think: true,
                    ..unwrapped()
                },
            )
            .unwrap(),
        );
        assert_eq!(result, " C:\\literal\\n $ o o o");
    }

    #[test]
    fn metadata_is_hidden_and_pony_only_removes_reserved_rows() {
        let pony = "$$$\nNAME: Example\nBALLOON TOP: 2\nBALLOON BOTTOM: 1\n$$$\n$balloon$\n$\\$\ncoloured pony\n$/$\n";
        let output = render(
            pony,
            "ignored",
            &RenderOptions {
                pony_only: true,
                ..unwrapped()
            },
        )
        .unwrap();
        assert_eq!(output, "coloured pony\n");
    }

    #[test]
    fn unicode_ansi_width_and_clipping_are_terminal_columns() {
        assert_eq!(visible_width("\x1b[31m界e\u{301}🙂\x1b[0m"), 5);
        assert_eq!(
            visible_width("\x1b]8;;https://example.org\x1b\\hello\x1b]8;;\x07"),
            5
        );
        assert_eq!(visible_width("👨‍👩‍👧‍👦🇬🇧"), 4);
        assert_eq!(
            plain(&clip("\x1b[31mA界e\u{301}Z\x1b[0m", 4)),
            "A界e\u{301}"
        );
        assert_eq!(plain(&clip("A界B", 2)), "A");
        assert_eq!(plain(&clip("A👨‍👩‍👧‍👦B", 3)), "A👨‍👩‍👧‍👦");
    }

    #[test]
    fn wrap_keeps_words_ansi_combining_and_wide_characters_intact() {
        let options = RenderOptions {
            wrap: Some(14),
            ..unwrapped()
        };
        let output =
            plain(&render("$balloon$", "hello world\n界界界界界界 e\u{301}", &options).unwrap());
        assert!(output.contains("hello"));
        assert!(output.contains("world"));
        assert!(output.contains("e\u{301}"));
        assert!(
            output.lines().all(|line| visible_width(line) <= 14),
            "{output}"
        );
        let wrapped = wrap_message("abcdefghijklmno", Some(8));
        assert_eq!(wrapped, ["abcdefg-", "hijklmno"]);
    }

    #[test]
    fn ansi_message_colors_resume_across_lines_and_do_not_color_borders() {
        let output = render(
            "$balloon$\n\x1b[32mPONY\x1b[0m",
            "\x1b[31mred\nred too",
            &unwrapped(),
        )
        .unwrap();
        assert!(output.contains("\x1b[31mred too\x1b[0m"));
        assert!(output.contains("\x1b[32mPONY\x1b[0m"));
        assert!(output.contains("red\x1b[0m"));
    }

    #[test]
    fn balloon_alignment_and_height_are_honored() {
        let left = plain(&render("$balloon4l12,6$", "hi", &unwrapped()).unwrap());
        assert_eq!(left.lines().count(), 6);
        assert!(left.lines().all(|line| visible_width(line) == 12));
        assert!(left.starts_with("     ______ "));
        for macro_name in ["$balloon4l12$", "$balloon4c12$", "$balloon4r12$"] {
            assert!(render(macro_name, "longer text here", &unwrapped()).is_ok());
        }
    }

    #[test]
    fn compact_preserves_paragraphs_and_tabs_expand() {
        assert_eq!(
            prepare_message("  first   line\nsecond\n\n third paragraph  ", true),
            "first line second\n\nthird paragraph"
        );
        assert_eq!(prepare_message("界\tx", false), "界      x");
    }

    #[test]
    fn malformed_inputs_report_errors_instead_of_panicking() {
        for pony in [
            "$unknown$",
            "$balloonnope$",
            "$balloon2r1$",
            "$balloon1,2,3$",
            "$balloon999999999$",
            "$unclosed",
            "$$$\nNAME: no end",
            "$=bad$",
        ] {
            assert!(render(pony, "test", &unwrapped()).is_err(), "{pony}");
        }
        for balloon in [":bad", "n:value", "unknown:yes"] {
            assert!(render(
                "$balloon$",
                "test",
                &RenderOptions {
                    balloon: Some(balloon.into()),
                    ..unwrapped()
                }
            )
            .is_err());
        }
    }

    #[test]
    fn every_bundled_pony_and_balloon_style_renders() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut count = 0;
        for directory in ["ponies", "extraponies", "ttyponies", "extrattyponies"] {
            for entry in std::fs::read_dir(root.join(directory)).unwrap() {
                let path = entry.unwrap().path();
                if path.extension().is_none_or(|ext| ext != "pony") {
                    continue;
                }
                let pony = std::fs::read_to_string(&path).unwrap();
                for pony_only in [false, true] {
                    let output = render(
                        &pony,
                        "Hello, 世界! literal \\n $price",
                        &RenderOptions {
                            pony_only,
                            ..RenderOptions::default()
                        },
                    )
                    .unwrap_or_else(|err| panic!("{}: {err}", path.display()));
                    assert!(!output.contains("$balloon"), "{}", path.display());
                    assert!(!output.contains("$$$"), "{}", path.display());
                    assert!(!output.is_empty(), "{}", path.display());
                }
                count += 1;
            }
        }
        assert!(count >= 577, "expected the full pony corpus, found {count}");
        for entry in std::fs::read_dir(root.join("balloons")).unwrap() {
            let path = entry.unwrap().path();
            if !path.is_file() {
                continue;
            }
            let balloon = std::fs::read_to_string(&path).unwrap();
            let output = render(
                "$balloon$\n$\\$ $/$ $X$",
                "Hello\n世界!",
                &RenderOptions {
                    balloon: Some(balloon),
                    ..unwrapped()
                },
            )
            .unwrap_or_else(|err| panic!("{}: {err}", path.display()));
            let widths: Vec<usize> = plain(&output).lines().map(visible_width).collect();
            assert!(
                widths[..widths.len() - 1].iter().all(|w| *w == widths[0]),
                "{}: {widths:?}",
                path.display()
            );
        }
    }
}
