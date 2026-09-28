//! A small Markdown parser and egui renderer for DM session notes.
//!
//! Deliberately not full CommonMark: it covers what actually shows up in session
//! notes (headings, emphasis, lists, tasks, quotes, code, tables, rules) plus
//! Obsidian-style `[[wikilinks]]`. The wikilinks are why this is hand-rolled
//! rather than a crate — they have to be clickable and resolve inside the vault,
//! and the CommonMark crates that do that want a different egui version.

use egui::RichText;

/// Where an inline link points.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Link {
    /// `[[Note Title]]` — resolved against the note vault.
    Wiki(String),
    /// A normal `[text](url)` hyperlink.
    Url(String),
}

/// A run of text sharing one set of inline styles.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Span {
    pub text: String,
    pub bold: bool,
    pub italic: bool,
    pub strike: bool,
    pub code: bool,
    pub link: Option<Link>,
}

/// What a list item is bulleted with.
#[derive(Clone, Debug, PartialEq)]
pub enum Marker {
    Bullet,
    Number(usize),
    /// A `- [ ]` / `- [x]` task. `line` is the source line index so a click can edit it.
    Task { checked: bool, line: usize },
}

#[derive(Clone, Debug, PartialEq)]
pub enum Block {
    Heading { level: u8, spans: Vec<Span> },
    Paragraph(Vec<Span>),
    ListItem { depth: usize, marker: Marker, spans: Vec<Span> },
    Quote(Vec<Vec<Span>>),
    Code { lang: String, text: String },
    Table { header: Vec<Vec<Span>>, rows: Vec<Vec<Vec<Span>>> },
    Rule,
}

/// Something the reader clicked in a rendered note.
#[derive(Clone, Debug, PartialEq)]
pub enum NoteAction {
    /// Follow a `[[wikilink]]` to the named note.
    OpenWiki(String),
    /// Follow an external hyperlink.
    OpenUrl(String),
    /// Flip the `- [ ]` / `- [x]` checkbox on this source line.
    ToggleTask(usize),
}

// ---------------------------------------------------------------- frontmatter

/// Split leading `---` YAML-ish frontmatter off a note, returning its key/value
/// pairs and the remaining body. Only a block starting on the very first line counts.
pub fn split_frontmatter(src: &str) -> (Vec<(String, String)>, &str) {
    let rest = match src.strip_prefix("---\n").or_else(|| src.strip_prefix("---\r\n")) {
        Some(r) => r,
        None => return (Vec::new(), src),
    };
    // Find the closing fence at the start of a line.
    let mut offset = 0;
    for line in rest.split_inclusive('\n') {
        if line.trim_end() == "---" {
            let front = &rest[..offset];
            let body = &rest[offset + line.len()..];
            let pairs = front
                .lines()
                .filter_map(|l| l.split_once(':'))
                .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
                .filter(|(k, _)| !k.is_empty())
                .collect();
            return (pairs, body.trim_start_matches('\n'));
        }
        offset += line.len();
    }
    (Vec::new(), src)
}

// -------------------------------------------------------------- block parsing

