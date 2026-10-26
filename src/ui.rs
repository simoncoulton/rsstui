use chrono::{DateTime, Local};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Position, Rect},
    style::{Style, Stylize},
    text::{Line, Span, Text},
    widgets::{Block, BorderType, Clear, List, ListItem, ListState, Paragraph, Wrap},
};
use ratatui_image::StatefulImage;
use tui_markdown::{Options, StyleSheet};

use crate::{
    app::{App, Filter, Focus, ImgState, Level, Mode, Row},
    config::Layout as Layout_,
    forms::TagChecklist,
    input::Input,
    theme::Theme,
};

/// Feeds pane width: 10% wider than the original 28.
const FEEDS_WIDTH: u16 = 31;

pub fn draw(frame: &mut Frame, app: &mut App) {
    let t = app.theme.clone();
    frame.render_widget(Block::new().style(t.base()), frame.area());

    let [header, main, footer] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .areas(frame.area());
    draw_header(frame, app, header);
    if app.zoom && app.focus == Focus::Article {
        draw_article(frame, app, main);
    } else {
        match app.config.layout {
            Layout_::Horizontal => {
                let [feeds, items, article] = Layout::horizontal([
                    Constraint::Length(FEEDS_WIDTH),
                    Constraint::Percentage(27),
                    Constraint::Min(20),
                ])
                .areas(main);
                draw_feeds(frame, app, feeds);
                draw_items(frame, app, items);
                draw_article(frame, app, article);
            }
            Layout_::Vertical => {
                let [feeds, right] =
                    Layout::horizontal([Constraint::Length(FEEDS_WIDTH), Constraint::Min(20)])
                        .areas(main);
                let [items, article] =
                    Layout::vertical([Constraint::Percentage(40), Constraint::Min(5)]).areas(right);
                draw_feeds(frame, app, feeds);
                draw_items(frame, app, items);
                draw_article(frame, app, article);
            }
        }
    }
    draw_footer(frame, app, footer);

    match &app.mode {
        Mode::Help => draw_help(frame, &t),
        Mode::AddFeed(_) => draw_add_feed(frame, app),
        Mode::ConfirmDelete => draw_confirm_delete(frame, app),
        Mode::Theme(_) => draw_theme(frame, app),
        Mode::Tags(_) => draw_tags(frame, app),
        Mode::Folder(_) => draw_folder(frame, app),
        Mode::TagFilter(_) => draw_tag_filter(frame, app),
        Mode::Normal | Mode::Search => {}
    }
}

// ---- shared building blocks ------------------------------------------------

/// Every pane and popup uses softly rounded corners.
fn rounded<'a>() -> Block<'a> {
    Block::bordered().border_type(BorderType::Rounded)
}

fn title(t: &Theme, s: &str) -> Span<'static> {
    Span::styled(format!(" {s} "), Style::new().fg(t.accent).bold())
}

/// A key in the accent colour, for inline use.
fn key(t: &Theme, k: &str) -> Span<'static> {
    Span::styled(k.to_string(), Style::new().fg(t.accent).bold())
}

/// `"Enter save  Esc cancel"`: items are separated by two spaces and the first
/// word of each is the key.
fn hint_line(t: &Theme, s: &str) -> Line<'static> {
    let mut spans = vec![Span::raw(" ")];
    for item in s.split("  ") {
        let (k, desc) = item.split_once(' ').unwrap_or((item, ""));
        spans.push(key(t, k));
        spans.push(Span::styled(format!(" {desc}  "), Style::new().fg(t.muted)));
    }
    Line::from(spans)
}

fn pane<'a>(t: &Theme, name: &str, focused: bool) -> Block<'a> {
    let color = if focused { t.accent } else { t.border };
    rounded()
        .title(title(t, name))
        .border_style(Style::new().fg(color))
}

fn centered(full: Rect, width: u16, height: u16) -> Rect {
    let w = width.min(full.width);
    let h = height.min(full.height);
    Rect {
        x: full.x + (full.width - w) / 2,
        y: full.y + (full.height - h) / 2,
        width: w,
        height: h,
    }
}

/// Clears the area and applies the page style, so a popup matches the page in
/// both opaque and transparent modes.
fn popup(frame: &mut Frame, t: &Theme, area: Rect) {
    frame.render_widget(Clear, area);
    frame.render_widget(Block::new().style(t.base()), area);
}

fn modal<'a>(t: &Theme, name: &str, hints: &str) -> Block<'a> {
    rounded()
        .border_style(Style::new().fg(t.accent))
        .title(title(t, name))
        .title_bottom(hint_line(t, hints))
}

fn set_cursor(frame: &mut Frame, pos: Position) {
    if frame.area().contains(pos) {
        frame.set_cursor_position(pos);
    }
}

/// Truncate to `width` cells with an ellipsis.
fn fit(s: &str, width: usize) -> String {
    if s.chars().count() <= width {
        return s.to_string();
    }
    let keep = width.saturating_sub(1);
    let mut out: String = s.chars().take(keep).collect();
    if width > 0 {
        out.push('…');
    }
    out
}

/// Draw a text field, or its placeholder while empty. Returns the cursor position.
fn draw_input(
    frame: &mut Frame,
    t: &Theme,
    area: Rect,
    input: &Input,
    placeholder: &str,
) -> Position {
    if input.text().is_empty() {
        frame.render_widget(
            Paragraph::new(fit(placeholder, area.width as usize)).style(Style::new().fg(t.muted)),
            area,
        );
        return Position::new(area.x, area.y);
    }
    let (shown, col) = input.view(area.width);
    frame.render_widget(Paragraph::new(shown), area);
    Position::new(area.x + col, area.y)
}

// ---- header ------------------------------------------------------------------

/// What the header says about the feed (or folder, or everything) in view.
struct Scope {
    label: String,
    tags: Vec<String>,
    refreshed: Option<DateTime<Local>>,
    loading: bool,
    failed: bool,
}

