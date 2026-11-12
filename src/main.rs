mod activation;
mod config;
mod highlight;
mod instance;
mod theme;

use config::Config;
use iced::futures::StreamExt;
use iced::keyboard::{Key, key::Named};
use iced::widget::{
    Id, Space, canvas, container, row, stack, scrollable, text, text_editor as text_editor_widget, text_input,
    text::LineHeight,
    text_editor::{self, Action, Binding, Content, Cursor, Edit, Motion, Position},
};
use iced::{
    Color, Element, Font, Length, Padding, Pixels, Size, Subscription, Task, Theme, alignment, window,
};
use std::path::PathBuf;
use theme::Palette;

const POINT_TO_PIXEL: f32 = 4.0 / 3.0;
const GUTTER_PAD: f32 = 20.0;
const TEXT_PAD: f32 = 12.0;
const TAB_WIDTH: usize = 4;
/// Wheel scrolling relative to the scrollable's built-in speed.
const SCROLL_SPEED: f32 = 2.1;
/// Scroll reports this soon after a wheel event predate it and are ignored.
const WHEEL_ECHO: std::time::Duration = std::time::Duration::from_millis(150);
/// PageUp / PageDown scroll by this share of the visible document height.
const PAGE_SCROLL: f32 = 0.75;
const CLOSE_SIZE: f32 = 24.0;
/// How long "saved" stays up after a save.
const SAVED_FOR: std::time::Duration = std::time::Duration::from_secs(2);
const DIM_GREY: Color = Color::from_rgb8(0x55, 0x55, 0x55);
const RULER_COLUMN: f32 = 80.0;
/// Advance width of a monospace glyph, relative to the font size.
const CHAR_WIDTH: f32 = 0.6;

fn main() -> iced::Result {
    let files: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    if instance::forward(&files) {
        return Ok(());
    }
    instance::listen();

    let config = config::load();
    iced::application(Rite::new, Rite::update, Rite::view)
        .title(Rite::title)
        .subscription(Rite::subscription)
        .theme(Rite::theme)
        .window_size(Size::new(config.window_width, config.window_height))
        // Unsaved changes are confirmed before quitting; see `Message::CloseWindow`.
        .exit_on_close_request(false)
        .default_font(Rite::font_for(&config))
        .run()
}

/// Everything that belongs to one open file.
struct Doc {
    /// Unique per document, used to refresh syntax highlighting on tab switches.
    id: u64,
    content: Content,
    path: Option<PathBuf>,
    dirty: bool,
    offset_y: f32,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    last_edit: EditKind,
}

impl Doc {
    fn empty() -> Self {
        static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        Self {
            id: NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            content: Content::new(),
            path: None,
            dirty: false,
            offset_y: 0.0,
            undo: Vec::new(),
            redo: Vec::new(),
            last_edit: EditKind::None,
        }
    }

    /// Opens `path`; a missing file gives an empty document that will be created on save.
    fn open(path: PathBuf) -> (Self, Option<String>) {
        let mut doc = Self::empty();
        let mut error = None;
        match std::fs::read_to_string(&path) {
            Ok(t) => doc.content = Content::with_text(&t.replace('\t', &" ".repeat(TAB_WIDTH))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                eprintln!("rite: cannot read {}: {e}", path.display());
                error = Some(format!("cannot read {}: {e}", path.display()));
            }
        }
        doc.path = Some(path);
        (doc, error)
    }

    fn name(&self) -> String {
        self.path
            .as_ref()
            .and_then(|p| p.file_name())
            .map_or("untitled".to_string(), |n| n.to_string_lossy().into_owned())
    }
}

struct Rite {
    /// The active document. Its slot in `tabs` holds a stale placeholder.
    doc: Doc,
    tabs: Vec<Doc>,
    active: usize,
    status: Option<String>,
    /// When "saved" was last shown; it hides itself `SAVED_FOR` later.
    saved_at: Option<std::time::Instant>,
    config: Config,
    font: Font,
    palette: Palette,
    scroll_id: Id,
    editor_id: Id,
    search_id: Id,
    /// The find bar is shown while this is `Some`.
    search: Option<String>,
    /// Waiting for the user to confirm losing unsaved changes.
    closing: Option<Closing>,
    shift: bool,
    /// When the wheel last moved the view; scroll reports right after it are stale echoes.
    last_wheel: std::time::Instant,
    viewport_h: f32,
    viewport_w: f32,
    /// Height of the scrolled document area, once the scrollable has reported it.
    body_h: Option<f32>,
}

/// What closing with unsaved changes is waiting on.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Closing {
    /// The tab holding the document with this id.
    Tab(u64),
    /// The whole window.
    Window,
}

/// Document state saved for undo and redo.
struct Snapshot {
    text: String,
    anchor: usize,
    head: usize,
}

#[derive(Clone, Copy, PartialEq)]
enum EditKind {
    None,
    Typing,
    Deleting,
    Other,
}

const UNDO_LIMIT: usize = 200;

#[derive(Debug, Clone)]
enum Message {
    Edit(Action),
    /// The document view's scroll offset and height.
    Scrolled(f32, f32),
    Wheel(iced::mouse::ScrollDelta),
    /// PageDown (true) / PageUp (false).
    Page(bool),
    /// Files to open, from a drop or another `rite` invocation.
    Remote(Vec<PathBuf>, Option<String>),
    Resized(Size),
    Save,
    SaveDialog(Option<PathBuf>),
    /// Hides "saved" once it has been shown for `SAVED_FOR`.
    HideSaved,
    Open,
    Opened(Option<PathBuf>),
    NewTab,
    CloseTab(usize),
    /// The unsaved document with this id may be closed.
    CloseConfirmed(u64),
    CancelClose,
    /// The window manager asked to close the window (Alt+F4, title bar button).
    CloseWindow,
    /// Quit even though there are unsaved changes.
    QuitConfirmed,
    /// Enter pressed outside any input: confirms a pending close.
    ConfirmKey,
    SwitchTab(usize),
    CycleTab(bool),
    Enter,
    Indent(bool),
    MoveLine(bool),
    DeleteWord,
    Undo,
    Redo,
    OpenSearch,
    Escape,
    SearchChanged(String),
    SearchNext,
    Shift(bool),
}

impl Rite {
    fn font_for(config: &Config) -> Font {
        // The font name must live for the whole program, so leak it once.
        Font::with_name(Box::leak(config.font_name.clone().into_boxed_str()))
    }

    fn new() -> (Self, Task<Message>) {
        let config = config::load();
        let palette = theme::load();
        let mut status = None;
        let mut tabs: Vec<Doc> = std::env::args_os()
            .skip(1)
            .map(|a| {
                let (doc, error) = Doc::open(PathBuf::from(a));
                status = status.take().or(error);
                doc
            })
            .collect();
        if tabs.is_empty() {
            tabs.push(Doc::empty());
        }
        let doc = std::mem::replace(&mut tabs[0], Doc::empty());

        let viewport_h = config.window_height;
        let viewport_w = config.window_width;
        let app = Self {
            font: Self::font_for(&config),
            doc,
            tabs,
            active: 0,
            status,
            saved_at: None,
            config,
            palette,
            scroll_id: Id::new("rite-scroll"),
            editor_id: Id::new("rite-editor"),
            search_id: Id::new("rite-search"),
            search: None,
            closing: None,
            shift: false,
            last_wheel: std::time::Instant::now(),
            viewport_h,
            viewport_w,
            body_h: None,
        };
        let focus = iced::widget::operation::focus(app.editor_id.clone());
        (app, focus)
    }

    fn title(&self) -> String {
        let name = self
            .doc
            .path
            .as_ref()
            .map_or("untitled".to_string(), |p| p.display().to_string());
        format!("{}{name} - rite", if self.doc.dirty { "* " } else { "" })
    }

    fn theme(&self) -> Theme {
        Theme::Dark
    }

    fn size_px(&self) -> f32 {
        self.config.font_size.points() * POINT_TO_PIXEL
    }

