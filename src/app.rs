use std::{
    collections::HashMap,
    process::Command,
    sync::mpsc::{self, Receiver, Sender},
    thread,
    time::{Duration, Instant},
};

use anyhow::Result;
use chrono::{DateTime, Local};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use image::DynamicImage;
use ratatui::{DefaultTerminal, widgets::ListState};
use ratatui_image::{picker::Picker, protocol::StatefulProtocol};
use reqwest::blocking::Client;

use crate::{
    config::{self, Config, FeedConfig, Paths, State},
    feed::{self, Item, Parsed},
    forms::{FolderChoice, FolderEditor, TagChecklist},
    input::Input,
    theme::{self, Named, Theme},
    ui,
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Feeds,
    Items,
    Article,
}

/// A line of the Feeds pane.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Row {
    All,
    Folder(String),
    Feed(usize),
}

/// Which feeds the Feeds pane shows, by tag.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Filter {
    All,
    Untagged,
    Tag(String),
}

pub enum Mode {
    Normal,
    AddFeed(AddFeed),
    ConfirmDelete,
    Search,
    Theme(ThemePicker),
    Tags(TagEditor),
    Folder(FolderEditor),
    TagFilter(FilterPicker),
    Help,
}

#[derive(Default)]
pub struct AddFeed {
    pub input: Input,
    /// Shown inside the dialog so the user can fix it without losing the input.
    pub error: Option<String>,
}

pub struct ThemePicker {
    pub cursor: usize,
    /// Restored on Esc, since moving previews live.
    original: String,
    original_transparent: bool,
}

pub struct TagEditor {
    pub feed: usize,
    pub list: TagChecklist,
}

pub struct FilterPicker {
    pub entries: Vec<(Filter, usize)>,
    pub cursor: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Ok,
    Error,
}

pub enum ImgState {
    Loading,
    Failed,
    Ready(Box<StatefulProtocol>),
}

enum Msg {
    Feed {
        url: String,
        result: Result<Parsed, String>,
    },
    Image {
        url: String,
        image: Option<DynamicImage>,
    },
}

#[derive(Default)]
pub struct FeedState {
    pub items: Vec<Item>,
    pub loading: bool,
    pub error: Option<String>,
    /// When the last fetch succeeded.
    pub refreshed: Option<DateTime<Local>>,
}

pub struct App {
    pub config: Config,
    pub state: State,
    pub paths: Paths,
    pub feeds: Vec<FeedState>,
    /// The Feeds pane, top to bottom; rebuilt by [`App::rebuild_rows`].
    pub rows: Vec<Row>,
    pub focus: Focus,
    pub mode: Mode,
    pub feed_list: ListState,
    pub item_list: ListState,
    /// (feed index, item index) for the items pane, newest first.
    pub visible: Vec<(usize, usize)>,
    pub scroll: u16,
    pub article_height: u16,
    pub images: HashMap<String, ImgState>,
    pub body_cache: Option<(String, ratatui::text::Text<'static>)>,
    /// Text search over the items pane.
    pub filter: String,
    pub tag_filter: Filter,
    pub unread_only: bool,
    pub zoom: bool,
    pub status: Option<(String, Level, Instant)>,
    pub themes: Vec<Named>,
    pub theme: Theme,
    pub theme_name: String,
    pub transparent: bool,
    pub dirty: bool,
    quit: bool,
    edit_requested: bool,
    picker: Picker,
    client: Client,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    last_item: Option<String>,
    last_refresh: Instant,
    last_paint: Instant,
}

impl App {
    pub fn new(
        config: Config,
        state: State,
        picker: Picker,
        themes: Vec<Named>,
        paths: Paths,
    ) -> Result<Self> {
        let (tx, rx) = mpsc::channel();
        let feeds = config.feeds.iter().map(|_| FeedState::default()).collect();
        let transparent = config.transparent_background;
        let active: Theme = theme::find(&themes, &config.theme)
            .unwrap_or(&themes[0])
            .clone();
        let mut app = Self {
            config,
            state,
            paths,
            feeds,
            rows: Vec::new(),
            focus: Focus::Feeds,
            mode: Mode::Normal,
            feed_list: ListState::default().with_selected(Some(0)),
            item_list: ListState::default(),
            visible: Vec::new(),
            scroll: 0,
            article_height: 0,
            images: HashMap::new(),
            body_cache: None,
            filter: String::new(),
            tag_filter: Filter::All,
            unread_only: false,
            zoom: false,
            status: None,
            themes,
            theme_name: active.name.clone(),
            theme: active.transparent(transparent),
            transparent,
            dirty: true,
            quit: false,
            edit_requested: false,
            picker,
            client: feed::client()?,
            tx,
            rx,
            last_item: None,
            last_refresh: Instant::now(),
            last_paint: Instant::now(),
        };
        app.rebuild_rows();
        app.refresh_all();
        Ok(app)
    }

    // ---- feeds pane --------------------------------------------------------

    pub fn selected_row(&self) -> Option<Row> {
        self.feed_list
            .selected()
            .and_then(|i| self.rows.get(i).cloned())
    }

    /// The selected feed, if the cursor is on one.
    pub fn selected_feed(&self) -> Option<usize> {
        match self.selected_row() {
            Some(Row::Feed(i)) => Some(i),
            _ => None,
        }
    }

    fn folder_of(&self, fi: usize) -> Option<&str> {
        self.config.feeds[fi].folder.as_deref()
    }

    pub fn feed_matches(&self, fi: usize) -> bool {
        let tags = &self.config.feeds[fi].tags;
        match &self.tag_filter {
            Filter::All => true,
            Filter::Untagged => tags.is_empty(),
            Filter::Tag(t) => tags.contains(t),
        }
    }

    /// Folders in the order they first appear in the config.
    pub fn folders(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for f in &self.config.feeds {
            if let Some(folder) = &f.folder
                && !out.contains(folder)
            {
                out.push(folder.clone());
            }
        }
        out
    }

