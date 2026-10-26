//! Drawing the app: the issue list, the issue detail (beside the list on
//! wide terminals), the header, the status line and help.

use chrono::{DateTime, Utc};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect, Size};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Clear, Paragraph, Row, Table, TableState};
use ratatui_image::picker::Picker;
use ratatui_image::{FilterType, FontSize, Resize, StatefulImage};
use unicode_width::UnicodeWidthStr;

use crate::app::{App, Confirm, FORM_FIELDS, FilterPanel, Focus, Form, ImageSlot, Overlay, PickList, SLOTS};
use crate::markdown;
use crate::model::{IssueDetail, IssueRow, Label, State, User};

/// Columns at which the detail goes beside the list instead of over it.
pub const SPLIT_WIDTH: u16 = 140;

const ACCENT: Color = Color::Rgb(0x5e, 0x6a, 0xd2);

fn dim() -> Style {
    Style::new().fg(Color::DarkGray)
}

/// `#rrggbb` to a colour.
pub fn hex(s: Option<&str>) -> Option<Color> {
    let h = s?.strip_prefix('#')?;
    if h.len() != 6 || !h.is_ascii() {
        return None;
    }
    let p = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).ok();
    Some(Color::Rgb(p(0)?, p(2)?, p(4)?))
}

/// The glyph for a workflow state type.
pub fn state_icon(kind: &str) -> &'static str {
    match kind {
        "triage" => "◇",
        "backlog" => "◌",
        "unstarted" => "○",
        "started" => "◔",
        "completed" => "●",
        "canceled" | "duplicate" => "⊘",
        _ => "·",
    }
}

fn state_color(s: &State) -> Color {
    hex(s.color.as_deref()).unwrap_or(match s.kind.as_str() {
        "triage" => Color::Rgb(0xfc, 0x78, 0x40),
        "started" => Color::Yellow,
        "completed" => ACCENT,
        _ => Color::Gray,
    })
}

fn state_span(s: Option<&State>, with_name: bool) -> Vec<Span<'static>> {
    let Some(s) = s else { return vec![Span::raw(" ")] };
    let c = state_color(s);
    let mut v = vec![Span::styled(state_icon(&s.kind), Style::new().fg(c))];
    if with_name {
        v.push(Span::raw(format!(" {}", s.name)));
    }
    v
}

/// Priority as three cells: `!!!` for urgent, bars for high to low.
pub fn priority_spans(p: i64) -> Vec<Span<'static>> {
    let on = Style::new().fg(Color::Gray);
    let off = dim();
    match p {
        1 => vec![Span::styled(
            "!!!",
            Style::new().fg(Color::Rgb(0xf2, 0x99, 0x4a)).add_modifier(Modifier::BOLD),
        )],
        2 => vec![Span::styled("▂▄▆", on)],
        3 => vec![Span::styled("▂▄", on), Span::styled("▆", off)],
        4 => vec![Span::styled("▂", on), Span::styled("▄▆", off)],
        _ => vec![Span::styled("───", off)],
    }
}

fn label_spans(labels: &[Label]) -> Vec<Span<'static>> {
    let mut v = Vec::new();
    for (i, l) in labels.iter().enumerate() {
        if i > 0 {
            v.push(Span::raw("  "));
        }
        v.push(Span::styled("●", Style::new().fg(hex(l.color.as_deref()).unwrap_or(Color::Gray))));
        v.push(Span::styled(format!(" {}", l.name), Style::new().fg(Color::Gray)));
    }
    v
}

/// "now", "5m", "3h", "2d", "4w", or the date for older.
pub fn ago(when: &str, now: DateTime<Utc>) -> String {
    let Ok(t) = DateTime::parse_from_rfc3339(when) else {
        return String::new();
    };
    let s = (now - t.with_timezone(&Utc)).num_seconds().max(0);
    match s {
        0..60 => "now".into(),
        60..3600 => format!("{}m", s / 60),
        3600..86_400 => format!("{}h", s / 3600),
        86_400..2_419_200 if s < 7 * 86_400 => format!("{}d", s / 86_400),
        86_400..2_419_200 => format!("{}w", s / (7 * 86_400)),
        _ => t.format("%b %-d").to_string(),
    }
}

fn initials(u: Option<&User>) -> String {
    u.map(User::initials).unwrap_or_default()
}

pub fn render(app: &mut App, f: &mut Frame) {
    let [header, body, footer] =
        Layout::vertical([Constraint::Length(1), Constraint::Min(1), Constraint::Length(1)]).areas(f.area());
    render_header(app, f, header);
    render_footer(app, f, footer);
    let detail_open = app.detail.is_some() && app.focus == Focus::Detail;
    if detail_open && body.width >= SPLIT_WIDTH {
        let [list, detail] =
            Layout::horizontal([Constraint::Percentage(45), Constraint::Percentage(55)]).areas(body);
        render_list(app, f, list, true);
        render_detail(app, f, detail, true);
    } else if detail_open {
        render_detail(app, f, body, false);
    } else {
        render_list(app, f, body, false);
    }
    if let Some(form) = &app.form {
        render_form(form, f, body);
    }
    if let Some(panel) = &app.filters {
        render_filters(panel, f, body);
    }
    match &app.overlay {
        Some(Overlay::Pick(p)) => render_picker(p, f, body),
        Some(Overlay::Confirm(c)) => render_confirm(c, f, body),
        None => {}
    }
    if app.help {
        render_help(f, body);
    }
}

/// A box centred in `area`, at most `w` x `h`.
fn centered(area: Rect, w: u16, h: u16) -> Rect {
    let (w, h) = (w.min(area.width), h.min(area.height));
    Rect { x: area.x + (area.width - w) / 2, y: area.y + (area.height - h) / 2, width: w, height: h }
}