    fn char_px(&self) -> f32 {
        self.size_px() * CHAR_WIDTH
    }

    fn gutter_w(&self) -> f32 {
        let digits = self.doc.content.line_count().max(1).to_string().len().max(2) as f32;
        digits * self.size_px() * 0.62 + GUTTER_PAD * 2.0
    }

    /// Width of the editor: whatever the gutter leaves of the window.
    fn editor_w(&self) -> f32 {
        (self.viewport_w - self.gutter_w()).max(self.char_px() * 4.0)
    }

    /// Where every line wraps at the current width.
    fn layout(&self) -> Layout {
        let wrap = Wrap {
            font: self.font,
            size: self.size_px(),
            line_h: self.line_px(),
            width: self.editor_w() - TEXT_PAD * 2.0,
        };
        let mut starts = Vec::with_capacity(self.doc.content.line_count() + 1);
        let mut breaks = Vec::with_capacity(self.doc.content.line_count());
        let mut row = 0;
        for l in self.doc.content.lines() {
            let b = wrap_line(&l.text, wrap);
            starts.push(row);
            row += b.len();
            breaks.push(b);
        }
        starts.push(row);
        Layout { starts, breaks }
    }

    /// Visual row holding the cursor.
    fn cursor_row(&self, layout: &Layout) -> usize {
        let pos = self.doc.content.cursor().position;
        let col = self
            .doc
            .content
            .line(pos.line)
            .map_or(0, |l| l.text[..pos.column.min(l.text.len())].chars().count());
        layout.at(pos.line, col).0
    }

    fn line_px(&self) -> f32 {
        self.size_px() * self.config.line_height
    }