    /// Rebuild the pane for the current tag filter and collapsed folders,
    /// keeping the cursor on the same row (or its folder if it was hidden).
    pub fn rebuild_rows(&mut self) {
        let keep = self.selected_row();
        let shown: Vec<usize> = (0..self.config.feeds.len())
            .filter(|&i| self.feed_matches(i))
            .collect();
        let mut rows = vec![Row::All];
        for folder in self.folders() {
            let members: Vec<usize> = shown
                .iter()
                .copied()
                .filter(|&i| self.folder_of(i) == Some(&folder))
                .collect();
            if members.is_empty() {
                continue;
            }
            let open = !self.state.collapsed.contains(&folder);
            rows.push(Row::Folder(folder));
            if open {
                rows.extend(members.into_iter().map(Row::Feed));
            }
        }
        rows.extend(
            shown
                .iter()
                .copied()
                .filter(|&i| self.folder_of(i).is_none())
                .map(Row::Feed),
        );

        let at = keep
            .and_then(|k| {
                rows.iter().position(|r| *r == k).or_else(|| match k {
                    Row::Feed(i) => self
                        .folder_of(i)
                        .and_then(|f| rows.iter().position(|r| *r == Row::Folder(f.into()))),
                    _ => None,
                })
            })
            .or_else(|| self.feed_list.selected().map(|i| i.min(rows.len() - 1)))
            .unwrap_or(0);
        self.rows = rows;
        self.feed_list.select(Some(at));
        self.dirty = true;
    }

    /// Feeds whose items the middle pane lists: the cursor's feed, folder or all.
    pub fn scope_feeds(&self) -> Vec<usize> {
        let matching = (0..self.config.feeds.len()).filter(|&i| self.feed_matches(i));
        match self.selected_row() {
            Some(Row::Feed(i)) => vec![i],
            Some(Row::Folder(f)) => matching
                .filter(|&i| self.folder_of(i) == Some(&f))
                .collect(),
            _ => matching.collect(),
        }
    }

    pub fn unread_in(&self, row: &Row) -> usize {
        let feeds: Vec<usize> = match row {
            Row::Feed(i) => vec![*i],
            Row::Folder(f) => (0..self.feeds.len())
                .filter(|&i| self.feed_matches(i) && self.folder_of(i) == Some(f))
                .collect(),
            Row::All => (0..self.feeds.len())
                .filter(|&i| self.feed_matches(i))
                .collect(),
        };
        feeds
            .iter()
            .map(|&i| {
                self.feeds[i]
                    .items
                    .iter()
                    .filter(|it| !self.is_read(&it.id))
                    .count()
            })
            .sum()
    }

    fn select_row(&mut self, at: usize) {
        self.feed_list.select(Some(at));
        self.rebuild_visible(false);
    }

    /// (tag, number of feeds carrying it), sorted by name.
    pub fn tag_counts(&self) -> Vec<(String, usize)> {
        let mut counts: std::collections::BTreeMap<String, usize> = Default::default();
        for f in &self.config.feeds {
            for t in &f.tags {
                *counts.entry(t.clone()).or_default() += 1;
            }
        }
        counts.into_iter().collect()
    }

    // ---- selection helpers -------------------------------------------------

    fn current(&self) -> Option<(usize, usize)> {
        self.item_list
            .selected()
            .and_then(|i| self.visible.get(i).copied())
    }

    pub fn current_item(&self) -> Option<&Item> {
        self.current().map(|(f, i)| &self.feeds[f].items[i])
    }

    pub fn is_read(&self, id: &str) -> bool {
        self.state.read.contains(id)
    }

    pub fn flash(&mut self, msg: impl Into<String>) {
        self.status = Some((msg.into(), Level::Ok, Instant::now()));
        self.dirty = true;
    }

    pub fn flash_err(&mut self, msg: impl Into<String>) {
        self.status = Some((msg.into(), Level::Error, Instant::now()));
        self.dirty = true;
    }

    // ---- theme -------------------------------------------------------------

    /// Switch palette live; `body_cache` holds theme-styled text, so drop it.
    fn apply_theme(&mut self, name: &str, transparent: bool) {
        if let Some(t) = theme::find(&self.themes, name) {
            self.theme_name = t.name.clone();
            self.theme = t.clone().transparent(transparent);
        }
        self.transparent = transparent;
        self.body_cache = None;
        self.dirty = true;
    }

    /// Re-read the theme files so edits show up without restarting.
    fn reload_themes(&mut self) -> Result<()> {
        self.themes = theme::load_themes(&self.paths.themes, &self.config.themes)?;
        Ok(())
    }

    // ---- data --------------------------------------------------------------

    fn refresh(&mut self, fi: usize) {
        let url = self.config.feeds[fi].url.clone();
        self.feeds[fi].loading = true;
        let (client, tx) = (self.client.clone(), self.tx.clone());
        thread::spawn(move || {
            let result = feed::fetch(&client, &url).map_err(|e| format!("{e:#}"));
            let _ = tx.send(Msg::Feed { url, result });
        });
        self.dirty = true;
    }

    fn refresh_all(&mut self) {
        for fi in 0..self.feeds.len() {
            self.refresh(fi);
        }
        self.last_refresh = Instant::now();
    }

    fn refresh_selected(&mut self) {
        match self.selected_row() {
            Some(Row::Feed(fi)) => self.refresh(fi),
            Some(Row::Folder(_)) => {
                for fi in self.scope_feeds() {
                    self.refresh(fi);
                }
            }
            _ => self.refresh_all(),
        }
    }

    fn handle_msg(&mut self, msg: Msg) {
        match msg {
            Msg::Feed { url, result } => {
                let Some(fi) = self.config.feeds.iter().position(|f| f.url == url) else {
                    return;
                };
                let fs = &mut self.feeds[fi];
                fs.loading = false;
                match result {
                    Ok(parsed) => {
                        fs.items = parsed.items;
                        fs.error = None;
                        fs.refreshed = Some(Local::now());
                        if let Some(title) = parsed.title
                            && self.config.feeds[fi].name.is_empty()
                        {
                            let mut feeds = self.config.feeds.clone();
                            feeds[fi].name = title;
                            if let Err(e) = self.commit_feeds(feeds) {
                                self.flash_err(format!("Could not save feed name: {e}"));
                            }
                        }
                        self.rebuild_visible(true);
                    }
                    Err(e) => fs.error = Some(e),
                }
            }
            Msg::Image { url, image } => {
                let st = match image {
                    Some(img) => ImgState::Ready(Box::new(self.picker.new_resize_protocol(img))),
                    None => ImgState::Failed,
                };
                self.images.insert(url, st);
            }
        }
        self.dirty = true;
    }