fn render_picker(p: &PickList, f: &mut Frame, area: Rect) {
    let items = p.visible();
    let rows = u16::try_from(items.len().max(1)).unwrap_or(u16::MAX);
    let rect = centered(area, 48, rows.saturating_add(4).min(20));
    let mut lines = vec![Line::from(vec![
        Span::styled("› ", Style::new().fg(ACCENT)),
        Span::raw(format!("{}▏", p.query)),
    ])];
    if p.waiting.is_some() {
        lines.push(Line::from(Span::styled("Loading…", dim())));
    } else if items.is_empty() {
        lines.push(Line::from(Span::styled("No match.", dim())));
    }
    let room = usize::from(rect.height.saturating_sub(3));
    let first = p.cursor.saturating_sub(room.saturating_sub(1));
    for (n, (_, item)) in items.iter().enumerate().skip(first).take(room) {
        let mark = match (p.multi, item.chosen) {
            (true, true) => "[x] ",
            (true, false) => "[ ] ",
            (false, true) => "✓ ",
            (false, false) => "  ",
        };
        let mut spans = vec![Span::styled(mark, Style::new().fg(ACCENT))];
        if let Some(c) = hex(item.color.as_deref()) {
            spans.push(Span::styled("● ", Style::new().fg(c)));
        }
        spans.push(Span::raw(item.label.clone()));
        if !item.hint.is_empty() && item.hint != item.label {
            spans.push(Span::styled(format!("  {}", item.hint), dim()));
        }
        let mut line = Line::from(spans);
        if n == p.cursor {
            line = line.style(Style::new().bg(Color::Rgb(0x2a, 0x2d, 0x3e)).add_modifier(Modifier::BOLD));
        }
        lines.push(line);
    }
    let title = format!(" {} ", p.title);
    f.render_widget(Clear, rect);
    f.render_widget(
        Paragraph::new(lines).block(Block::bordered().title(title).border_style(Style::new().fg(ACCENT))),
        rect,
    );
}

fn render_confirm(c: &Confirm, f: &mut Frame, area: Rect) {
    let w = 92.min(area.width.saturating_sub(4)).max(20);
    let mut lines: Vec<Line> = c.lines.iter().map(|l| Line::raw(l.clone())).collect();
    lines.push(Line::raw(""));
    for chunk in wrap_plain(&c.command, usize::from(w.saturating_sub(4))) {
        lines.push(Line::from(Span::styled(chunk, dim())));
    }
    lines.push(Line::raw(""));
    lines.push(Line::from(vec![
        Span::styled("y", Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)),
        Span::raw(" send   "),
        Span::styled("n", Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)),
        Span::raw(" cancel"),
    ]));
    let h = u16::try_from(lines.len()).unwrap_or(u16::MAX).saturating_add(2);
    let rect = centered(area, w, h);
    f.render_widget(Clear, rect);
    let block = Block::bordered().title(format!(" {} ", c.title)).border_style(Style::new().fg(ACCENT));
    f.render_widget(Paragraph::new(lines).block(block), rect);
}

/// Break plain text into lines of at most `w` cells.
fn wrap_plain(s: &str, w: usize) -> Vec<String> {
    let mut out = vec![String::new()];
    for word in s.split(' ') {
        let last = out.last_mut().expect("one line");
        if !last.is_empty() && last.width() + 1 + word.width() > w {
            out.push(word.to_string());
        } else {
            if !last.is_empty() {
                last.push(' ');
            }
            last.push_str(word);
        }
    }
    out
}

fn render_filters(panel: &FilterPanel, f: &mut Frame, area: Rect) {
    let mut lines = Vec::new();
    for (i, slot) in panel.slots.iter().enumerate() {
        let name = SLOTS.iter().find(|s| s.0 == *slot).map_or("", |s| s.1);
        let focused = panel.row == i;
        let style = if focused { Style::new().fg(ACCENT).add_modifier(Modifier::BOLD) } else { dim() };
        let values = panel.get(*slot);
        let value = if values.is_empty() { Span::styled("any", dim()) } else { Span::raw(values.join(", ")) };
        lines.push(Line::from(vec![
            Span::styled(format!("{} {name:<13}", if focused { "›" } else { " " }), style),
            value,
        ]));
    }
    if !panel.extra.is_empty() {
        lines
            .push(Line::from(vec![Span::styled("  Also         ", dim()), Span::raw(panel.extra.join(" "))]));
    }
    lines.push(Line::raw(""));
    let mut argv = vec!["issue".to_string(), "list".to_string()];
    argv.extend(panel.args());
    let command = crate::app::shell_line(&argv, None);
    for chunk in wrap_plain(&command, 94) {
        lines.push(Line::from(Span::styled(format!("  {chunk}"), dim())));
    }
    lines.push(Line::raw(""));
    lines.push(match &panel.naming {
        Some(name) => Line::from(vec![
            Span::styled("  Save as view: ", Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)),
            Span::raw(format!("{name}▏")),
            Span::styled("   enter save · esc back", dim()),
        ]),
        None => Line::from(Span::styled(
            "  ↑↓ row · enter pick · d clear · ctrl-a apply · ctrl-s save as view · esc close",
            dim(),
        )),
    });
    let rect = centered(area, 100, u16::try_from(lines.len()).unwrap_or(u16::MAX).saturating_add(2));
    f.render_widget(Clear, rect);
    let block = Block::bordered().title(" Filters ").border_style(Style::new().fg(ACCENT));
    f.render_widget(Paragraph::new(lines).block(block), rect);
}

fn render_form(form: &Form, f: &mut Frame, area: Rect) {
    let label = |name: &str, i: usize| {
        let focused = form.field == i;
        let style = if focused { Style::new().fg(ACCENT).add_modifier(Modifier::BOLD) } else { dim() };
        Span::styled(format!("{} {name:<13}", if focused { "›" } else { " " }), style)
    };
    let value = |v: Option<String>, empty: &str| match v.filter(|v| !v.is_empty()) {
        Some(v) => Span::raw(v),
        None => Span::styled(empty.to_string(), dim()),
    };
    let priorities = ["No priority", "Urgent", "High", "Medium", "Low"];
    let mut lines = Vec::new();
    for (i, name) in FORM_FIELDS.iter().enumerate() {
        let v = match *name {
            "Title" => {
                let cursor = if form.field == i { "▏" } else { "" };
                Span::styled(format!("{}{cursor}", form.title), Style::new().add_modifier(Modifier::BOLD))
            }
            "Team" => value(Some(form.team.clone()), "pick a team"),
            "Status" => value(form.state.clone(), "the team's default"),
            "Priority" => value(
                priorities.get(usize::try_from(form.priority).unwrap_or(0)).map(|s| (*s).to_string()),
                "",
            ),
            "Assignee" => value(form.assignee.as_ref().map(|a| a.1.clone()), "unassigned"),
            "Labels" => value(Some(form.labels.join(", ")), "none"),
            "Project" => value(form.project.clone(), "none"),
            _ => value(form.description.lines().next().map(str::to_string), "enter opens $EDITOR"),
        };
        lines.push(Line::from(vec![label(name, i), v]));
        if *name == "Description" {
            for l in form.description.lines().skip(1).take(6) {
                lines.push(Line::from(vec![Span::raw(" ".repeat(15)), Span::styled(l.to_string(), dim())]));
            }
        }
    }
    lines.push(Line::raw(""));
    lines.push(Line::from(Span::styled(
        "  tab/↑↓ field · enter pick or edit · ctrl-s create · esc discard",
        dim(),
    )));
    let rect = centered(area, 100, u16::try_from(lines.len()).unwrap_or(u16::MAX).saturating_add(2));
    f.render_widget(Clear, rect);
    let block = Block::bordered().title(" New issue ").border_style(Style::new().fg(ACCENT));
    f.render_widget(Paragraph::new(lines).block(block), rect);
}

