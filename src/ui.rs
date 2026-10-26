//! Drawing the app: the issue list, the issue detail (beside the list on
//! wide terminals), the header, the status line and help.

use chrono::{DateTime, Utc};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Clear, Paragraph, Row, Table, TableState};
use unicode_width::UnicodeWidthStr;

use crate::app::{App, Focus};
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
            Style::new()
                .fg(Color::Rgb(0xf2, 0x99, 0x4a))
                .add_modifier(Modifier::BOLD),
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
        v.push(Span::styled(
            "●",
            Style::new().fg(hex(l.color.as_deref()).unwrap_or(Color::Gray)),
        ));
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
    if app.help {
        render_help(f, body);
    }
}

fn render_header(app: &App, f: &mut Frame, area: Rect) {
    let org = app
        .auth
        .as_ref()
        .map_or("Linear", |a| a.organization.name.as_str());
    let mut spans = vec![Span::styled(
        format!(" {org} "),
        Style::new()
            .fg(Color::White)
            .bg(ACCENT)
            .add_modifier(Modifier::BOLD),
    )];
    for (i, v) in app.views.iter().enumerate() {
        let style = if i == app.view {
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
        format!("{n}{more} issues")
    };
    let left_w: usize = spans.iter().map(|s| s.content.width()).sum();
    let pad = usize::from(area.width).saturating_sub(left_w + right.width() + 1);
    spans.push(Span::raw(" ".repeat(pad)));
    spans.push(Span::styled(right, dim()));
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn render_footer(app: &App, f: &mut Frame, area: Rect) {
    let line = if app.filtering || !app.filter.is_empty() {
        let cursor = if app.filtering { "▏" } else { "" };
        Line::from(vec![
            Span::styled(" / ", Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)),
            Span::raw(format!("{}{cursor}", app.filter)),
            Span::styled("   enter keep · esc clear", dim()),
        ])
    } else if let Some(s) = &app.status {
        Line::from(Span::styled(format!(" {s}"), Style::new().fg(Color::Red)))
    } else {
        let hints = match app.focus {
            Focus::List => {
                "j/k move · enter open · tab view · / filter · r refresh · O browser · ? help · q quit"
            }
            Focus::Detail => "j/k scroll · ]/[ next/prev · r reload · O browser · esc back · ? help",
        };
        Line::from(Span::styled(format!(" {hints}"), dim()))
    };
    f.render_widget(Paragraph::new(line), area);
}

fn render_list(app: &App, f: &mut Frame, area: Rect, narrow: bool) {
    let rows = app.visible();
    if rows.is_empty() {
        let msg = if app.loading {
            "Loading…"
        } else if app.filter.is_empty() {
            "No issues in this view."
        } else {
            "No issues match the filter."
        };
        f.render_widget(Paragraph::new(Span::styled(format!(" {msg}"), dim())), area);
        return;
    }
    let wide = !narrow && area.width >= 110;
    let table_rows: Vec<Row> = rows.iter().map(|r| list_row(r, app.now, wide)).collect();
    let mut widths = vec![
        Constraint::Length(3),
        Constraint::Length(8),
        Constraint::Length(1),
        Constraint::Fill(1),
    ];
    if wide {
        widths.extend([
            Constraint::Length(2),
            Constraint::Length(28),
            Constraint::Length(4),
        ]);
    } else {
        widths.push(Constraint::Length(2));
    }
    let highlight = Style::new()
        .bg(Color::Rgb(0x2a, 0x2d, 0x3e))
        .add_modifier(Modifier::BOLD);
    let table = Table::new(table_rows, widths)
        .column_spacing(2)
        .row_highlight_style(highlight);
    let mut state = TableState::default().with_selected(Some(app.selected));
    f.render_stateful_widget(table, area, &mut state);
}

fn list_row(r: &IssueRow, now: DateTime<Utc>, wide: bool) -> Row<'static> {
    let mut cells = vec![
        Cell::from(Line::from(priority_spans(r.priority))),
        Cell::from(Span::styled(r.identifier.clone(), dim())),
        Cell::from(Line::from(state_span(r.state.as_ref(), false))),
        Cell::from(r.title.clone()),
        Cell::from(Span::styled(
            initials(r.assignee.as_ref()),
            Style::new().fg(Color::Cyan),
        )),
    ];
    if wide {
        cells.push(Cell::from(Line::from(label_spans(&r.labels))));
        cells.push(Cell::from(Span::styled(ago(&r.updated_at, now), dim())));
    }
    Row::new(cells)
}