    /// Persist a new feed list to the config, then adopt it.
    fn commit_feeds(&mut self, feeds: Vec<FeedConfig>) -> Result<(), String> {
        config::set_feeds(&self.paths.config, &feeds).map_err(|e| format!("{e:#}"))?;
        self.config.feeds = feeds;
        Ok(())
    }

    fn ensure_image(&mut self) {
        if !self.config.images {
            return;
        }
        let Some(url) = self.current_item().and_then(|i| i.image.clone()) else {
            return;
        };
        if self.images.contains_key(&url) {
            return;
        }
        self.images.insert(url.clone(), ImgState::Loading);
        let (client, tx) = (self.client.clone(), self.tx.clone());
        thread::spawn(move || {
            let image = feed::fetch_image(&client, &url).ok();
            let _ = tx.send(Msg::Image { url, image });
        });
    }

    pub fn rebuild_visible(&mut self, keep_selection: bool) {
        // Follow the selected item as the list reshuffles, except at the top:
        // there, newly arrived feeds should not drag the selection down the list.
        let at_top = self.item_list.selected().unwrap_or(0) == 0;
        let keep = (keep_selection && !at_top)
            .then(|| self.current_item().map(|i| i.id.clone()))
            .flatten();
        let needle = self.filter.to_lowercase();
        let matches = |it: &Item| {
            needle.is_empty()
                || it.title.to_lowercase().contains(&needle)
                || it
                    .author
                    .as_deref()
                    .is_some_and(|a| a.to_lowercase().contains(&needle))
                || it.html.to_lowercase().contains(&needle)
        };
        let mut v = Vec::new();
        for fi in self.scope_feeds() {
            for (ii, it) in self.feeds[fi].items.iter().enumerate() {
                let hide = self.unread_only
                    && self.state.read.contains(&it.id)
                    && keep.as_ref() != Some(&it.id);
                if !hide && matches(it) {
                    v.push((fi, ii));
                }
            }
        }
        // Stable, so undated items keep feed order (and sort last).
        v.sort_by_key(|&(f, i)| std::cmp::Reverse(self.feeds[f].items[i].date));

        let prev = self.item_list.selected().unwrap_or(0);
        let sel = keep
            .and_then(|id| v.iter().position(|&(f, i)| self.feeds[f].items[i].id == id))
            .unwrap_or(if keep_selection {
                prev.min(v.len().saturating_sub(1))
            } else {
                0
            });
        self.visible = v;
        self.item_list
            .select((!self.visible.is_empty()).then_some(sel));
        self.dirty = true;
    }

    fn save_state(&mut self) {
        if let Err(e) = self.state.save(&self.paths.state) {
            self.flash_err(format!("Could not save state: {e:#}"));
        }
    }

    fn set_read(&mut self, id: &str, read: bool) {
        let changed = if read {
            self.state.read.insert(id.to_string())
        } else {
            self.state.read.remove(id)
        };
        if changed {
            self.save_state();
            self.dirty = true;
        }
    }

    fn mark_current_read(&mut self) {
        if let Some(id) = self.current_item().map(|i| i.id.clone()) {
            self.set_read(&id, true);
        }
    }

    // ---- ticking -----------------------------------------------------------

    fn tick(&mut self) {
        while let Ok(msg) = self.rx.try_recv() {
            self.handle_msg(msg);
        }
        if self
            .status
            .as_ref()
            .is_some_and(|(_, _, t)| t.elapsed() > Duration::from_secs(4))
        {
            self.status = None;
            self.dirty = true;
        }
        // The header's "refreshed N min ago" ages even when nothing else changes.
        if self.last_paint.elapsed() > Duration::from_secs(30) {
            self.dirty = true;
        }
        let mins = self.config.refresh_minutes;
        if mins > 0 && self.last_refresh.elapsed() > Duration::from_secs(mins * 60) {
            self.refresh_all();
        }
        let id = self.current_item().map(|i| i.id.clone());
        if id != self.last_item {
            self.last_item = id;
            self.scroll = 0;
            self.dirty = true;
        }
        self.ensure_image();
    }

    // ---- input -------------------------------------------------------------

    fn on_key(&mut self, key: KeyEvent) {
        self.dirty = true;
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.quit = true;
            return;
        }
        match self.mode {
            Mode::AddFeed(_) => self.key_add_feed(key),
            Mode::ConfirmDelete => {
                if key.code == KeyCode::Char('y') {
                    self.delete_selected_feed();
                }
                self.mode = Mode::Normal;
            }
            Mode::Search => match key.code {
                KeyCode::Esc => {
                    self.filter.clear();
                    self.mode = Mode::Normal;
                    self.rebuild_visible(false);
                }
                KeyCode::Enter => self.mode = Mode::Normal,
                KeyCode::Backspace => {
                    self.filter.pop();
                    self.rebuild_visible(false);
                }
                KeyCode::Char(c) => {
                    self.filter.push(c);
                    self.rebuild_visible(false);
                }
                _ => {}
            },
            Mode::Theme(_) => self.key_theme(key),
            Mode::Tags(_) => self.key_tags(key),
            Mode::Folder(_) => self.key_folder(key),
            Mode::TagFilter(_) => self.key_tag_filter(key),
            Mode::Help => self.mode = Mode::Normal,
            Mode::Normal => self.on_key_normal(key),
        }
    }

    /// Plain characters are text here, so `q`, `a` and friends can be typed in a URL.
    fn key_add_feed(&mut self, key: KeyEvent) {
        let Mode::AddFeed(form) = &mut self.mode else {
            return;
        };
        match key.code {
            KeyCode::Esc => self.mode = Mode::Normal,
            KeyCode::Enter => {
                let input = form.input.text().trim().to_string();
                match self.try_add_feed(&input) {
                    Ok(()) => self.mode = Mode::Normal,
                    Err(e) => {
                        if let Mode::AddFeed(form) = &mut self.mode {
                            form.error = Some(e);
                        }
                    }
                }
            }
            _ => {
                if form.input.handle_key(&key) {
                    form.error = None;
                }
            }
        }
    }