fn render_header(app: &App, f: &mut Frame, area: Rect) {
    let org = app.auth.as_ref().map_or("Linear", |a| a.organization.name.as_str());
    let mut spans = vec![Span::styled(
        format!(" {org} "),
        Style::new().fg(Color::White).bg(ACCENT).add_modifier(Modifier::BOLD),
    )];
    if app.search.is_none() && app.filtered.is_some() {
        spans.push(Span::raw("  "));
        spans.push(Span::styled("Filter", Style::new().add_modifier(Modifier::BOLD | Modifier::UNDERLINED)));
    }
    if let Some(q) = &app.search {
        spans.push(Span::raw("  "));
        spans.push(Span::styled(
            format!("Search \"{q}\""),
            Style::new().add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
        ));
    }
    for (i, v) in app.views.iter().enumerate() {
        let style = if i == app.view && app.search.is_none() && app.filtered.is_none() {
            Style::new().add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
        } else {
            dim()
        };
        spans.push(Span::raw("  "));
        spans.push(Span::styled(format!("{} {}", i + 1, v.name), style));
    }
    let right = if app.loading {
        "loading…".to_string()
    } else {
        let n = app.visible().len();
        let more = if app.next_cursor.is_some() { "+" } else { "" };
        let by = app.order().map(|o| format!("by {o} · ")).unwrap_or_default();
        format!("{by}{n}{more} issues")
    };
    let live = match &app.live {
        Some(l) if l.ready && l.mode == "poll" => {
            Span::styled("● live · polling  ", Style::new().fg(Color::Green))
        }
        Some(l) if l.ready => Span::styled("● live  ", Style::new().fg(Color::Green)),
        Some(_) => Span::styled("○ connecting  ", dim()),
        None => Span::raw(""),
    };
    let left_w: usize = spans.iter().map(|s| s.content.width()).sum();
    let pad = usize::from(area.width).saturating_sub(left_w + live.content.width() + right.width() + 1);
    spans.push(Span::raw(" ".repeat(pad)));
    spans.push(live);
    spans.push(Span::styled(right, dim()));
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn render_footer(app: &App, f: &mut Frame, area: Rect) {
    let slash = || Span::styled(" / ", Style::new().fg(ACCENT).add_modifier(Modifier::BOLD));
    let line = if app.filtering {
        Line::from(vec![
            slash(),
            Span::raw(format!("{}▏", app.filter)),
            Span::styled("   enter search all issues · ↓ keep as filter · esc clear", dim()),
        ])
    } else if let Some(s) = &app.status {
        Line::from(Span::styled(format!(" {s}"), Style::new().fg(Color::Red)))
    } else if let Some(n) = &app.notice {
        Line::from(Span::styled(format!(" ✓ {n}"), Style::new().fg(Color::Green)))
    } else if !app.filter.is_empty() {
        Line::from(vec![slash(), Span::raw(app.filter.clone()), Span::styled("   esc clear", dim())])
    } else {
        let hints = match (app.focus, &app.overlay, &app.form) {
            (_, Some(Overlay::Pick(p)), _) if p.multi => {
                "type to filter · ↑↓ move · space toggle · enter apply · esc cancel"
            }
            (_, Some(Overlay::Pick(_)), _) => "type to filter · ↑↓ move · enter choose · esc cancel",
            (_, Some(Overlay::Confirm(_)), _) => "y send · n cancel",
            (_, None, Some(_)) => "tab/↑↓ field · enter pick or edit · ctrl-s create · esc discard",
            (_, None, None) if app.filters.is_some() => {
                "↑↓ row · enter pick · d clear · ctrl-a apply · ctrl-s save · esc close"
            }
            (Focus::List, None, None) => {
                "j/k move · enter open · / filter · c new · s state · a assignee · p priority · ? help · q quit"
            }
            (Focus::Detail, None, None) => {
                "j/k scroll · c comment · R reply · E description · s/a/p/L edit · ]/[ next · esc back · ? help"
            }
        };
        Line::from(Span::styled(format!(" {hints}"), dim()))
    };
    f.render_widget(Paragraph::new(line), area);
}

fn render_list(app: &App, f: &mut Frame, area: Rect, narrow: bool) {
    let rows = app.visible();
    if rows.is_empty() {
        let msg = if app.loading {
            "Loading…".to_string()
        } else if !app.filter.is_empty() {
            "No issues match the filter.".to_string()
        } else if let Some(q) = &app.search {
            format!("No issues match \"{q}\".")
        } else {
            "No issues in this view.".to_string()
        };
        f.render_widget(Paragraph::new(Span::styled(format!(" {msg}"), dim())), area);
        return;
    }
    let wide = !narrow && area.width >= 110;
    let table_rows: Vec<Row> = rows.iter().map(|r| list_row(r, app.now, wide)).collect();
    let mut widths =
        vec![Constraint::Length(3), Constraint::Length(8), Constraint::Length(1), Constraint::Fill(1)];
    if wide {
        widths.extend([Constraint::Length(2), Constraint::Length(28), Constraint::Length(4)]);
    } else {
        widths.push(Constraint::Length(2));
    }
    let highlight = Style::new().bg(Color::Rgb(0x2a, 0x2d, 0x3e)).add_modifier(Modifier::BOLD);
    let table = Table::new(table_rows, widths).column_spacing(2).row_highlight_style(highlight);
    let mut state = TableState::default().with_selected(Some(app.selected));
    f.render_stateful_widget(table, area, &mut state);
}

fn list_row(r: &IssueRow, now: DateTime<Utc>, wide: bool) -> Row<'static> {
    let mut cells = vec![
        Cell::from(Line::from(priority_spans(r.priority))),
        Cell::from(Span::styled(r.identifier.clone(), dim())),
        Cell::from(Line::from(state_span(r.state.as_ref(), false))),
        Cell::from(r.title.clone()),
        Cell::from(Span::styled(initials(r.assignee.as_ref()), Style::new().fg(Color::Cyan))),
    ];
    if wide {
        cells.push(Cell::from(Line::from(label_spans(&r.labels))));
        cells.push(Cell::from(Span::styled(ago(&r.updated_at, now), dim())));
    }
    Row::new(cells)
}