    fn subscription(&self) -> Subscription<Message> {
        Subscription::batch([
            window::resize_events().map(|(_, size)| Message::Resized(size)),
            window::close_requests().map(|_| Message::CloseWindow),
            iced::event::listen_with(|event, status, _| match event {
                iced::Event::Window(window::Event::FileDropped(path)) => Some(Message::Remote(vec![path], None)),
                iced::Event::Keyboard(iced::keyboard::Event::ModifiersChanged(m)) => Some(Message::Shift(m.shift())),
                iced::Event::Keyboard(iced::keyboard::Event::KeyReleased { modifiers, .. }) => {
                    Some(Message::Shift(modifiers.shift()))
                }
                iced::Event::Window(window::Event::Unfocused) => Some(Message::Shift(false)),
                iced::Event::Keyboard(iced::keyboard::Event::KeyPressed { key, modifiers, .. }) => match key {
                    Key::Named(Named::Escape) => Some(Message::Escape),
                    // A focused editor or input handles Enter itself.
                    Key::Named(Named::Enter) if status == iced::event::Status::Ignored => {
                        Some(Message::ConfirmKey)
                    }
                    Key::Character(c) if modifiers.command() && c.as_str() == "f" => Some(Message::OpenSearch),
                    _ => None,
                },
                _ => None,
            }),
            Subscription::run(|| instance::incoming().map(|(files, token)| Message::Remote(files, token))),
        ])
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Edit(action) => {
                if matches!(action, Action::Scroll { .. }) {
                    self.doc.content.perform(action);
                    return Task::none();
                }
                if matches!(action, Action::Edit(_)) && (self.search.is_some() || self.closing.is_some()) {
                    return Task::none();
                }
                if let Action::Edit(edit) = &action {
                    self.record(match edit {
                        Edit::Insert(_) => EditKind::Typing,
                        Edit::Backspace | Edit::Delete => EditKind::Deleting,
                        _ => EditKind::Other,
                    });
                    self.mark_dirty();
                    match edit {
                        Edit::Insert(c) => insert_char(&mut self.doc.content, *c),
                        _ => self.doc.content.perform(action),
                    }
                } else {
                    self.doc.last_edit = EditKind::None;
                    self.doc.content.perform(action);
                }
                return self.reveal_cursor();
            }
            Message::Enter => {
                self.record(EditKind::Other);
                self.mark_dirty();
                enter_with_indent(&mut self.doc.content);
                return self.reveal_cursor();
            }
            Message::DeleteWord => {
                self.record(EditKind::Other);
                self.mark_dirty();
                delete_word(&mut self.doc.content);
                return self.reveal_cursor();
            }
            Message::Undo => {
                self.restore(true);
                return self.reveal_cursor();
            }
            Message::Redo => {
                self.restore(false);
                return self.reveal_cursor();
            }
            Message::Indent(indent) => {
                self.shift_indent(indent);
                return self.reveal_cursor();
            }
            Message::MoveLine(up) => {
                self.move_lines(up);
                return self.reveal_cursor();
            }
            Message::OpenSearch => {
                self.saved_at = None;
                // Start from the selected text, if it is a single line.
                if let Some(sel) = self.doc.content.selection().filter(|s| !s.is_empty() && !s.contains('\n')) {
                    self.search = Some(sel);
                } else {
                    self.search.get_or_insert_with(String::new);
                }
                return iced::widget::operation::focus(self.search_id.clone())
                    .chain(iced::widget::operation::select_all(self.search_id.clone()));
            }
            Message::Escape => {
                if self.closing.take().is_some() {
                    return Task::none();
                }
                if self.search.take().is_some() {
                    return iced::widget::operation::focus(self.editor_id.clone());
                }
            }
            Message::SearchChanged(query) => self.search = Some(query),
            // Keep the find input focused, also after its button is clicked.
            Message::SearchNext => {
                return self
                    .search_next(!self.shift)
                    .chain(iced::widget::operation::focus(self.search_id.clone()));
            }
            Message::Shift(shift) => self.shift = shift,
            Message::Scrolled(y, height) => {
                self.body_h = Some(height);
                if self.last_wheel.elapsed() > WHEEL_ECHO {
                    self.doc.offset_y = y;
                }
            }
            Message::Wheel(delta) => {
                let dy = match delta {
                    iced::mouse::ScrollDelta::Lines { y, .. } => -y * 60.0 * SCROLL_SPEED,
                    iced::mouse::ScrollDelta::Pixels { y, .. } => -y * SCROLL_SPEED,
                };
                return self.scroll_by(dy);
            }
            Message::Page(down) => {
                let dy = self.body_h.unwrap_or(self.viewport_h) * PAGE_SCROLL;
                return self.scroll_by(if down { dy } else { -dy });
            }
            Message::Remote(paths, token) => {
                let mut tasks: Vec<Task<Message>> = paths.into_iter().map(|p| self.open_path(p)).collect();
                if let Some(token) = token {
                    tasks.push(window::latest().and_then(move |id| {
                        let token = token.clone();
                        window::run(id, move |w| activation::activate(w, &token)).discard()
                    }));
                }
                // Un-minimize, and bounce through always-on-top since a plain
                // focus request is ignored by many window managers. Wayland
                // ignores focus requests altogether; there the only way in is
                // an xdg-activation request, which winit sends for attention.
                tasks.push(window::latest().and_then(|id| {
                    window::minimize(id, false)
                        .chain(window::set_level(id, window::Level::AlwaysOnTop))
                        .chain(window::set_level(id, window::Level::Normal))
                        .chain(window::gain_focus(id))
                        .chain(window::request_user_attention(id, Some(window::UserAttention::Critical)))
                }));
                return Task::batch(tasks);
            }
            Message::Resized(size) => {
                self.viewport_h = size.height;
                self.viewport_w = size.width;
            }
            Message::Save => return self.save(),
            Message::SaveDialog(path) => {
                if let Some(path) = path {
                    self.doc.path = Some(path);
                    return self.save();
                }
            }
            Message::Open => {
                return Task::perform(
                    async {
                        rfd::AsyncFileDialog::new()
                            .pick_file()
                            .await
                            .map(|h| h.path().to_path_buf())
                    },
                    Message::Opened,
                );
            }
            Message::Opened(path) => {
                if let Some(path) = path {
                    return self.open_path(path);
                }
            }
            Message::NewTab => return self.add_tab(Doc::empty()),
            Message::CloseTab(i) => return self.close_tab(i),
            Message::CancelClose => self.closing = None,
            Message::ConfirmKey => match self.closing {
                Some(Closing::Tab(id)) => return self.update(Message::CloseConfirmed(id)),
                Some(Closing::Window) => return self.update(Message::QuitConfirmed),
                None => {}
            },
            Message::CloseWindow => {
                if !self.docs().any(|d| d.dirty) {
                    return iced::exit();
                }
                self.closing = Some(Closing::Window);
                self.saved_at = None;
            }
            Message::HideSaved => {
                if self.saved_at.is_some_and(|t| t.elapsed() >= SAVED_FOR) {
                    self.saved_at = None;
                }
            }
            Message::QuitConfirmed => return iced::exit(),
            Message::CloseConfirmed(id) => {
                self.closing = None;
                let index = (0..self.tabs.len()).find(|&i| {
                    let doc = if i == self.active { &self.doc } else { &self.tabs[i] };
                    doc.id == id
                });
                if let Some(i) = index {
                    return self.force_close_tab(i);
                }
            }
            Message::SwitchTab(i) => return self.switch_to(i),
            Message::CycleTab(forward) => {
                let n = self.tabs.len();
                return self.switch_to(if forward { (self.active + 1) % n } else { (self.active + n - 1) % n });
            }
        }
        Task::none()
    }

    fn snapshot(&self) -> Snapshot {
        let cur = self.doc.content.cursor();
        let head = offset_of(&self.doc.content, cur.position);
        let anchor = cur.selection.map_or(head, |a| offset_of(&self.doc.content, a));
        Snapshot { text: self.doc.content.text(), anchor, head }
    }

    /// Saves the state before an edit; runs of typing or deleting share one entry.
    fn record(&mut self, kind: EditKind) {
        if kind == EditKind::Other || kind != self.doc.last_edit {
            let snap = self.snapshot();
            self.doc.undo.push(snap);
            if self.doc.undo.len() > UNDO_LIMIT {
                self.doc.undo.remove(0);
            }
            self.doc.redo.clear();
        }
        self.doc.last_edit = kind;
    }

    fn restore(&mut self, undo: bool) {
        let from = if undo { &mut self.doc.undo } else { &mut self.doc.redo };
        let Some(target) = from.pop() else { return };
        let current = self.snapshot();
        if undo {
            self.doc.redo.push(current);
        } else {
            self.doc.undo.push(current);
        }
        self.doc.content = Content::with_text(&target.text);
        let position = pos_of(&self.doc.content, target.head);
        let selection = (target.anchor != target.head).then(|| pos_of(&self.doc.content, target.anchor));
        self.doc.content.move_to(Cursor { position, selection });
        self.doc.last_edit = EditKind::None;
        self.doc.dirty = true;
    }

    fn mark_dirty(&mut self) {
        self.doc.dirty = true;
        self.status = None;
        self.saved_at = None;
    }

    /// Enter / Shift+Enter in the find bar: select the next or previous match
    /// relative to the cursor, wrapping around.
    fn search_next(&mut self, forward: bool) -> Task<Message> {
        let needle = fold(self.search.as_deref().unwrap_or(""));
        if needle.is_empty() {
            return Task::none();
        }
        let cur = self.doc.content.cursor();
        let key = |p: Position| (p.line, p.column);
        let anchor = cur.selection.unwrap_or(cur.position);
        let (lo, hi) = (key(anchor).min(key(cur.position)), key(anchor).max(key(cur.position)));
        let all: Vec<(usize, Found)> = self
            .doc
            .content
            .lines()
            .enumerate()
            .flat_map(|(i, l)| find_all(&l.text, &needle).into_iter().map(move |m| (i, m)))
            .collect();
        let target = if forward {
            all.iter().find(|(l, m)| (*l, m.start) >= hi).or(all.first())
        } else {
            all.iter().rev().find(|(l, m)| (*l, m.start) < lo).or(all.last())
        };
        let Some(&(line, m)) = target else { return Task::none() };
        self.doc.last_edit = EditKind::None;
        self.doc.content.move_to(Cursor {
            position: Position { line, column: m.end },
            selection: Some(Position { line, column: m.start }),
        });

        // Put the match line in the middle of the viewport.
        let lh = self.line_px();
        let layout = self.layout();
        let max = (layout.rows() as f32 - 1.0) * lh;
        let row = layout.at(line, m.col).0;
        let target = (row as f32 * lh + lh / 2.0 - self.viewport_h / 2.0).clamp(0.0, max.max(0.0));
        self.doc.offset_y = target;
        iced::widget::operation::scroll_to(
            self.scroll_id.clone(),
            scrollable::AbsoluteOffset { x: None, y: Some(target) },
        )
    }

    /// Tab / Shift+Tab over the selected lines (or the current line).
    fn shift_indent(&mut self, indent: bool) {
        self.record(EditKind::Other);
        let cur = self.doc.content.cursor();
        let anchor = cur.selection.unwrap_or(cur.position);
        let key = |p: &Position| (p.line, p.column);
        let (lo, hi) = if key(&anchor) <= key(&cur.position) {
            (anchor, cur.position)
        } else {
            (cur.position, anchor)
        };
        let mut last = hi.line;
        if hi.column == 0 && hi.line > lo.line {
            last -= 1;
        }

        let mut text = String::new();
        let mut changed = vec![0usize; self.doc.content.line_count()];
        for (i, line) in self.doc.content.lines().enumerate() {
            if (lo.line..=last).contains(&i) {
                if indent {
                    if !line.text.is_empty() {
                        text.push_str(&" ".repeat(TAB_WIDTH));
                        changed[i] = TAB_WIDTH;
                    }
                    text.push_str(&line.text);
                } else {
                    let n = line.text.chars().take(TAB_WIDTH).take_while(|c| *c == ' ').count();
                    changed[i] = n;
                    text.push_str(&line.text[n..]);
                }
            } else {
                text.push_str(&line.text);
            }
            text.push_str(line.ending.as_str());
        }

        let adjust = |p: Position| Position {
            line: p.line,
            column: if indent {
                p.column + changed[p.line]
            } else {
                p.column.saturating_sub(changed[p.line])
            },
        };
        let (position, selection) = (adjust(cur.position), cur.selection.map(adjust));
        self.doc.content = Content::with_text(&text);
        self.doc.content.move_to(Cursor { position, selection });
        self.mark_dirty();
    }

    /// Alt+Up / Alt+Down: moves the selected lines (or the current line) past
    /// their neighbour and re-indents them to fit their new surroundings.
    fn move_lines(&mut self, up: bool) {
        let cur = self.doc.content.cursor();
        let anchor = cur.selection.unwrap_or(cur.position);
        let key = |p: &Position| (p.line, p.column);
        let (lo, hi) = if key(&anchor) <= key(&cur.position) {
            (anchor, cur.position)
        } else {
            (cur.position, anchor)
        };
        let mut last = hi.line;
        if hi.column == 0 && hi.line > lo.line {
            last -= 1;
        }
        let first = lo.line;
        let count = self.doc.content.line_count();
        if (up && first == 0) || (!up && last + 1 >= count) {
            return;
        }
        self.record(EditKind::Other);

        let mut texts: Vec<String> = Vec::with_capacity(count);
        let mut endings: Vec<&'static str> = Vec::with_capacity(count);
        for line in self.doc.content.lines() {
            texts.push(line.text.to_string());
            endings.push(line.ending.as_str());
        }

        // Swap the neighbour across the block; line endings stay where they are.
        let (new_first, new_last) = if up {
            let moved = texts.remove(first - 1);
            texts.insert(last, moved);
            (first - 1, last - 1)
        } else {
            let moved = texts.remove(last + 1);
            texts.insert(first, moved);
            (first + 1, last + 1)
        };

        // Indent the block like the code above it: one level deeper after an
        // opening bracket or colon, one level less when it starts by closing one.
        let leading = |t: &str| t.chars().take_while(|c| *c == ' ').count();
        let above = texts[..new_first].iter().rev().find(|t| !t.trim().is_empty());
        let mut target = above.map_or(0, |t| {
            let opens = t.trim_end().ends_with(['{', '(', '[', ':']);
            leading(t) + if opens { TAB_WIDTH } else { 0 }
        });
        let head = texts[new_first..=new_last].iter().find(|t| !t.trim().is_empty());
        let current = head.map_or(0, |t| leading(t));
        if head.is_some_and(|t| t.trim_start().starts_with(['}', ')', ']'])) {
            target = target.saturating_sub(TAB_WIDTH);
        }

        let mut shift = vec![0isize; texts.len()];
        for i in new_first..=new_last {
            if texts[i].trim().is_empty() {
                continue;
            }
            if target >= current {
                let n = target - current;
                texts[i].insert_str(0, &" ".repeat(n));
                shift[i] = n as isize;
            } else {
                let n = (current - target).min(leading(&texts[i]));
                texts[i].drain(..n);
                shift[i] = -(n as isize);
            }
        }

        let mut text = String::new();
        for (t, e) in texts.iter().zip(&endings) {
            text.push_str(t);
            text.push_str(e);
        }

        let adjust = |p: Position| {
            let line = if up { p.line - 1 } else { p.line + 1 };
            Position { line, column: (p.column as isize + shift[line]).max(0) as usize }
        };
        let (position, selection) = (adjust(cur.position), cur.selection.map(adjust));
        self.doc.content = Content::with_text(&text);
        self.doc.content.move_to(Cursor { position, selection });
        self.mark_dirty();
    }

    /// Scrolls the view by `dy` pixels without moving the cursor.
    fn scroll_by(&mut self, dy: f32) -> Task<Message> {
        // Scroll reports right after this are stale echoes of the old offset.
        self.last_wheel = std::time::Instant::now();
        if dy == 0.0 {
            return Task::none();
        }
        let rows = self.layout().rows();
        let max = (rows as f32 - 1.0) * self.line_px();
        let target = (self.doc.offset_y + dy).clamp(0.0, max.max(0.0));
        self.doc.offset_y = target;
        iced::widget::operation::scroll_to(
            self.scroll_id.clone(),
            scrollable::AbsoluteOffset { x: None, y: Some(target) },
        )
    }

    /// The editor is laid out at its full size inside a scrollable, so keep
    /// the cursor in view ourselves.
    fn reveal_cursor(&mut self) -> Task<Message> {
        let lh = self.line_px();
        let top = self.cursor_row(&self.layout()) as f32 * lh;
        let bottom = top + lh;
        let target = if top < self.doc.offset_y {
            top
        } else if bottom > self.doc.offset_y + self.viewport_h {
            bottom - self.viewport_h
        } else {
            return Task::none();
        };
        self.doc.offset_y = target;
        iced::widget::operation::scroll_to(
            self.scroll_id.clone(),
            scrollable::AbsoluteOffset { x: None, y: Some(target) },
        )
    }

    /// Saves to the document's path; an untitled document asks for one first.
    fn save(&mut self) -> Task<Message> {
        let Some(path) = &self.doc.path else {
            return Task::perform(
                async {
                    rfd::AsyncFileDialog::new()
                        .save_file()
                        .await
                        .map(|h| h.path().to_path_buf())
                },
                Message::SaveDialog,
            );
        };
        let mut text = self.doc.content.text();
        if !text.ends_with('\n') {
            text.push('\n');
        }
        match std::fs::write(path, text) {
            Ok(()) => {
                self.doc.dirty = false;
                self.status = None;
                self.saved_at = Some(std::time::Instant::now());
                return Task::perform(
                    async {
                        let (tx, rx) = iced::futures::channel::oneshot::channel();
                        std::thread::spawn(move || {
                            std::thread::sleep(SAVED_FOR);
                            let _ = tx.send(());
                        });
                        let _ = rx.await;
                    },
                    |_| Message::HideSaved,
                );
            }
            Err(e) => {
                eprintln!("rite: cannot write {}: {e}", path.display());
                self.saved_at = None;
                self.status = Some(format!("save failed: {e}"));
            }
        }
        Task::none()
    }

    /// Restores the active document's scroll position and focuses the editor for typing.
    fn show_doc(&self) -> Task<Message> {
        Task::batch([
            iced::widget::operation::scroll_to(
                self.scroll_id.clone(),
                scrollable::AbsoluteOffset { x: None, y: Some(self.doc.offset_y) },
            ),
            iced::widget::operation::focus(self.editor_id.clone()),
        ])
    }

    fn switch_to(&mut self, index: usize) -> Task<Message> {
        if index == self.active || index >= self.tabs.len() {
            return Task::none();
        }
        std::mem::swap(&mut self.doc, &mut self.tabs[self.active]);
        self.active = index;
        std::mem::swap(&mut self.doc, &mut self.tabs[self.active]);
        self.show_doc()
    }

    fn add_tab(&mut self, doc: Doc) -> Task<Message> {
        self.tabs.push(doc);
        self.switch_to(self.tabs.len() - 1)
    }

    /// Closes a tab, first asking for confirmation when it has unsaved changes.
    fn close_tab(&mut self, index: usize) -> Task<Message> {
        let Some(doc) = (if index == self.active { Some(&self.doc) } else { self.tabs.get(index) }) else {
            return Task::none();
        };
        if !doc.dirty {
            return self.force_close_tab(index);
        }
        self.closing = Some(Closing::Tab(doc.id));
        self.saved_at = None;
        Task::none()
    }

    /// Every open document; the active one comes first.
    fn docs(&self) -> impl Iterator<Item = &Doc> {
        std::iter::once(&self.doc)
            .chain(self.tabs.iter().enumerate().filter(|&(i, _)| i != self.active).map(|(_, d)| d))
    }

    /// The bar asking whether to close what `self.closing` names and lose its unsaved changes.
    fn close_bar(&self) -> Option<Element<'_, Message>> {
        let (prompt, label, confirm) = match self.closing? {
            Closing::Tab(id) => {
                let doc = self.docs().find(|d| d.id == id)?;
                let prompt = format!("\"{}\" has unsaved changes. Closing it will lose them.", doc.name());
                (prompt, "Close without saving", Message::CloseConfirmed(id))
            }
            Closing::Window => {
                let dirty: Vec<String> = self.docs().filter(|d| d.dirty).map(|d| d.name()).collect();
                let prompt = match dirty.as_slice() {
                    [name] => format!("\"{name}\" has unsaved changes. Quitting will lose them."),
                    names => format!("{} files have unsaved changes. Quitting will lose them.", names.len()),
                };
                (prompt, "Quit without saving", Message::QuitConfirmed)
            }
        };
        let button = |label: &'static str, message: Message, fill: Color, label_color: Color| {
            iced::widget::button(text(label).size(13))
                .padding([5, 10])
                .on_press(message)
                .style(move |_, status| bar_button_style(fill, label_color, status))
        };
        Some(
            container(
                row![
                    text(prompt).size(13).width(Length::Fill),
                    button(label, confirm, Color::from_rgb8(0xe0, 0x3e, 0x3e), Color::BLACK),
                    button("Cancel", Message::CancelClose, DIM_GREY, Color::WHITE),
                ]
                .spacing(8)
                .align_y(alignment::Vertical::Center),
            )
            .padding([8, 8])
            .width(Length::Fill)
            .style(|_| container::Style {
                background: Some(iced::Background::Color(Color::from_rgba(0.0, 0.0, 0.0, 0.3))),
                ..Default::default()
            })
            .into(),
        )
    }

    fn force_close_tab(&mut self, index: usize) -> Task<Message> {
        if index >= self.tabs.len() {
            return Task::none();
        }
        if self.tabs.len() == 1 {
            self.doc = Doc::empty();
            return self.show_doc();
        }
        if index == self.active {
            self.tabs.remove(index);
            self.active = index.min(self.tabs.len() - 1);
            std::mem::swap(&mut self.doc, &mut self.tabs[self.active]);
            return self.show_doc();
        }
        self.tabs.remove(index);
        if index < self.active {
            self.active -= 1;
        }
        Task::none()
    }

    fn open_path(&mut self, path: PathBuf) -> Task<Message> {
        let existing = (0..self.tabs.len()).find(|&i| {
            let doc = if i == self.active { &self.doc } else { &self.tabs[i] };
            doc.path.as_ref() == Some(&path)
        });
        if let Some(i) = existing {
            return self.switch_to(i);
        }
        let (doc, error) = Doc::open(path);
        self.status = error;
        // Reuse an untouched untitled tab instead of leaving it behind.
        if self.doc.path.is_none() && !self.doc.dirty && self.doc.content.is_empty() {
            self.doc = doc;
            return self.show_doc();
        }
        self.add_tab(doc)
    }

    fn tab_bar(&self) -> Element<'_, Message> {
        let p = self.palette;
        let mut bar = iced::widget::Row::new().spacing(2);
        for i in 0..self.tabs.len() {
            let doc = if i == self.active { &self.doc } else { &self.tabs[i] };
            let active = i == self.active;
            let label = format!("{}{}", if doc.dirty { "● " } else { "" }, doc.name());
            let tab = iced::widget::button(text(label).size(13))
                .padding(0)
                .on_press(Message::SwitchTab(i))
                .style(move |_, _| tab_style(p, active));
            let close = iced::widget::button(
                text("×")
                    .size(20)
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .align_x(alignment::Horizontal::Center)
                    .align_y(alignment::Vertical::Center),
            )
            .padding(0)
            .width(CLOSE_SIZE)
            .height(CLOSE_SIZE)
            .on_press(Message::CloseTab(i))
            .style(move |_, status| round_hover_style(tab_style(p, active), Color::from_rgb8(0xe0, 0x3e, 0x3e), status));
            bar = bar.push(
                container(row![tab, close].spacing(10).align_y(alignment::Vertical::Center))
                    .padding(Padding { top: 10.0, right: 4.0, bottom: 10.0, left: 16.0 })
                    .style(move |_| container::Style {
                        background: active.then(|| iced::Background::Color(p.background)),
                        ..Default::default()
                    }),
            );
        }
        let tabs = scrollable(bar)
            .width(Length::Fill)
            .direction(scrollable::Direction::Horizontal(
                scrollable::Scrollbar::new().width(0).scroller_width(0),
            ));
        let plus = iced::widget::button(
            text("+")
                .size(20)
                .width(Length::Fill)
                .height(Length::Fill)
                .align_x(alignment::Horizontal::Center)
                .align_y(alignment::Vertical::Center),
        )
        .padding(0)
        .width(CLOSE_SIZE)
        .height(CLOSE_SIZE)
        .on_press(Message::NewTab)
        .style(move |_, status| round_hover_style(tab_style(p, false), Color::WHITE, status));

        let bar = container(
            row![tabs, plus]
                .spacing(8)
                .align_y(alignment::Vertical::Center),
        )
        .padding(Padding::ZERO.right(8))
        .width(Length::Fill)
        .style(|_| container::Style {
            background: Some(iced::Background::Color(Color::from_rgba(0.0, 0.0, 0.0, 0.3))),
            ..Default::default()
        });
        // The tab buttons capture their own clicks, so only empty space gets here.
        iced::widget::mouse_area(bar).on_double_click(Message::NewTab).into()
    }

    fn view(&self) -> Element<'_, Message> {
        let p = self.palette;
        let size = self.size_px();
        let lh = self.line_px();

        // Extra space after the last line so the file can scroll up to the top of the window.
        let past_end = (self.viewport_h - lh * 2.0).max(0.0);

        let count = self.doc.content.line_count().max(1);
        let gutter_w = self.gutter_w();
        let layout = self.layout();
        let starts = &layout.starts;
        // Wrapped lines get blank gutter rows after their number.
        let mut numbers = String::new();
        for n in 0..count {
            if n > 0 {
                numbers.push('\n');
            }
            numbers.push_str(&(n + 1).to_string());
            for _ in 1..starts[n + 1] - starts[n] {
                numbers.push('\n');
            }
        }

        let gutter = iced::widget::column![
            container(
                text(numbers)
                    .font(self.font)
                    .size(size)
                    .line_height(LineHeight::Absolute(Pixels(lh)))
                    .color(p.gutter)
                    .align_x(alignment::Horizontal::Right)
                    .width(Length::Fill),
            )
            .padding(Padding::ZERO.left(GUTTER_PAD).right(GUTTER_PAD))
            .width(gutter_w),
            Space::new().height(past_end),
        ];

        let first_line = self.doc.content.line(0).map(|l| l.text.to_string()).unwrap_or_default();
        let hl_settings = highlight::Settings {
            palette: p,
            lang: highlight::detect(self.doc.path.as_deref(), &first_line),
            doc_id: self.doc.id,
        };
        let selecting = self.doc.content.selection().is_some();
        let close_index = self.active;
        let searching = self.search.is_some();
        let closing = self.closing.is_some();
        let editor = text_editor_widget(&self.doc.content)
            .font(self.font)
            .size(size)
            .line_height(LineHeight::Absolute(Pixels(lh)))
            .padding(Padding { top: 0.0, right: TEXT_PAD, bottom: past_end, left: TEXT_PAD })
            .height(Length::Shrink)
            .id(self.editor_id.clone())
            // `wrap_line` mirrors this wrapping to place the gutter numbers and overlays.
            .wrapping(text::Wrapping::WordOrGlyph)
            .width(self.editor_w())
            .on_action(Message::Edit)
            .key_binding(move |press| {
                let binding = match (&press.key, press.modifiers.command()) {
                    (Key::Character(c), true) if c.as_str() == "s" => Some(Binding::Custom(Message::Save)),
                    (Key::Character(c), true) if c.as_str() == "z" && !press.modifiers.shift() => {
                        Some(Binding::Custom(Message::Undo))
                    }
                    (Key::Character(c), true) if c.as_str() == "y" || c.as_str() == "Z" => {
                        Some(Binding::Custom(Message::Redo))
                    }
                    (Key::Named(Named::Backspace), true) => Some(Binding::Custom(Message::DeleteWord)),
                    (Key::Character(c), true) if c.as_str() == "f" => Some(Binding::Custom(Message::OpenSearch)),
                    // Ctrl+D does nothing.
                    (Key::Character(c), true) if c.as_str() == "d" => None,
                    (Key::Named(Named::ArrowUp), _) if press.modifiers.alt() => {
                        Some(Binding::Custom(Message::MoveLine(true)))
                    }
                    (Key::Named(Named::ArrowDown), _) if press.modifiers.alt() => {
                        Some(Binding::Custom(Message::MoveLine(false)))
                    }
                    (Key::Named(Named::Enter), false) => Some(Binding::Custom(Message::Enter)),
                    (Key::Character(c), _) if press.modifiers.alt() && c.len() == 1 && c.as_bytes()[0] >= b'1' && c.as_bytes()[0] <= b'9' => {
                        Some(Binding::Custom(Message::SwitchTab((c.as_bytes()[0] - b'1') as usize)))
                    }
                    (Key::Character(c), true) if c.as_str() == "n" => Some(Binding::Custom(Message::NewTab)),
                    (Key::Named(Named::Tab), true) => Some(Binding::Custom(Message::CycleTab(!press.modifiers.shift()))),
                    (Key::Character(c), true) if c.as_str() == "t" => Some(Binding::Custom(Message::NewTab)),
                    (Key::Character(c), true) if c.as_str() == "o" => Some(Binding::Custom(Message::Open)),
                    (Key::Character(c), true) if c.as_str() == "w" => {
                        Some(Binding::Custom(Message::CloseTab(close_index)))
                    }
                    (Key::Named(Named::Tab), _) if press.modifiers.shift() => {
                        Some(Binding::Custom(Message::Indent(false)))
                    }
                    (Key::Named(Named::Tab), false) if selecting => Some(Binding::Custom(Message::Indent(true))),
                    (Key::Named(Named::Tab), false) => Some(Binding::Sequence(
                        (0..TAB_WIDTH).map(|_| Binding::Insert(' ')).collect(),
                    )),
                    // Page motions in the editor would jump to the document's ends
                    // (its height is unbounded), so these only scroll the view.
                    (Key::Named(Named::PageDown), _) => Some(Binding::Custom(Message::Page(true))),
                    (Key::Named(Named::PageUp), _) => Some(Binding::Custom(Message::Page(false))),
                    _ => Binding::from_key_press(press),
                };
                if !searching && !closing {
                    return binding;
                }
                // A bar is open: navigation and tab commands only, no editing.
                // Enter answers the close confirmation first, then the find bar.
                match binding {
                    Some(Binding::Custom(Message::Enter)) if closing => Some(Binding::Custom(Message::ConfirmKey)),
                    Some(Binding::Custom(Message::Enter)) => Some(Binding::Custom(Message::SearchNext)),
                    Some(
                        b @ (Binding::Move(_)
                        | Binding::Select(_)
                        | Binding::SelectWord
                        | Binding::SelectLine
                        | Binding::SelectAll
                        | Binding::Copy
                        | Binding::Unfocus),
                    ) => Some(b),
                    Some(
                        b @ Binding::Custom(
                            Message::Save
                            | Message::Open
                            | Message::NewTab
                            | Message::CloseTab(_)
                            | Message::SwitchTab(_)
                            | Message::CycleTab(_)
                            | Message::Page(_)
                            | Message::OpenSearch,
                        ),
                    ) => Some(b),
                    _ => None,
                }
            })
            .highlight_with::<highlight::Syntax>(hl_settings, highlight::format)
            .style(move |_, _| text_editor::Style {
                background: iced::Background::Color(iced::Color::TRANSPARENT),
                border: iced::Border::default(),
                placeholder: p.gutter,
                value: p.foreground,
                selection: p.selection,
            });

        let line_of = |row: usize| starts.partition_point(|&s| s <= row).saturating_sub(1).min(count - 1);
        let first = line_of((self.doc.offset_y / lh) as usize).saturating_sub(1);
        let last = (line_of(((self.doc.offset_y + self.viewport_h) / lh) as usize) + 2).min(count);
        let visible = first..last.max(first);

        let texts: Vec<String> = self.doc.content.lines().map(|l| l.text.to_string()).collect();
        let cur = self.doc.content.cursor();
        let needle = self.doc.content.selection().filter(|s| !s.is_empty() && !s.contains('\n'));

        // Other occurrences of the selected text.
        let mut matches = Vec::new();
        if let Some(needle) = &needle {
            let own = cur.selection.map_or(cur.position, |a| {
                if (a.line, a.column) <= (cur.position.line, cur.position.column) { a } else { cur.position }
            });
            let n_chars = needle.chars().count();
            for line in visible.clone() {
                let Some(t) = texts.get(line) else { break };
                for (byte, _) in t.match_indices(needle.as_str()) {
                    if line == own.line && byte == own.column {
                        continue;
                    }
                    matches.push((line, t[..byte].chars().count(), n_chars));
                }
            }
        }

        // Every match of the search query.
        let needle = self.search.as_deref().map(fold).filter(|n| !n.is_empty());
        let mut found = Vec::new();
        let mut found_total = 0;
        if let Some(needle) = &needle {
            for (line, t) in texts.iter().enumerate() {
                for m in find_all(t, needle) {
                    found_total += 1;
                    if visible.contains(&line) {
                        found.push((line, m.col, needle.len()));
                    }
                }
            }
        }

        let guides = Guides {
            first,
            layout: layout.clone(),
            indents: fill_indents(&texts)[visible].to_vec(),
            matches,
            found,
            origin_x: TEXT_PAD,
            char_w: size * CHAR_WIDTH,
            line_h: lh,
            color: Color { a: 0.14, ..p.foreground },
            ruler: Color { a: 0.22, ..p.foreground },
            highlight: Color { a: 0.2, ..p.foreground },
            found_color: Color::from_rgba8(0xf5, 0xc5, 0x42, 0.4),
        };
        let overlay = canvas(guides).width(Length::Fill).height(Length::Fill);

        let body = scrollable(
            iced::widget::mouse_area(row![gutter, stack![editor, overlay]]).on_scroll(Message::Wheel),
        )
            .id(self.scroll_id.clone())
            .on_scroll(|v| Message::Scrolled(v.absolute_offset().y, v.bounds().height))
            .width(Length::Fill)
            .height(Length::Fill);

        let mut page = iced::widget::column![self.tab_bar(), body];
        if let Some(query) = &self.search {
            let mut find = row![
                text_input("Find", query)
                    .id(self.search_id.clone())
                    .on_input(Message::SearchChanged)
                    .on_submit(Message::SearchNext)
                    .size(13)
                    .padding([5, 6]),
            ]
            .spacing(8)
            .align_y(alignment::Vertical::Center);
            if !query.is_empty() {
                find = find.push(text(format!("{found_total} found")).size(12).color(p.gutter));
            }
            find = find.push(
                iced::widget::button(canvas(SearchIcon(Color::WHITE)).width(16).height(BAR_LINE_H))
                    .padding([5, 10])
                    .on_press(Message::SearchNext)
                    .style(|_, status| bar_button_style(DIM_GREY, Color::WHITE, status)),
            );
            page = page.push(
                container(find)
                .padding([8, 8])
                .width(Length::Fill)
                .style(|_| container::Style {
                    background: Some(iced::Background::Color(Color::from_rgba(0.0, 0.0, 0.0, 0.3))),
                    ..Default::default()
                }),
            );
        }
        if let Some(bar) = self.close_bar() {
            page = page.push(bar);
        }
        if self.saved_at.is_some() && self.search.is_none() && self.closing.is_none() {
            page = page.push(
                container(
                    container(text("saved").size(12).color(Color::from_rgb8(0x5f, 0xd0, 0x6a)))
                        .padding([2, 10])
                        .style(|_| container::Style {
                            background: Some(iced::Background::Color(DIM_GREY)),
                            border: iced::Border { radius: 999.0.into(), ..Default::default() },
                            ..Default::default()
                        }),
                )
                .padding([4, 8])
                .width(Length::Fill),
            );
        }
        if let Some(status) = &self.status {
            page = page.push(
                container(text(status.clone()).size(12).color(p.gutter))
                    .padding([2, 8])
                    .width(Length::Fill),
            );
        }

        container(page)
            .width(Length::Fill)
            .height(Length::Fill)
            .style(move |_| container::Style {
                background: Some(iced::Background::Color(p.background)),
                text_color: Some(p.foreground),
                ..Default::default()
            })
            .into()
    }
}