pub fn parse(src: &str) -> Vec<Block> {
    let lines: Vec<&str> = src.lines().collect();
    let mut blocks = Vec::new();
    let mut para: Vec<&str> = Vec::new();
    let mut i = 0;

    while i < lines.len() {
        let raw = lines[i];
        let trimmed = raw.trim_start();
        let indent = raw.len() - trimmed.len();

        if trimmed.starts_with("```") {
            flush_para(&mut para, &mut blocks);
            let lang = trimmed[3..].trim().to_string();
            i += 1;
            let start = i;
            while i < lines.len() && !lines[i].trim_start().starts_with("```") {
                i += 1;
            }
            let text = lines[start..i].join("\n");
            if i < lines.len() {
                i += 1; // closing fence
            }
            blocks.push(Block::Code { lang, text });
            continue;
        }

        if trimmed.is_empty() {
            flush_para(&mut para, &mut blocks);
            i += 1;
            continue;
        }

        if is_rule(trimmed) {
            flush_para(&mut para, &mut blocks);
            blocks.push(Block::Rule);
            i += 1;
            continue;
        }

        if let Some((level, rest)) = heading(trimmed) {
            flush_para(&mut para, &mut blocks);
            blocks.push(Block::Heading { level, spans: parse_inline(rest) });
            i += 1;
            continue;
        }

        if trimmed.starts_with('>') {
            flush_para(&mut para, &mut blocks);
            let mut quoted = Vec::new();
            while i < lines.len() && lines[i].trim_start().starts_with('>') {
                let l = lines[i].trim_start().trim_start_matches('>');
                quoted.push(parse_inline(l.strip_prefix(' ').unwrap_or(l)));
                i += 1;
            }
            blocks.push(Block::Quote(quoted));
            continue;
        }

        if trimmed.contains('|')
            && i + 1 < lines.len()
            && is_table_delimiter(lines[i + 1].trim())
        {
            flush_para(&mut para, &mut blocks);
            let header = split_table_row(trimmed);
            i += 2;
            let mut rows = Vec::new();
            while i < lines.len() && lines[i].trim().contains('|') {
                rows.push(split_table_row(lines[i].trim()));
                i += 1;
            }
            blocks.push(Block::Table { header, rows });
            continue;
        }

        if let Some((marker, content)) = list_item(trimmed, i) {
            flush_para(&mut para, &mut blocks);
            blocks.push(Block::ListItem {
                depth: (indent / 2).min(6),
                marker,
                spans: parse_inline(content),
            });
            i += 1;
            continue;
        }

        para.push(trimmed);
        i += 1;
    }

    flush_para(&mut para, &mut blocks);
    blocks
}

fn flush_para(para: &mut Vec<&str>, blocks: &mut Vec<Block>) {
    if para.is_empty() {
        return;
    }
    let text = para.join(" ");
    para.clear();
    blocks.push(Block::Paragraph(parse_inline(&text)));
}

fn is_rule(t: &str) -> bool {
    let t = t.trim();
    t.len() >= 3
        && (t.chars().all(|c| c == '-') || t.chars().all(|c| c == '*') || t.chars().all(|c| c == '_'))
}

fn heading(t: &str) -> Option<(u8, &str)> {
    let hashes = t.chars().take_while(|&c| c == '#').count();
    if hashes == 0 || hashes > 6 {
        return None;
    }
    let rest = &t[hashes..];
    let body = rest.strip_prefix(' ')?;
    Some((hashes as u8, body.trim_end()))
}

fn list_item(t: &str, line: usize) -> Option<(Marker, &str)> {
    for prefix in ["- ", "* ", "+ "] {
        if let Some(rest) = t.strip_prefix(prefix) {
            if let Some(r) = rest.strip_prefix("[ ]") {
                return Some((Marker::Task { checked: false, line }, r.trim_start()));
            }
            if let Some(r) = rest.strip_prefix("[x]").or_else(|| rest.strip_prefix("[X]")) {
                return Some((Marker::Task { checked: true, line }, r.trim_start()));
            }
            return Some((Marker::Bullet, rest));
        }
    }
    let digits = t.chars().take_while(|c| c.is_ascii_digit()).count();
    if digits > 0 {
        let rest = &t[digits..];
        if let Some(r) = rest.strip_prefix(". ").or_else(|| rest.strip_prefix(") ")) {
            let n = t[..digits].parse().unwrap_or(1);
            return Some((Marker::Number(n), r));
        }
    }
    None
}

fn is_table_delimiter(t: &str) -> bool {
    !t.is_empty()
        && t.contains('-')
        && t.chars().all(|c| matches!(c, '-' | ':' | '|' | ' '))
}

fn split_table_row(t: &str) -> Vec<Vec<Span>> {
    t.trim().trim_matches('|').split('|').map(|c| parse_inline(c.trim())).collect()
}