/// How an image in the text is shown.
#[derive(Debug, Clone, PartialEq)]
pub enum ImageView {
    /// Drawn in a box of this many cells.
    Box { cols: u16, rows: u16 },
    /// Not drawn: a note after its name says why.
    Note(String),
}

/// Where a drawn image goes in the detail's lines.
#[derive(Debug, Clone, PartialEq)]
pub struct Placement {
    pub line: usize,
    pub x: u16,
    pub cols: u16,
    pub rows: u16,
    /// The upload path, the key into `App::images`.
    pub path: String,
}

/// The detail laid out: its lines, and the images to draw over them.
#[derive(Debug, Default)]
pub struct DetailLayout {
    pub lines: Vec<Line<'static>>,
    pub images: Vec<Placement>,
}

/// Decides how each image is shown, given its URL and the columns free.
pub type ImageViewer<'a> = dyn Fn(&str, u16) -> ImageView + 'a;

/// Markdown into `out`, indented by `pad`, reserving rows for images.
fn push_markdown(out: &mut DetailLayout, md: &str, width: u16, pad: u16, view: &ImageViewer) {
    let r = markdown::render(md, width);
    for (k, line) in r.lines.into_iter().enumerate() {
        let mut spans = vec![Span::raw(" ".repeat(usize::from(pad)))];
        let image = r.images.iter().find(|im| im.line == k);
        let prefix_w: usize = line.spans.iter().rev().skip(1).map(|s| s.content.width()).sum();
        spans.extend(line.spans);
        let Some(image) = image else {
            out.lines.push(Line::from(spans));
            continue;
        };
        let free = width.saturating_sub(u16::try_from(prefix_w).unwrap_or(width));
        match (view(&image.url, free), crate::images::upload_path(&image.url)) {
            (ImageView::Box { cols, rows }, Some(path)) => {
                out.images.push(Placement {
                    line: out.lines.len(),
                    x: pad + u16::try_from(prefix_w).unwrap_or(0),
                    cols,
                    rows,
                    path: path.to_string(),
                });
                out.lines.push(Line::from(spans));
                out.lines.extend((1..rows).map(|_| Line::raw("")));
            }
            (ImageView::Note(note), _) => {
                spans.push(Span::styled(format!("  {note}"), dim()));
                out.lines.push(Line::from(spans));
            }
            (ImageView::Box { .. }, None) => out.lines.push(Line::from(spans)),
        }
    }
}

/// The detail laid out for a pane `width` wide.
pub fn detail_lines(i: &IssueDetail, width: u16, now: DateTime<Utc>, view: &ImageViewer) -> DetailLayout {
    let w = width.saturating_sub(2).max(10);
    let mut layout = DetailLayout::default();
    let out = &mut layout.lines;
    let row = &i.row;
    out.push(Line::from(vec![
        Span::styled(row.identifier.clone(), dim()),
        Span::raw("  "),
        Span::styled(row.title.clone(), Style::new().add_modifier(Modifier::BOLD)),
    ]));
    let sep = || Span::styled("  ·  ", dim());
    let mut meta = state_span(row.state.as_ref(), true);
    meta.push(sep());
    meta.extend(priority_spans(row.priority));
    meta.push(Span::raw(format!(" {}", row.priority_label)));
    meta.push(sep());
    meta.push(match &row.assignee {
        Some(a) => Span::styled(a.name.clone(), Style::new().fg(Color::Cyan)),
        None => Span::styled("Unassigned", dim()),
    });
    if let Some(e) = i.estimate {
        meta.push(sep());
        meta.push(Span::raw(format!("{e} pt")));
    }
    out.push(Line::from(meta));
    let mut meta2 = Vec::new();
    if let Some(p) = &row.project {
        meta2.push(Span::styled(format!("▸ {}", p.name), Style::new().fg(Color::Gray)));
    }
    if let Some(c) = &row.cycle {
        if !meta2.is_empty() {
            meta2.push(sep());
        }
        meta2.push(Span::styled(format!("Cycle {}", c.number), Style::new().fg(Color::Gray)));
    }
    if let Some(d) = &i.due_date {
        if !meta2.is_empty() {
            meta2.push(sep());
        }
        meta2.push(Span::styled(format!("Due {d}"), Style::new().fg(Color::Rgb(0xf2, 0x99, 0x4a))));
    }
    if !row.labels.is_empty() {
        if !meta2.is_empty() {
            meta2.push(sep());
        }
        meta2.extend(label_spans(&row.labels));
    }
    if !meta2.is_empty() {
        out.push(Line::from(meta2));
    }
    if let Some(p) = &i.parent {
        out.push(Line::from(vec![
            Span::styled("↑ ", dim()),
            Span::styled(p.identifier.clone(), dim()),
            Span::raw(format!(" {}", p.title)),
        ]));
    }
    out.push(Line::from(Span::styled("─".repeat(usize::from(w)), dim())));
    match i.description.as_deref().filter(|d| !d.trim().is_empty()) {
        Some(d) => push_markdown(&mut layout, d, w, 0, view),
        None => out.push(Line::from(Span::styled("No description.", dim()))),
    }
    let out = &mut layout.lines;
    let section =
        |title: String| Line::from(Span::styled(title, Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)));
    if !i.children.is_empty() {
        out.push(Line::raw(""));
        out.push(section(format!("Sub-issues ({})", i.children.len())));
        for c in &i.children {
            let mut l = state_span(c.state.as_ref(), false);
            l.push(Span::styled(format!(" {}", c.identifier), dim()));
            l.push(Span::raw(format!(" {}", c.title)));
            out.push(Line::from(l));
        }
    }
    if !i.relations.is_empty() {
        out.push(Line::raw(""));
        out.push(section(format!("Relations ({})", i.relations.len())));
        for r in &i.relations {
            out.push(Line::from(vec![
                Span::styled(format!("{} ", r.kind), dim()),
                Span::styled(r.related_issue.identifier.clone(), dim()),
                Span::raw(format!(" {}", r.related_issue.title)),
            ]));
        }
    }
    if !i.attachments.is_empty() {
        out.push(Line::raw(""));
        out.push(section(format!("Attachments ({})", i.attachments.len())));
        for a in &i.attachments {
            out.push(Line::from(vec![Span::styled("↗ ", dim()), Span::raw(a.title.clone())]));
        }
    }
    let threads = i.threads();
    out.push(Line::raw(""));
    out.push(section(format!("Comments ({})", threads.len())));
    if threads.is_empty() {
        out.push(Line::from(Span::styled("No comments yet.", dim())));
    }
    for (c, reply) in threads {
        let indent = if reply { "    " } else { "" };
        let name = c.user.as_ref().map_or("Someone", |u| u.name.as_str()).to_string();
        let mut head = vec![Span::raw(indent)];
        if reply {
            head.push(Span::styled("↪ ", dim()));
        }
        head.push(Span::styled(name, Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD)));
        head.push(Span::styled(format!(" · {}", ago(&c.created_at, now)), dim()));
        layout.lines.push(Line::from(head));
        let pad = if reply { 6 } else { 2 };
        push_markdown(&mut layout, &c.body, w.saturating_sub(pad), pad, view);
    }
    layout
}

