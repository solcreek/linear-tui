//! Markdown (CommonMark + GFM tables, strikethrough, task lists) to styled,
//! wrapped terminal lines.
//!
//! Wrapping counts terminal cells with `unicode-width`, so CJK text (two
//! cells a character, breakable anywhere) wraps correctly. Links are
//! numbered and collected so the UI can open them; images are collected
//! with the line they sit on, so the UI can draw them there.

use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// An image in the text: where its placeholder line is, and what it shows.
#[derive(Debug, Clone, PartialEq)]
pub struct ImageRef {
    pub line: usize,
    pub url: String,
    pub alt: String,
}

/// Rendered Markdown.
#[derive(Debug, Default)]
pub struct Rendered {
    pub lines: Vec<Line<'static>>,
    /// Link targets; link `n` is `links[n - 1]`.
    pub links: Vec<String>,
    pub images: Vec<ImageRef>,
}

/// Styles for the parts of a document.
pub mod theme {
    use super::*;
    pub fn heading(level: usize) -> Style {
        let s = Style::new().add_modifier(Modifier::BOLD);
        if level <= 2 { s.add_modifier(Modifier::UNDERLINED) } else { s }
    }
    pub fn code() -> Style {
        Style::new().fg(Color::Yellow)
    }
    pub fn link() -> Style {
        Style::new().fg(Color::Blue).add_modifier(Modifier::UNDERLINED)
    }
    pub fn dim() -> Style {
        Style::new().add_modifier(Modifier::DIM)
    }
    pub fn quote() -> Style {
        Style::new().add_modifier(Modifier::ITALIC)
    }
}

/// Render `markdown` to lines at most `width` cells wide.
pub fn render(markdown: &str, width: u16) -> Rendered {
    let opts = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    let mut r = Renderer { width: usize::from(width).max(8), ..Default::default() };
    for event in Parser::new_ext(markdown, opts) {
        r.event(event);
    }
    r.flush();
    r.out
}

/// The images in a document: (URL, alt text), in order.
pub fn image_urls(markdown: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut current: Option<(String, String)> = None;
    for e in Parser::new_ext(markdown, Options::ENABLE_TABLES) {
        match e {
            Event::Start(Tag::Image { dest_url, .. }) => {
                current = Some((dest_url.to_string(), String::new()))
            }
            Event::Text(t) | Event::Code(t) => {
                if let Some((_, alt)) = &mut current {
                    alt.push_str(&t);
                }
            }
            Event::End(TagEnd::Image) => out.extend(current.take()),
            _ => {}
        }
    }
    out
}

#[derive(Default)]
struct Table {
    rows: Vec<Vec<String>>,
    header_rows: usize,
    cell: String,
    in_head: bool,
}

#[derive(Default)]
struct Renderer {
    width: usize,
    out: Rendered,
    /// The paragraph being built: text runs and their styles.
    runs: Vec<(String, Style)>,
    styles: Vec<Style>,
    /// Open lists: the next number for ordered ones.
    lists: Vec<Option<u64>>,
    /// The marker for the first line of the current list item.
    marker: Option<String>,
    quotes: usize,
    heading: Option<usize>,
    code: Option<String>,
    table: Option<Table>,
    /// Open links: their number, shown after the text.
    links: Vec<usize>,
    /// Inside an image: its URL and the alt text so far.
    image: Option<(String, String)>,
}

impl Renderer {
    fn style(&self) -> Style {
        self.styles.iter().fold(Style::new(), |a, s| a.patch(*s))
    }

    fn text(&mut self, t: &str) {
        if let Some((_, alt)) = &mut self.image {
            alt.push_str(t);
        } else if let Some(table) = &mut self.table {
            table.cell.push_str(t);
        } else if let Some(code) = &mut self.code {
            code.push_str(t);
        } else {
            let style = self.style();
            self.runs.push((t.to_string(), style));
        }
    }