fn scope(app: &App) -> Scope {
    let feeds = app.scope_feeds();
    let refreshed = feeds.iter().filter_map(|&i| app.feeds[i].refreshed).max();
    let (label, tags) = match app.selected_row() {
        Some(Row::Feed(i)) => {
            let f = &app.config.feeds[i];
            let label = match &f.folder {
                Some(folder) => format!("{folder} ▸ {}", f.label()),
                None => f.label().to_string(),
            };
            (label, f.tags.clone())
        }
        Some(Row::Folder(f)) => (f, Vec::new()),
        _ => ("All feeds".to_string(), Vec::new()),
    };
    Scope {
        label,
        tags,
        refreshed,
        loading: feeds.iter().any(|&i| app.feeds[i].loading),
        failed: feeds.iter().any(|&i| app.feeds[i].error.is_some()),
    }
}

fn fmt_ago(then: DateTime<Local>, now: DateTime<Local>) -> String {
    let secs = (now - then).num_seconds().max(0);
    match secs {
        0..=44 => "just now".into(),
        45..=3599 => format!("{}m ago", (secs / 60).max(1)),
        3600..=86399 => format!("{}h ago", secs / 3600),
        _ => format!("{}d ago", secs / 86400),
    }
}

fn fmt_refreshed(then: DateTime<Local>, now: DateTime<Local>) -> String {
    let when = if then.date_naive() == now.date_naive() {
        then.format("%H:%M")
    } else {
        then.format("%b %e %H:%M")
    };
    format!("{when} · {}", fmt_ago(then, now))
}

fn draw_header(frame: &mut Frame, app: &App, area: Rect) {
    let t = &app.theme;
    let sc = scope(app);
    let mut block = rounded()
        .border_style(Style::new().fg(t.border))
        .title(title(t, "rsstui"))
        .title_bottom(
            Line::styled(
                format!(" {} ", fit(&sc.label, 60)),
                Style::new().fg(t.muted),
            )
            .right_aligned(),
        );
    if !sc.tags.is_empty() {
        let tags = sc
            .tags
            .iter()
            .map(|x| format!("#{x}"))
            .collect::<Vec<_>>()
            .join(" ");
        block = block.title_bottom(
            Line::styled(format!(" {tags} "), Style::new().fg(t.muted)).left_aligned(),
        );
    }
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let unread = app.unread_in(&Row::All);
    let items: usize = app
        .scope_feeds()
        .iter()
        .map(|&i| app.feeds[i].items.len())
        .sum();
    let mut spans = vec![
        Span::raw(format!("{} feeds", app.config.feeds.len())),
        Span::styled("  ·  ", Style::new().fg(t.muted)),
        Span::styled(format!("{unread} unread"), Style::new().bold()),
        Span::styled("  ·  ", Style::new().fg(t.muted)),
        Span::raw(format!("{items} items in view")),
        Span::raw("     "),
    ];
    spans.push(Span::styled("Refreshed ", Style::new().fg(t.muted)));
    match (sc.loading, sc.refreshed) {
        (true, _) => spans.push(Span::styled("refreshing…", Style::new().fg(t.warning))),
        (false, Some(at)) => spans.push(Span::raw(fmt_refreshed(at, Local::now()))),
        (false, None) => spans.push(Span::styled("never", Style::new().fg(t.muted))),
    }
    if sc.failed && !sc.loading {
        spans.push(Span::styled("  (a fetch failed)", Style::new().fg(t.error)));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), inner);
}

// ---- panes -----------------------------------------------------------------

fn filter_label(f: &Filter) -> String {
    match f {
        Filter::All => "all feeds".into(),
        Filter::Untagged => "untagged".into(),
        Filter::Tag(t) => format!("#{t}"),
    }
}

fn draw_feeds(frame: &mut Frame, app: &mut App, area: Rect) {
    let t = app.theme.clone();
    let focused = app.focus == Focus::Feeds;
    let rows: Vec<ListItem> = app
        .rows
        .iter()
        .map(|row| {
            let unread = app.unread_in(row);
            match row {
                Row::All => feed_row(&t, "All".into(), unread, false, false),
                Row::Folder(f) => {
                    let arrow = if app.state.collapsed.contains(f) {
                        "▸"
                    } else {
                        "▾"
                    };
                    feed_row(&t, format!("{arrow} {f}"), unread, false, false)
                        .style(Style::new().bold())
                }
                Row::Feed(i) => {
                    let cfg = &app.config.feeds[*i];
                    let fs = &app.feeds[*i];
                    let indent = if cfg.folder.is_some() { "   " } else { "" };
                    feed_row(
                        &t,
                        format!("{indent}{}", cfg.label()),
                        unread,
                        fs.loading,
                        fs.error.is_some(),
                    )
                }
            }
        })
        .collect();
    let name = match &app.tag_filter {
        Filter::All => "Feeds".to_string(),
        f => format!("Feeds {}", filter_label(f)),
    };
    let list = List::new(rows)
        .block(pane(&t, &name, focused))
        .highlight_style(t.selection(focused));
    frame.render_stateful_widget(list, area, &mut app.feed_list);
}

fn feed_row(
    t: &Theme,
    name: String,
    unread: usize,
    loading: bool,
    error: bool,
) -> ListItem<'static> {
    let mut spans = vec![Span::raw(name)];
    if loading {
        spans.push(Span::styled(" …", Style::new().fg(t.warning)));
    }
    if error {
        spans.push(Span::styled(" err", Style::new().fg(t.error)));
    }
    if unread > 0 {
        spans.push(Span::styled(format!(" {unread}"), Style::new().bold()));
    }
    ListItem::new(Line::from(spans))
}

fn draw_items(frame: &mut Frame, app: &mut App, area: Rect) {
    let t = app.theme.clone();
    let focused = app.focus == Focus::Items;
    let multi = !matches!(app.selected_row(), Some(Row::Feed(_)));
    let rows: Vec<ListItem> = app
        .visible
        .iter()
        .map(|&(fi, ii)| {
            let it = &app.feeds[fi].items[ii];
            let read = app.is_read(&it.id);
            let (dot, style) = if read {
                ("  ", Style::new().fg(t.muted))
            } else {
                ("● ", Style::new().bold())
            };
            let mut meta = fmt_date(it.date, "%b %e");
            if multi {
                meta = format!("{} · {meta}", app.config.feeds[fi].label());
            }
            ListItem::new(Text::from(vec![
                Line::from(vec![Span::raw(dot), Span::styled(it.title.clone(), style)]),
                Line::styled(format!("  {meta}"), Style::new().fg(t.muted)),
            ]))
        })
        .collect();

    let mut name = String::from("Items");
    if app.unread_only {
        name.push_str(" (unread)");
    }
    if !app.filter.is_empty() || matches!(app.mode, Mode::Search) {
        name.push_str(&format!(" /{}", app.filter));
    }
    let mut block = pane(&t, &name, focused);
    if let Some(err) = app
        .selected_feed()
        .and_then(|fi| app.feeds[fi].error.as_deref())
    {
        block = block.title_bottom(Line::styled(format!(" {err} "), Style::new().fg(t.error)));
    } else if rows.is_empty() {
        let msg = if app.filter.is_empty() {
            " empty "
        } else {
            " no matches "
        };
        block = block.title_bottom(Line::styled(msg, Style::new().fg(t.muted)));
    }
    let list = List::new(rows)
        .block(block)
        .highlight_style(t.selection(focused));
    frame.render_stateful_widget(list, area, &mut app.item_list);
}