/// Wrap positions of every line, in visual rows.
#[derive(Clone)]
struct Layout {
    /// First visual row of every line, plus the total row count as a last entry.
    starts: Vec<usize>,
    /// Per line, the character column each visual row begins at.
    breaks: Vec<Vec<usize>>,
}

impl Layout {
    fn rows(&self) -> usize {
        self.starts.last().copied().unwrap_or(1)
    }

    /// (visual row, column within that row) of a character column.
    fn at(&self, line: usize, col: usize) -> (usize, usize) {
        let (row, x, _) = self.at_row_end(line, col);
        (row, x)
    }

    /// Like `at`, plus the character column where that visual row ends.
    fn at_row_end(&self, line: usize, col: usize) -> (usize, usize, usize) {
        let line = line.min(self.breaks.len().saturating_sub(1));
        let b = &self.breaks[line];
        let r = b.partition_point(|&s| s <= col).saturating_sub(1);
        (self.starts[line] + r, col - b[r], b.get(r + 1).copied().unwrap_or(usize::MAX))
    }
}

/// What decides where the editor wraps a line.
#[derive(Clone, Copy, PartialEq)]
struct Wrap {
    font: Font,
    size: f32,
    line_h: f32,
    /// Width available to the text, in pixels.
    width: f32,
}