    fn event(&mut self, e: Event) {
        match e {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(t) => self.text(&t),
            Event::Code(t) => {
                if self.table.is_some() || self.image.is_some() {
                    self.text(&t);
                } else {
                    let style = self.style().patch(theme::code());
                    self.runs.push((t.to_string(), style));
                }
            }
            Event::SoftBreak => self.text(" "),
            Event::HardBreak => self.text("\n"),
            Event::Html(t) | Event::InlineHtml(t) => {
                let style = self.style().patch(theme::dim());
                self.runs.push((t.trim_end_matches('\n').to_string(), style));
            }
            Event::Rule => {
                self.block_gap();
                let prefix = self.prefix(false);
                let w = self.width.saturating_sub(spans_width(&prefix));
                let mut spans = prefix;
                spans.push(Span::styled("─".repeat(w), theme::dim()));
                self.out.lines.push(Line::from(spans));
            }
            Event::TaskListMarker(done) => {
                let style = if done { theme::dim() } else { Style::new() };
                self.runs.push((if done { "[x] " } else { "[ ] " }.to_string(), style));
            }
            Event::FootnoteReference(t) | Event::InlineMath(t) | Event::DisplayMath(t) => self.text(&t),
        }
    }

    fn start(&mut self, tag: Tag) {
        match tag {
            Tag::Paragraph => self.block_gap(),
            Tag::Heading { level, .. } => {
                self.block_gap();
                self.heading = Some(level as usize);
                self.styles.push(theme::heading(level as usize));
            }
            Tag::BlockQuote(_) => {
                self.flush();
                self.block_gap();
                self.quotes += 1;
                self.styles.push(theme::quote());
            }
            Tag::CodeBlock(_) => {
                self.flush();
                self.block_gap();
                self.code = Some(String::new());
            }
            Tag::List(start) => {
                self.flush();
                if self.lists.is_empty() {
                    self.block_gap();
                }
                self.lists.push(start);
            }
            Tag::Item => {
                self.flush();
                let depth = self.lists.len();
                let marker = match self.lists.last_mut() {
                    Some(Some(n)) => {
                        *n += 1;
                        format!("{}. ", *n - 1)
                    }
                    _ => if depth > 1 { "◦ " } else { "• " }.to_string(),
                };
                self.marker = Some(marker);
            }
            Tag::Emphasis => self.styles.push(Style::new().add_modifier(Modifier::ITALIC)),
            Tag::Strong => self.styles.push(Style::new().add_modifier(Modifier::BOLD)),
            Tag::Strikethrough => self.styles.push(Style::new().add_modifier(Modifier::CROSSED_OUT)),
            Tag::Link { dest_url, .. } => {
                self.out.links.push(dest_url.to_string());
                self.links.push(self.out.links.len());
                self.styles.push(theme::link());
            }
            Tag::Image { dest_url, .. } => self.image = Some((dest_url.to_string(), String::new())),
            Tag::Table(_) => {
                self.flush();
                self.block_gap();
                self.table = Some(Table::default());
            }
            Tag::TableHead => {
                if let Some(t) = &mut self.table {
                    t.in_head = true;
                    t.rows.push(Vec::new());
                }
            }
            Tag::TableRow => {
                if let Some(t) = &mut self.table {
                    t.rows.push(Vec::new());
                }
            }
            Tag::TableCell => {
                if let Some(t) = &mut self.table {
                    t.cell.clear();
                }
            }
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph => self.flush(),
            TagEnd::Heading(_) => {
                self.flush();
                self.styles.pop();
                self.heading = None;
            }
            TagEnd::BlockQuote(_) => {
                self.flush();
                self.quotes -= 1;
                self.styles.pop();
            }
            TagEnd::CodeBlock => self.end_code(),
            TagEnd::List(_) => {
                self.flush();
                self.lists.pop();
            }
            TagEnd::Item => self.flush(),
            TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough => {
                self.styles.pop();
            }
            TagEnd::Link => {
                self.styles.pop();
                if let Some(n) = self.links.pop() {
                    self.runs.push((format!("[{n}]"), theme::dim()));
                }
            }
            TagEnd::Image => {
                if let Some((url, alt)) = self.image.take() {
                    self.flush();
                    self.block_gap();
                    let mut spans = self.prefix(true);
                    let label = if alt.is_empty() { "image".to_string() } else { alt.clone() };
                    spans.push(Span::styled(format!("□ {label}"), theme::dim()));
                    self.out.images.push(ImageRef { line: self.out.lines.len(), url, alt });
                    self.out.lines.push(Line::from(spans));
                }
            }
            TagEnd::TableCell => {
                if let Some(t) = &mut self.table {
                    let cell = std::mem::take(&mut t.cell);
                    if let Some(row) = t.rows.last_mut() {
                        row.push(cell.trim().to_string());
                    }
                }
            }
            TagEnd::TableHead => {
                if let Some(t) = &mut self.table {
                    t.in_head = false;
                    t.header_rows = t.rows.len();
                }
            }
            TagEnd::Table => self.end_table(),
            _ => {}
        }
    }