// ------------------------------------------------------------- inline parsing

pub fn parse_inline(src: &str) -> Vec<Span> {
    let ch: Vec<char> = src.chars().collect();
    let mut out: Vec<Span> = Vec::new();
    let mut buf = String::new();
    let (mut bold, mut italic, mut strike) = (false, false, false);
    let mut i = 0;

    while i < ch.len() {
        if ch[i] == '`' {
            if let Some(end) = find_char(&ch, i + 1, '`') {
                push_text(&mut out, &mut buf, bold, italic, strike);
                out.push(Span {
                    text: ch[i + 1..end].iter().collect(),
                    bold,
                    italic,
                    strike,
                    code: true,
                    link: None,
                });
                i = end + 1;
                continue;
            }
        }

        if ch[i] == '[' && ch.get(i + 1) == Some(&'[') {
            if let Some(end) = find_seq(&ch, i + 2, "]]") {
                let inner: String = ch[i + 2..end].iter().collect();
                let (target, label) = match inner.split_once('|') {
                    Some((t, a)) => (t.trim().to_string(), a.trim().to_string()),
                    None => (inner.trim().to_string(), inner.trim().to_string()),
                };
                push_text(&mut out, &mut buf, bold, italic, strike);
                out.push(Span {
                    text: label,
                    bold,
                    italic,
                    strike,
                    code: false,
                    link: Some(Link::Wiki(target)),
                });
                i = end + 2;
                continue;
            }
        }

        if ch[i] == '[' {
            if let Some(close) = find_char(&ch, i + 1, ']') {
                if ch.get(close + 1) == Some(&'(') {
                    if let Some(paren) = find_char(&ch, close + 2, ')') {
                        push_text(&mut out, &mut buf, bold, italic, strike);
                        out.push(Span {
                            text: ch[i + 1..close].iter().collect(),
                            bold,
                            italic,
                            strike,
                            code: false,
                            link: Some(Link::Url(ch[close + 2..paren].iter().collect())),
                        });
                        i = paren + 1;
                        continue;
                    }
                }
            }
        }

        if ch[i] == '~' && ch.get(i + 1) == Some(&'~') {
            push_text(&mut out, &mut buf, bold, italic, strike);
            strike = !strike;
            i += 2;
            continue;
        }
        if ch[i] == '*' && ch.get(i + 1) == Some(&'*') {
            push_text(&mut out, &mut buf, bold, italic, strike);
            bold = !bold;
            i += 2;
            continue;
        }
        if ch[i] == '_' && ch.get(i + 1) == Some(&'_') && at_word_edge(&ch, i, 2) {
            push_text(&mut out, &mut buf, bold, italic, strike);
            bold = !bold;
            i += 2;
            continue;
        }
        if ch[i] == '*' {
            push_text(&mut out, &mut buf, bold, italic, strike);
            italic = !italic;
            i += 1;
            continue;
        }
        if ch[i] == '_' && at_word_edge(&ch, i, 1) {
            push_text(&mut out, &mut buf, bold, italic, strike);
            italic = !italic;
            i += 1;
            continue;
        }

        buf.push(ch[i]);
        i += 1;
    }

    push_text(&mut out, &mut buf, bold, italic, strike);
    out
}

fn push_text(out: &mut Vec<Span>, buf: &mut String, bold: bool, italic: bool, strike: bool) {
    if buf.is_empty() {
        return;
    }
    out.push(Span {
        text: std::mem::take(buf),
        bold,
        italic,
        strike,
        code: false,
        link: None,
    });
}

/// Underscores only mark emphasis at a word edge, so `snake_case` survives intact.
fn at_word_edge(ch: &[char], i: usize, len: usize) -> bool {
    let before = i.checked_sub(1).and_then(|j| ch.get(j)).copied();
    let after = ch.get(i + len).copied();
    !(before.is_some_and(char::is_alphanumeric) && after.is_some_and(char::is_alphanumeric))
}