/// The detail as lines for a pane `width` wide.
pub fn detail_lines(i: &IssueDetail, width: u16, now: DateTime<Utc>) -> Vec<Line<'static>> {
    let w = width.saturating_sub(2).max(10);
    let mut out: Vec<Line<'static>> = Vec::new();
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
        meta2.push(Span::styled(
            format!("▸ {}", p.name),
            Style::new().fg(Color::Gray),
        ));
    }
    if let Some(c) = &row.cycle {
        if !meta2.is_empty() {
            meta2.push(sep());
        }
        meta2.push(Span::styled(
            format!("Cycle {}", c.number),
            Style::new().fg(Color::Gray),
        ));
    }
    if let Some(d) = &i.due_date {
        if !meta2.is_empty() {
            meta2.push(sep());
        }
        meta2.push(Span::styled(
            format!("Due {d}"),
            Style::new().fg(Color::Rgb(0xf2, 0x99, 0x4a)),
        ));
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
        Some(d) => out.extend(markdown::render(d, w).lines),
        None => out.push(Line::from(Span::styled("No description.", dim()))),
    }
    let section = |title: String| {
        Line::from(Span::styled(
            title,
            Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
        ))
    };
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
            out.push(Line::from(vec![
                Span::styled("↗ ", dim()),
                Span::raw(a.title.clone()),
            ]));
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
        head.push(Span::styled(
            name,
            Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        ));
        head.push(Span::styled(format!(" · {}", ago(&c.created_at, now)), dim()));
        out.push(Line::from(head));
        let pad = if reply { 6 } else { 2 };
        for l in markdown::render(&c.body, w.saturating_sub(pad)).lines {
            let mut spans = vec![Span::raw(" ".repeat(usize::from(pad)))];
            spans.extend(l.spans);
            out.push(Line::from(spans));
        }
    }
    out
}

fn render_detail(app: &mut App, f: &mut Frame, area: Rect, beside: bool) {
    let block = if beside {
        Block::new().borders(Borders::LEFT).border_style(dim())
    } else {
        Block::new()
    };
    let inner = block.inner(area);
    f.render_widget(block, area);
    let inner = Rect {
        x: inner.x + 1,
        width: inner.width.saturating_sub(1),
        ..inner
    };
    let now = app.now;
    let Some(d) = app.detail.as_mut() else { return };
    let lines = match (&d.issue, &d.error) {
        (_, Some(e)) => vec![Line::from(Span::styled(
            format!("{}: {e}", d.id),
            Style::new().fg(Color::Red),
        ))],
        (Some(i), None) => detail_lines(i, inner.width, now),
        (None, None) => vec![Line::from(Span::styled(format!("Loading {}…", d.id), dim()))],
    };
    let max = u16::try_from(lines.len())
        .unwrap_or(u16::MAX)
        .saturating_sub(inner.height);
    d.scroll = d.scroll.min(max);
    f.render_widget(Paragraph::new(lines).scroll((d.scroll, 0)), inner);
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
        ("/", "filter the list"),
        ("n", "load more"),
        ("r", "refresh"),
        ("O", "open in browser"),
        ("q", "quit"),
    ];
    let w = 44.min(area.width);
    let h = (u16::try_from(keys.len()).unwrap_or(0) + 2).min(area.height);
    let rect = Rect {
        x: area.x + (area.width - w) / 2,
        y: area.y + (area.height - h) / 2,
        width: w,
        height: h,
    };
    let lines: Vec<Line> = keys
        .iter()
        .map(|(k, what)| {
            Line::from(vec![
                Span::styled(format!(" {k:<14}"), Style::new().fg(ACCENT)),
                Span::raw(*what),
            ])
        })
        .collect();
    f.render_widget(Clear, rect);
    f.render_widget(
        Paragraph::new(lines).block(Block::bordered().title(" Keys ").border_style(dim())),
        rect,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{App, Effect, Msg, Request, default_views};
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
            let run = |argv: &[String]| fake::run(argv, now()).stdout;
            let msg = match req {
                Request::List {
                    generation,
                    argv,
                    append,
                } => Msg::Listed {
                    generation,
                    append,
                    result: Ok(serde_json::from_str(&run(&argv)).unwrap()),
                },
                Request::Show { id, argv } => {
                    let s: crate::model::ShowResult = serde_json::from_str(&run(&argv)).unwrap();
                    Msg::Shown {
                        id,
                        result: Ok(Box::new(s.issue)),
                    }
                }
                Request::Auth { argv } => Msg::Authed(Ok(serde_json::from_str(&run(&argv)).unwrap())),
            };
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
                (0..w)
                    .map(|x| buf[(x, y)].symbol().to_string())
                    .collect::<String>()
                    .trim_end()
                    .to_string()
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
        assert!(
            s.contains("!!!  ENG-158   ○  Customers in Brazil can't pay with Pix"),
            "{s}"
        );
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
        assert!(
            s.contains("ENG-142  Checkout fails when a coupon code has trailing whitespace"),
            "{s}"
        );
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
        let text: Vec<String> = detail_lines(&i, 60, now())
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
        press(&mut app, KeyCode::Enter);
        assert!(screen(&draw(&mut app, 100, 8)).contains(" / z   enter keep"));
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
        let s = |kind: &str| State {
            name: String::new(),
            kind: kind.into(),
            color: None,
        };
        assert_eq!(state_color(&s("triage")), Color::Rgb(0xfc, 0x78, 0x40));
        assert_eq!(state_color(&s("started")), Color::Yellow);
        assert_eq!(state_color(&s("completed")), ACCENT);
        assert_eq!(state_color(&s("backlog")), Color::Gray);
        assert_eq!(state_span(None, true)[0].content, " ");
        let text = |p| {
            priority_spans(p)
                .iter()
                .map(|s| s.content.to_string())
                .collect::<String>()
        };
        assert_eq!(
            [text(0), text(1), text(2), text(3), text(4)],
            ["───", "!!!", "▂▄▆", "▂▄▆", "▂▄▆"]
        );
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