    fn open_theme_picker(&mut self) {
        // Themes are files: pick up edits and new files each time.
        if let Err(e) = self.reload_themes() {
            self.flash_err(format!("Themes not reloaded: {e:#}"));
        }
        let cursor = self
            .themes
            .iter()
            .position(|t| t.name == self.theme_name)
            .unwrap_or(0);
        self.mode = Mode::Theme(ThemePicker {
            cursor,
            original: self.theme_name.clone(),
            original_transparent: self.transparent,
        });
    }

    fn key_theme(&mut self, key: KeyEvent) {
        use KeyCode::*;
        let last = self.themes.len().saturating_sub(1);
        let Mode::Theme(picker) = &mut self.mode else {
            return;
        };
        let cur = picker.cursor;
        let moved = match key.code {
            Char('j') | Down => Some((cur + 1).min(last)),
            Char('k') | Up => Some(cur.saturating_sub(1)),
            Char('g') | Home => Some(0),
            Char('G') | End => Some(last),
            _ => None,
        };
        if let Some(n) = moved {
            picker.cursor = n;
            let name = self.themes[n].name.clone();
            let transparent = self.transparent;
            self.apply_theme(&name, transparent);
            return;
        }
        let (original, original_transparent) =
            (picker.original.clone(), picker.original_transparent);
        match key.code {
            Char('b') => {
                let (name, on) = (self.theme_name.clone(), !self.transparent);
                self.apply_theme(&name, on);
            }
            Enter => {
                self.mode = Mode::Normal;
                let mut saved = config::set_value(
                    &self.paths.config,
                    "theme",
                    &config::yaml_scalar(&self.theme_name),
                );
                if saved.is_ok() && self.transparent != original_transparent {
                    saved = config::set_value(
                        &self.paths.config,
                        "transparent_background",
                        &self.transparent.to_string(),
                    );
                }
                match saved {
                    Ok(()) => {
                        self.config.theme = self.theme_name.clone();
                        self.config.transparent_background = self.transparent;
                        self.flash(format!("Theme: {}", self.theme_name));
                    }
                    Err(e) => self.flash_err(format!("Theme not saved: {e:#}")),
                }
            }
            Esc | Char('q') => {
                self.apply_theme(&original, original_transparent);
                self.mode = Mode::Normal;
            }
            _ => {}
        }
    }

    // ---- tags ---------------------------------------------------------------

    fn open_tag_editor(&mut self) {
        let Some(fi) = self.selected_feed() else {
            return self.flash_err("Select a feed to tag");
        };
        let list = TagChecklist::new(self.tag_counts(), &self.config.feeds[fi].tags);
        self.mode = Mode::Tags(TagEditor { feed: fi, list });
    }

    fn key_tags(&mut self, key: KeyEvent) {
        let Mode::Tags(ed) = &mut self.mode else {
            return;
        };
        // Esc first backs out of a pending delete, then closes.
        if key.code == KeyCode::Esc && ed.list.confirm_delete.is_none() {
            self.mode = Mode::Normal;
            return;
        }
        let save = ed.list.handle_key(&key);
        let deleted = ed.list.take_deleted();
        let fi = ed.feed;
        if let Some(tag) = deleted {
            let feeds: Vec<FeedConfig> = self
                .config
                .feeds
                .iter()
                .cloned()
                .map(|mut f| {
                    f.tags.retain(|t| *t != tag);
                    f
                })
                .collect();
            match self.commit_feeds(feeds) {
                Ok(()) => {
                    if let Mode::Tags(ed) = &mut self.mode {
                        ed.list.remove(&tag);
                    }
                    self.fix_tag_filter();
                }
                Err(e) => self.set_tag_error(e),
            }
        }
        if save && let Mode::Tags(ed) = &self.mode {
            let tags = ed.list.checked();
            let mut feeds = self.config.feeds.clone();
            feeds[fi].tags = tags;
            match self.commit_feeds(feeds) {
                Ok(()) => {
                    self.mode = Mode::Normal;
                    self.fix_tag_filter();
                    self.rebuild_rows();
                    self.rebuild_visible(true);
                    self.flash("Tags saved");
                }
                Err(e) => self.set_tag_error(e),
            }
        }
    }

    fn set_tag_error(&mut self, e: String) {
        if let Mode::Tags(ed) = &mut self.mode {
            ed.list.error = Some(e);
        }
    }

    /// Drop a tag filter that no longer matches any feed.
    fn fix_tag_filter(&mut self) {
        let any = (0..self.config.feeds.len()).any(|i| self.feed_matches(i));
        if !any && self.tag_filter != Filter::All {
            self.tag_filter = Filter::All;
            self.rebuild_rows();
            self.rebuild_visible(false);
        }
    }

    fn open_tag_filter(&mut self) {
        let counts = self.tag_counts();
        if counts.is_empty() {
            return self.flash_err("No tags yet: press t to tag the selected feed");
        }
        let untagged = self
            .config
            .feeds
            .iter()
            .filter(|f| f.tags.is_empty())
            .count();
        let mut entries = vec![
            (Filter::All, self.config.feeds.len()),
            (Filter::Untagged, untagged),
        ];
        entries.extend(counts.into_iter().map(|(t, n)| (Filter::Tag(t), n)));
        let cursor = entries
            .iter()
            .position(|(f, _)| *f == self.tag_filter)
            .unwrap_or(0);
        self.mode = Mode::TagFilter(FilterPicker { entries, cursor });
    }

    fn key_tag_filter(&mut self, key: KeyEvent) {
        use KeyCode::*;
        let Mode::TagFilter(picker) = &mut self.mode else {
            return;
        };
        let last = picker.entries.len().saturating_sub(1);
        match key.code {
            Down | Char('j') => picker.cursor = (picker.cursor + 1).min(last),
            Up | Char('k') => picker.cursor = picker.cursor.saturating_sub(1),
            Home | Char('g') => picker.cursor = 0,
            End | Char('G') => picker.cursor = last,
            Esc | Char('q') => self.mode = Mode::Normal,
            Enter => {
                let chosen = picker.entries.get(picker.cursor).cloned();
                self.mode = Mode::Normal;
                // Counts were taken when the picker opened; skip a filter that emptied since.
                if let Some((filter, n)) = chosen
                    && (n > 0 || filter == Filter::All)
                {
                    self.tag_filter = filter;
                    self.rebuild_rows();
                    self.rebuild_visible(false);
                }
            }
            _ => {}
        }
    }