    /// A blank line before a block, except at the top and inside lists.
    fn block_gap(&mut self) {
        let last_blank =
            self.out.lines.last().is_none_or(|l| spans_width(&l.spans) == 0 || is_prefix_only(l));
        if self.lists.is_empty() && !last_blank {
            let prefix = self.prefix(false);
            self.out.lines.push(Line::from(prefix));
        }
    }

    /// Quote bars and list indentation; `first` takes the item's marker.
    fn prefix(&mut self, first: bool) -> Vec<Span<'static>> {
        let mut spans = Vec::new();
        for _ in 0..self.quotes {
            spans.push(Span::styled("│ ", theme::dim()));
        }
        if !self.lists.is_empty() {
            spans.push(Span::raw("  ".repeat(self.lists.len() - 1)));
            let marker = if first { self.marker.take() } else { None };
            let indent = if self.lists.last().copied().flatten().is_some() { "   " } else { "  " };
            match marker {
                Some(m) => spans.push(Span::styled(m, theme::dim())),
                None => spans.push(Span::raw(indent)),
            }
        }
        spans
    }

    /// Wrap the pending runs into lines.
    fn flush(&mut self) {
        if self.runs.is_empty() || self.runs.iter().all(|(t, _)| t.trim().is_empty()) {
            self.runs.clear();
            return;
        }
        let runs = std::mem::take(&mut self.runs);
        let first = self.prefix(true);
        let cont = self.prefix(false);
        let lines = wrap(&runs, first, cont, self.width);
        self.out.lines.extend(lines);
    }

    fn end_code(&mut self) {
        let Some(code) = self.code.take() else { return };
        let style = theme::code();
        for line in code.trim_end_matches('\n').split('\n') {
            let mut first = self.prefix(true);
            first.push(Span::styled("  ", style));
            let mut cont = self.prefix(false);
            cont.push(Span::styled("  ", style));
            if line.is_empty() {
                self.out.lines.push(Line::from(first));
                continue;
            }
            // Code keeps its spaces: break by width, not at words.
            let lines = wrap_chars(line, style, first, cont, self.width);
            self.out.lines.extend(lines);
        }
    }

    fn end_table(&mut self) {
        let Some(t) = self.table.take() else { return };
        let cols = t.rows.iter().map(Vec::len).max().unwrap_or(0);
        let prefix_w = spans_width(&self.prefix(false));
        let sep = 3; // " │ "
        let room = self.width.saturating_sub(prefix_w + sep * cols.saturating_sub(1));
        let mut widths: Vec<usize> = (0..cols)
            .map(|c| t.rows.iter().filter_map(|r| r.get(c)).map(|s| s.width()).max().unwrap_or(0))
            .collect();
        // Shrink the widest column until the table fits.
        while widths.iter().sum::<usize>() > room {
            let (i, w) = widths.iter().copied().enumerate().max_by_key(|&(_, w)| w).expect("a column");
            if w <= 3 {
                break;
            }
            widths[i] = w - 1;
        }
        for (r, row) in t.rows.iter().enumerate() {
            let mut spans = self.prefix(false);
            let style =
                if r < t.header_rows { Style::new().add_modifier(Modifier::BOLD) } else { Style::new() };
            for (c, w) in widths.iter().enumerate() {
                if c > 0 {
                    spans.push(Span::styled(" │ ", theme::dim()));
                }
                let cell = row.get(c).map(String::as_str).unwrap_or("");
                spans.push(Span::styled(pad(&truncate(cell, *w), *w), style));
            }
            self.out.lines.push(Line::from(spans));
            if r + 1 == t.header_rows {
                let mut rule = self.prefix(false);
                let parts: Vec<String> = widths.iter().map(|w| "─".repeat(*w)).collect();
                rule.push(Span::styled(parts.join("─┼─"), theme::dim()));
                self.out.lines.push(Line::from(rule));
            }
        }
    }
}