fn find_char(ch: &[char], from: usize, target: char) -> Option<usize> {
    (from..ch.len()).find(|&j| ch[j] == target)
}

fn find_seq(ch: &[char], from: usize, pat: &str) -> Option<usize> {
    let p: Vec<char> = pat.chars().collect();
    if ch.len() < p.len() {
        return None;
    }
    (from..=ch.len() - p.len()).find(|&j| ch[j..j + p.len()] == p[..])
}

// ------------------------------------------------------------------ rendering

const HEADING_SIZES: [f32; 6] = [20.0, 17.5, 15.5, 14.0, 13.0, 12.5];

/// Render parsed blocks, returning whatever the reader clicked.
pub fn render(ui: &mut egui::Ui, blocks: &[Block], body_size: f32) -> Option<NoteAction> {
    let mut action = None;
    let mut table_seq = 0usize;
    for (idx, block) in blocks.iter().enumerate() {
        render_block(ui, block, body_size, idx, &mut table_seq, &mut action);
    }
    action
}

fn render_block(
    ui: &mut egui::Ui,
    block: &Block,
    body_size: f32,
    idx: usize,
    table_seq: &mut usize,
    action: &mut Option<NoteAction>,
) {
    match block {
        Block::Heading { level, spans } => {
            ui.add_space(if *level <= 2 { 8.0 } else { 5.0 });
            let size = HEADING_SIZES[(*level as usize - 1).min(5)];
            spans_ui(ui, spans, size, true, action);
            if *level <= 2 {
                ui.add_space(1.0);
                ui.separator();
            }
        }
        Block::Paragraph(spans) => {
            ui.add_space(3.0);
            spans_ui(ui, spans, body_size, false, action);
        }
        Block::ListItem { depth, marker, spans } => {
            ui.horizontal_top(|ui| {
                ui.add_space(8.0 + *depth as f32 * 14.0);
                match marker {
                    Marker::Bullet => {
                        ui.label(RichText::new(if depth % 2 == 0 { "•" } else { "◦" }).size(body_size));
                    }
                    Marker::Number(n) => {
                        ui.label(RichText::new(format!("{}.", n)).size(body_size));
                    }
                    Marker::Task { checked, line } => {
                        let mut done = *checked;
                        if ui.checkbox(&mut done, "").changed() {
                            *action = Some(NoteAction::ToggleTask(*line));
                        }
                    }
                }
                let dim = matches!(marker, Marker::Task { checked: true, .. });
                spans_ui_dimmed(ui, spans, body_size, false, dim, action);
            });
        }
        Block::Quote(lines) => {
            ui.add_space(4.0);
            let accent = ui.visuals().selection.bg_fill;
            egui::Frame::new()
                .fill(ui.visuals().faint_bg_color)
                .stroke(egui::Stroke::NONE)
                .inner_margin(egui::Margin::symmetric(10, 7))
                .outer_margin(egui::Margin { left: 4, ..Default::default() })
                .show(ui, |ui| {
                    let top = ui.min_rect().left_top();
                    for spans in lines {
                        spans_ui(ui, spans, body_size, false, action);
                    }
                    // Accent bar down the left edge of the quote.
                    let rect = ui.min_rect();
                    ui.painter().rect_filled(
                        egui::Rect::from_min_max(
                            egui::pos2(top.x - 10.0, rect.top() - 7.0),
                            egui::pos2(top.x - 7.0, rect.bottom() + 7.0),
                        ),
                        1.0,
                        accent,
                    );
                });
            ui.add_space(4.0);
        }
        Block::Code { lang: _, text } => {
            ui.add_space(4.0);
            egui::Frame::new()
                .fill(ui.visuals().extreme_bg_color)
                .inner_margin(egui::Margin::symmetric(8, 6))
                .show(ui, |ui| {
                    ui.label(RichText::new(text).monospace().size(body_size - 1.0));
                });
            ui.add_space(4.0);
        }
        Block::Table { header, rows } => {
            ui.add_space(4.0);
            *table_seq += 1;
            egui::Grid::new(("note_table", idx, *table_seq))
                .striped(true)
                .spacing([14.0, 4.0])
                .show(ui, |ui| {
                    for cell in header {
                        cell_ui(ui, cell, body_size, true, action);
                    }
                    ui.end_row();
                    for row in rows {
                        for cell in row {
                            cell_ui(ui, cell, body_size, false, action);
                        }
                        ui.end_row();
                    }
                });
            ui.add_space(4.0);
        }
        Block::Rule => {
            ui.add_space(4.0);
            ui.separator();
        }
    }
}