    // ---- folders ------------------------------------------------------------

    fn open_folder_editor(&mut self) {
        let Some(fi) = self.selected_feed() else {
            return self.flash_err("Select a feed to move");
        };
        let current = self.config.feeds[fi].folder.clone();
        self.mode = Mode::Folder(FolderEditor::new(fi, self.folders(), current.as_deref()));
    }

    fn key_folder(&mut self, key: KeyEvent) {
        let Mode::Folder(ed) = &mut self.mode else {
            return;
        };
        if key.code == KeyCode::Esc {
            self.mode = Mode::Normal;
            return;
        }
        let Some(choice) = ed.handle_key(&key) else {
            return;
        };
        let fi = ed.feed;
        let mut feeds = self.config.feeds.clone();
        feeds[fi].folder = match choice {
            FolderChoice::None => None,
            FolderChoice::Folder(f) => Some(f),
        };
        let folder = feeds[fi].folder.clone();
        match self.commit_feeds(feeds) {
            Ok(()) => {
                self.mode = Mode::Normal;
                // Make sure the destination is open so the feed stays in view.
                if let Some(f) = &folder {
                    self.state.collapsed.remove(f);
                    self.save_state();
                }
                self.rebuild_rows();
                if let Some(at) = self.rows.iter().position(|r| *r == Row::Feed(fi)) {
                    self.select_row(at);
                }
                self.flash(match folder {
                    Some(f) => format!("Moved to {f}"),
                    None => "Removed from its folder".to_string(),
                });
            }
            Err(e) => {
                if let Mode::Folder(ed) = &mut self.mode {
                    ed.error = Some(e);
                }
            }
        }
    }

    fn toggle_folder(&mut self, folder: &str) {
        if !self.state.collapsed.remove(folder) {
            self.state.collapsed.insert(folder.to_string());
        }
        self.save_state();
        self.rebuild_rows();
    }

    // ---- normal mode ---------------------------------------------------------

    fn on_key_normal(&mut self, key: KeyEvent) {
        use KeyCode::*;
        match key.code {
            Char('q') => self.quit = true,
            Char('?') => self.mode = Mode::Help,
            Char('E') => self.edit_requested = true,
            Char('T') => self.open_theme_picker(),
            Char('t') => self.open_tag_editor(),
            Char('f') => self.open_tag_filter(),
            Char('F') => self.open_folder_editor(),
            Char('/') => {
                self.mode = Mode::Search;
                self.focus = Focus::Items;
            }
            Tab => self.cycle_focus(1),
            BackTab => self.cycle_focus(-1),
            Char('R') => self.refresh_all(),
            Char('r') => self.refresh_selected(),
            Char('a') => self.mode = Mode::AddFeed(AddFeed::default()),
            Char('i') => {
                let on = !self.config.images;
                match config::set_value(&self.paths.config, "images", &on.to_string()) {
                    Ok(()) => self.config.images = on,
                    Err(e) => self.flash_err(format!("Could not save setting: {e:#}")),
                }
            }
            Char('u') => {
                self.unread_only ^= true;
                self.rebuild_visible(true);
            }
            Char('z') => self.zoom ^= true,
            Char('v') => {
                let next = self.config.layout.toggled();
                match config::set_value(&self.paths.config, "layout", next.name()) {
                    Ok(()) => {
                        self.config.layout = next;
                        self.flash(format!("Layout: {}", next.name()));
                    }
                    Err(e) => self.flash_err(format!("Could not save layout: {e:#}")),
                }
            }
            _ => match self.focus {
                Focus::Feeds => self.on_key_feeds(key),
                Focus::Items => self.on_key_items(key),
                Focus::Article => self.on_key_article(key),
            },
        }
    }

    fn cycle_focus(&mut self, dir: i8) {
        self.focus = match (self.focus, dir > 0) {
            (Focus::Feeds, true) | (Focus::Article, false) => Focus::Items,
            (Focus::Items, true) | (Focus::Feeds, false) => Focus::Article,
            (Focus::Article, true) | (Focus::Items, false) => Focus::Feeds,
        };
    }

    fn on_key_feeds(&mut self, key: KeyEvent) {
        use KeyCode::*;
        let len = self.rows.len();
        let cur = self
            .feed_list
            .selected()
            .unwrap_or(0)
            .min(len.saturating_sub(1));
        let row = self.rows.get(cur).cloned();
        let folder_open = |app: &App, f: &str| !app.state.collapsed.contains(f);
        let next = match key.code {
            Char('j') | Down => (cur + 1).min(len - 1),
            Char('k') | Up => cur.saturating_sub(1),
            Char('g') | Home => 0,
            Char('G') | End => len - 1,
            Esc if self.tag_filter != Filter::All => {
                self.tag_filter = Filter::All;
                self.rebuild_rows();
                self.rebuild_visible(false);
                return;
            }
            Char(' ') | Enter if matches!(row, Some(Row::Folder(_))) => {
                if let Some(Row::Folder(f)) = &row {
                    self.toggle_folder(f);
                    self.rebuild_visible(false);
                }
                cur
            }
            Char('l') | Right | Enter => {
                match &row {
                    // `l` opens a closed folder first, then moves on.
                    Some(Row::Folder(f)) if !folder_open(self, f) => {
                        self.toggle_folder(f);
                    }
                    Some(Row::Folder(_)) => {
                        if let Some(first) =
                            self.rows.get(cur + 1).filter(|r| matches!(r, Row::Feed(_)))
                        {
                            let _ = first;
                            self.focus = Focus::Items;
                        }
                    }
                    _ => self.focus = Focus::Items,
                }
                cur
            }
            Char('h') | Left => match &row {
                Some(Row::Folder(f)) if folder_open(self, f) => {
                    self.toggle_folder(f);
                    cur
                }
                Some(Row::Feed(i)) => match self.folder_of(*i) {
                    Some(f) => self
                        .rows
                        .iter()
                        .position(|r| *r == Row::Folder(f.into()))
                        .unwrap_or(cur),
                    None => cur,
                },
                _ => cur,
            },
            Char('d') if matches!(row, Some(Row::Feed(_))) => {
                self.mode = Mode::ConfirmDelete;
                cur
            }
            _ => cur,
        };
        if next != cur {
            self.select_row(next);
        }
    }