fn draw_article(frame: &mut Frame, app: &mut App, area: Rect) {
    let t = app.theme.clone();
    let focused = app.focus == Focus::Article;
    let block = pane(&t, "Article", focused);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    app.article_height = inner.height;

    let Some(&(fi, ii)) = app.item_list.selected().and_then(|i| app.visible.get(i)) else {
        let muted = Style::new().fg(t.muted);
        let text = if app.config.feeds.is_empty() {
            // First run: the config ships with no feeds, so say how to add some.
            Text::from(vec![
                Line::styled("No feeds yet.", Style::new().bold()),
                Line::raw(""),
                Line::from(vec![
                    Span::styled("Press ", muted),
                    key(&t, "a"),
                    Span::styled(" to add a feed by URL, or ", muted),
                    key(&t, "E"),
                    Span::styled(" to edit the config file.", muted),
                ]),
                Line::from(vec![
                    Span::styled("Press ", muted),
                    key(&t, "?"),
                    Span::styled(" for all keys.", muted),
                ]),
            ])
        } else {
            Text::styled("No article selected", muted)
        };
        frame.render_widget(Paragraph::new(text).wrap(Wrap { trim: true }), inner);
        return;
    };
    let item = &app.feeds[fi].items[ii];

    // Header: wrapped title (2 lines), meta, link.
    let header_h = 5.min(inner.height);
    let mut meta = vec![app.config.feeds[fi].label().to_string()];
    meta.extend(item.author.clone());
    meta.push(fmt_date(item.date, "%Y-%m-%d %H:%M"));
    let header = Text::from(vec![
        Line::from(item.title.clone()).bold(),
        Line::styled(meta.join(" · "), Style::new().fg(t.muted)),
        Line::from(item.link.clone().unwrap_or_default()).underlined(),
    ]);

    // Image slot: tall once decoded, a single status line while loading.
    let img_url = item.image.as_deref().filter(|_| app.config.images);
    let img_state = img_url.and_then(|u| app.images.get_mut(u));
    let room = inner.height.saturating_sub(header_h + 6);
    let img_h = match &img_state {
        Some(ImgState::Ready(_)) => (inner.height / 5 * 2).clamp(6, 28).min(room),
        Some(ImgState::Loading) => 1.min(room),
        _ => 0,
    };

    let [head_area, img_area, text_area] = Layout::vertical([
        Constraint::Length(header_h),
        Constraint::Length(img_h),
        Constraint::Min(0),
    ])
    .areas(inner);
    frame.render_widget(Paragraph::new(header).wrap(Wrap { trim: true }), head_area);

    match img_state {
        Some(ImgState::Ready(proto)) if img_h > 0 => {
            frame.render_stateful_widget(StatefulImage::default(), img_area, proto.as_mut());
        }
        Some(ImgState::Loading) if img_h > 0 => {
            frame.render_widget(
                Paragraph::new("Loading image…").style(Style::new().fg(t.muted).italic()),
                img_area,
            );
        }
        _ => {}
    }

    // Body: HTML -> markdown -> styled text, built once per item and theme.
    if !matches!(&app.body_cache, Some((id, _)) if *id == item.id) {
        let md = html2md::parse_html(&item.html);
        app.body_cache = Some((item.id.clone(), render_markdown(&clean_markdown(&md), &t)));
    }
    let text = app.body_cache.as_ref().expect("just filled").1.clone();
    let para = Paragraph::new(text).wrap(Wrap { trim: false });
    let total = u16::try_from(para.line_count(text_area.width)).unwrap_or(u16::MAX);
    let max_scroll = total.saturating_sub(text_area.height);
    app.scroll = app.scroll.min(max_scroll);
    frame.render_widget(para.scroll((app.scroll, 0)), text_area);

    if max_scroll > 0 {
        let pct = app.scroll as u32 * 100 / max_scroll as u32;
        let tag = Line::styled(format!(" {pct}% "), Style::new().fg(t.muted)).right_aligned();
        frame.render_widget(
            tag,
            Rect {
                y: area.bottom().saturating_sub(1),
                ..area
            },
        );
    }
}

// ---- footer ----------------------------------------------------------------

type Hint = (u8, &'static str, &'static str);

/// Drop the highest-ranked (rightmost on ties) hint until the line fits.
/// Rank 0 (add, help, quit) is never dropped.
fn fit_hints(mut hints: Vec<Hint>, width: u16) -> Vec<Hint> {
    let used = |h: &[Hint]| {
        1 + h
            .iter()
            .map(|(_, k, d)| k.chars().count() + d.chars().count() + 3)
            .sum::<usize>()
    };
    while used(&hints) > width as usize {
        let Some(max) = hints.iter().map(|h| h.0).max().filter(|&r| r > 0) else {
            break;
        };
        let at = hints
            .iter()
            .rposition(|h| h.0 == max)
            .expect("max came from the list");
        hints.remove(at);
    }
    hints
}