fn spans_ui(ui: &mut egui::Ui, spans: &[Span], size: f32, force_bold: bool, action: &mut Option<NoteAction>) {
    spans_ui_dimmed(ui, spans, size, force_bold, false, action);
}

/// One table cell. Unlike prose, a cell lays out on a single line — a Grid hands
/// each column its content width, and wrapping inside one shreds short cells like
/// "Throne Room" into a word per line.
fn cell_ui(ui: &mut egui::Ui, spans: &[Span], size: f32, force_bold: bool, action: &mut Option<NoteAction>) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        for span in spans {
            let text = styled(span, size, force_bold, false);
            match &span.link {
                Some(link) => {
                    let resp = ui
                        .add(
                            egui::Label::new(text.color(ui.visuals().hyperlink_color).underline())
                                .wrap_mode(egui::TextWrapMode::Extend)
                                .sense(egui::Sense::click()),
                        )
                        .on_hover_cursor(egui::CursorIcon::PointingHand);
                    if resp.clicked() {
                        *action = Some(match link {
                            Link::Wiki(t) => NoteAction::OpenWiki(t.clone()),
                            Link::Url(u) => NoteAction::OpenUrl(u.clone()),
                        });
                    }
                }
                None => {
                    ui.add(egui::Label::new(text).wrap_mode(egui::TextWrapMode::Extend));
                }
            }
        }
    });
}

fn spans_ui_dimmed(
    ui: &mut egui::Ui,
    spans: &[Span],
    size: f32,
    force_bold: bool,
    dim: bool,
    action: &mut Option<NoteAction>,
) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        ui.set_row_height(size * 1.35);
        for span in spans {
            if let Some(link) = &span.link {
                let text = styled(span, size, force_bold, false)
                    .color(ui.visuals().hyperlink_color)
                    .underline();
                let resp = ui
                    .add(egui::Label::new(text).sense(egui::Sense::click()))
                    .on_hover_cursor(egui::CursorIcon::PointingHand);
                if resp.clicked() {
                    *action = Some(match link {
                        Link::Wiki(t) => NoteAction::OpenWiki(t.clone()),
                        Link::Url(u) => NoteAction::OpenUrl(u.clone()),
                    });
                }
            } else if span.code {
                ui.label(styled(span, size, force_bold, dim));
            } else {
                // One label per word so `horizontal_wrapped` can break lines. Trailing
                // whitespace rides along with its word, so a break never leaves a
                // visible gap at the start of the next line.
                for word in word_tokens(&span.text) {
                    let mut piece = span.clone();
                    piece.text = word;
                    ui.label(styled(&piece, size, force_bold, dim));
                }
            }
        }
    });
}

fn styled(span: &Span, size: f32, force_bold: bool, dim: bool) -> RichText {
    let mut text = RichText::new(&span.text).size(size);
    if span.bold || force_bold {
        text = text.strong();
    }
    if span.italic {
        text = text.italics();
    }
    if span.strike || dim {
        text = text.strikethrough();
    }
    if span.code {
        text = text.monospace().background_color(egui::Color32::from_black_alpha(40));
    }
    if dim {
        text = text.weak();
    }
    text
}