/// Wrapped lines are cached by their text; the cache is dropped when it grows past this.
const WRAP_CACHE_LIMIT: usize = 100_000;

struct WrapCache {
    wrap: Option<Wrap>,
    buffer: Option<iced::advanced::graphics::text::cosmic_text::Buffer>,
    lines: std::collections::HashMap<String, Vec<usize>>,
}

thread_local! {
    static WRAP_CACHE: std::cell::RefCell<WrapCache> = std::cell::RefCell::new(WrapCache {
        wrap: None,
        buffer: None,
        lines: std::collections::HashMap::new(),
    });
}

/// Character columns at which each visual row of `text` begins. The line is
/// laid out by the same text engine and settings as the editor, so line
/// breaking rules (after `-`, `/`, ...) and hanging spaces match exactly.
fn wrap_line(text: &str, wrap: Wrap) -> Vec<usize> {
    use iced::advanced::graphics::text::{cosmic_text, font_system, to_attributes};
    WRAP_CACHE.with_borrow_mut(|cache| {
        if cache.wrap != Some(wrap) || cache.lines.len() > WRAP_CACHE_LIMIT {
            cache.lines.clear();
            cache.buffer = None;
            cache.wrap = Some(wrap);
        }
        if let Some(b) = cache.lines.get(text) {
            return b.clone();
        }

        let mut fs = font_system().write().expect("font system");
        let fs = fs.raw();
        let buffer = cache.buffer.get_or_insert_with(|| {
            let mut b = cosmic_text::Buffer::new(fs, cosmic_text::Metrics::new(wrap.size, wrap.line_h));
            b.set_wrap(fs, cosmic_text::Wrap::WordOrGlyph);
            b.set_size(fs, Some(wrap.width), None);
            b
        });
        buffer.set_text(fs, text, &to_attributes(wrap.font), cosmic_text::Shaping::Advanced, None);
        buffer.shape_until_scroll(fs, false);
        let mut breaks = vec![0];
        for run in buffer.layout_runs().skip(1) {
            if let Some(byte) = run.glyphs.iter().map(|g| g.start).min() {
                breaks.push(text.get(..byte).map_or(0, |t| t.chars().count()));
            }
        }
        cache.lines.insert(text.to_string(), breaks.clone());
        breaks
    })
}