fn footer_hints(app: &App, width: u16) -> Line<'static> {
    let has_item = app.current_item().is_some();
    let row = app.selected_row();
    let mut h: Vec<Hint> = match app.focus {
        Focus::Feeds => vec![
            (1, "j/k", "move"),
            (2, "l", "open"),
            (0, "a", "add"),
            (2, "d", "delete"),
            (2, "r", "refresh"),
            (3, "/", "search"),
        ],
        Focus::Items => vec![
            (1, "j/k", "move"),
            (1, "Enter", "read"),
            (0, "a", "add"),
            (3, "m", "toggle read"),
            (4, "M", "all read"),
            (3, "u", "unread only"),
            (2, "/", "search"),
            (3, "o", "browser"),
        ],
        Focus::Article => vec![
            (1, "j/k", "scroll"),
            (2, "Space", "page"),
            (0, "a", "add"),
            (3, "n/p", "next/prev"),
            (3, "o", "browser"),
            (2, "h", "back"),
            (4, "z", "zoom"),
            (5, "i", "images"),
        ],
    };
    // Hints that act on a selection are pointless without one.
    h.retain(|(_, k, _)| match (app.focus, *k) {
        (Focus::Feeds, "d") => matches!(row, Some(Row::Feed(_))),
        (Focus::Feeds, _) => true,
        (_, "Enter" | "m" | "M" | "o" | "n/p" | "Space" | "z") => has_item,
        _ => true,
    });
    h.push((0, "?", "help"));
    h.push((0, "q", "quit"));

    let mut spans = vec![Span::raw(" ")];
    for (_, k, d) in fit_hints(h, width) {
        spans.push(key(&app.theme, k));
        spans.push(Span::styled(
            format!(" {d}  "),
            Style::new().fg(app.theme.muted),
        ));
    }
    Line::from(spans)
}

fn draw_footer(frame: &mut Frame, app: &App, area: Rect) {
    let t = &app.theme;
    let line = if matches!(app.mode, Mode::Search) {
        let prompt = "/";
        set_cursor(
            frame,
            Position::new(
                area.x
                    .saturating_add((prompt.len() + app.filter.chars().count()) as u16),
                area.y,
            ),
        );
        let right = hint_line(t, "Enter keep  Esc clear").right_aligned();
        frame.render_widget(right, area);
        Line::from(vec![key(t, prompt), Span::raw(app.filter.clone())])
    } else {
        match &app.status {
            Some((msg, level, _)) => {
                let color = if *level == Level::Error {
                    t.error
                } else {
                    t.success
                };
                Line::styled(format!(" {msg}"), Style::new().fg(color))
            }
            None => footer_hints(app, area.width),
        }
    };
    frame.render_widget(line, area);
}

// ---- modals ----------------------------------------------------------------

fn draw_help(frame: &mut Frame, t: &Theme) {
    const SECTIONS: &[(&str, &[(&str, &str)])] = &[
        (
            "Navigation",
            &[
                ("j/k ↑/↓", "move / scroll"),
                ("h/l ←/→", "change pane (in Feeds: close / open a folder)"),
                ("g/G", "top / bottom"),
                ("Enter", "open item / toggle folder"),
                ("Esc", "back / clear search or tag filter"),
            ],
        ),
        (
            "Feeds & folders",
            &[
                ("a", "add feed (joins the folder under the cursor)"),
                ("d", "delete feed"),
                ("F", "move feed to a folder (or a new one)"),
                ("t", "edit the feed's tags"),
                ("f", "filter feeds by tag"),
                ("Space", "open / close a folder"),
                ("r / R", "refresh feed or folder / all"),
            ],
        ),
        (
            "Items & articles",
            &[
                ("/", "search items (Enter keeps, Esc clears)"),
                ("m / M", "toggle read / mark all read"),
                ("u", "unread only"),
                ("o", "open in browser"),
                ("n / p", "next / previous article"),
                ("z", "zoom article"),
                ("v", "toggle items/article layout (side by side or stacked)"),
                ("i", "toggle images"),
            ],
        ),
        (
            "General",
            &[
                ("T", "choose theme (b toggles transparency)"),
                ("E", "edit config file ($VISUAL / $EDITOR)"),
                ("?", "this help"),
                ("q", "quit"),
            ],
        ),
    ];
    let mut lines: Vec<Line> = Vec::new();
    for (i, (name, rows)) in SECTIONS.iter().enumerate() {
        if i > 0 {
            lines.push(Line::raw(""));
        }
        lines.push(Line::from(format!(" {name}")).bold());
        for (k, desc) in *rows {
            lines.push(Line::from(vec![
                Span::styled(format!("   {k:<9}"), Style::new().fg(t.accent).bold()),
                Span::raw(*desc),
            ]));
        }
    }
    let area = centered(frame.area(), 72, lines.len() as u16 + 2);
    popup(frame, t, area);
    frame.render_widget(
        Paragraph::new(lines).block(modal(t, "Help", "any key close")),
        area,
    );
}

fn draw_add_feed(frame: &mut Frame, app: &App) {
    let t = &app.theme;
    let Mode::AddFeed(form) = &app.mode else {
        return;
    };
    let area = centered(frame.area(), 64, 7);
    popup(frame, t, area);
    let block = modal(t, "Add feed", "Enter add  Esc cancel");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let [label, field, err] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(0),
    ])
    .areas(inner);
    frame.render_widget(
        Paragraph::new("Feed or site URL").style(Style::new().fg(t.muted)),
        label,
    );
    let cursor = draw_input(frame, t, field, &form.input, "https://example.com/feed.xml");
    if let Some(e) = &form.error {
        frame.render_widget(
            Paragraph::new(e.clone())
                .style(Style::new().fg(t.error))
                .wrap(Wrap { trim: true }),
            err,
        );
    }
    set_cursor(frame, cursor);
}

fn draw_confirm_delete(frame: &mut Frame, app: &App) {
    let t = &app.theme;
    let name = app
        .selected_feed()
        .map_or("", |fi| app.config.feeds[fi].label());
    let area = centered(frame.area(), 54, 6);
    popup(frame, t, area);
    let block = modal(t, "Remove feed", "y remove  n cancel");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let lines = vec![
        Line::from(vec![
            Span::raw("Remove "),
            Span::styled(name.to_string(), Style::new().bold()),
            Span::raw("?"),
        ]),
        Line::styled(
            "It is only removed from your list.",
            Style::new().fg(t.muted),
        ),
    ];
    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: true })
            .style(Style::new().fg(t.warning)),
        inner,
    );
}