fn is_prefix_only(l: &Line) -> bool {
    l.spans.iter().all(|s| s.content.trim().is_empty() || s.content.trim() == "│")
}

fn spans_width(spans: &[Span]) -> usize {
    spans.iter().map(|s| s.content.width()).sum()
}

fn truncate(s: &str, w: usize) -> String {
    if s.width() <= w {
        return s.to_string();
    }
    let mut out = String::new();
    let mut used = 0;
    for c in s.chars() {
        let cw = c.width().unwrap_or(0);
        if used + cw + 1 > w {
            break;
        }
        out.push(c);
        used += cw;
    }
    out.push('…');
    out
}

fn pad(s: &str, w: usize) -> String {
    format!("{s}{}", " ".repeat(w.saturating_sub(s.width())))
}

/// A piece of text for wrapping.
enum Atom {
    Word(String, Style),
    Space(Style),
    Break,
}

fn atoms(runs: &[(String, Style)]) -> Vec<Atom> {
    let mut out = Vec::new();
    for (text, style) in runs {
        let mut word = String::new();
        let push_word = |word: &mut String, out: &mut Vec<Atom>| {
            if !word.is_empty() {
                out.push(Atom::Word(std::mem::take(word), *style));
            }
        };
        for c in text.chars() {
            if c == '\n' {
                push_word(&mut word, &mut out);
                out.push(Atom::Break);
            } else if c.is_whitespace() {
                push_word(&mut word, &mut out);
                out.push(Atom::Space(*style));
            } else if c.width() == Some(2) {
                // Wide (CJK) characters may break anywhere.
                push_word(&mut word, &mut out);
                out.push(Atom::Word(c.to_string(), *style));
            } else {
                word.push(c);
            }
        }
        push_word(&mut word, &mut out);
    }
    out
}

/// Greedy word wrap of styled runs; `first` and `cont` are line prefixes.
fn wrap(
    runs: &[(String, Style)],
    first: Vec<Span<'static>>,
    cont: Vec<Span<'static>>,
    width: usize,
) -> Vec<Line<'static>> {
    let avail = width.saturating_sub(spans_width(&cont)).max(1);
    let mut lines = Vec::new();
    let mut cur = first;
    let mut used = 0;
    let mut space: Option<Style> = None;
    let mut new_line = |cur: &mut Vec<Span<'static>>, used: &mut usize| {
        lines.push(Line::from(std::mem::replace(cur, cont.clone())));
        *used = 0;
    };
    for atom in atoms(runs) {
        match atom {
            Atom::Break => {
                new_line(&mut cur, &mut used);
                space = None;
            }
            Atom::Space(s) => {
                if used > 0 {
                    space = Some(s);
                }
            }
            Atom::Word(w, style) => {
                let ww = w.width();
                let gap = usize::from(space.is_some());
                if used > 0 && used + gap + ww > avail {
                    new_line(&mut cur, &mut used);
                    space = None;
                }
                if let Some(s) = space.take().filter(|_| used > 0) {
                    cur.push(Span::styled(" ", s));
                    used += 1;
                }
                if ww <= avail - used {
                    cur.push(Span::styled(w, style));
                    used += ww;
                } else {
                    // Longer than a line: break it by characters.
                    for c in w.chars() {
                        let cw = c.width().unwrap_or(0);
                        if used + cw > avail && used > 0 {
                            new_line(&mut cur, &mut used);
                        }
                        cur.push(Span::styled(c.to_string(), style));
                        used += cw;
                    }
                }
            }
        }
    }
    if used > 0 || lines.is_empty() {
        lines.push(Line::from(cur));
    }
    lines.into_iter().map(merge).collect()
}