/// Case-insensitive comparison form of `s`, one folded char per input char.
fn fold(s: &str) -> Vec<char> {
    s.chars().map(|c| c.to_lowercase().next().unwrap_or(c)).collect()
}

#[derive(Clone, Copy)]
struct Found {
    /// Start column in characters.
    col: usize,
    /// Byte range within the line.
    start: usize,
    end: usize,
}

/// Non-overlapping case-insensitive occurrences of `needle` in `line`.
fn find_all(line: &str, needle: &[char]) -> Vec<Found> {
    let chars: Vec<(usize, char)> = line.char_indices().collect();
    let folded: Vec<char> = chars.iter().map(|&(_, c)| c.to_lowercase().next().unwrap_or(c)).collect();
    let n = needle.len();
    let mut out = Vec::new();
    let mut i = 0;
    while n > 0 && i + n <= folded.len() {
        if folded[i..i + n] == *needle {
            let end = chars.get(i + n).map_or(line.len(), |c| c.0);
            out.push(Found { col: i, start: chars[i].0, end });
            i += n;
        } else {
            i += 1;
        }
    }
    out
}

fn indent_columns(line: &str) -> Option<usize> {
    let n = line.chars().take_while(|c| *c == ' ').count();
    (n < line.chars().count()).then_some(n)
}

