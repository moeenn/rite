mod config;
mod highlight;
mod theme;

use config::Config;
use iced::keyboard::{Key, key::Named};
use iced::widget::{
    Id, Space, canvas, container, row, stack, scrollable, text, text_editor as text_editor_widget,
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
const CLOSE_SIZE: f32 = 24.0;
const RULER_COLUMN: f32 = 80.0;
/// Advance width of a monospace glyph, relative to the font size.
const CHAR_WIDTH: f32 = 0.6;

fn main() -> iced::Result {
    let config = config::load();
    iced::application(Rite::new, Rite::update, Rite::view)
        .title(Rite::title)
        .subscription(Rite::subscription)
        .theme(Rite::theme)
        .window_size(Size::new(config.window_width, config.window_height))
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
    /// Additional cursors as (anchor, head) byte offsets into the document.
    extras: Vec<(usize, usize)>,
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
            extras: Vec::new(),
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
    config: Config,
    font: Font,
    palette: Palette,
    scroll_id: Id,
    viewport_h: f32,
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
    Scrolled(f32),
    Resized(Size),
    Save,
    SaveDialog(Option<PathBuf>),
    Open,
    Opened(Option<PathBuf>),
    NewTab,
    CloseTab(usize),
    /// The unsaved document with this id may be closed.
    CloseConfirmed(u64),
    Noop,
    SwitchTab(usize),
    CycleTab(bool),
    Enter,
    Indent(bool),
    MoveLine(bool),
    SelectNext,
    DeleteWord,
    Undo,
    Redo,
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
        let app = Self {
            font: Self::font_for(&config),
            doc,
            tabs,
            active: 0,
            status,
            config,
            palette,
            scroll_id: Id::new("rite-scroll"),
            viewport_h,
        };
        (app, Task::none())
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

    fn line_px(&self) -> f32 {
        self.size_px() * self.config.line_height
    }

    fn subscription(&self) -> Subscription<Message> {
        window::resize_events().map(|(_, size)| Message::Resized(size))
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Edit(action) => {
                if matches!(action, Action::Scroll { .. }) {
                    self.doc.content.perform(action);
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
                        Edit::Insert(c) => {
                            let c = *c;
                            self.apply(|content| insert_char(content, c));
                        }
                        _ => self.apply(|c| c.perform(action.clone())),
                    }
                } else {
                    self.doc.last_edit = EditKind::None;
                    self.doc.extras.clear();
                    self.doc.content.perform(action);
                }
                return self.reveal_cursor();
            }
            Message::Enter => {
                self.record(EditKind::Other);
                self.mark_dirty();
                self.apply(enter_with_indent);
                return self.reveal_cursor();
            }
            Message::DeleteWord => {
                self.record(EditKind::Other);
                self.mark_dirty();
                self.apply(delete_word);
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
            Message::SelectNext => {
                self.select_next();
                return self.reveal_cursor();
            }
            Message::Scrolled(y) => self.doc.offset_y = y,
            Message::Resized(size) => self.viewport_h = size.height,
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
            Message::Noop => {}
            Message::CloseConfirmed(id) => {
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
        self.doc.extras.clear();
        self.doc.last_edit = EditKind::None;
        self.doc.dirty = true;
    }

    fn mark_dirty(&mut self) {
        self.doc.dirty = true;
        self.status = None;
    }

    /// Runs an edit once per cursor. With several cursors the edits run from the
    /// end of the document backwards so earlier offsets stay valid, and the
    /// cursors are then moved by the net size change of the edits after them.
    fn apply(&mut self, mut edit: impl FnMut(&mut Content)) {
        if self.doc.extras.is_empty() {
            edit(&mut self.doc.content);
            return;
        }

        let cur = self.doc.content.cursor();
        let head = offset_of(&self.doc.content, cur.position);
        let anchor = cur.selection.map_or(head, |a| offset_of(&self.doc.content, a));

        let mut sels: Vec<(usize, usize, bool)> = self.doc.extras.iter().map(|&(a, h)| (a, h, false)).collect();
        sels.push((anchor, head, true));
        sels.sort_by_key(|&(a, h, _)| std::cmp::Reverse(a.min(h)));
        sels.dedup_by_key(|&mut (a, h, _)| (a, h));

        let mut done: Vec<(usize, isize, bool)> = Vec::new();
        for (a, h, primary) in sels {
            let before = total_len(&self.doc.content) as isize;
            let cursor = Cursor {
                position: pos_of(&self.doc.content, h),
                selection: (a != h).then(|| pos_of(&self.doc.content, a)),
            };
            self.doc.content.move_to(cursor);
            edit(&mut self.doc.content);
            let after = total_len(&self.doc.content) as isize;
            let at = offset_of(&self.doc.content, self.doc.content.cursor().position);
            done.push((at, after - before, primary));
        }

        self.doc.extras.clear();
        let mut shift = 0isize;
        let mut finals: Vec<(usize, bool)> = Vec::new();
        // `done` is in descending document order; walk it from the document start.
        for &(at, delta, primary) in done.iter().rev() {
            finals.push(((at as isize + shift) as usize, primary));
            shift += delta;
        }
        for (at, primary) in finals {
            if primary {
                let position = pos_of(&self.doc.content, at);
                self.doc.content.move_to(Cursor { position, selection: None });
            } else {
                self.doc.extras.push((at, at));
            }
        }
    }

    /// Ctrl+D: keep the current selection as an extra cursor and select the
    /// next identical piece of text.
    fn select_next(&mut self) {
        let Some(needle) = self.doc.content.selection().filter(|s| !s.is_empty()) else {
            return;
        };
        let cur = self.doc.content.cursor();
        let head = offset_of(&self.doc.content, cur.position);
        let anchor = cur.selection.map_or(head, |a| offset_of(&self.doc.content, a));
        let (start, end) = (anchor.min(head), anchor.max(head));

        let mut taken: Vec<(usize, usize)> = self.doc.extras.iter().map(|&(a, h)| (a.min(h), a.max(h))).collect();
        taken.push((start, end));
        let from = taken.iter().map(|t| t.1).max().unwrap_or(end);

        let text = self.doc.content.text();
        let found = text[from..]
            .find(&needle)
            .map(|i| i + from)
            .or_else(|| text[..from].find(&needle));
        let Some(next) = found else { return };
        if taken.iter().any(|t| t.0 == next) {
            return;
        }

        self.doc.extras.push((start, end));
        self.doc.content.move_to(Cursor {
            position: pos_of(&self.doc.content, next + needle.len()),
            selection: Some(pos_of(&self.doc.content, next)),
        });
    }

    /// Tab / Shift+Tab over the selected lines (or the current line).
    fn shift_indent(&mut self, indent: bool) {
        if !self.doc.extras.is_empty() {
            return;
        }
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
        if !self.doc.extras.is_empty() {
            return;
        }
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

    /// The editor is laid out at its full height inside a scrollable, so keep
    /// the cursor line in view ourselves.
    fn reveal_cursor(&mut self) -> Task<Message> {
        let lh = self.line_px();
        let top = self.doc.content.cursor().position.line as f32 * lh;
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
                self.status = Some("saved".into());
            }
            Err(e) => {
                eprintln!("rite: cannot write {}: {e}", path.display());
                self.status = Some(format!("save failed: {e}"));
            }
        }
        Task::none()
    }

    fn scroll_to_saved(&self) -> Task<Message> {
        iced::widget::operation::scroll_to(
            self.scroll_id.clone(),
            scrollable::AbsoluteOffset { x: None, y: Some(self.doc.offset_y) },
        )
    }

    fn switch_to(&mut self, index: usize) -> Task<Message> {
        if index == self.active || index >= self.tabs.len() {
            return Task::none();
        }
        std::mem::swap(&mut self.doc, &mut self.tabs[self.active]);
        self.active = index;
        std::mem::swap(&mut self.doc, &mut self.tabs[self.active]);
        self.scroll_to_saved()
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
        let id = doc.id;
        let description = format!("\"{}\" has unsaved changes. Closing it will lose them.", doc.name());
        Task::perform(
            async move {
                rfd::AsyncMessageDialog::new()
                    .set_title("Unsaved changes")
                    .set_description(description)
                    .set_level(rfd::MessageLevel::Warning)
                    .set_buttons(rfd::MessageButtons::OkCancelCustom("Close and lose changes".into(), "Cancel".into()))
                    .show()
                    .await
            },
            move |result| match result {
                rfd::MessageDialogResult::Custom(label) if label == "Close and lose changes" => {
                    Message::CloseConfirmed(id)
                }
                rfd::MessageDialogResult::Ok => Message::CloseConfirmed(id),
                _ => Message::Noop,
            },
        )
    }

    fn force_close_tab(&mut self, index: usize) -> Task<Message> {
        if index >= self.tabs.len() {
            return Task::none();
        }
        if self.tabs.len() == 1 {
            self.doc = Doc::empty();
            return self.scroll_to_saved();
        }
        if index == self.active {
            self.tabs.remove(index);
            self.active = index.min(self.tabs.len() - 1);
            std::mem::swap(&mut self.doc, &mut self.tabs[self.active]);
            return self.scroll_to_saved();
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
            return self.scroll_to_saved();
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

        container(
            row![tabs, plus]
                .spacing(8)
                .align_y(alignment::Vertical::Center),
        )
        .padding(Padding::ZERO.right(8))
        .width(Length::Fill)
        .style(|_| container::Style {
            background: Some(iced::Background::Color(Color::from_rgba(0.0, 0.0, 0.0, 0.3))),
            ..Default::default()
        })
        .into()
    }

    fn view(&self) -> Element<'_, Message> {
        let p = self.palette;
        let size = self.size_px();
        let lh = self.line_px();

        // Extra space after the last line so the file can scroll up to the top of the window.
        let past_end = (self.viewport_h - lh * 2.0).max(0.0);

        let count = self.doc.content.line_count().max(1);
        let digits = count.to_string().len().max(2) as f32;
        let gutter_w = digits * size * 0.62 + GUTTER_PAD * 2.0;
        let numbers = (1..=count).map(|n| n.to_string()).collect::<Vec<_>>().join("\n");

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
        let editor = text_editor_widget(&self.doc.content)
            .font(self.font)
            .size(size)
            .line_height(LineHeight::Absolute(Pixels(lh)))
            .padding(Padding { top: 0.0, right: TEXT_PAD, bottom: past_end, left: TEXT_PAD })
            .height(Length::Shrink)
            .on_action(Message::Edit)
            .key_binding(move |press| {
                match (&press.key, press.modifiers.command()) {
                    (Key::Character(c), true) if c.as_str() == "s" => Some(Binding::Custom(Message::Save)),
                    (Key::Character(c), true) if c.as_str() == "z" && !press.modifiers.shift() => {
                        Some(Binding::Custom(Message::Undo))
                    }
                    (Key::Character(c), true) if c.as_str() == "y" || c.as_str() == "Z" => {
                        Some(Binding::Custom(Message::Redo))
                    }
                    (Key::Named(Named::Backspace), true) => Some(Binding::Custom(Message::DeleteWord)),
                    (Key::Character(c), true) if c.as_str() == "d" => Some(Binding::Custom(Message::SelectNext)),
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
                    (Key::Named(Named::PageDown), false) => Some(Binding::Move(Motion::PageDown)),
                    (Key::Named(Named::PageUp), false) => Some(Binding::Move(Motion::PageUp)),
                    _ => Binding::from_key_press(press),
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

        let first = ((self.doc.offset_y / lh) as usize).saturating_sub(1);
        let last = (((self.doc.offset_y + self.viewport_h) / lh) as usize + 2).min(count);
        let visible = first..last.max(first);

        let texts: Vec<String> = self.doc.content.lines().map(|l| l.text.to_string()).collect();
        let cur = self.doc.content.cursor();
        let needle = self.doc.content.selection().filter(|s| !s.is_empty() && !s.contains('\n'));
        let char_col = |p: Position| texts.get(p.line).map_or(0, |t| t[..p.column.min(t.len())].chars().count());

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

        // Extra cursors: (line, start column, end column, head line, head column).
        let extras = self
            .doc.extras
            .iter()
            .map(|&(a, h)| {
                let (pa, ph) = (pos_of(&self.doc.content, a), pos_of(&self.doc.content, h));
                let (sa, sh) = (char_col(pa), char_col(ph));
                (pa.line, sa, if pa.line == ph.line { sh } else { sa }, ph.line, sh)
            })
            .collect();

        let guides = Guides {
            first,
            indents: fill_indents(&texts)[visible].to_vec(),
            matches,
            extras,
            origin_x: gutter_w + TEXT_PAD,
            char_w: size * CHAR_WIDTH,
            line_h: lh,
            color: Color { a: 0.14, ..p.foreground },
            ruler: Color { a: 0.22, ..p.foreground },
            highlight: Color { a: 0.2, ..p.foreground },
            cursor: p.foreground,
        };
        let overlay = canvas(guides).width(Length::Fill).height(Length::Fill);

        let body = scrollable(stack![row![gutter, editor], overlay])
            .id(self.scroll_id.clone())
            .on_scroll(|v| Message::Scrolled(v.absolute_offset().y))
            .width(Length::Fill)
            .height(Length::Fill);

        let mut page = iced::widget::column![self.tab_bar(), body];
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

fn total_len(content: &Content) -> usize {
    content.lines().map(|l| l.text.len() + l.ending.as_str().len()).sum()
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
fn insert_char(content: &mut Content, c: char) {
    let cursor = content.cursor();
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
/// text and extra cursors, drawn over the editor.
struct Guides {
    first: usize,
    indents: Vec<usize>,
    /// (line, start column, length) in characters.
    matches: Vec<(usize, usize, usize)>,
    /// (line, start column, end column, head line, head column) in characters.
    extras: Vec<(usize, usize, usize, usize, usize)>,
    origin_x: f32,
    char_w: f32,
    line_h: f32,
    color: Color,
    ruler: Color,
    highlight: Color,
    cursor: Color,
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
        let line_y = |line: usize| line as f32 * self.line_h;

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
                        let y = line_y(self.first + start);
                        let h = (i - start) as f32 * self.line_h;
                        frame.fill_rectangle(iced::Point::new(x, y), Size::new(1.0, h), self.color);
                        run = None;
                    }
                    _ => {}
                }
            }
        }

        for &(line, col, len) in &self.matches {
            frame.fill_rectangle(
                iced::Point::new(col_x(col), line_y(line)),
                Size::new(len as f32 * self.char_w, self.line_h),
                self.highlight,
            );
        }

        for &(line, from, to, head_line, head_col) in &self.extras {
            if to > from {
                frame.fill_rectangle(
                    iced::Point::new(col_x(from), line_y(line)),
                    Size::new((to - from) as f32 * self.char_w, self.line_h),
                    Color { a: 0.3, ..self.cursor },
                );
            }
            frame.fill_rectangle(
                iced::Point::new(col_x(head_col), line_y(head_line)),
                Size::new(2.0, self.line_h),
                self.cursor,
            );
        }
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