    fn on_key_items(&mut self, key: KeyEvent) {
        use KeyCode::*;
        match key.code {
            Char('j') | Down => self.move_item(1),
            Char('k') | Up => self.move_item(-1),
            Char('g') | Home => self
                .item_list
                .select((!self.visible.is_empty()).then_some(0)),
            Char('G') | End => self.item_list.select(self.visible.len().checked_sub(1)),
            Char('h') | Left => self.focus = Focus::Feeds,
            Esc if !self.filter.is_empty() => {
                self.filter.clear();
                self.rebuild_visible(false);
            }
            Char('l') | Right | Enter => {
                if self.current().is_some() {
                    self.mark_current_read();
                    self.focus = Focus::Article;
                }
            }
            Char('m') => self.toggle_read(),
            Char('M') => self.mark_all_read(),
            Char('o') => self.open_link(),
            _ => {}
        }
    }

    fn on_key_article(&mut self, key: KeyEvent) {
        use KeyCode::*;
        let page = self.article_height.saturating_sub(2).max(1);
        match key.code {
            Char('j') | Down => self.scroll = self.scroll.saturating_add(1),
            Char('k') | Up => self.scroll = self.scroll.saturating_sub(1),
            Char(' ') | PageDown => self.scroll = self.scroll.saturating_add(page),
            Char('b') | PageUp => self.scroll = self.scroll.saturating_sub(page),
            Char('g') | Home => self.scroll = 0,
            Char('G') | End => self.scroll = u16::MAX,
            Char('n') => self.step_article(1),
            Char('p') => self.step_article(-1),
            Char('h') | Left | Esc => {
                self.zoom = false;
                self.focus = Focus::Items;
            }
            Char('m') => self.toggle_read(),
            Char('o') | Enter => self.open_link(),
            _ => {}
        }
    }

    fn move_item(&mut self, delta: isize) {
        let Some(cur) = self.item_list.selected() else {
            return;
        };
        let next = cur.saturating_add_signed(delta).min(self.visible.len() - 1);
        self.item_list.select(Some(next));
    }

    fn step_article(&mut self, delta: isize) {
        self.move_item(delta);
        self.mark_current_read();
    }

    fn toggle_read(&mut self) {
        if let Some(id) = self.current_item().map(|i| i.id.clone()) {
            let read = self.is_read(&id);
            self.set_read(&id, !read);
            if self.unread_only {
                self.rebuild_visible(true);
            }
        }
    }

    fn mark_all_read(&mut self) {
        let ids: Vec<String> = self
            .visible
            .iter()
            .map(|&(f, i)| self.feeds[f].items[i].id.clone())
            .collect();
        self.state.read.extend(ids);
        self.save_state();
        self.flash("Marked all as read");
    }

    fn open_link(&mut self) {
        match self.current_item().and_then(|i| i.link.clone()) {
            Some(link) => {
                if open::that_detached(&link).is_err() {
                    self.flash_err("Could not open browser");
                }
            }
            None => self.flash_err("This item has no link"),
        }
    }

    fn try_add_feed(&mut self, input: &str) -> Result<(), String> {
        if input.is_empty() {
            return Err("Enter a feed URL".into());
        }
        let url = if input.contains("://") {
            input.to_string()
        } else {
            format!("https://{input}")
        };
        let parsed = reqwest::Url::parse(&url).map_err(|e| format!("Not a valid URL: {e}"))?;
        if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
            return Err("Use an http:// or https:// URL".into());
        }
        if self.config.feeds.iter().any(|f| f.url == url) {
            return Err("That feed is already in your list".into());
        }
        // A new feed lands in the folder the cursor is in.
        let folder = match self.selected_row() {
            Some(Row::Folder(f)) => Some(f),
            Some(Row::Feed(i)) => self.folder_of(i).map(String::from),
            _ => None,
        };
        let mut feeds = self.config.feeds.clone();
        feeds.push(FeedConfig {
            name: String::new(),
            url,
            folder,
            tags: Vec::new(),
        });
        self.commit_feeds(feeds)?;
        self.feeds.push(FeedState::default());
        let fi = self.feeds.len() - 1;
        self.tag_filter = Filter::All;
        self.rebuild_rows();
        if let Some(at) = self.rows.iter().position(|r| *r == Row::Feed(fi)) {
            self.select_row(at);
        }
        self.refresh(fi);
        Ok(())
    }

    /// Hand the terminal to `$VISUAL`/`$EDITOR` on the config file, then reload it.
    fn edit_config(&mut self, terminal: &mut DefaultTerminal) {
        let editor = std::env::var("VISUAL")
            .or_else(|_| std::env::var("EDITOR"))
            .ok()
            .filter(|e| !e.trim().is_empty())
            .unwrap_or_else(|| "vi".into());
        let mut parts = editor.split_whitespace();
        let program = parts.next().unwrap_or("vi");

        ratatui::restore();
        let status = Command::new(program)
            .args(parts)
            .arg(&self.paths.config)
            .status();
        *terminal = ratatui::init();
        self.dirty = true;

        match status {
            Ok(_) => self.reload_config(),
            Err(e) => self.flash_err(format!("Could not run '{program}': {e}")),
        }
    }

    /// Apply an edited config, keeping loaded items for feeds that are unchanged.
    fn reload_config(&mut self) {
        let config = match Config::load(&self.paths.config) {
            Ok(c) => c,
            Err(e) => return self.flash_err(format!("Config not reloaded: {e:#}")),
        };
        let themes = match theme::load_themes(&self.paths.themes, &config.themes) {
            Ok(t) => t,
            Err(e) => return self.flash_err(format!("Config not reloaded: {e:#}")),
        };
        if theme::find(&themes, &config.theme).is_none() {
            return self.flash_err(format!(
                "Config not reloaded: unknown theme '{}'",
                config.theme
            ));
        }
        let mut old: HashMap<String, FeedState> = self
            .config
            .feeds
            .iter()
            .map(|f| f.url.clone())
            .zip(self.feeds.drain(..))
            .collect();
        let mut added = Vec::new();
        self.feeds = config
            .feeds
            .iter()
            .enumerate()
            .map(|(i, f)| {
                old.remove(&f.url).unwrap_or_else(|| {
                    added.push(i);
                    FeedState::default()
                })
            })
            .collect();
        self.themes = themes;
        let (name, transparent) = (config.theme.clone(), config.transparent_background);
        self.config = config;
        self.apply_theme(&name, transparent);
        self.fix_tag_filter();
        self.rebuild_rows();
        self.rebuild_visible(false);
        for fi in added {
            self.refresh(fi);
        }
        self.flash("Config reloaded");
    }

    fn delete_selected_feed(&mut self) {
        let Some(fi) = self.selected_feed() else {
            return;
        };
        let mut feeds = self.config.feeds.clone();
        let removed = feeds.remove(fi);
        if let Err(e) = self.commit_feeds(feeds) {
            return self.flash_err(format!("Could not remove feed: {e}"));
        }
        self.feeds.remove(fi);
        self.fix_tag_filter();
        self.rebuild_rows();
        self.rebuild_visible(false);
        self.flash(format!("Removed {}", removed.label()));
    }
}