/// Indentation per line. Blank lines take the smaller of the neighbouring
/// indents so the guides run unbroken through them.
fn fill_indents(texts: &[String]) -> Vec<usize> {
    let raw: Vec<Option<usize>> = texts.iter().map(|t| indent_columns(t)).collect();
    let mut prev = vec![0; raw.len()];
    let mut next = vec![0; raw.len()];
    let mut last = 0;
    for (i, r) in raw.iter().enumerate() {
        if let Some(n) = r {
            last = *n;
        }
        prev[i] = last;
    }
    last = 0;
    for (i, r) in raw.iter().enumerate().rev() {
        if let Some(n) = r {
            last = *n;
        }
        next[i] = last;
    }
    raw.iter()
        .enumerate()
        .map(|(i, r)| r.unwrap_or_else(|| prev[i].min(next[i])))
        .collect()
}

fn offset_of(content: &Content, p: Position) -> usize {
    let mut off = 0;
    for (i, l) in content.lines().enumerate() {
        if i == p.line {
            return off + p.column.min(l.text.len());
        }
        off += l.text.len() + l.ending.as_str().len();
    }
    off
}

fn pos_of(content: &Content, offset: usize) -> Position {
    let mut rest = offset;
    let mut last = Position { line: 0, column: 0 };
    for (i, l) in content.lines().enumerate() {
        let len = l.text.len();
        if rest <= len {
            return Position { line: i, column: rest };
        }
        rest -= (len + l.ending.as_str().len()).min(rest);
        last = Position { line: i, column: len };
    }
    last
}

fn closer(open: char) -> Option<char> {
    Some(match open {
        '(' => ')',
        '{' => '}',
        '[' => ']',
        '"' => '"',
        '\'' => '\'',
        _ => return None,
    })
}

/// Types a character, auto-closing pairs and typing over an existing closer.
/// A quote typed over a selection wraps it instead of replacing it.
fn insert_char(content: &mut Content, c: char) {
    let cursor = content.cursor();
    if matches!(c, '"' | '\'' | '`')
        && let Some(anchor) = cursor.selection
        && let Some(sel) = content.selection().filter(|s| !s.is_empty())
    {
        wrap_selection(content, c, anchor, cursor.position, &sel);
        return;
    }
    let pos = cursor.position;
    let line = content.line(pos.line).map(|l| l.text.to_string()).unwrap_or_default();
    let next = line.get(pos.column..).and_then(|r| r.chars().next());
    let prev = line.get(..pos.column).and_then(|r| r.chars().next_back());
    let selecting = cursor.selection.is_some();

    let is_closer = matches!(c, ')' | '}' | ']' | '"' | '\'');
    if !selecting && is_closer && next == Some(c) {
        content.perform(Action::Move(Motion::Right));
        return;
    }

    let quote = c == '"' || c == '\'';
    let word_before = prev.is_some_and(|p| p.is_alphanumeric() || p == '_');
    let word_after = next.is_some_and(|n| n.is_alphanumeric() || n == '_');
    match closer(c) {
        Some(close) if !selecting && !word_after && !(quote && word_before) => {
            content.perform(Action::Edit(Edit::Insert(c)));
            content.perform(Action::Edit(Edit::Insert(close)));
            content.perform(Action::Move(Motion::Left));
        }
        _ => content.perform(Action::Edit(Edit::Insert(c))),
    }
}

/// Puts `quote` on both sides of the selected text `sel` and keeps that text selected.
fn wrap_selection(content: &mut Content, quote: char, anchor: Position, head: Position, sel: &str) {
    let forward = (anchor.line, anchor.column) <= (head.line, head.column);
    let lo = if forward { anchor } else { head };
    content.perform(Action::Edit(Edit::Paste(std::sync::Arc::new(format!("{quote}{sel}{quote}")))));

    let start = Position { line: lo.line, column: lo.column + quote.len_utf8() };
    let end = match sel.rsplit_once('\n') {
        Some((before, last)) => Position { line: lo.line + before.matches('\n').count() + 1, column: last.len() },
        None => Position { line: start.line, column: start.column + sel.len() },
    };
    let (position, selection) = if forward { (end, start) } else { (start, end) };
    content.move_to(Cursor { position, selection: Some(selection) });
}