/// How images look given what the app has loaded: drawn at their natural
/// size or smaller, at most `max_rows` tall.
fn viewer<'a>(app: &'a App, font: FontSize, max_rows: u16) -> impl Fn(&str, u16) -> ImageView + 'a {
    move |url, free| {
        let Some(path) = crate::images::upload_path(url) else {
            return ImageView::Note("external image, not loaded".into());
        };
        if !app.images_on {
            return ImageView::Note("images off (i)".into());
        }
        match app.images.get(path) {
            None | Some(ImageSlot::Loading) => ImageView::Note("loading…".into()),
            Some(ImageSlot::Failed(e)) => ImageView::Note(format!("not shown: {e}")),
            Some(ImageSlot::Ready(pic)) => {
                let fit = Resize::Fit(None).size_for(&pic.image, font, Size::new(free, max_rows));
                let natural = Resize::natural_size(&pic.image, font);
                let cols = fit.width.min(natural.width).max(1);
                let rows = fit.height.min(natural.height).max(1);
                ImageView::Box { cols, rows }
            }
        }
    }
}

fn render_detail(app: &mut App, f: &mut Frame, area: Rect, beside: bool) {
    let block = if beside { Block::new().borders(Borders::LEFT).border_style(dim()) } else { Block::new() };
    let inner = block.inner(area);
    f.render_widget(block, area);
    let inner = Rect { x: inner.x + 1, width: inner.width.saturating_sub(1), ..inner };
    let font = app.picker.as_ref().map_or(FontSize::new(10, 20), Picker::font_size);
    let max_rows = (inner.height * 3 / 5).clamp(1, 30);
    let Some(d) = app.detail.as_ref() else { return };
    let layout = match (&d.issue, &d.error) {
        (_, Some(e)) => DetailLayout {
            lines: vec![Line::from(Span::styled(format!("{}: {e}", d.id), Style::new().fg(Color::Red)))],
            images: Vec::new(),
        },
        (Some(i), None) => detail_lines(i, inner.width, app.now, &viewer(app, font, max_rows)),
        (None, None) => DetailLayout {
            lines: vec![Line::from(Span::styled(format!("Loading {}…", d.id), dim()))],
            images: Vec::new(),
        },
    };
    let max = u16::try_from(layout.lines.len()).unwrap_or(u16::MAX).saturating_sub(inner.height);
    let Some(d) = app.detail.as_mut() else { return };
    d.scroll = d.scroll.min(max);
    let scroll = usize::from(d.scroll);
    f.render_widget(Paragraph::new(layout.lines).scroll((d.scroll, 0)), inner);
    // Draw each image whose whole box is on screen; a partly visible one
    // keeps its name line.
    let Some(picker) = app.picker.as_ref() else { return };
    for p in layout.images {
        let top = p.line.checked_sub(scroll);
        let Some(top) = top.filter(|t| t + usize::from(p.rows) <= usize::from(inner.height)) else {
            continue;
        };
        let Some(ImageSlot::Ready(pic)) = app.images.get_mut(&p.path) else {
            continue;
        };
        let rect = Rect {
            x: inner.x + p.x,
            y: inner.y + u16::try_from(top).unwrap_or(0),
            width: p.cols.min(inner.width.saturating_sub(p.x)),
            height: p.rows,
        };
        let proto = pic.proto.get_or_insert_with(|| picker.new_resize_protocol(pic.image.clone()));
        f.render_stateful_widget(
            StatefulImage::default().resize(Resize::Fit(Some(FilterType::Triangle))),
            rect,
            proto,
        );
    }
}