fn draw_theme(frame: &mut Frame, app: &App) {
    let t = &app.theme;
    let Mode::Theme(picker) = &app.mode else {
        return;
    };
    let full = frame.area();
    let height = (app.themes.len() as u16 + 4).clamp(5, full.height.saturating_sub(2).max(5));
    let area = centered(full, full.width.saturating_sub(4).clamp(1, 56), height);
    popup(frame, t, area);
    let block = modal(
        t,
        "Theme",
        "↑↓ preview  b background  Enter save  Esc cancel",
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let [bg_row, list_area] =
        Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(inner);
    let bg = if app.transparent {
        "transparent (terminal's own)"
    } else {
        "theme colour"
    };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("Background  ", Style::new().fg(t.muted)),
            Span::styled(bg, Style::new().fg(t.accent)),
        ])),
        bg_row,
    );
    let name_w = app
        .themes
        .iter()
        .map(|th| th.name.chars().count())
        .max()
        .unwrap_or(0);
    let items: Vec<ListItem> = app
        .themes
        .iter()
        .map(|th| {
            let mut spans = vec![Span::raw(format!("{:<name_w$}  ", th.name))];
            // Each theme's own colours, so they can be compared before choosing.
            spans.extend(
                th.swatch()
                    .into_iter()
                    .map(|c| Span::styled("██ ", Style::new().fg(c))),
            );
            ListItem::new(Line::from(spans))
        })
        .collect();
    let list = List::new(items)
        .highlight_style(t.selection(true))
        .highlight_symbol("▶ ");
    let mut state = ListState::default().with_selected(Some(picker.cursor));
    frame.render_stateful_widget(list, list_area, &mut state);
}

/// The tag checklist: a field for a new tag, then every known tag as a checkbox.
fn draw_checklist(
    frame: &mut Frame,
    t: &Theme,
    area: Rect,
    list: &TagChecklist,
) -> Option<Position> {
    if area.height == 0 || area.width < 4 {
        return None;
    }
    let [field_row, rows_area] =
        Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(area);
    let [mark, field] =
        Layout::horizontal([Constraint::Length(2), Constraint::Min(1)]).areas(field_row);
    frame.render_widget(
        Paragraph::new("+ ").style(Style::new().fg(t.accent).bold()),
        mark,
    );
    let cursor = draw_input(frame, t, field, &list.input, "new tag (Enter adds it)");
    let on_field = list.on_input();

    let height = rows_area.height as usize;
    if height == 0 {
        return on_field.then_some(cursor);
    }
    if list.rows.is_empty() {
        frame.render_widget(
            Paragraph::new("No tags yet: type one above").style(Style::new().fg(t.muted)),
            rows_area,
        );
        return on_field.then_some(cursor);
    }
    // Keep the highlighted tag in view.
    let at = list.cursor.saturating_sub(1);
    let start = (at + 1).saturating_sub(height);
    let lines: Vec<Line> = list
        .rows
        .iter()
        .enumerate()
        .skip(start)
        .take(height)
        .map(|(i, row)| {
            let here = list.cursor == i + 1;
            let count = if row.count > 0 {
                format!(" ({})", row.count)
            } else {
                String::new()
            };
            let text = format!(
                "{} [{}] {}{}",
                if here { "▶" } else { " " },
                if row.checked { "x" } else { " " },
                fit(&row.name, (rows_area.width as usize).saturating_sub(10)),
                count
            );
            if here {
                Line::styled(text, t.selection(true))
            } else if row.checked {
                Line::styled(text, Style::new().fg(t.accent))
            } else {
                Line::raw(text)
            }
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), rows_area);
    on_field.then_some(cursor)
}