/// Ctrl+Backspace: delete the whitespace and the word before the cursor.
/// When only whitespace precedes the cursor, delete just that whitespace.
fn delete_word(content: &mut Content) {
    let cursor = content.cursor();
    if cursor.selection.is_some() {
        content.perform(Action::Edit(Edit::Backspace));
        return;
    }
    let pos = cursor.position;
    let line = content.line(pos.line).map(|l| l.text.to_string()).unwrap_or_default();
    let before: Vec<char> = line.get(..pos.column).unwrap_or("").chars().collect();
    if before.is_empty() {
        content.perform(Action::Edit(Edit::Backspace));
        return;
    }

    let mut n = 0;
    let mut rest = before.iter().rev().peekable();
    while rest.next_if(|c| c.is_whitespace()).is_some() {
        n += 1;
    }
    if let Some(&&first) = rest.peek() {
        let is_word = |c: char| c.is_alphanumeric() || c == '_';
        let class = is_word(first);
        while rest.next_if(|c| !c.is_whitespace() && is_word(**c) == class).is_some() {
            n += 1;
        }
    }
    for _ in 0..n {
        content.perform(Action::Edit(Edit::Backspace));
    }
}

/// Break the line and keep the indentation of the line being left.
fn enter_with_indent(content: &mut Content) {
    let pos = content.cursor().position;
    let indent = content.line(pos.line).map_or(0, |l| {
        let before = l.text.get(..pos.column).unwrap_or(&l.text);
        before.chars().take_while(|c| *c == ' ').count()
    });
    let between_braces = content.line(pos.line).is_some_and(|l| {
        l.text.get(..pos.column).is_some_and(|b| b.ends_with('{'))
            && l.text.get(pos.column..).is_some_and(|a| a.starts_with('}'))
    });

    let spaces = |content: &mut Content, n: usize| {
        for _ in 0..n {
            content.perform(Action::Edit(Edit::Insert(' ')));
        }
    };

    content.perform(Action::Edit(Edit::Enter));
    if between_braces {
        // `{|}` becomes an indented empty line with `}` two lines down.
        spaces(content, indent + TAB_WIDTH);
        content.perform(Action::Edit(Edit::Enter));
        spaces(content, indent);
        content.perform(Action::Move(Motion::Up));
        content.perform(Action::Move(Motion::End));
    } else {
        spaces(content, indent);
    }
}

/// Dim indentation guides, the long-line ruler, occurrences of the selected
/// text and search matches, drawn over the editor.
struct Guides {
    first: usize,
    layout: Layout,
    indents: Vec<usize>,
    /// (line, start column, length) in characters.
    matches: Vec<(usize, usize, usize)>,
    /// Search matches, as (line, start column, length) in characters.
    found: Vec<(usize, usize, usize)>,
    origin_x: f32,
    char_w: f32,
    line_h: f32,
    color: Color,
    ruler: Color,
    highlight: Color,
    found_color: Color,
}

impl<M> canvas::Program<M> for Guides {
    type State = ();

    fn draw(
        &self,
        _: &(),
        renderer: &iced::Renderer,
        _: &Theme,
        bounds: iced::Rectangle,
        _: iced::mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        let mut frame = canvas::Frame::new(renderer, bounds.size());
        let col_x = |col: usize| self.origin_x + col as f32 * self.char_w;
        let row_y = |row: usize| row as f32 * self.line_h;
        let starts = &self.layout.starts;
        let last = starts.len() - 2;
        // One rectangle per visual row covered by `len` characters from `col`.
        let spans = |line: usize, col: usize, len: usize| {
            let mut out = Vec::new();
            let (mut c, mut left) = (col, len);
            while left > 0 {
                let (row, x, end) = self.layout.at_row_end(line, c);
                let n = left.min(end.saturating_sub(c).max(1));
                out.push((x, row, n));
                c += n;
                left -= n;
            }
            out
        };

        frame.fill_rectangle(
            iced::Point::new((col_x(RULER_COLUMN as usize)).floor() + 0.5, 0.0),
            Size::new(1.0, bounds.height),
            self.ruler,
        );

        // One rectangle per unbroken run of lines at each indent level.
        let max_level = self.indents.iter().max().map_or(0, |n| n / TAB_WIDTH);
        for level in 0..max_level {
            let x = col_x(level * TAB_WIDTH).floor() + 0.5;
            let mut run: Option<usize> = None;
            for i in 0..=self.indents.len() {
                let active = self.indents.get(i).is_some_and(|n| n / TAB_WIDTH > level);
                match (active, run) {
                    (true, None) => run = Some(i),
                    (false, Some(start)) => {
                        let from = starts[(self.first + start).min(last)];
                        let to = starts[(self.first + i).min(last + 1)];
                        let y = row_y(from);
                        let h = (to - from) as f32 * self.line_h;
                        frame.fill_rectangle(iced::Point::new(x, y), Size::new(1.0, h), self.color);
                        run = None;
                    }
                    _ => {}
                }
            }
        }

        for &(line, col, len) in &self.matches {
            for (x, row, n) in spans(line, col, len) {
                frame.fill_rectangle(
                    iced::Point::new(col_x(x), row_y(row)),
                    Size::new(n as f32 * self.char_w, self.line_h),
                    self.highlight,
                );
            }
        }

        for &(line, col, len) in &self.found {
            for (x, row, n) in spans(line, col, len) {
                frame.fill_rectangle(
                    iced::Point::new(col_x(x), row_y(row)),
                    Size::new(n as f32 * self.char_w, self.line_h),
                    self.found_color,
                );
            }
        }
        vec![frame.into_geometry()]
    }
}

/// Height of a line of 13px bar text at iced's default 1.3 line height.
const BAR_LINE_H: f32 = 13.0 * 1.3;

/// A magnifying glass drawn in the given color.
struct SearchIcon(Color);

impl<M> canvas::Program<M> for SearchIcon {
    type State = ();

    fn draw(
        &self,
        _: &(),
        renderer: &iced::Renderer,
        _: &Theme,
        bounds: iced::Rectangle,
        _: iced::mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        let mut frame = canvas::Frame::new(renderer, bounds.size());
        let s = bounds.width.min(bounds.height);
        let top = (bounds.height - s) / 2.0;
        let r = s * 0.3;
        let center = iced::Point::new(r + 1.5, top + r + 1.5);
        let stroke = canvas::Stroke::default().with_color(self.0).with_width(1.6);
        frame.stroke(&canvas::Path::circle(center, r), stroke);
        let d = r * std::f32::consts::FRAC_1_SQRT_2;
        frame.stroke(
            &canvas::Path::line(
                iced::Point::new(center.x + d, center.y + d),
                iced::Point::new(s - 1.5, top + s - 1.5),
            ),
            stroke.with_line_cap(canvas::LineCap::Round),
        );
        vec![frame.into_geometry()]
    }
}

fn tab_style(p: Palette, active: bool) -> iced::widget::button::Style {
    iced::widget::button::Style {
        background: None,
        text_color: if active { p.foreground } else { p.gutter },
        border: iced::Border::default(),
        ..Default::default()
    }
}

/// A borderless bar button filled with `fill`, a little brighter while hovered.
fn bar_button_style(fill: Color, text_color: Color, status: iced::widget::button::Status) -> iced::widget::button::Style {
    use iced::widget::button::Status;
    let lift = if matches!(status, Status::Hovered | Status::Pressed) { 0.08 } else { 0.0 };
    let background = Color {
        r: (fill.r + lift).min(1.0),
        g: (fill.g + lift).min(1.0),
        b: (fill.b + lift).min(1.0),
        ..fill
    };
    iced::widget::button::Style {
        background: Some(iced::Background::Color(background)),
        text_color,
        border: iced::Border { radius: 4.0.into(), ..Default::default() },
        ..Default::default()
    }
}

/// While hovered the button becomes a filled disc of `hover` with a black label.
fn round_hover_style(
    base: iced::widget::button::Style,
    hover: Color,
    status: iced::widget::button::Status,
) -> iced::widget::button::Style {
    use iced::widget::button::Status;
    if matches!(status, Status::Hovered | Status::Pressed) {
        iced::widget::button::Style {
            background: Some(iced::Background::Color(hover)),
            text_color: Color::BLACK,
            border: iced::Border { radius: (CLOSE_SIZE / 2.0).into(), ..Default::default() },
            ..Default::default()
        }
    } else {
        base
    }
}