fn word_tokens(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_ws = false;
    for c in s.chars() {
        if c.is_whitespace() {
            cur.push(c);
            in_ws = true;
        } else {
            if in_ws {
                out.push(std::mem::take(&mut cur));
                in_ws = false;
            }
            cur.push(c);
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Flip the checkbox on `line` of a note's source, returning the edited text.
pub fn toggle_task_line(src: &str, line: usize) -> String {
    let mut lines: Vec<String> = src.lines().map(str::to_string).collect();
    if let Some(l) = lines.get_mut(line) {
        if let Some(pos) = l.find("[ ]") {
            l.replace_range(pos..pos + 3, "[x]");
        } else if let Some(pos) = l.to_lowercase().find("[x]") {
            l.replace_range(pos..pos + 3, "[ ]");
        }
    }
    let mut out = lines.join("\n");
    if src.ends_with('\n') {
        out.push('\n');
    }
    out
}

/// First non-empty, non-heading line of a note, for one-line previews.
pub fn summary_line(src: &str) -> String {
    for line in src.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') || t.starts_with("---") {
            continue;
        }
        let plain: String = parse_inline(t).iter().map(|s| s.text.as_str()).collect();
        let plain = plain.trim_start_matches(['-', '*', '>', ' ']).trim();
        if !plain.is_empty() {
            return plain.to_string();
        }
    }
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_of(spans: &[Span]) -> String {
        spans.iter().map(|s| s.text.as_str()).collect()
    }

    #[test]
    fn frontmatter_splits_off_binding() {
        let (front, body) = split_frontmatter("---\nroom: abc-123\n---\n\nThe dais.\n");
        assert_eq!(front, vec![("room".to_string(), "abc-123".to_string())]);
        assert_eq!(body, "The dais.\n");
    }

    #[test]
    fn no_frontmatter_leaves_body_untouched() {
        let (front, body) = split_frontmatter("# Title\n\nBody");
        assert!(front.is_empty());
        assert_eq!(body, "# Title\n\nBody");
    }

    #[test]
    fn unterminated_frontmatter_is_not_frontmatter() {
        let (front, body) = split_frontmatter("---\nroom: abc\nstill going");
        assert!(front.is_empty());
        assert_eq!(body, "---\nroom: abc\nstill going");
    }

    #[test]
    fn parses_headings_and_paragraphs() {
        let blocks = parse("# Throne Room\n\nThe dais is cracked.\nBeneath it, a stair.");
        assert_eq!(blocks.len(), 2);
        assert!(matches!(blocks[0], Block::Heading { level: 1, .. }));
        match &blocks[1] {
            // Consecutive lines join into one paragraph, as Markdown does.
            Block::Paragraph(spans) => {
                assert_eq!(text_of(spans), "The dais is cracked. Beneath it, a stair.")
            }
            other => panic!("expected paragraph, got {:?}", other),
        }
    }

    #[test]
    fn parses_wikilink_with_and_without_alias() {
        let spans = parse_inline("Ask [[NPC - Vex]] or [[Plot - Sigil|the sigil]].");
        let links: Vec<&Span> = spans.iter().filter(|s| s.link.is_some()).collect();
        assert_eq!(links.len(), 2);
        assert_eq!(links[0].link, Some(Link::Wiki("NPC - Vex".into())));
        assert_eq!(links[0].text, "NPC - Vex");
        assert_eq!(links[1].link, Some(Link::Wiki("Plot - Sigil".into())));
        assert_eq!(links[1].text, "the sigil");
    }

    #[test]
    fn parses_emphasis_and_code() {
        let spans = parse_inline("**bold** and *soft* and `code`");
        assert!(spans.iter().any(|s| s.bold && s.text == "bold"));
        assert!(spans.iter().any(|s| s.italic && s.text == "soft"));
        assert!(spans.iter().any(|s| s.code && s.text == "code"));
    }

    #[test]
    fn underscores_inside_a_word_are_literal() {
        let spans = parse_inline("the room_id field");
        assert_eq!(text_of(&spans), "the room_id field");
        assert!(spans.iter().all(|s| !s.italic));
    }

    #[test]
    fn parses_tasks_bullets_and_numbers() {
        let blocks = parse("- [ ] Hide the key\n- [x] Light the brazier\n- plain\n1. first");
        let markers: Vec<&Marker> = blocks
            .iter()
            .filter_map(|b| match b {
                Block::ListItem { marker, .. } => Some(marker),
                _ => None,
            })
            .collect();
        assert_eq!(markers.len(), 4);
        assert_eq!(markers[0], &Marker::Task { checked: false, line: 0 });
        assert_eq!(markers[1], &Marker::Task { checked: true, line: 1 });
        assert_eq!(markers[2], &Marker::Bullet);
        assert_eq!(markers[3], &Marker::Number(1));
    }

    #[test]
    fn toggling_a_task_edits_only_that_line() {
        let src = "- [ ] one\n- [ ] two";
        let out = toggle_task_line(src, 1);
        assert_eq!(out, "- [ ] one\n- [x] two");
        assert_eq!(toggle_task_line(&out, 1), src);
    }

    #[test]
    fn parses_quote_code_rule_and_table() {
        let blocks = parse(
            "> Read this aloud.\n> Second line.\n\n```\nraw\n```\n\n---\n\n| Item | Qty |\n|---|---|\n| Rope | 2 |",
        );
        assert!(matches!(&blocks[0], Block::Quote(lines) if lines.len() == 2));
        assert!(matches!(&blocks[1], Block::Code { text, .. } if text == "raw"));
        assert!(matches!(blocks[2], Block::Rule));
        match &blocks[3] {
            Block::Table { header, rows } => {
                assert_eq!(header.len(), 2);
                assert_eq!(rows.len(), 1);
                assert_eq!(text_of(&rows[0][0]), "Rope");
            }
            other => panic!("expected table, got {:?}", other),
        }
    }

    #[test]
    fn unclosed_code_fence_still_terminates() {
        let blocks = parse("```\nno closing fence");
        assert!(matches!(&blocks[0], Block::Code { text, .. } if text == "no closing fence"));
    }

    #[test]
    fn summary_skips_headings_and_strips_markup() {
        let src = "---\n\n# Throne Room\n\nThe **dais** is [[cracked]].";
        assert_eq!(summary_line(src), "The dais is cracked.");
    }

    #[test]
    fn summary_of_an_empty_note_is_empty() {
        assert_eq!(summary_line("# Only a heading\n\n"), "");
    }

    const KITCHEN_SINK: &str = "\
# Throne Room

The **dais** is *cracked*; see [[NPC - Vex]] and [Rules](https://example.com).

## Loot
| Item | Qty |
|------|-----|
| Rope | 2 |

- [ ] Hide the key
- [x] Light the brazier
  - nested bullet
1. First
2. Second

> Read this aloud to the party.
> The air smells of iron.

```
raw block
```

---

Trailing `code` and ~~cut~~ text.
";

    #[test]
    fn rendering_every_block_kind_lays_out_without_panicking() {
        let ctx = egui::Context::default();
        let blocks = parse(KITCHEN_SINK);
        // Two frames: egui allocates ids on the first and reuses them on the second,
        // so a duplicate-id bug in the table/grid code shows up here.
        for _ in 0..2 {
            let _ = ctx.run(Default::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    render(ui, &blocks, 13.5);
                });
            });
        }
    }

    #[test]
    fn two_tables_in_one_note_both_render() {
        let ctx = egui::Context::default();
        let blocks = parse("| A |\n|---|\n| 1 |\n\ntext\n\n| B |\n|---|\n| 2 |");
        assert_eq!(blocks.iter().filter(|b| matches!(b, Block::Table { .. })).count(), 2);
        // Each table needs its own egui Grid id; sharing one makes the second
        // collapse into the first, so lay both out twice and check they persist.
        for _ in 0..2 {
            let _ = ctx.run(Default::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    render(ui, &blocks, 13.5);
                });
            });
        }
        assert!(ctx.data(|d| d.len()) > 0, "grid state was stored");
    }
}