fn render_help(f: &mut Frame, area: Rect) {
    let keys = [
        ("j k ↑ ↓", "move / scroll"),
        ("g G", "top / bottom"),
        ("ctrl-d ctrl-u", "half page"),
        ("enter l", "open issue"),
        ("esc h", "back"),
        ("] [", "next / previous issue"),
        ("tab 1-9", "switch view"),
        ("/", "filter; enter searches all"),
        ("n", "load more"),
        ("c", "new issue / comment (detail)"),
        ("E", "edit description (detail)"),
        ("R", "reply in a thread (detail)"),
        ("s a p", "state / assignee / priority"),
        ("L P", "labels / project"),
        ("A", "assign to me"),
        ("x", "archive"),
        ("o", "sort: updated, created, priority"),
        ("f", "filters; save as a view"),
        ("u", "undo the last change"),
        ("r", "refresh"),
        ("O", "open in browser"),
        ("q", "quit"),
    ];
    // Two columns, so it fits a 24-line terminal.
    let half = keys.len().div_ceil(2);
    let cell = |k: Option<&(&str, &str)>| match k {
        Some((k, what)) => {
            vec![Span::styled(format!(" {k:<14}"), Style::new().fg(ACCENT)), Span::raw(format!("{what:<30}"))]
        }
        None => Vec::new(),
    };
    let lines: Vec<Line> = (0..half)
        .map(|r| {
            let mut spans = cell(keys.get(r));
            spans.extend(cell(keys.get(r + half)));
            Line::from(spans)
        })
        .collect();
    let rect = centered(area, 92, u16::try_from(half).unwrap_or(0) + 2);
    f.render_widget(Clear, rect);
    f.render_widget(Paragraph::new(lines).block(Block::bordered().title(" Keys ").border_style(dim())), rect);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{App, Effect, Msg, default_views};
    use crate::fake;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn now() -> DateTime<Utc> {
        "2026-10-26T12:00:00Z".parse().unwrap()
    }

    fn feed(app: &mut App, fx: Vec<Effect>) {
        for f in fx {
            let Effect::Run(req) = f else { continue };
            let msg = req.run(&fake::InProcess { now: now(), state: None });
            let more = app.update(msg);
            feed(app, more);
        }
    }

    fn app_on(view: char) -> App {
        let mut app = App::new(default_views(), now());
        let fx = app.start();
        feed(&mut app, fx);
        let fx = app.update(Msg::Key(KeyEvent::new(KeyCode::Char(view), KeyModifiers::NONE)));
        feed(&mut app, fx);
        app
    }

    fn press(app: &mut App, c: KeyCode) {
        let fx = app.update(Msg::Key(KeyEvent::new(c, KeyModifiers::NONE)));
        feed(app, fx);
    }

    fn draw(app: &mut App, w: u16, h: u16) -> Vec<String> {
        let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
        t.draw(|f| render(app, f)).unwrap();
        let buf = t.backend().buffer().clone();
        (0..h)
            .map(|y| {
                (0..w).map(|x| buf[(x, y)].symbol().to_string()).collect::<String>().trim_end().to_string()
            })
            .collect()
    }

    fn screen(lines: &[String]) -> String {
        lines.join("\n")
    }

    #[test]
    fn list_screen() {
        let mut app = app_on('2');
        let s = screen(&draw(&mut app, 120, 16));
        assert!(s.contains(" Northwind "), "{s}");
        assert!(s.contains("1 My issues  2 Active  3 Triage  4 Recent"));
        assert!(s.contains("12 issues"));
        assert!(s.contains("!!!  ENG-158   ○  Customers in Brazil can't pay with Pix"), "{s}");
        assert!(s.contains("● Bug  ● Customer reported"));
        assert!(s.contains("12m"), "relative update time: {s}");
        assert!(s.contains("j/k move · enter open"));
    }

    #[test]
    fn narrow_list_drops_columns() {
        let mut app = app_on('2');
        let s = screen(&draw(&mut app, 80, 10));
        assert!(!s.contains("Customer reported"));
        assert!(s.contains("JB"));
    }

    #[test]
    fn detail_beside_the_list_when_wide() {
        let mut app = app_on('1');
        press(&mut app, KeyCode::Enter);
        let s = screen(&draw(&mut app, 160, 48));
        assert!(s.contains("ENG-142  Checkout fails when a coupon code has trailing whitespace"), "{s}");
        assert!(s.contains("◔ In Progress  ·  !!! Urgent  ·  Maya Patel  ·  2 pt"));
        assert!(s.contains("▸ Checkout v2  ·  Cycle 24"));
        assert!(s.contains("↑ ENG-120 Checkout v2 rollout plan"));
        assert!(s.contains("Steps to reproduce"));
        assert!(s.contains("ENG-143"), "the list stays on the left");
        assert!(s.contains("j/k scroll"));
    }

    #[test]
    fn detail_full_screen_when_narrow_and_scrolls() {
        let mut app = app_on('1');
        press(&mut app, KeyCode::Enter);
        let s = screen(&draw(&mut app, 100, 30));
        assert!(!s.contains("ENG-143  "), "no list beside it");
        press(&mut app, KeyCode::Char('G'));
        let s = screen(&draw(&mut app, 100, 30));
        assert!(s.contains("Comments (4)"), "{s}");
        assert!(s.contains("↪ Maya Patel · 3d"), "{s}");
        assert!(s.contains("Deploying to staging after review."));
        assert!(app.detail.as_ref().unwrap().scroll < u16::MAX, "clamped");
    }

    #[test]
    fn detail_states() {
        let mut app = app_on('1');
        app.update(Msg::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)));
        assert!(screen(&draw(&mut app, 100, 10)).contains("Loading ENG-142…"));
        app.update(Msg::Shown {
            id: "ENG-142".into(),
            result: Err(crate::ctl::Failure::Failed("boom".into())),
        });
        assert!(screen(&draw(&mut app, 100, 10)).contains("ENG-142: boom"));
    }

    #[test]
    fn a_bare_issue() {
        let mut i = IssueDetail::default();
        i.row.identifier = "ENG-1".into();
        let text: Vec<String> = detail_lines(&i, 60, now(), &|_, _| ImageView::Note(String::new()))
            .lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();
        assert!(text.contains(&"No description.".to_string()));
        assert!(text.contains(&"No comments yet.".to_string()));
        assert!(text[1].contains("Unassigned"));
    }

    #[test]
    fn due_dates_and_children_states() {
        let mut app = app_on('1');
        press(&mut app, KeyCode::Char('j'));
        press(&mut app, KeyCode::Char('j'));
        press(&mut app, KeyCode::Enter);
        let s = screen(&draw(&mut app, 100, 20));
        assert!(s.contains("ENG-140"), "{s}");
        assert!(s.contains("Due 2026-12-01"), "{s}");
    }

    #[test]
    fn filter_status_help_and_empty() {
        let mut app = app_on('1');
        press(&mut app, KeyCode::Char('/'));
        press(&mut app, KeyCode::Char('z'));
        let s = screen(&draw(&mut app, 100, 8));
        assert!(s.contains(" / z▏"));
        assert!(s.contains("No issues match the filter."));
        assert!(s.contains("enter search all issues · ↓ keep as filter"));
        press(&mut app, KeyCode::Down);
        assert!(screen(&draw(&mut app, 100, 8)).contains(" / z   esc clear"));
        press(&mut app, KeyCode::Esc);
        app.status = Some("rate limited".into());
        assert!(screen(&draw(&mut app, 100, 8)).contains(" rate limited"));
        press(&mut app, KeyCode::Char('?'));
        let s = screen(&draw(&mut app, 100, 20));
        assert!(s.contains("Keys"));
        assert!(s.contains("open in browser"));
        let mut empty = App::new(default_views(), now());
        assert!(screen(&draw(&mut empty, 80, 5)).contains("No issues in this view."));
        empty.loading = true;
        let s = screen(&draw(&mut empty, 80, 5));
        assert!(s.contains("Loading…") && s.contains("loading…") && s.contains(" Linear "));
        empty.next_cursor = Some("x".into());
        empty.loading = false;
        assert!(screen(&draw(&mut empty, 80, 5)).contains("0+ issues"));
    }

    fn demo_image() -> Box<image::DynamicImage> {
        use crate::images::Source;
        let bytes = crate::images::Demo.fetch("https://uploads.linear.app/demo/checkout-error.png").unwrap();
        Box::new(crate::images::decode(&bytes).unwrap())
    }

    const DEMO_URL: &str = "https://uploads.linear.app/demo/checkout-error.png?signature=demo-300";

    fn detail_with_image(picker: bool) -> App {
        let mut app = app_on('1');
        if picker {
            app.picker = Some(ratatui_image::picker::Picker::halfblocks());
        }
        let fx = app.update(Msg::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)));
        feed(&mut app, fx);
        app
    }

    #[test]
    fn images_show_their_state_until_drawn() {
        let mut app = detail_with_image(true);
        let s = screen(&draw(&mut app, 160, 60));
        assert!(s.contains("□ checkout-error.png  loading…"), "{s}");
        app.update(Msg::Image { url: DEMO_URL.into(), result: Err("HTTP 500".into()) });
        assert!(screen(&draw(&mut app, 160, 60)).contains("□ checkout-error.png  not shown: HTTP 500"));
        app.update(Msg::Key(KeyEvent::new(KeyCode::Char('i'), KeyModifiers::NONE)));
        assert!(screen(&draw(&mut app, 160, 60)).contains("□ checkout-error.png  images off (i)"));
    }

    #[test]
    fn a_loaded_image_is_drawn_in_its_box() {
        let mut app = detail_with_image(true);
        app.update(Msg::Image { url: DEMO_URL.into(), result: Ok(demo_image()) });
        let mut t = Terminal::new(TestBackend::new(160, 40)).unwrap();
        // Rows of the detail pane mostly painted with a background colour.
        let mut painted = |app: &mut App| -> Vec<u16> {
            t.draw(|f| render(app, f)).unwrap();
            let buf = t.backend().buffer();
            (0..40).filter(|&y| (73..160).filter(|&x| buf[(x, y)].bg != Color::Reset).count() > 40).collect()
        };
        app.detail.as_mut().unwrap().scroll = 10;
        let rows = painted(&mut app);
        // 588x376 px at 10x20 px a cell: 59 x 19 cells, in one block.
        assert_eq!(rows.len(), 19, "{rows:?}");
        assert!(rows.windows(2).all(|w| w[1] == w[0] + 1));
        // One more line down and the top of the box is off screen: not drawn.
        let top_line = rows[0] - 1 + 10;
        app.detail.as_mut().unwrap().scroll = top_line + 1;
        assert!(painted(&mut app).is_empty(), "partly visible images are not drawn");
        app.detail.as_mut().unwrap().scroll = top_line;
        assert_eq!(painted(&mut app).len(), 19);
    }

    #[test]
    fn without_a_picker_rows_are_reserved_but_nothing_drawn() {
        let mut app = detail_with_image(false);
        app.update(Msg::Image { url: DEMO_URL.into(), result: Ok(demo_image()) });
        let s = screen(&draw(&mut app, 160, 60));
        assert!(s.contains("□ checkout-error.png"));
        assert!(!s.contains('▀'));
    }

    #[test]
    fn image_views() {
        let mut app = detail_with_image(false);
        app.update(Msg::Image { url: DEMO_URL.into(), result: Ok(demo_image()) });
        let font = FontSize::new(10, 20);
        let v = viewer(&app, font, 30);
        assert_eq!(v(DEMO_URL, 100), ImageView::Box { cols: 59, rows: 19 }, "natural size");
        assert_eq!(v(DEMO_URL, 30), ImageView::Box { cols: 30, rows: 10 }, "shrunk to the width");
        let short = viewer(&app, font, 5);
        assert_eq!(short(DEMO_URL, 100), ImageView::Box { cols: 16, rows: 5 }, "shrunk to the height");
        assert_eq!(v("https://example.com/x.png", 100), ImageView::Note("external image, not loaded".into()));
        assert_eq!(v("https://uploads.linear.app/other", 100), ImageView::Note("loading…".into()));
    }

    #[test]
    fn images_in_lists_and_comments_are_placed_after_their_indent() {
        let i = IssueDetail {
            description: Some("- ![a](https://uploads.linear.app/p/a)".into()),
            comments: vec![crate::model::Comment {
                id: "c".into(),
                body: "![b](https://uploads.linear.app/p/b)\n\n![c](https://elsewhere.example/c)".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let layout = detail_lines(&i, 80, now(), &|_, _| ImageView::Box { cols: 4, rows: 3 });
        let xs: Vec<(u16, &str)> = layout.images.iter().map(|p| (p.x, p.path.as_str())).collect();
        assert_eq!(xs, [(2, "/p/a"), (2, "/p/b")]);
        assert!(layout.images.iter().all(|p| p.rows == 3 && p.cols == 4));
        let text: Vec<String> =
            layout.lines.iter().map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect()).collect();
        assert!(text.iter().any(|l| l.ends_with("□ c")), "a box for a non-upload is not placed: {text:?}");
    }

    #[test]
    fn search_results_and_none() {
        let mut app = app_on('1');
        press(&mut app, KeyCode::Char('/'));
        for c in "coupon".chars() {
            press(&mut app, KeyCode::Char(c));
        }
        press(&mut app, KeyCode::Enter);
        let s = screen(&draw(&mut app, 120, 10));
        assert!(s.contains("Northwind   Search \"coupon\"  1 My issues"), "{s}");
        assert!(s.contains("ENG-131"));
        press(&mut app, KeyCode::Esc);
        press(&mut app, KeyCode::Char('/'));
        for c in "zebra".chars() {
            press(&mut app, KeyCode::Char(c));
        }
        press(&mut app, KeyCode::Enter);
        assert!(screen(&draw(&mut app, 120, 10)).contains("No issues match \"zebra\"."));
    }

    #[test]
    fn a_picker_over_the_list() {
        let mut app = app_on('1');
        press(&mut app, KeyCode::Char('s'));
        let s = screen(&draw(&mut app, 120, 24));
        assert!(s.contains(" Status "), "{s}");
        assert!(s.contains("› ▏"));
        assert!(s.contains("✓ ● In Progress  started"), "{s}");
        assert!(s.contains("type to filter · ↑↓ move · enter choose"));
        press(&mut app, KeyCode::Char('z'));
        press(&mut app, KeyCode::Char('z'));
        assert!(screen(&draw(&mut app, 120, 24)).contains("No match."));
        press(&mut app, KeyCode::Esc);
        press(&mut app, KeyCode::Char('L'));
        let s = screen(&draw(&mut app, 120, 24));
        assert!(s.contains("[x] ● Bug") && s.contains("[ ] ● Feature"), "{s}");
        assert!(s.contains("space toggle · enter apply"));
        // While the lookup loads.
        let mut loading = app_on('1');
        loading.update(Msg::Key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE)));
        assert!(screen(&draw(&mut loading, 120, 24)).contains("Loading…"));
    }

    #[test]
    fn a_long_picker_scrolls_to_its_cursor() {
        let mut app = app_on('1');
        press(&mut app, KeyCode::Char('L'));
        for _ in 0..6 {
            press(&mut app, KeyCode::Down);
        }
        let s = screen(&draw(&mut app, 120, 9));
        assert!(s.contains("Customer reported"), "{s}");
    }

    #[test]
    fn the_confirm_dialog_shows_the_command() {
        let mut app = app_on('1');
        press(&mut app, KeyCode::Char('x'));
        let s = screen(&draw(&mut app, 120, 24));
        assert!(s.contains(" Archive ENG-142? "), "{s}");
        assert!(s.contains("ENG-142  Checkout fails when a coupon code has trailing whitespace"));
        assert!(s.contains("linearctl issue archive ENG-142 --yes --json"));
        assert!(s.contains("y send   n cancel"));
        assert!(s.contains(" y send · n cancel"), "footer");
        press(&mut app, KeyCode::Char('n'));
        assert!(screen(&draw(&mut app, 120, 24)).contains(" ✓ Not sent."));
    }

    #[test]
    fn the_new_issue_form() {
        let mut app = app_on('1');
        press(&mut app, KeyCode::Char('c'));
        for c in "Gift cards".chars() {
            press(&mut app, KeyCode::Char(c));
        }
        app.form.as_mut().unwrap().description = "Line one\nLine two".into();
        app.form.as_mut().unwrap().assignee = Some(("me".into(), "Me".into()));
        let s = screen(&draw(&mut app, 120, 30));
        assert!(s.contains(" New issue "), "{s}");
        assert!(s.contains("› Title        Gift cards▏"), "{s}");
        assert!(s.contains("  Team         ENG"));
        assert!(s.contains("  Status       the team's default"));
        assert!(s.contains("  Priority     No priority"));
        assert!(s.contains("  Assignee     Me"));
        assert!(s.contains("  Labels       none"));
        assert!(s.contains("  Description  Line one"));
        assert!(s.contains("Line two"));
        assert!(s.contains("ctrl-s create · esc discard"));
        app.form.as_mut().unwrap().description.clear();
        app.form.as_mut().unwrap().field = 1;
        assert!(screen(&draw(&mut app, 120, 30)).contains("enter opens $EDITOR"));
    }

    #[test]
    fn the_header_says_when_updates_are_live() {
        let mut app = app_on('1');
        assert!(!screen(&draw(&mut app, 120, 5)).contains("live"));
        let live = |mode: &str, ready| crate::app::Live {
            generation: 1,
            args: vec![],
            mode: mode.into(),
            ready,
            heard: now(),
        };
        app.live = Some(live("", false));
        assert!(screen(&draw(&mut app, 120, 5)).contains("○ connecting  by priority · 4 issues"));
        app.live = Some(live("subscription", true));
        assert!(screen(&draw(&mut app, 120, 5)).contains("● live  by priority · 4 issues"));
        app.live = Some(live("poll", true));
        assert!(screen(&draw(&mut app, 120, 5)).contains("● live · polling  by priority · 4 issues"));
    }

    #[test]
    fn plain_wrapping() {
        assert_eq!(wrap_plain("aa bb cc", 5), ["aa bb", "cc"]);
        assert_eq!(wrap_plain("", 5), [""]);
    }

    #[test]
    fn helpers() {
        assert_eq!(hex(Some("#5e6ad2")), Some(Color::Rgb(0x5e, 0x6a, 0xd2)));
        assert_eq!(hex(Some("5e6ad2")), None);
        assert_eq!(hex(Some("#5e6")), None);
        assert_eq!(hex(Some("#zzzzzz")), None);
        assert_eq!(hex(Some("#é12345")), None);
        assert_eq!(hex(None), None);
        for (k, g) in [
            ("triage", "◇"),
            ("backlog", "◌"),
            ("unstarted", "○"),
            ("started", "◔"),
            ("completed", "●"),
            ("canceled", "⊘"),
            ("duplicate", "⊘"),
            ("odd", "·"),
        ] {
            assert_eq!(state_icon(k), g);
        }
        let s = |kind: &str| State { name: String::new(), kind: kind.into(), color: None };
        assert_eq!(state_color(&s("triage")), Color::Rgb(0xfc, 0x78, 0x40));
        assert_eq!(state_color(&s("started")), Color::Yellow);
        assert_eq!(state_color(&s("completed")), ACCENT);
        assert_eq!(state_color(&s("backlog")), Color::Gray);
        assert_eq!(state_span(None, true)[0].content, " ");
        let text = |p| priority_spans(p).iter().map(|s| s.content.to_string()).collect::<String>();
        assert_eq!([text(0), text(1), text(2), text(3), text(4)], ["───", "!!!", "▂▄▆", "▂▄▆", "▂▄▆"]);
        assert_eq!(initials(None), "");
    }

    #[test]
    fn ago_buckets() {
        let at = |s: i64| (now() - chrono::Duration::seconds(s)).to_rfc3339();
        assert_eq!(ago(&at(5), now()), "now");
        assert_eq!(ago(&at(-50), now()), "now", "clock skew");
        assert_eq!(ago(&at(300), now()), "5m");
        assert_eq!(ago(&at(3 * 3600), now()), "3h");
        assert_eq!(ago(&at(2 * 86_400), now()), "2d");
        assert_eq!(ago(&at(15 * 86_400), now()), "2w");
        assert_eq!(ago(&at(60 * 86_400), now()), "Aug 27");
        assert_eq!(ago("not a time", now()), "");
    }
}