pub fn run(
    terminal: &mut DefaultTerminal,
    config: Config,
    state: State,
    picker: Picker,
    themes: Vec<Named>,
    paths: Paths,
) -> Result<()> {
    let mut app = App::new(config, state, picker, themes, paths)?;
    while !app.quit {
        app.tick();
        if std::mem::take(&mut app.edit_requested) {
            app.edit_config(terminal);
        }
        if app.dirty {
            app.dirty = false;
            app.last_paint = Instant::now();
            terminal.draw(|f| ui::draw(f, &mut app))?;
        }
        if event::poll(Duration::from_millis(100))? {
            match event::read()? {
                Event::Key(k) if k.kind == KeyEventKind::Press => app.on_key(k),
                Event::Resize(..) => app.dirty = true,
                _ => {}
            }
        }
    }
    Ok(())
}

#[cfg(test)]
impl App {
    /// An app over a private temp config directory seeded with the defaults.
    pub fn for_test(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("rsstui-app-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let paths = Paths::in_dir(&dir);
        config::write_atomic(&paths.config, &config::sample_config_text()).unwrap();
        let config = Config::load(&paths.config).unwrap();
        let themes = theme::load_themes(&paths.themes, &config.themes).unwrap();
        let mut app = App::new(
            config,
            State::default(),
            Picker::halfblocks(),
            themes,
            paths,
        )
        .unwrap();
        // No network in tests: forget the fetches App::new started.
        for f in &mut app.feeds {
            f.loading = false;
        }
        app
    }
}

#[cfg(test)]
impl App {
    pub fn set_theme_for_test(&mut self, name: &str, transparent: bool) {
        self.apply_theme(name, transparent);
    }