/// Break one line of code by width, keeping every space.
fn wrap_chars(
    text: &str,
    style: Style,
    first: Vec<Span<'static>>,
    cont: Vec<Span<'static>>,
    width: usize,
) -> Vec<Line<'static>> {
    let avail = width.saturating_sub(spans_width(&cont)).max(1);
    let mut lines = Vec::new();
    let mut cur = first;
    let mut buf = String::new();
    let mut used = 0;
    for c in text.chars() {
        let cw = c.width().unwrap_or(0);
        if used + cw > avail && used > 0 {
            cur.push(Span::styled(std::mem::take(&mut buf), style));
            lines.push(Line::from(std::mem::replace(&mut cur, cont.clone())));
            used = 0;
        }
        buf.push(c);
        used += cw;
    }
    cur.push(Span::styled(buf, style));
    lines.push(Line::from(cur));
    lines
}

/// Join neighbouring spans of the same style.
fn merge(line: Line<'static>) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    for s in line.spans {
        match spans.last_mut() {
            Some(last) if last.style == s.style => {
                last.content = format!("{}{}", last.content, s.content).into()
            }
            _ => spans.push(s),
        }
    }
    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(r: &Rendered) -> Vec<String> {
        r.lines.iter().map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect::<String>()).collect()
    }

    fn lines(md: &str, width: u16) -> Vec<String> {
        text(&render(md, width))
    }

    fn style_of(r: &Rendered, needle: &str) -> Style {
        r.lines.iter().flat_map(|l| &l.spans).find(|s| s.content.contains(needle)).expect(needle).style
    }

    #[test]
    fn empty() {
        assert!(render("", 40).lines.is_empty());
        assert!(render("   \n\n", 40).lines.is_empty());
    }

    #[test]
    fn paragraphs_wrap_and_are_separated() {
        let l = lines("one two three four five six\n\nsecond", 12);
        assert_eq!(l, ["one two", "three four", "five six", "", "second"]);
        assert!(l.iter().all(|x| x.width() <= 12));
    }

    #[test]
    fn soft_and_hard_breaks() {
        assert_eq!(lines("a\nb", 40), ["a b"]);
        assert_eq!(lines("a  \nb", 40), ["a", "b"]);
    }

    #[test]
    fn cjk_wraps_by_cell_and_anywhere() {
        let l = lines("優惠碼後面有空白時結帳會失敗", 10);
        assert!(l.iter().all(|x| x.width() <= 10), "{l:?}");
        assert_eq!(l.concat(), "優惠碼後面有空白時結帳會失敗");
        assert_eq!(l[0].width(), 10);
    }

    #[test]
    fn long_words_are_broken() {
        let l = lines("https://example.com/a/very/long/path", 10);
        assert!(l.iter().all(|x| x.width() <= 10), "{l:?}");
        assert_eq!(l.concat(), "https://example.com/a/very/long/path");
    }

    #[test]
    fn inline_styles() {
        let r = render("**bold** *it* ~~gone~~ `code`", 40);
        assert!(style_of(&r, "bold").add_modifier.contains(Modifier::BOLD));
        assert!(style_of(&r, "it").add_modifier.contains(Modifier::ITALIC));
        assert!(style_of(&r, "gone").add_modifier.contains(Modifier::CROSSED_OUT));
        assert_eq!(style_of(&r, "code").fg, Some(Color::Yellow));
        assert_eq!(text(&r), ["bold it gone code"]);
    }

    #[test]
    fn headings() {
        let r = render("# Title\n\ntext\n\n### Small", 40);
        assert_eq!(text(&r), ["Title", "", "text", "", "Small"]);
        assert!(style_of(&r, "Title").add_modifier.contains(Modifier::UNDERLINED));
        let small = style_of(&r, "Small");
        assert!(
            small.add_modifier.contains(Modifier::BOLD) && !small.add_modifier.contains(Modifier::UNDERLINED)
        );
    }

    #[test]
    fn lists() {
        let md = "- one\n- two\n  - nested\n\n1. first\n2. second\n";
        assert_eq!(lines(md, 40), ["• one", "• two", "  ◦ nested", "", "1. first", "2. second"]);
    }

    #[test]
    fn list_items_wrap_under_their_text() {
        assert_eq!(lines("- alpha beta gamma", 12), ["• alpha beta", "  gamma"]);
        assert_eq!(lines("3. alpha beta gamma", 12), ["3. alpha", "   beta", "   gamma"]);
    }

    #[test]
    fn loose_list_paragraphs() {
        assert_eq!(lines("- one\n\n  more\n- two\n", 40), ["• one", "  more", "• two"]);
    }

    #[test]
    fn task_lists() {
        let r = render("- [x] done\n- [ ] todo", 40);
        assert_eq!(text(&r), ["• [x] done", "• [ ] todo"]);
        assert!(style_of(&r, "[x]").add_modifier.contains(Modifier::DIM));
    }

    #[test]
    fn code_blocks_keep_spaces_and_break_by_width() {
        let r = render("before\n\n```json\n{\n  \"a\": 1\n\n}\n```\nafter", 40);
        assert_eq!(text(&r), ["before", "", "  {", "    \"a\": 1", "  ", "  }", "", "after"]);
        assert_eq!(style_of(&r, "\"a\"").fg, Some(Color::Yellow));
        let long = lines("```\nabcdefghijklmnop\n```", 10);
        assert_eq!(long, ["  abcdefgh", "  ijklmnop"]);
    }

    #[test]
    fn quotes() {
        let r = render("> quoted words here\n\nafter", 14);
        assert_eq!(text(&r), ["│ quoted words", "│ here", "", "after"]);
        assert!(style_of(&r, "quoted").add_modifier.contains(Modifier::ITALIC));
    }

    #[test]
    fn links_are_numbered_and_collected() {
        let r = render("see [the PR](https://x.example/1) and [docs](https://x.example/2)", 60);
        assert_eq!(text(&r), ["see the PR[1] and docs[2]"]);
        assert_eq!(r.links, ["https://x.example/1", "https://x.example/2"]);
        assert!(style_of(&r, "the PR").add_modifier.contains(Modifier::UNDERLINED));
    }

    #[test]
    fn images_get_a_placeholder_line() {
        let r = render("text\n\n![shot.png](https://uploads.linear.app/a/b)\n\n![](https://x/y.png)", 40);
        assert_eq!(text(&r), ["text", "", "□ shot.png", "", "□ image"]);
        assert_eq!(
            r.images[0],
            ImageRef { line: 2, url: "https://uploads.linear.app/a/b".into(), alt: "shot.png".into() }
        );
        assert_eq!(r.images[1].line, 4);
    }

    #[test]
    fn lists_image_urls() {
        let md = "a ![one](https://x/1.png) b\n\n![`two`](https://x/2.png)\n\n[not](https://x/3)";
        assert_eq!(
            image_urls(md),
            [
                ("https://x/1.png".to_string(), "one".to_string()),
                ("https://x/2.png".to_string(), "two".to_string())
            ]
        );
    }

    #[test]
    fn rule() {
        assert_eq!(lines("a\n\n---\n\nb", 10), ["a", "", "──────────", "", "b"]);
    }

    #[test]
    fn tables() {
        let md = "| Step | Result |\n|---|---|\n| `paste` | 422 |\n| trim | ok |";
        assert_eq!(lines(md, 40), ["Step  │ Result", "──────┼───────", "paste │ 422   ", "trim  │ ok    "]);
        let narrow = lines("| a very long header | b |\n|---|---|\n| x | y |", 16);
        assert!(narrow.iter().all(|l| l.width() <= 16), "{narrow:?}");
        assert!(narrow[0].contains('…'));
    }

    #[test]
    fn html_is_shown_dim() {
        let r = render("a <b>c</b>", 40);
        assert_eq!(text(&r), ["a <b>c</b>"]);
        assert!(style_of(&r, "<b>").add_modifier.contains(Modifier::DIM));
    }

    #[test]
    fn helpers() {
        assert_eq!(truncate("abcdef", 4), "abc…");
        assert_eq!(truncate("ab", 4), "ab");
        assert_eq!(pad("ab", 4), "ab  ");
    }

    #[test]
    fn the_demo_issue_renders_within_width() {
        let seed: serde_json::Value = serde_json::from_str(crate::fake::SEED).unwrap();
        for issue in seed["issues"].as_array().unwrap() {
            for w in [30, 60, 100] {
                let r = render(issue["description"].as_str().unwrap(), w);
                let too_wide: Vec<_> = text(&r).into_iter().filter(|l| l.width() > usize::from(w)).collect();
                assert!(too_wide.is_empty(), "{} at {w}: {too_wide:?}", issue["identifier"]);
            }
        }
    }
}