fn draw_tags(frame: &mut Frame, app: &App) {
    let t = &app.theme;
    let Mode::Tags(editor) = &app.mode else {
        return;
    };
    let full = frame.area();
    let width = full.width.saturating_sub(4).clamp(1, 60);
    let height = (editor.list.rows.len() as u16 + 6)
        .max(9)
        .min(full.height.saturating_sub(2).max(1));
    let area = centered(full, width, height);
    popup(frame, t, area);
    let name = app.config.feeds.get(editor.feed).map_or("", |f| f.label());
    let block = modal(
        t,
        &format!("Tags — {}", fit(name, (width as usize).saturating_sub(16))),
        "Space toggle  Enter save  Del delete tag  Esc cancel",
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let [list_area, note] =
        Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(inner);
    if let Some(pos) = draw_checklist(frame, t, list_area, &editor.list) {
        set_cursor(frame, pos);
    }
    let (text, color) = if let Some(tag) = &editor.list.confirm_delete {
        let n = editor
            .list
            .rows
            .iter()
            .find(|r| &r.name == tag)
            .map_or(0, |r| r.count);
        (format!("Remove #{tag} from {n} feed(s)? y / n"), t.warning)
    } else if let Some(e) = &editor.list.error {
        (e.clone(), t.error)
    } else {
        return;
    };
    frame.render_widget(
        Paragraph::new(fit(&text, note.width as usize)).style(Style::new().fg(color)),
        note,
    );
}

fn draw_folder(frame: &mut Frame, app: &App) {
    let t = &app.theme;
    let Mode::Folder(ed) = &app.mode else { return };
    let full = frame.area();
    let width = full.width.saturating_sub(4).clamp(1, 52);
    let height = (ed.folders.len() as u16 + 7)
        .max(9)
        .min(full.height.saturating_sub(2).max(1));
    let area = centered(full, width, height);
    popup(frame, t, area);
    let name = app.config.feeds.get(ed.feed).map_or("", |f| f.label());
    let block = modal(
        t,
        &format!(
            "Folder — {}",
            fit(name, (width as usize).saturating_sub(16))
        ),
        "Enter choose  Esc cancel",
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let [field_row, rows_area, note] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(inner);
    let [mark, field] =
        Layout::horizontal([Constraint::Length(2), Constraint::Min(1)]).areas(field_row);
    frame.render_widget(
        Paragraph::new("+ ").style(Style::new().fg(t.accent).bold()),
        mark,
    );
    let cursor = draw_input(frame, t, field, &ed.input, "new folder (Enter creates it)");
    if ed.on_input() {
        set_cursor(frame, cursor);
    }

    let current = app
        .config
        .feeds
        .get(ed.feed)
        .and_then(|f| f.folder.as_deref());
    let entries = std::iter::once("(no folder)".to_string()).chain(ed.folders.iter().cloned());
    let height = rows_area.height as usize;
    let start = (ed.cursor.saturating_sub(1) + 1).saturating_sub(height);
    let lines: Vec<Line> = entries
        .enumerate()
        .skip(start)
        .take(height)
        .map(|(i, name)| {
            let here = ed.cursor == i + 1;
            let is_current = if i == 0 {
                current.is_none()
            } else {
                current == Some(name.as_str())
            };
            let text = format!(
                "{} {}{}",
                if here { "▶" } else { " " },
                fit(&name, (rows_area.width as usize).saturating_sub(14)),
                if is_current { "  (current)" } else { "" }
            );
            if here {
                Line::styled(text, t.selection(true))
            } else {
                Line::raw(text)
            }
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), rows_area);
    if let Some(e) = &ed.error {
        frame.render_widget(
            Paragraph::new(fit(e, note.width as usize)).style(Style::new().fg(t.error)),
            note,
        );
    }
}

fn draw_tag_filter(frame: &mut Frame, app: &App) {
    let t = &app.theme;
    let Mode::TagFilter(picker) = &app.mode else {
        return;
    };
    let full = frame.area();
    let height = (picker.entries.len() as u16 + 2).clamp(3, full.height.saturating_sub(2).max(3));
    let area = centered(full, full.width.saturating_sub(4).clamp(1, 44), height);
    popup(frame, t, area);
    let block = modal(
        t,
        "Filter feeds by tag",
        "↑↓ choose  Enter apply  Esc cancel",
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let name_w = picker
        .entries
        .iter()
        .map(|(f, _)| filter_label(f).chars().count())
        .max()
        .unwrap_or(0);
    let items: Vec<ListItem> = picker
        .entries
        .iter()
        .map(|(f, n)| {
            ListItem::new(Line::from(vec![
                Span::raw(format!("{:<name_w$}  ", filter_label(f))),
                Span::styled(n.to_string(), Style::new().fg(t.muted)),
            ]))
        })
        .collect();
    let list = List::new(items)
        .highlight_style(t.selection(true))
        .highlight_symbol("▶ ");
    let mut state = ListState::default().with_selected(Some(picker.cursor));
    frame.render_stateful_widget(list, inner, &mut state);
}

// ---- markdown --------------------------------------------------------------

/// Routes tui-markdown's styles through the theme palette.
#[derive(Clone)]
struct MdStyles(Theme);

impl StyleSheet for MdStyles {
    fn heading(&self, level: u8) -> Style {
        let s = Style::new().fg(self.0.accent).bold();
        match level {
            1 => s.underlined(),
            2 => s,
            3 => s.italic(),
            _ => Style::new().fg(self.0.fg).italic(),
        }
    }
    fn code(&self) -> Style {
        self.0.code()
    }
    fn link(&self) -> Style {
        Style::new().fg(self.0.info).underlined()
    }
    fn blockquote(&self) -> Style {
        Style::new().fg(self.0.muted)
    }
    fn heading_meta(&self) -> Style {
        Style::new().fg(self.0.muted)
    }
    fn metadata_block(&self) -> Style {
        Style::new().fg(self.0.warning)
    }
}

/// Render markdown, drawing fenced code blocks as shaded, unfenced code
/// (tui-markdown would print the fence lines literally).
fn render_markdown(md: &str, t: &Theme) -> Text<'static> {
    let options = Options::new(MdStyles(t.clone()));
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut prose = String::new();
    let mut in_code = false;

    let flush = |prose: &mut String, lines: &mut Vec<Line<'static>>| {
        if !prose.trim().is_empty() {
            lines.extend(owned(tui_markdown::from_str_with_options(prose, &options)).lines);
        }
        prose.clear();
    };

    for line in md.lines() {
        if line.trim_start().starts_with("```") {
            if !in_code {
                flush(&mut prose, &mut lines);
            }
            lines.push(Line::raw(""));
            in_code = !in_code;
        } else if in_code {
            lines.push(Line::styled(format!(" {line} "), t.code()));
        } else {
            prose.push_str(line);
            prose.push('\n');
        }
    }
    flush(&mut prose, &mut lines);
    Text::from(lines)
}

/// Detach rendered markdown from the string it borrows so it can be cached.
fn owned(text: Text<'_>) -> Text<'static> {
    let lines = text.lines.into_iter().map(|l| {
        let spans: Vec<Span<'static>> = l
            .spans
            .into_iter()
            .map(|s| Span::styled(s.content.into_owned(), s.style))
            .collect();
        Line {
            spans,
            style: l.style,
            alignment: l.alignment,
        }
    });
    Text::from(lines.collect::<Vec<_>>())
}

/// Drop markdown image syntax (shown separately above) and collapse blank runs.
fn clean_markdown(md: &str) -> String {
    // Empty-text links are heading anchors (`[](#id)`); they only add noise.
    let mut md = md.to_string();
    while let Some(start) = md.find("[](") {
        let Some(len) = md[start..].find(')') else {
            break;
        };
        md.replace_range(start..start + len + 1, "");
    }
    let mut out = String::with_capacity(md.len());
    let mut blank = 0;
    for line in md.lines() {
        let line = line.trim_end();
        if line.trim_start().starts_with("![") && line.trim_end().ends_with(')') {
            continue;
        }
        blank = if line.is_empty() { blank + 1 } else { 0 };
        if blank <= 1 {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

fn fmt_date(ts: Option<i64>, fmt: &str) -> String {
    ts.and_then(|t| DateTime::from_timestamp(t, 0))
        .map(|d| d.with_timezone(&Local).format(fmt).to_string())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use ratatui::{Terminal, backend::TestBackend, buffer::Buffer, style::Color};

    use super::*;
    use crate::{
        app::AddFeed,
        feed::Item,
        theme::{builtin_themes, find},
    };

    fn app(name: &str) -> App {
        let mut app = App::for_test(name);
        let hn = app
            .config
            .feeds
            .iter()
            .position(|f| f.name == "Hacker News")
            .unwrap();
        app.feeds[hn].items.push(Item {
            id: "a".into(),
            title: "First post".into(),
            link: Some("https://example.com/1".into()),
            author: Some("Ann".into()),
            date: Some(1_700_000_000),
            html: "<p>Hello <b>world</b></p><pre>let x = 1;</pre>".into(),
            image: None,
        });
        app.rebuild_visible(false);
        app
    }

    fn render(app: &mut App, w: u16, h: u16) -> Buffer {
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| draw(f, app)).unwrap();
        term.backend().buffer().clone()
    }

    fn text(buf: &Buffer) -> String {
        (0..buf.area.height)
            .map(|y| {
                (0..buf.area.width)
                    .map(|x| buf[(x, y)].symbol().to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn find_text(buf: &Buffer, needle: &str) -> Option<(u16, u16)> {
        (0..buf.area.height).find_map(|y| {
            let row: String = (0..buf.area.width)
                .map(|x| buf[(x, y)].symbol().to_string())
                .collect();
            row.find(needle)
                .map(|i| (row[..i].chars().count() as u16, y))
        })
    }

    fn goto(app: &mut App, row: Row) {
        let at = app.rows.iter().position(|r| *r == row).unwrap();
        app.feed_list.select(Some(at));
        app.rebuild_visible(false);
    }

    fn modes() -> Vec<fn(&mut App)> {
        vec![
            |_| {},
            |a| a.mode = Mode::Help,
            |a| {
                let mut form = AddFeed::default();
                form.input.set(&"https://x".repeat(20));
                form.error = Some("bad".into());
                a.mode = Mode::AddFeed(form);
            },
            |a| {
                goto(a, Row::Feed(0));
                a.mode = Mode::ConfirmDelete;
            },
            |a| a.mode = Mode::Search,
            |a| {
                a.zoom = true;
                a.focus = Focus::Article;
            },
            |a| {
                goto(a, Row::Feed(0));
                a.on_key_for_test('t');
            },
            |a| {
                goto(a, Row::Feed(0));
                a.on_key_for_test('F');
            },
            |a| a.on_key_for_test('f'),
            |a| a.on_key_for_test('T'),
            |a| a.on_key_for_test('v'),
        ]
    }

    #[test]
    fn nothing_panics_at_any_size() {
        for (n, setup) in modes().into_iter().enumerate() {
            for (w, h) in [
                (0, 0),
                (1, 1),
                (3, 2),
                (10, 3),
                (24, 5),
                (40, 10),
                (80, 24),
                (200, 60),
            ] {
                let mut a = app(&format!("size{n}"));
                setup(&mut a);
                render(&mut a, w, h);
            }
        }
    }

    #[test]
    fn header_is_a_titled_box_with_the_refresh_time_of_the_active_feed() {
        let mut a = app("header");
        let hn = a
            .config
            .feeds
            .iter()
            .position(|f| f.name == "Hacker News")
            .unwrap();
        goto(&mut a, Row::Feed(hn));
        let out = text(&render(&mut a, 140, 30));
        let top: Vec<&str> = out.lines().take(3).collect();
        assert!(
            top[0].contains("rsstui") && top[0].starts_with('╭'),
            "{top:?}"
        );
        assert!(top[2].starts_with('╰'), "{top:?}");
        assert!(
            top[1].contains("Refreshed") && top[1].contains("never"),
            "{top:?}"
        );
        assert!(top[2].contains("News ▸ Hacker News"), "{top:?}");
        assert!(top[2].contains("#aggregator #news"), "{top:?}");

        a.feeds[hn].refreshed = Some(Local::now());
        let out = text(&render(&mut a, 140, 30));
        assert!(out.lines().nth(1).unwrap().contains("just now"), "{out}");

        // Another feed has its own time.
        let rust = a
            .config
            .feeds
            .iter()
            .position(|f| f.name == "Rust Blog")
            .unwrap();
        goto(&mut a, Row::Feed(rust));
        assert!(
            text(&render(&mut a, 140, 30))
                .lines()
                .nth(1)
                .unwrap()
                .contains("never")
        );
    }

    #[test]
    fn panes_have_rounded_corners() {
        let mut a = app("round");
        let buf = render(&mut a, 120, 30);
        let (x, y) = find_text(&buf, "Feeds").unwrap();
        assert_eq!(buf[(x - 2, y)].symbol(), "╭");
        for p in ["Items", "Article"] {
            let (x, y) = find_text(&buf, p).unwrap();
            assert_eq!(buf[(x - 2, y)].symbol(), "╭", "{p}");
        }
    }

    #[test]
    fn feeds_pane_shows_folders_with_their_feeds_indented() {
        let mut a = app("tree");
        let out = text(&render(&mut a, 140, 40));
        assert!(out.contains("▾ News"), "{out}");
        assert!(out.contains("▾ Writers"), "{out}");
        assert!(
            out.contains("   Hacker News"),
            "feeds sit under their folder"
        );
        a.state.collapsed.insert("News".into());
        a.rebuild_rows();
        let out = text(&render(&mut a, 140, 40));
        assert!(
            out.contains("▸ News") && !out.contains("   Lobsters"),
            "{out}"
        );
    }

    #[test]
    fn shows_article_with_unfenced_code() {
        let mut a = app("article");
        let hn = a
            .config
            .feeds
            .iter()
            .position(|f| f.name == "Hacker News")
            .unwrap();
        goto(&mut a, Row::Feed(hn));
        let out = text(&render(&mut a, 140, 40));
        assert!(
            out.contains("First post") && out.contains("Hello world") && out.contains("let x = 1;")
        );
        assert!(!out.contains("```"));
    }

    #[test]
    fn footer_hint_keys_use_the_accent_colour() {
        let mut a = app("hints");
        let buf = render(&mut a, 140, 30);
        let (x, y) = find_text(&buf, "add").expect("footer shows add");
        assert_eq!(buf[(x - 2, y)].symbol(), "a");
        assert_eq!(buf[(x - 2, y)].fg, a.theme.accent);
    }

    #[test]
    fn narrow_footer_keeps_add_help_quit() {
        let mut a = app("narrow");
        let out = text(&render(&mut a, 40, 12));
        let footer = out.lines().last().unwrap();
        for k in ["add", "help", "quit"] {
            assert!(footer.contains(k), "{footer:?} lost {k}");
        }
    }

    #[test]
    fn selection_hints_hide_without_a_selection() {
        let mut a = app("nosel");
        a.focus = Focus::Items;
        goto(&mut a, Row::Feed(0));
        a.feeds[0].items.clear();
        a.rebuild_visible(false);
        let out = text(&render(&mut a, 200, 30));
        let footer = out.lines().last().unwrap();
        assert!(
            !footer.contains("toggle read") && !footer.contains("browser"),
            "{footer:?}"
        );
        a.focus = Focus::Feeds;
        goto(&mut a, Row::All);
        let out = text(&render(&mut a, 200, 30));
        assert!(
            !out.lines().last().unwrap().contains("delete"),
            "no feed under the cursor"
        );
    }

    #[test]
    fn popups_keep_the_page_background() {
        let nord = find(&builtin_themes(), "nord").unwrap().clone();
        for transparent in [false, true] {
            let mut a = app("bg");
            a.set_theme_for_test("nord", transparent);
            a.mode = Mode::Help;
            let buf = render(&mut a, 120, 50);
            let (x, y) = find_text(&buf, "Navigation").expect("help open");
            let want = if transparent { Color::Reset } else { nord.bg };
            assert_eq!(buf[(x + 20, y)].bg, want, "transparent={transparent}");
        }
    }

    #[test]
    fn add_feed_dialog_shows_its_error_inline() {
        let mut a = app("adderr");
        let mut form = AddFeed::default();
        form.input.set("nope");
        form.error = Some("Not a valid URL".into());
        a.mode = Mode::AddFeed(form);
        let buf = render(&mut a, 100, 30);
        let (x, y) = find_text(&buf, "Not a valid URL").expect("error shown in dialog");
        assert_eq!(buf[(x, y)].fg, a.theme.error);
        let (x, y) = find_text(&buf, "Esc").expect("dialog hints");
        assert_eq!(buf[(x, y)].fg, a.theme.accent);
    }

    #[test]
    fn tag_folder_and_filter_dialogs_render_their_contents() {
        let mut a = app("dialogs");
        let go = a
            .config
            .feeds
            .iter()
            .position(|f| f.name == "Go Blog")
            .unwrap();
        goto(&mut a, Row::Feed(go));
        a.on_key_for_test('t');
        let out = text(&render(&mut a, 120, 40));
        assert!(
            out.contains("Tags — Go Blog") && out.contains("[x] go") && out.contains("[ ] rust"),
            "{out}"
        );

        let mut a = app("dialogs2");
        goto(&mut a, Row::Feed(go));
        a.on_key_for_test('F');
        let out = text(&render(&mut a, 120, 40));
        assert!(
            out.contains("Folder — Go Blog")
                && out.contains("(no folder)")
                && out.contains("Languages & Platforms  (current)"),
            "{out}"
        );

        let mut a = app("dialogs3");
        a.on_key_for_test('f');
        let out = text(&render(&mut a, 120, 40));
        assert!(
            out.contains("Filter feeds by tag")
                && out.contains("untagged")
                && out.contains("#news"),
            "{out}"
        );
    }

    #[test]
    fn theme_picker_lists_every_theme_file() {
        let mut a = app("themes");
        a.on_key_for_test('T');
        let out = text(&render(&mut a, 100, 40));
        for n in [
            "default",
            "nord",
            "tokyo-night",
            "catppuccin-frappe",
            "kanagawa",
        ] {
            assert!(out.contains(n), "{n} missing:\n{out}");
        }
    }

    #[test]
    fn layout_toggle_stacks_items_above_the_article_and_is_saved() {
        let mut a = app("layout");
        let side = render(&mut a, 160, 40);
        let (ix, iy) = find_text(&side, "Items").unwrap();
        let (ax, ay) = find_text(&side, "Article").unwrap();
        assert_eq!(iy, ay, "side by side: same row");
        assert!(ax > ix);

        a.on_key_for_test('v');
        assert_eq!(a.config.layout, crate::config::Layout::Vertical);
        let stacked = render(&mut a, 160, 40);
        let (ix2, iy2) = find_text(&stacked, "Items").unwrap();
        let (ax2, ay2) = find_text(&stacked, "Article").unwrap();
        assert_eq!(ix2, ax2, "stacked: same column");
        assert!(ay2 > iy2);
        assert!(
            crate::config::Config::load(&a.paths.config).unwrap().layout
                == crate::config::Layout::Vertical
        );

        a.on_key_for_test('v');
        assert_eq!(a.config.layout, crate::config::Layout::Horizontal);
    }

    #[test]
    fn feeds_pane_is_31_wide_and_items_27_percent() {
        let mut a = app("widths");
        let buf = render(&mut a, 200, 40);
        let (items_x, _) = find_text(&buf, "Items").unwrap();
        assert_eq!(
            items_x - 2,
            31,
            "items pane starts after the 31-wide feeds pane"
        );
        let (article_x, _) = find_text(&buf, "Article").unwrap();
        assert_eq!(article_x - 2 - 31, 54, "27% of 200");
    }

    #[test]
    fn an_empty_config_explains_how_to_add_feeds_and_never_panics() {
        for (w, h) in [(0, 0), (1, 1), (10, 3), (40, 10), (80, 24), (160, 50)] {
            let mut a = app("empty");
            a.config.feeds.clear();
            a.feeds.clear();
            a.rebuild_rows();
            a.rebuild_visible(false);
            let buf = render(&mut a, w, h);
            if w >= 80 {
                let out = text(&buf);
                assert!(
                    out.contains("No feeds yet.") && out.contains("add a feed"),
                    "{out}"
                );
                assert!(out.contains("0 feeds"));
            }
        }
        // And the keys that need a feed say so instead of doing nothing.
        let mut a = app("empty2");
        a.config.feeds.clear();
        a.feeds.clear();
        a.rebuild_rows();
        for c in ['t', 'F', 'd', 'f', 'o', 'm', 'r', 'R', 'M', 'u'] {
            a.on_key_for_test(c);
            a.on_key_for_test('\u{1b}'); // harmless if a dialog opened
        }
    }

    #[test]
    fn fmt_ago_buckets() {
        let now = Local::now();
        let ago = |s: i64| fmt_ago(now - chrono::Duration::seconds(s), now);
        assert_eq!(ago(5), "just now");
        assert_eq!(ago(120), "2m ago");
        assert_eq!(ago(7200), "2h ago");
        assert_eq!(ago(3 * 86400), "3d ago");
    }
}