    pub fn on_key_for_test(&mut self, c: char) {
        self.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(app: &mut App, code: KeyCode) {
        app.on_key(KeyEvent::new(code, KeyModifiers::NONE));
    }

    fn type_str(app: &mut App, s: &str) {
        for c in s.chars() {
            press(app, KeyCode::Char(c));
        }
    }

    fn goto(app: &mut App, row: Row) {
        let at = app.rows.iter().position(|r| *r == row).expect("row exists");
        app.select_row(at);
    }

    fn feed_named(app: &App, name: &str) -> usize {
        app.config
            .feeds
            .iter()
            .position(|f| f.name == name)
            .unwrap()
    }

    #[test]
    fn default_feeds_are_grouped_into_folders() {
        let app = App::for_test("folders");
        assert_eq!(app.rows[0], Row::All);
        let folders: Vec<_> = app
            .rows
            .iter()
            .filter_map(|r| {
                if let Row::Folder(f) = r {
                    Some(f.as_str())
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(folders, ["News", "Languages & Platforms", "Writers"]);
        // Every feed is listed under its folder.
        let feeds = app
            .rows
            .iter()
            .filter(|r| matches!(r, Row::Feed(_)))
            .count();
        assert_eq!(feeds, app.config.feeds.len());
    }

    #[test]
    fn collapsing_a_folder_hides_its_feeds_and_is_remembered() {
        let mut app = App::for_test("collapse");
        goto(&mut app, Row::Folder("News".into()));
        app.focus = Focus::Feeds;
        press(&mut app, KeyCode::Enter);
        let hn = feed_named(&app, "Hacker News");
        assert!(!app.rows.contains(&Row::Feed(hn)));
        assert!(app.state.collapsed.contains("News"));
        assert!(app.state.read.is_empty());
        press(&mut app, KeyCode::Char(' '));
        assert!(app.rows.contains(&Row::Feed(hn)));
    }

    #[test]
    fn folder_scope_lists_its_feeds_items() {
        let mut app = App::for_test("scope");
        let (hn, rust) = (
            feed_named(&app, "Hacker News"),
            feed_named(&app, "Rust Blog"),
        );
        for (fi, id) in [(hn, "hn1"), (rust, "r1")] {
            app.feeds[fi].items.push(Item {
                id: id.into(),
                title: id.into(),
                link: None,
                author: None,
                date: None,
                html: String::new(),
                image: None,
            });
        }
        goto(&mut app, Row::Folder("News".into()));
        assert_eq!(app.visible, [(hn, 0)]);
        goto(&mut app, Row::All);
        assert_eq!(app.visible.len(), 2);
        assert_eq!(
            app.unread_in(&Row::Folder("Languages & Platforms".into())),
            1
        );
    }

    #[test]
    fn tagging_a_feed_saves_to_the_config_and_filters() {
        let mut app = App::for_test("tags");
        let rust = feed_named(&app, "Rust Blog");
        goto(&mut app, Row::Feed(rust));
        press(&mut app, KeyCode::Char('t'));
        assert!(matches!(app.mode, Mode::Tags(_)));
        type_str(&mut app, "fav");
        press(&mut app, KeyCode::Enter); // adds the typed tag
        press(&mut app, KeyCode::Enter); // saves
        assert!(matches!(app.mode, Mode::Normal));
        assert!(app.config.feeds[rust].tags.contains(&"fav".to_string()));
        let on_disk = Config::load(&app.paths.config).unwrap();
        assert!(
            on_disk.feeds[rust].tags.contains(&"fav".to_string()),
            "persisted"
        );
        assert!(
            std::fs::read_to_string(&app.paths.config)
                .unwrap()
                .contains("# rsstui configuration."),
            "comments kept"
        );

        press(&mut app, KeyCode::Char('f'));
        let Mode::TagFilter(p) = &mut app.mode else {
            panic!("filter picker not open")
        };
        p.cursor = p
            .entries
            .iter()
            .position(|(f, _)| *f == Filter::Tag("fav".into()))
            .unwrap();
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.tag_filter, Filter::Tag("fav".into()));
        assert_eq!(
            app.rows
                .iter()
                .filter(|r| matches!(r, Row::Feed(_)))
                .count(),
            1
        );
        press(&mut app, KeyCode::Esc);
        assert_eq!(app.tag_filter, Filter::All);
    }

    #[test]
    fn moving_a_feed_to_a_new_folder() {
        let mut app = App::for_test("move");
        let go = feed_named(&app, "Go Blog");
        goto(&mut app, Row::Feed(go));
        press(&mut app, KeyCode::Char('F'));
        type_str(&mut app, "Favourites");
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.config.feeds[go].folder.as_deref(), Some("Favourites"));
        assert!(app.rows.contains(&Row::Folder("Favourites".into())));
        assert_eq!(
            app.selected_row(),
            Some(Row::Feed(go)),
            "cursor follows the feed"
        );
        assert_eq!(
            Config::load(&app.paths.config).unwrap().feeds[go]
                .folder
                .as_deref(),
            Some("Favourites")
        );
    }

    #[test]
    fn text_fields_take_command_letters() {
        let mut a = App::for_test("text");
        press(&mut a, KeyCode::Char('a'));
        type_str(&mut a, "quit?");
        assert!(!a.quit, "q must be typed, not obeyed");
        let Mode::AddFeed(form) = &a.mode else {
            panic!("dialog closed")
        };
        assert_eq!(form.input.text(), "quit?");

        let mut a = App::for_test("text2");
        press(&mut a, KeyCode::Char('/'));
        type_str(&mut a, "qTEtf");
        assert!(!a.quit && !a.edit_requested && matches!(a.mode, Mode::Search));
        assert_eq!(a.filter, "qTEtf");

        let mut a = App::for_test("text3");
        let go = feed_named(&a, "Go Blog");
        goto(&mut a, Row::Feed(go));
        press(&mut a, KeyCode::Char('F'));
        for _ in 0..8 {
            press(&mut a, KeyCode::Up); // up to the new-folder field
        }
        type_str(&mut a, "jkq");
        let Mode::Folder(ed) = &a.mode else { panic!() };
        assert_eq!(ed.input.text(), "jkq");
    }

    #[test]
    fn bad_input_keeps_the_dialog_open_with_an_error() {
        for (input, want) in [
            ("", "Enter a feed URL"),
            ("ftp://x.org/feed", "http"),
            ("http://", "valid URL"),
            ("https://hnrss.org/frontpage", "already"),
        ] {
            let mut a = App::for_test("bad");
            press(&mut a, KeyCode::Char('a'));
            type_str(&mut a, input);
            press(&mut a, KeyCode::Enter);
            let Mode::AddFeed(form) = &a.mode else {
                panic!("dialog closed for {input:?}")
            };
            assert!(
                form.error.as_deref().is_some_and(|e| e.contains(want)),
                "{input:?}: {:?}",
                form.error
            );
            assert_eq!(form.input.text(), input, "input is kept so it can be fixed");
        }
    }

    #[test]
    fn adding_a_feed_joins_the_cursors_folder_and_persists() {
        let mut a = App::for_test("add");
        goto(&mut a, Row::Folder("Writers".into()));
        press(&mut a, KeyCode::Char('a'));
        type_str(&mut a, "example.com/feed.xml");
        press(&mut a, KeyCode::Enter);
        let fi = a.config.feeds.len() - 1;
        assert_eq!(a.config.feeds[fi].url, "https://example.com/feed.xml");
        assert_eq!(a.config.feeds[fi].folder.as_deref(), Some("Writers"));
        assert_eq!(
            Config::load(&a.paths.config).unwrap().feeds.len(),
            a.config.feeds.len()
        );
    }

    #[test]
    fn removing_a_feed_needs_y() {
        let mut a = App::for_test("remove");
        let go = feed_named(&a, "Go Blog");
        goto(&mut a, Row::Feed(go));
        let before = a.config.feeds.len();
        press(&mut a, KeyCode::Char('d'));
        press(&mut a, KeyCode::Char('x'));
        assert_eq!(a.config.feeds.len(), before, "anything but y cancels");
        press(&mut a, KeyCode::Char('d'));
        press(&mut a, KeyCode::Char('y'));
        assert_eq!(a.config.feeds.len(), before - 1);
        assert_eq!(
            Config::load(&a.paths.config).unwrap().feeds.len(),
            before - 1
        );
    }

    #[test]
    fn theme_picker_previews_and_esc_restores_and_enter_saves() {
        let mut a = App::for_test("theme");
        let (before, was) = (a.theme_name.clone(), a.transparent);
        press(&mut a, KeyCode::Char('T'));
        press(&mut a, KeyCode::Char('j'));
        press(&mut a, KeyCode::Char('b'));
        assert_ne!(a.theme_name, before, "moving previews the theme");
        assert_ne!(a.transparent, was);
        press(&mut a, KeyCode::Esc);
        assert_eq!((a.theme_name.clone(), a.transparent), (before, was));

        press(&mut a, KeyCode::Char('T'));
        press(&mut a, KeyCode::Char('j'));
        let picked = a.theme_name.clone();
        press(&mut a, KeyCode::Enter);
        assert_eq!(Config::load(&a.paths.config).unwrap().theme, picked);
    }

    #[test]
    fn theme_files_are_picked_up_when_the_picker_opens() {
        let mut a = App::for_test("dyn");
        let before = a.themes.len();
        std::fs::write(
            a.paths.themes.join("my-own.yml"),
            "base: nord\naccent: \"#112233\"\n",
        )
        .unwrap();
        press(&mut a, KeyCode::Char('T'));
        assert_eq!(a.themes.len(), before + 1);
        assert!(theme::find(&a.themes, "my-own").is_some());
    }
}
