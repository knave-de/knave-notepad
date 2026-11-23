use crate::{
    AppResult,
    editor::{Document, columns},
    file_document::{FileDocument, SaveError},
    host::Application,
    library_preview::{Preview, Previews},
    paint::{self, Metrics},
    recovery::{Recovery, Snapshot},
    store::{LibraryFile, Note, Notebook, Settings, Storage, now},
    vim::VimEditor,
};
mod files;
mod window;

use crossterm::event::{KeyCode, KeyModifiers as ModalModifiers};
use knave_ui::{DisplayList, Rect, TextAlign, toolkit::*};
use std::{
    cell::RefCell,
    collections::{HashMap, VecDeque},
    path::{Path, PathBuf},
    rc::Rc,
    time::{Duration, Instant},
};
use window::{ViewPosition, WindowView};
use winit::keyboard::{Key as NativeKey, ModifiersState, NamedKey};

const EDITOR: ElementId = ElementId(20);
const STRIP: ElementId = ElementId(19);
const STATUS: ElementId = ElementId(21);
const SEARCH: ElementId = ElementId(30);
const FIND: ElementId = ElementId(70);
const REPLACE: ElementId = ElementId(71);
const PAGE_SIZE: usize = 60;
const LIBRARY_TAB_WIDTH: f32 = 112.0;
const DOCUMENT_TAB_BASE: u64 = 200_000;
#[derive(Clone, Copy, Default, PartialEq)]
enum OverviewSort {
    #[default]
    DateModified,
    DateCreated,
    TitleAscending,
}
#[derive(Clone, Copy, PartialEq)]
enum Page {
    Overview,
    Editor,
    Settings,
}
struct Tab {
    doc: Document,
    vim: Option<VimEditor>,
}
#[derive(Clone)]
struct TabHit {
    id: String,
    rect: Rect,
    close: Rect,
    bookmark: Option<Rect>,
}
enum FilePrompt {
    Close(String),
    CloseWindow(Vec<String>),
    Conflict(String),
}

pub struct Notes {
    storage: Storage,
    view: WindowView,
    window: u64,
    windows: HashMap<u64, WindowView>,
    generation: u64,
    shared_text: HashMap<String, (u64, String)>,
    book: Notebook,
    previews: Previews,
    settings: Settings,
    tabs: HashMap<String, Tab>,
    files: HashMap<String, FileDocument>,
    recovery: Option<Recovery>,
    recovery_after_crash: bool,
    recoverable: VecDeque<(String, Snapshot)>,
    checkpoints: HashMap<String, Instant>,
    modal_store:
        Rc<RefCell<modalkit::editing::store::Store<modalkit::editing::application::EmptyInfo>>>,
    save_error: Option<String>,
}
trait Enabled {
    fn enabled(self, value: bool) -> Self;
}
impl Enabled for Element {
    fn enabled(mut self, value: bool) -> Self {
        self.enabled = value;
        self
    }
}
fn label(id: u64, value: impl Into<String>) -> Element {
    let mut e = Element::new(id, Widget::Text(value.into()));
    e.paint_surface = false;
    e
}
fn button(id: u64, value: impl Into<String>) -> Element {
    Element::new(id, Widget::Button(value.into())).layout(Layout {
        width: Length::Px(84.0),
        height: Length::Px(34.0),
        padding: 6.0,
        ..Default::default()
    })
}
fn compact(id: u64, value: impl Into<String>) -> Element {
    let mut e = button(id, value);
    e.layout.width = Length::Px(34.0);
    e
}
fn icon_button(id: u64, asset: &str, name: &str) -> Element {
    let icon = TablerIcon::named(asset, IconVariant::Outline).expect("bundled Notepad icon");
    let mut button = Element::tabler_button(id, 91_000 + id, icon, name);
    button.layout.width = Length::Px(34.0);
    button.layout.height = Length::Px(34.0);
    button
}

fn paint_icon(
    list: &mut DisplayList,
    name: &str,
    variant: IconVariant,
    bounds: Rect,
    color: knave_ui::Color,
    clip: Rect,
) {
    let icon = TablerIcon::named(name, variant).expect("bundled Notepad icon");
    list.commands.push(knave_ui::DisplayCommand::Image {
        bounds,
        image: icon.image(color),
        style: knave_ui::ImageStyle::default(),
        transform: knave_ui::Transform2D::IDENTITY,
        clip: Some(clip),
    });
}

fn overflow_menu(id: u64, icon_id: u64, name: &str, items: Vec<MenuItem>) -> Element {
    let icon = TablerIcon::named("dots", IconVariant::Outline).unwrap();
    let mut menu = Element::new(
        id,
        Widget::Menu {
            label: String::new(),
            items,
        },
    )
    .layout(Layout {
        flow: Flow::Overlay,
        align: Align::Center,
        width: Length::Px(34.0),
        height: Length::Px(34.0),
        ..Default::default()
    })
    .children(vec![icon.element(icon_id)]);
    menu.semantics.name = Some(name.into());
    menu.tooltip = Some(name.into());
    menu
}

fn file_dialog(message: String, choices: &[(u64, &str)]) -> Element {
    let mut message = label(101, message);
    message.style.text.wrap = knave_ui::TextWrap::WordOrGlyph;
    message.layout.height = Length::Fill;
    let actions = choices
        .iter()
        .map(|&(id, title)| {
            let mut button = button(id, title);
            button.layout.width = Length::Fill;
            button
        })
        .collect();
    Element::new(100, Widget::Popup { modal: true })
        .children(vec![
            message,
            row(109, actions).layout(Layout {
                flow: Flow::Row,
                height: Length::Px(34.0),
                gap: 6.0,
                ..Default::default()
            }),
        ])
        .layout(Layout {
            width: Length::Px(572.0),
            height: Length::Px(180.0),
            padding: 16.0,
            gap: 12.0,
            ..Default::default()
        })
}

fn row(id: u64, children: Vec<Element>) -> Element {
    let mut e = Element::new(id, Widget::Panel)
        .layout(Layout {
            flow: Flow::Row,
            align: Align::Center,
            gap: 6.0,
            ..Default::default()
        })
        .children(children);
    e.paint_surface = false;
    e
}
fn panel(id: u64, children: Vec<Element>) -> Element {
    let mut e = Element::new(id, Widget::Panel).children(children);
    e.paint_surface = false;
    e
}
fn field(id: u64, value: &str) -> AppResult<Element> {
    Ok(
        Element::new(id, Widget::TextInput(TextEdit::new(value)?)).layout(Layout {
            height: Length::Px(32.0),
            padding: 6.0,
            ..Default::default()
        }),
    )
}
fn toggle(id: u64, value: &str, checked: bool) -> Element {
    Element::new(
        id,
        Widget::Toggle {
            label: value.into(),
            checked,
        },
    )
    .layout(Layout {
        height: Length::Px(36.0),
        ..Default::default()
    })
}
fn heading(id: u64, text: &str) -> Element {
    Element::heading(id, text, knave_ui::HeadingLevel::H2)
}
fn stepper(
    id: u64,
    title: &str,
    value: String,
    minus: u64,
    plus: u64,
    can_minus: bool,
    can_plus: bool,
) -> Element {
    let mut number = label(id + 8000, value);
    number.layout.width = Length::Px(56.0);
    number.style.text.align = TextAlign::Center;
    row(
        id,
        vec![
            label(id + 8100, title),
            compact(minus, "−").enabled(can_minus),
            number,
            compact(plus, "+").enabled(can_plus),
        ],
    )
    .layout(Layout {
        flow: Flow::Row,
        align: Align::Center,
        height: Length::Px(44.0),
        gap: 8.0,
        ..Default::default()
    })
}
impl Notes {
    pub fn new(storage: Storage) -> AppResult<Self> {
        let (book, settings) = storage.load_library()?;
        let mut app = Self {
            storage,
            view: WindowView::new()?,
            window: 0,
            windows: HashMap::new(),
            generation: 0,
            shared_text: HashMap::new(),
            book,
            previews: Previews::default(),
            settings,
            tabs: HashMap::new(),
            files: HashMap::new(),
            recovery: None,
            recovery_after_crash: false,
            recoverable: VecDeque::new(),
            checkpoints: HashMap::new(),
            modal_store: Rc::new(RefCell::new(Default::default())),
            save_error: None,
        };
        app.previews.start(&app.book.files);
        app.save();
        app.rebuild()?;
        Ok(app)
    }
    fn elsewhere(&self, id: &str) -> bool {
        self.windows
            .values()
            .any(|view| view.tab_order.iter().any(|tab| tab == id))
    }

    fn share_text(&mut self, id: &str) {
        if self.elsewhere(id)
            && let Some(tab) = self.tabs.get(id)
        {
            self.shared_text
                .entry(id.to_owned())
                .or_insert_with(|| (tab.doc.revision, tab.doc.text.clone()));
        }
    }

    fn update_shared_views(&mut self, id: &str) {
        let Some((revision, previous)) = self.shared_text.get_mut(id) else {
            return;
        };
        let doc = &self.tabs[id].doc;
        if *revision == doc.revision && *previous == doc.text {
            return;
        }
        *revision = doc.revision;
        let prefix = previous
            .chars()
            .zip(doc.text.chars())
            .take_while(|(a, b)| a == b)
            .map(|(c, _)| c.len_utf8())
            .sum::<usize>();
        let suffix = previous[prefix..]
            .chars()
            .rev()
            .zip(doc.text[prefix..].chars().rev())
            .take_while(|(a, b)| a == b)
            .map(|(c, _)| c.len_utf8())
            .sum::<usize>();
        let old_end = previous.len() - suffix;
        let new_end = doc.text.len() - suffix;
        let adjust = |position: usize| {
            if position <= prefix {
                position
            } else if position >= old_end {
                new_end + position - old_end
            } else {
                new_end
            }
        };
        let row = previous[..prefix]
            .bytes()
            .filter(|&byte| byte == b'\n')
            .count();
        let old_end_row = row
            + previous[prefix..old_end]
                .bytes()
                .filter(|&byte| byte == b'\n')
                .count();
        let new_end_row = row
            + doc.text[prefix..new_end]
                .bytes()
                .filter(|&byte| byte == b'\n')
                .count();
        for view in self.windows.values_mut() {
            if let Some(position) = view.positions.get_mut(id) {
                position.cursor = adjust(position.cursor);
                position.anchor = adjust(position.anchor);
                if position.scroll_row > row {
                    position.scroll_row =
                        new_end_row + position.scroll_row.saturating_sub(old_end_row);
                }
            }
        }
        previous.clone_from(&doc.text);
        self.generation = self.generation.wrapping_add(1);
    }

    fn switch_window(&mut self, id: u64) -> AppResult {
        if self.window == id {
            return Ok(());
        }
        let mut next = match self.windows.remove(&id) {
            Some(view) => view,
            None => {
                let mut view = WindowView::new()?;
                view.scene
                    .set_theme(self.view.scene.theme_policy().scheme)?;
                view
            }
        };
        let positions = self
            .book
            .tabs
            .iter()
            .filter_map(|id| {
                self.tabs
                    .get(id)
                    .map(|tab| (id.clone(), ViewPosition::capture(&tab.doc)))
            })
            .collect();
        let next_positions = std::mem::take(&mut next.positions);
        std::mem::swap(&mut self.book.tabs, &mut next.tab_order);
        let mut previous = std::mem::replace(&mut self.view, next);
        previous.positions = positions;
        previous.tab_order = std::mem::take(&mut self.view.tab_order);
        self.windows.insert(self.window, previous);
        self.window = id;
        for tab_id in &self.book.tabs {
            if let Some(tab) = self.tabs.get_mut(tab_id) {
                next_positions
                    .get(tab_id)
                    .copied()
                    .unwrap_or_default()
                    .restore(tab);
            }
        }
        if self.view.seen_generation != self.generation
            || self.view.scene.widget(ElementId(2)).is_none()
        {
            self.view.seen_generation = self.generation;
            self.rebuild()?;
        }
        Ok(())
    }

    fn forget_window(&mut self, id: u64) {
        if let Some(view) = self.windows.remove(&id) {
            for tab in view.tab_order {
                if !self.book.tabs.contains(&tab) && !self.elsewhere(&tab) {
                    self.tabs.remove(&tab);
                    self.files.remove(&tab);
                    self.shared_text.remove(&tab);
                    self.checkpoints.remove(&tab);
                }
            }
        }
    }

    fn save(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.view.seen_generation = self.generation;
        self.save_error = self
            .storage
            .save(&self.book, &self.settings)
            .err()
            .map(|e| format!("Save failed: {e}. Retry with Ctrl+S."));
    }
    fn current(&self) -> Option<&Tab> {
        self.view.active.as_ref().and_then(|id| self.tabs.get(id))
    }
    fn current_mut(&mut self) -> Option<&mut Tab> {
        self.view
            .active
            .as_ref()
            .and_then(|id| self.tabs.get_mut(id))
    }
    fn status(&self) -> String {
        if let Some(error) = &self.save_error {
            return error.clone();
        }
        if let Some(message) = &self.view.message {
            return message.clone();
        }
        if self.view.page != Page::Editor {
            let count = self.book.notes.len() + self.book.files.len();
            return if count == 1 {
                "1 document".into()
            } else {
                format!("{count} documents")
            };
        }
        if let Some(tab) = self.current() {
            let doc = &tab.doc;
            let file = self.view.active.as_ref().and_then(|id| self.files.get(id));
            let mode = tab
                .vim
                .as_ref()
                .map(|v| format!("{:?}", v.mode()))
                .unwrap_or_else(|| if doc.overwrite { "OVR" } else { "INS" }.into());
            let indent = if self.settings.spaces {
                "Spaces"
            } else {
                "Tabs"
            };
            let path = file
                .map(|file| format!("    {}", file.path.display()))
                .unwrap_or_default();
            let selected = if doc.selection().is_empty() {
                String::new()
            } else {
                format!(" · {} selected", doc.selected().chars().count())
            };
            format!(
                "{}    UTF-8 · {}    {indent}: {}    Ln {}, Col {}{selected}    {mode}{path}",
                if let Some(file) = file {
                    if file.dirty(&doc.text) {
                        "Modified"
                    } else {
                        "Saved"
                    }
                } else if doc.text.is_empty() {
                    "Empty note"
                } else if self.save_error.is_none() {
                    "Saved"
                } else {
                    "Unsaved"
                },
                file.map_or("LF", FileDocument::line_endings),
                self.settings.tab_width,
                doc.row() + 1,
                doc.column(self.settings.tab_width as usize) + 1
            )
        } else {
            String::new()
        }
    }
    fn refresh_status(&mut self) -> AppResult {
        self.view.scene.set_text(STATUS, self.status())?;
        if self.view.scene.widget(ElementId(61)).is_some() {
            self.view
                .scene
                .set_visible(ElementId(61), self.save_error.is_some())?;
        }
        Ok(())
    }
    fn ensure_tab(&mut self, id: &str) -> AppResult {
        if !self.tabs.contains_key(id) {
            let note = self
                .book
                .notes
                .iter_mut()
                .find(|n| n.id == id)
                .ok_or("Missing note")?;
            self.storage.materialize(note)?;
            let doc = Document::new(&note.text);
            let vim = self
                .settings
                .vim
                .then(|| VimEditor::with_store(&doc.text, 0, self.modal_store.clone()));
            self.tabs.insert(id.into(), Tab { doc, vim });
        }
        Ok(())
    }
    fn leave_settings(&mut self) -> AppResult {
        self.view.page = self.view.settings_return.take().unwrap_or(Page::Overview);
        self.rebuild()?;
        if self.view.page == Page::Editor {
            self.focus_editor()?;
        }
        Ok(())
    }
    fn focus_editor(&mut self) -> AppResult {
        self.view.focused = true;
        self.view.scene.set_focus(Some(EDITOR))?;
        Ok(())
    }
    fn open(&mut self, id: String) -> AppResult {
        if let Err(error) = self.ensure_tab(&id) {
            self.view.message = Some(format!("Could not open note: {error}"));
            return self.refresh_status();
        }
        if !self.book.tabs.contains(&id) {
            if self.elsewhere(&id)
                && let Some(tab) = self.tabs.get_mut(&id)
            {
                ViewPosition::default().restore(tab);
            }
            self.book.tabs.push(id.clone());
            self.share_text(&id);
        }
        self.view.active = Some(id);
        self.view.page = Page::Editor;
        self.view.message = None;
        self.view.reveal_tab = true;
        self.view.preedit.clear();
        self.save();
        self.rebuild()?;
        self.focus_editor()
    }
    fn create(&mut self) -> AppResult {
        let note = Note::new();
        let id = note.id.clone();
        self.book.notes.push(note);
        self.open(id)
    }
    fn close(&mut self, id: &str) -> AppResult {
        if !self.elsewhere(id)
            && self
                .files
                .get(id)
                .is_some_and(|file| file.dirty(&self.tabs[id].doc.text))
        {
            self.view.file_prompt = Some(FilePrompt::Close(id.into()));
            return self.rebuild();
        }
        self.remove_tab(id)
    }

    fn remove_tab(&mut self, id: &str) -> AppResult {
        if !self.elsewhere(id) {
            if self.files.contains_key(id) && !self.clear_recovery(id)? {
                return Ok(());
            }
            self.files.remove(id);
            self.tabs.remove(id);
            self.shared_text.remove(id);
        }
        let index = self.book.tabs.iter().position(|t| t == id).unwrap_or(0);
        self.book.tabs.retain(|t| t != id);
        if self.view.active.as_deref() == Some(id) {
            self.view.active = None;
            if let Some(next) = self
                .book
                .tabs
                .get(index.min(self.book.tabs.len().saturating_sub(1)))
                .cloned()
            {
                return self.open(next);
            }
            self.view.page = Page::Overview;
            self.view.focused = false;
        }
        self.save();
        self.rebuild()
    }
    fn changed(&mut self) -> AppResult {
        if let Some(id) = self.view.active.clone() {
            self.update_shared_views(&id);
        }
        if let Some(id) = &self.view.active
            && self.recovery.is_some()
            && self.files.contains_key(id)
        {
            self.checkpoints
                .entry(id.clone())
                .or_insert_with(|| Instant::now() + Duration::from_millis(500));
        }
        let mut changed = false;
        let mut title_changed = false;
        if let Some(id) = &self.view.active
            && let Some(tab) = self.tabs.get(id)
            && let Some(note) = self.book.notes.iter_mut().find(|n| &n.id == id)
            && note.text != tab.doc.text
        {
            let previous_title = note.title();
            note.text.clone_from(&tab.doc.text);
            note.updated = now();
            title_changed = note.title() != previous_title;
            changed = true;
        }
        if changed {
            self.save();
            if title_changed {
                self.rebuild()?;
            }
        }
        let metrics = self.view.metrics;
        let width = self.settings.tab_width as usize;
        if let Some(tab) = self.current_mut() {
            metrics.reveal(&mut tab.doc, width);
        }
        self.refresh_status()
    }
    fn sync_vim(&mut self, result: Result<(), String>) -> AppResult {
        self.view.message = result.err();
        let mut clipboard_copy = None;
        if let Some(tab) = self.current_mut()
            && let Some(vim) = &mut tab.vim
        {
            clipboard_copy = vim.take_clipboard_copy();
            let text = vim.text();
            let (cursor, anchor) = vim.positions();
            tab.doc.set_external(text, cursor, anchor);
        }
        if let Some(text) = clipboard_copy {
            self.view.effects.push(Action::Copy(text));
        }
        self.changed()
    }
    fn reset_vim_from_doc(&mut self) {
        let error = if let Some(tab) = self.current_mut()
            && let Some(vim) = &mut tab.vim
        {
            if let Err(error) = vim.apply_external(&tab.doc.text, tab.doc.cursor) {
                let text = vim.text();
                let (cursor, anchor) = vim.positions();
                tab.doc.set_external(text, cursor, anchor);
                Some(error)
            } else {
                None
            }
        } else {
            None
        };
        if let Some(error) = error {
            self.view.message = Some(format!("Could not apply modal edit: {error}"));
        }
    }
    fn switch_tab(&mut self, delta: isize) -> AppResult {
        if self.book.tabs.is_empty() {
            return Ok(());
        }
        let index = self
            .view
            .active
            .as_ref()
            .and_then(|id| self.book.tabs.iter().position(|t| t == id))
            .unwrap_or(0);
        let index = (index as isize + delta).rem_euclid(self.book.tabs.len() as isize) as usize;
        self.open(self.book.tabs[index].clone())
    }
    fn find(&mut self, backwards: bool) -> AppResult {
        let query = self.view.find_query.clone();
        let found = self
            .current_mut()
            .is_some_and(|tab| tab.doc.find(&query, backwards));
        self.view.message = (!found).then(|| "No match".into());
        if found
            && let Some(tab) = self.current_mut()
            && let Some(vim) = &mut tab.vim
        {
            vim.place(&tab.doc.text, tab.doc.cursor);
        }
        self.changed()
    }
    fn import(&mut self) -> AppResult {
        let Some(path) = rfd::FileDialog::new()
            .set_title("Import as Note")
            .pick_file()
        else {
            return Ok(());
        };
        match std::fs::read_to_string(&path) {
            Ok(text) => {
                let mut note = Note::new();
                note.text = crate::editor::normalize(&text);
                note.name = path.file_name().map(|s| s.to_string_lossy().into_owned());
                note.keep_empty = true;
                let id = note.id.clone();
                self.book.notes.push(note);
                self.open(id)?;
            }
            Err(error) => {
                self.view.message = Some(format!("Could not open document: {error}"));
                self.refresh_status()?;
            }
        }
        Ok(())
    }
    fn export(&mut self) -> AppResult {
        let Some(id) = &self.view.active else {
            return Ok(());
        };
        self.export_note(&id.clone())
    }
    fn export_note(&mut self, id: &str) -> AppResult {
        let Some(note) = self.book.notes.iter_mut().find(|n| n.id == id) else {
            return Ok(());
        };
        if let Err(error) = self.storage.materialize(note) {
            self.view.message = Some(format!("Cannot export note: {error}"));
            return self.refresh_status();
        }
        let Some(path) = rfd::FileDialog::new()
            .set_title("Export text document")
            .set_file_name(format!("{}.txt", note.title()))
            .save_file()
        else {
            return Ok(());
        };
        match FileDocument::export(&path, &note.text) {
            Ok(()) => self.view.message = Some(format!("Exported to {}", path.display())),
            Err(e) => self.view.message = Some(format!("Export failed: {e}")),
        };
        self.refresh_status()
    }
    fn finish_rename(&mut self) -> AppResult {
        let name = self.view.rename_text.trim();
        if name.is_empty() {
            return Ok(());
        }
        if let Some(id) = self.view.rename_note.take() {
            if let Some(note) = self.book.notes.iter_mut().find(|n| n.id == id) {
                if let Err(error) = self.storage.materialize(note) {
                    self.view.message = Some(format!("Cannot rename note: {error}"));
                    return self.rebuild();
                }
                note.name = Some(name.to_string());
                note.updated = now();
            }
            self.save();
        }
        self.rebuild()
    }
    fn delete_note(&mut self, id: &str) -> AppResult {
        self.book.notes.retain(|note| note.id != id);
        self.book.tabs.retain(|tab| tab != id);
        self.tabs.remove(id);
        if self.view.active.as_deref() == Some(id) {
            self.view.active = None;
        }
        self.save();
        self.rebuild()
    }
    fn command(&mut self) -> AppResult {
        let command = self.view.command.trim().trim_start_matches(':').to_owned();
        self.view.command_open = false;
        self.view.command.clear();
        self.rebuild()?;
        self.focus_editor()?;
        match command.as_str() {
            "w" => {
                self.save_current()?;
            }
            "q" => {
                if let Some(id) = self.view.active.clone() {
                    self.close(&id)?;
                }
            }
            "wq" | "x" => {
                if let Some(id) = self.view.active.clone() {
                    if self.save_current()? {
                        self.close(&id)?;
                    } else if matches!(self.view.file_prompt, Some(FilePrompt::Conflict(_))) {
                        self.view.close_after_save = Some(id);
                    }
                }
            }
            "e" => self.pick_file()?,
            command if command.starts_with("e ") => {
                if let Err(error) = self.open_file(Path::new(command[2..].trim())) {
                    self.view.message = Some(format!("Open failed: {error}"));
                    self.refresh_status()?;
                }
            }
            c if c.starts_with("set ") => {
                let mut error = None;
                for option in c[4..].split_whitespace() {
                    match option {
                        "nu" | "number" => self.settings.line_numbers = true,
                        "nonu" | "nonumber" => self.settings.line_numbers = false,
                        "rnu" | "relativenumber" => self.settings.relative_numbers = true,
                        "nornu" | "norelativenumber" => self.settings.relative_numbers = false,
                        "et" | "expandtab" => self.settings.spaces = true,
                        "noet" | "noexpandtab" => self.settings.spaces = false,
                        "ai" | "autoindent" => self.settings.auto_indent = true,
                        "noai" | "noautoindent" => self.settings.auto_indent = false,
                        value if value.starts_with("ts=") || value.starts_with("tabstop=") => {
                            if let Some(width) = value
                                .split('=')
                                .nth(1)
                                .and_then(|s| s.parse::<u16>().ok())
                                .filter(|v| (1..=16).contains(v))
                            {
                                self.settings.tab_width = width;
                            } else {
                                error = Some("Tab width must be 1–16".into());
                            }
                        }
                        _ => error = Some(format!("Unknown setting: {option}")),
                    }
                }
                self.view.message = error;
                self.save();
                self.refresh_status()?;
            }
            c if c.parse::<usize>().is_ok() => {
                let line = c.parse::<usize>().unwrap().saturating_sub(1);
                if let Some(tab) = self.current_mut() {
                    let byte = tab.doc.start(line.min(tab.doc.lines() - 1));
                    tab.doc.place(byte, false);
                    if let Some(vim) = &mut tab.vim {
                        vim.place(&tab.doc.text, byte);
                    }
                }
                self.changed()?;
            }
            _ => {
                self.view.message = Some(format!("Unknown command: {command}"));
                self.refresh_status()?;
            }
        }
        Ok(())
    }
    fn document_tab_title(&self, id: &str) -> String {
        self.files.get(id).map_or_else(
            || {
                self.book
                    .notes
                    .iter()
                    .find(|note| note.id == id)
                    .map(Note::title)
                    .unwrap_or_default()
            },
            |file| {
                file.path
                    .file_name()
                    .unwrap_or(file.path.as_os_str())
                    .to_string_lossy()
                    .into_owned()
            },
        )
    }

    fn tab_width(&self, strip_width: f32) -> f32 {
        (strip_width / self.book.tabs.len().max(1) as f32).clamp(155.0, 280.0)
    }

    fn tab_controls(&mut self) -> Element {
        let extent = (self.view.size[0] - LIBRARY_TAB_WIDTH).max(0.0);
        let width = self.tab_width(extent);
        if self.view.reveal_tab {
            if let Some(index) = self
                .view
                .active
                .as_ref()
                .and_then(|id| self.book.tabs.iter().position(|tab| tab == id))
            {
                let left = index as f32 * width;
                self.view.tab_scroll = self
                    .view
                    .tab_scroll
                    .clamp((left + width - extent).min(left), left);
            }
            self.view.reveal_tab = false;
        }
        self.view.tab_scroll = self
            .view
            .tab_scroll
            .clamp(0.0, (width * self.book.tabs.len() as f32 - extent).max(0.0));
        let first = (self.view.tab_scroll / width).floor() as usize;
        let visible = (extent / width).ceil() as usize + 1;
        let children = self
            .book
            .tabs
            .iter()
            .enumerate()
            .skip(first)
            .take(visible)
            .flat_map(|(index, id)| {
                let left = index as f32 * width - self.view.tab_scroll;
                let base = DOCUMENT_TAB_BASE + index as u64 * 3;
                let file = self.files.get(id);
                let mut tab = button(base, "");
                let title = self.document_tab_title(id);
                tab.semantics.name = Some(title.clone());
                tab.semantics.role = Some(Role::Tab);
                tab.semantics.current =
                    self.view.page == Page::Editor && self.view.active.as_ref() == Some(id);
                tab.tooltip = Some(
                    self.files
                        .get(id)
                        .map_or_else(|| title.clone(), |file| file.path.display().to_string()),
                );
                tab.paint_surface = false;
                tab.layout.width = Length::Px(width - if file.is_some() { 53.0 } else { 29.0 });
                tab.layout.height = Length::Px(38.0);
                tab.layout.offset = [left, 0.0];

                let mut close = button(base + 1, "");
                close.semantics.name = Some(format!("Close {title}"));
                close.tooltip = Some("Close tab".into());
                close.paint_surface = false;
                close.layout.width = Length::Px(24.0);
                close.layout.height = Length::Px(26.0);
                close.layout.offset = [left + width - 29.0, 6.0];
                let bookmark = file.map(|file| {
                    let action = if self.bookmark_index(file).is_some() {
                        "Remove from Library"
                    } else {
                        "Add to Library"
                    };
                    let mut bookmark = button(base + 2, "");
                    bookmark.semantics.name = Some(action.into());
                    bookmark.tooltip = Some(action.into());
                    bookmark.paint_surface = false;
                    bookmark.layout.width = Length::Px(24.0);
                    bookmark.layout.height = Length::Px(26.0);
                    bookmark.layout.offset = [left + width - 53.0, 6.0];
                    bookmark
                });
                [Some(tab), Some(close), bookmark].into_iter().flatten()
            })
            .collect();
        let mut strip = panel(STRIP.0, children).layout(Layout {
            flow: Flow::Overlay,
            align: Align::Start,
            height: Length::Px(38.0),
            gap: 0.0,
            ..Default::default()
        });
        strip.paint_surface = false;
        strip
    }

    fn rebuild(&mut self) -> AppResult {
        // Keep the active blank buffer editable (including undo), but discard
        // empty drafts as soon as the user leaves them.
        let keep = if self.view.page == Page::Editor
            || (self.view.page == Page::Settings && self.view.settings_return == Some(Page::Editor))
        {
            self.view.active.as_deref()
        } else {
            None
        };
        let inactive_active: Vec<_> = self
            .windows
            .values()
            .filter_map(|view| view.active.as_deref())
            .collect();
        self.book.notes.retain(|note| {
            note.retained()
                || Some(note.id.as_str()) == keep
                || inactive_active.contains(&note.id.as_str())
        });
        self.book.tabs.retain(|id| {
            self.files.contains_key(id) || self.book.notes.iter().any(|note| &note.id == id)
        });
        self.tabs.retain(|id, _| {
            self.files.contains_key(id) || self.book.notes.iter().any(|note| &note.id == id)
        });
        if self.view.active.as_ref().is_some_and(|id| {
            !self.files.contains_key(id) && !self.book.notes.iter().any(|note| &note.id == id)
        }) {
            self.view.active = None;
            if self.view.page == Page::Editor {
                self.view.page = Page::Overview;
                self.view.focused = false;
            }
        }
        let editing = self.view.page == Page::Editor;
        let file = self
            .view
            .active
            .as_ref()
            .filter(|_| editing)
            .and_then(|id| self.files.get(id));
        let external = file.is_some();
        let page_title = if self.view.page == Page::Settings {
            "Preferences"
        } else {
            ""
        };
        let mut title = label(3, page_title);
        title.style.text.align = TextAlign::Center;
        title.style.text.weight = 600;
        title.style.text.wrap = knave_ui::TextWrap::None;
        let mut tools = Vec::new();
        if self.view.page == Page::Settings {
            tools.push(button(2, "Back"));
        }
        tools.extend([
            icon_button(4, "plus", "New note"),
            title,
            icon_button(9, "folder-open", "Open File"),
        ]);
        if editing {
            tools.push(icon_button(11, "search", "Find"));
        }
        if external {
            tools.push(icon_button(12, "device-floppy", "Save"));
        }
        if self.view.page != Page::Settings {
            let mut actions = vec![MenuItem::option(1, "Import as Note…")];
            if editing {
                actions.push(MenuItem::option(
                    2,
                    if external {
                        "Save As…"
                    } else {
                        "Export as File…"
                    },
                ));
            }
            tools.push(overflow_menu(16, 91_016, "More actions", actions));
        }
        tools.push(icon_button(5, "settings", "Settings"));
        let mut header = row(1, tools)
            .layout(Layout {
                flow: Flow::Row,
                height: Length::Px(46.0),
                padding: 6.0,
                gap: 6.0,
                ..Default::default()
            })
            .class("header");
        header.paint_surface = true;
        header.layout.align = Align::Center;
        let mut body = vec![header];
        if self.view.page != Page::Settings {
            let strip = self.tab_controls();
            let books = TablerIcon::named("books", IconVariant::Outline).unwrap();
            let mut library =
                button(15, "").children(vec![books.element(91_015), label(17, "Library")]);
            library.layout.flow = Flow::Row;
            library.layout.align = Align::Center;
            library.layout.width = Length::Px(LIBRARY_TAB_WIDTH);
            library.layout.height = Length::Px(38.0);
            library.layout.gap = 4.0;
            library.semantics.name = Some("Library".into());
            library.semantics.role = Some(Role::Tab);
            library.semantics.current = self.view.page == Page::Overview;
            if self.view.page == Page::Overview {
                library = library.class("library-active");
            }
            let mut tab_list = row(18, vec![library, strip]).layout(Layout {
                flow: Flow::Row,
                height: Length::Px(38.0),
                gap: 0.0,
                ..Default::default()
            });
            tab_list.semantics.role = Some(Role::TabList);
            body.push(tab_list);
        }
        if editing {
            if self.view.find_open {
                body.push(
                    row(
                        72,
                        vec![
                            field(FIND.0, &self.view.find_query)?,
                            compact(73, "↑"),
                            compact(74, "↓"),
                            compact(75, "×"),
                        ],
                    )
                    .layout(Layout {
                        flow: Flow::Row,
                        height: Length::Px(40.0),
                        padding: 4.0,
                        gap: 4.0,
                        ..Default::default()
                    }),
                );
                body.push(
                    row(
                        76,
                        vec![
                            field(REPLACE.0, &self.view.replacement)?,
                            button(77, "Replace"),
                            button(78, "Replace all"),
                        ],
                    )
                    .layout(Layout {
                        flow: Flow::Row,
                        height: Length::Px(40.0),
                        padding: 4.0,
                        gap: 4.0,
                        ..Default::default()
                    }),
                );
            }
            if self.view.command_open {
                body.push(
                    row(
                        81,
                        vec![
                            {
                                let mut e = label(82, ":");
                                e.layout.width = Length::Px(16.0);
                                e
                            },
                            field(80, &self.view.command)?,
                        ],
                    )
                    .layout(Layout {
                        flow: Flow::Row,
                        height: Length::Px(40.0),
                        padding: 4.0,
                        ..Default::default()
                    }),
                );
            }
            // A focus/IME target only. The document is rendered by paint::document, never TextInput.
            let mut surface =
                Element::new(EDITOR.0, Widget::Button(String::new())).layout(Layout {
                    height: Length::Fill,
                    gap: 0.0,
                    ..Default::default()
                });
            surface.paint_surface = false;
            surface.cursor = Some(CursorShape::Text);
            body.push(surface);
        } else if self.view.page == Page::Overview {
            let mut search = field(SEARCH.0, &self.view.query)?;
            search.layout.width = Length::Px(260.0);
            search = search.class("library-search");
            let mut sort_label = label(54, "Sort by:");
            sort_label.layout.width = Length::Px(56.0);
            let mut sort_dropdown = Element::new(
                56,
                Widget::Dropdown {
                    label: match self.view.overview_sort {
                        OverviewSort::DateModified => "Date modified".into(),
                        OverviewSort::DateCreated => "Date created".into(),
                        OverviewSort::TitleAscending => "Title".into(),
                    },
                    items: vec![
                        MenuItem::option(1, "Date modified"),
                        MenuItem::option(2, "Date created"),
                        MenuItem::option(3, "Title"),
                    ],
                    selected: Some(match self.view.overview_sort {
                        OverviewSort::DateModified => 1,
                        OverviewSort::DateCreated => 2,
                        OverviewSort::TitleAscending => 3,
                    }),
                },
            )
            .class("sort-dropdown")
            .layout(Layout {
                width: Length::Px(180.0),
                height: Length::Px(34.0),
                ..Default::default()
            });
            sort_dropdown.style.text.align = TextAlign::Start;
            sort_dropdown.layout.padding = 6.0;
            let mut sort_controls = row(57, vec![sort_label, sort_dropdown]);
            sort_controls.layout.width = Length::Auto;
            body.push(
                row(
                    31,
                    vec![
                        {
                            let mut e = label(33, "Search");
                            e.layout.width = Length::Px(48.0);
                            e
                        },
                        search,
                        sort_controls,
                    ],
                )
                .layout(Layout {
                    flow: Flow::Row,
                    height: Length::Px(74.0),
                    align: Align::Center,
                    padding: 20.0,
                    ..Default::default()
                }),
            );
            if let Some(id) = &self.view.pending_delete {
                let title = self
                    .book
                    .notes
                    .iter()
                    .find(|note| &note.id == id)
                    .map(Note::title)
                    .unwrap_or_default();
                let mut message = label(90, format!("Delete “{title}”?\nThis cannot be undone."));
                message.style.text.wrap = knave_ui::TextWrap::WordOrGlyph;
                body.push(
                    Element::new(89, Widget::Popup { modal: true })
                        .children(vec![
                            message,
                            row(93, vec![button(91, "Delete"), button(92, "Cancel")]),
                        ])
                        .layout(Layout {
                            width: Length::Px(360.0),
                            height: Length::Px(150.0),
                            padding: 20.0,
                            gap: 20.0,
                            ..Default::default()
                        }),
                );
            }

            if self.view.rename_note.is_some() {
                let mut name = field(86, &self.view.rename_text)?;
                name.layout.width = Length::Fill;
                body.push(
                    Element::new(84, Widget::Popup { modal: true })
                        .children(vec![
                            label(85, "Rename note"),
                            name,
                            row(94, vec![button(87, "Save"), button(88, "Cancel")]),
                        ])
                        .layout(Layout {
                            width: Length::Px(360.0),
                            height: Length::Px(190.0),
                            padding: 20.0,
                            gap: 16.0,
                            ..Default::default()
                        }),
                );
            }
            let query = self.view.query.to_lowercase();
            if !query.is_empty() {
                for note in &mut self.book.notes {
                    if let Err(error) = self.storage.materialize(note) {
                        eprintln!("Note {} could not be searched: {error}", note.id);
                    }
                }
            }
            let external_cards: Vec<_> = self
                .book
                .files
                .iter()
                .map(|entry| {
                    let (text, created, updated) = match self.previews.get(entry) {
                        Some(Preview::Available {
                            text,
                            created,
                            modified,
                        }) => (text.as_str(), *created, *modified),
                        Some(Preview::Missing) => ("File missing", entry.added, entry.added),
                        Some(Preview::Unavailable) => {
                            ("File unavailable", entry.added, entry.added)
                        }
                        None => ("", entry.added, entry.added),
                    };
                    let location = entry.path.parent().unwrap_or(&entry.path).display();
                    Note {
                        id: entry.id.clone(),
                        text: format!("{location}\n{text}"),
                        created,
                        updated,
                        name: Some(entry.title()),
                        keep_empty: false,
                        indexed_title: None,
                    }
                })
                .collect();
            let mut notes: Vec<_> = self
                .book
                .notes
                .iter()
                .chain(external_cards.iter())
                .filter_map(|note| {
                    let title = note.title();
                    let title_lower = title.to_lowercase();
                    let matches = query.is_empty()
                        || title_lower.contains(&query)
                        || note.text.to_lowercase().contains(&query);
                    matches.then_some((note, title_lower))
                })
                .collect();
            notes.sort_by(|a, b| match self.view.overview_sort {
                OverviewSort::DateModified => {
                    b.0.updated
                        .cmp(&a.0.updated)
                        .then_with(|| a.0.id.cmp(&b.0.id))
                }
                OverviewSort::DateCreated => {
                    let a_created = if a.0.created == 0 {
                        a.0.updated
                    } else {
                        a.0.created
                    };
                    let b_created = if b.0.created == 0 {
                        b.0.updated
                    } else {
                        b.0.created
                    };
                    b_created.cmp(&a_created).then_with(|| a.0.id.cmp(&b.0.id))
                }
                OverviewSort::TitleAscending => a.1.cmp(&b.1).then_with(|| a.0.id.cmp(&b.0.id)),
            });
            self.view.results = notes.iter().map(|(note, _)| note.id.clone()).collect();
            self.view.overview_page = self
                .view
                .overview_page
                .min(notes.len().saturating_sub(1) / PAGE_SIZE);
            let cols = ((self.view.size[0] - 32.0) / 220.0).floor().max(1.0) as usize;
            let cards = notes
                .iter()
                .enumerate()
                .skip(self.view.overview_page * PAGE_SIZE)
                .take(PAGE_SIZE)
                .map(|(index, note)| {
                    let slot = (index % PAGE_SIZE) as u64;
                    let external = self.book.files.iter().any(|entry| entry.id == note.0.id);
                    let mut cover =
                        button(1000 + slot, "")
                            .class("document-cover")
                            .layout(Layout {
                                width: Length::Fill,
                                height: Length::Px(220.0),
                                padding: 16.0,
                                align: Align::Center,
                                ..Default::default()
                            });
                    let mut excerpt: String = note.0.text.chars().take(512).collect();
                    if excerpt.len() < note.0.text.len() {
                        excerpt.push('…');
                    }
                    let excerpt = excerpt.replace('\t', "    ");
                    let mut preview = label(10000 + slot, excerpt).class("document-preview");
                    preview.layout = Layout {
                        width: Length::Fill,
                        height: Length::Fill,
                        padding: 1.0,
                        ..Default::default()
                    };
                    preview.ellipsize = true;
                    preview.style.text.align = TextAlign::Start;
                    preview.style.text.wrap = knave_ui::TextWrap::WordOrGlyph;

                    let mut title = label(6000 + slot, note.0.title()).class("document-title");
                    title.layout.height = Length::Px(32.0);
                    title.layout.width = Length::Fill;
                    title.ellipsize = true;
                    title.style.text.wrap = knave_ui::TextWrap::WordOrGlyph;
                    let mut menu = overflow_menu(
                        2000 + slot,
                        93_000 + slot,
                        if external {
                            "File actions"
                        } else {
                            "Note actions"
                        },
                        if external {
                            vec![
                                MenuItem::option(5, "Locate File…"),
                                MenuItem::option(4, "Remove from Library"),
                            ]
                        } else {
                            vec![
                                MenuItem::option(1, "Rename…"),
                                MenuItem::option(2, "Export as File…"),
                                MenuItem::Separator,
                                MenuItem::option(3, "Delete…"),
                            ]
                        },
                    )
                    .class("document-menu");
                    menu.layout.width = Length::Px(32.0);
                    menu.layout.height = Length::Px(30.0);
                    let age = now().saturating_sub(note.0.updated) / 1000;
                    let modified = if age < 60 {
                        "Modified just now".to_owned()
                    } else if age < 3600 {
                        format!("Modified {} min ago", age / 60)
                    } else if age < 86400 {
                        format!("Modified {} hours ago", age / 3600)
                    } else {
                        format!("Modified {} days ago", age / 86400)
                    };
                    let icon = TablerIcon::named(
                        if external { "file-text" } else { "notebook" },
                        IconVariant::Outline,
                    )
                    .unwrap();
                    cover.semantics.name = Some(note.0.title());
                    cover.children = vec![
                        row(7000 + slot, vec![icon.element(92_000 + slot), title, menu]).layout(
                            Layout {
                                flow: Flow::Row,
                                height: Length::Px(34.0),
                                gap: 4.0,
                                ..Default::default()
                            },
                        ),
                        preview,
                        label(60000 + slot, modified)
                            .class("document-date")
                            .layout(Layout {
                                height: Length::Px(20.0),
                                ..Default::default()
                            }),
                    ];
                    cover.layout.flow = Flow::Column;
                    cover.layout.gap = 12.0;
                    cover
                })
                .collect::<Vec<_>>();
            let grid = panel(35, cards).layout(Layout {
                flow: Flow::Grid { columns: cols },
                padding: 20.0,
                gap: 24.0,
                ..Default::default()
            });
            body.push(
                Element::new(34, Widget::Scroll)
                    .layout(Layout {
                        height: Length::Fill,
                        ..Default::default()
                    })
                    .children(vec![grid]),
            );
            if self.view.results.is_empty() {
                self.view.message = Some(
                    if self.view.query.is_empty() {
                        "No documents yet. Choose + to start writing."
                    } else {
                        "No matching documents."
                    }
                    .into(),
                );
            }
            if self.view.results.len() > PAGE_SIZE {
                body.push(row(
                    36,
                    vec![
                        button(37, "Previous").enabled(self.view.overview_page > 0),
                        label(38, format!("Page {}", self.view.overview_page + 1)),
                        button(39, "Next").enabled(
                            (self.view.overview_page + 1) * PAGE_SIZE < self.view.results.len(),
                        ),
                    ],
                ));
            }
        } else {
            let settings = vec![
                heading(40, "Editor"),
                stepper(
                    41,
                    "Font size",
                    format!("{} px", self.settings.font_size),
                    46,
                    48,
                    self.settings.font_size > 10,
                    self.settings.font_size < 40,
                ),
                toggle(42, "Vim motions", self.settings.vim),
                toggle(43, "Show line numbers", self.settings.line_numbers),
                toggle(44, "Relative line numbers", self.settings.relative_numbers),
                toggle(49, "Insert spaces instead of tabs", self.settings.spaces),
                stepper(
                    50,
                    "Tab width",
                    self.settings.tab_width.to_string(),
                    52,
                    54,
                    self.settings.tab_width > 1,
                    self.settings.tab_width < 16,
                ),
                toggle(55, "Automatic indentation", self.settings.auto_indent),
                toggle(56, "Highlight current line", self.settings.highlight_line),
                toggle(57, "Show spaces and tabs", self.settings.show_whitespace),
                toggle(63, "Word wrap", self.settings.word_wrap),
                label(
                    58,
                    "Tab indents. Shift+Tab unindents. Ctrl+Tab switches documents.",
                ),
            ];
            let settings = panel(59, settings).layout(Layout {
                width: Length::Fill,
                padding: 24.0,
                gap: 8.0,
                ..Default::default()
            });
            body.push(
                Element::new(60, Widget::Scroll)
                    .layout(Layout {
                        height: Length::Fill,
                        ..Default::default()
                    })
                    .children(vec![settings]),
            );
        }
        let mut status = label(STATUS.0, self.status());
        status.style.text.font_size = 12.0;
        status.style.text.line_height = 18.0;
        let mut retry = button(61, "Retry save");
        retry.visible = self.save_error.is_some();
        retry.layout.height = Length::Px(24.0);
        body.push(row(62, vec![status, retry]).class("status").layout(Layout {
            flow: Flow::Row,
            height: Length::Px(28.0),
            padding: 4.0,
            gap: 4.0,
            ..Default::default()
        }));
        // Resolve extra editor roles through theme tokens without overriding widgets.
        for (id, class) in [
            (90001, "note-surface"),
            (90002, "note-selection"),
            (90003, "note-accent"),
            (90004, "note-on-accent"),
        ] {
            let mut probe = label(id, "").class(class);
            probe.visible = false;
            body.push(probe);
        }
        if let Some(prompt) = &self.view.file_prompt {
            let dialog = match prompt {
                FilePrompt::Close(id) => file_dialog(
                    format!(
                        "Save changes before closing?\n{}",
                        self.files[id].path.display()
                    ),
                    &[(102, "Save"), (103, "Discard"), (105, "Cancel")],
                ),
                FilePrompt::CloseWindow(ids) => {
                    let mut paths = ids
                        .iter()
                        .take(3)
                        .map(|id| self.files[id].path.display().to_string())
                        .collect::<Vec<_>>();
                    if ids.len() > 3 {
                        paths.push(format!("And {} more", ids.len() - 3));
                    }
                    file_dialog(
                        format!(
                            "Save changes to {} files before closing?\n{}",
                            ids.len(),
                            paths.join("\n")
                        ),
                        &[(102, "Save All"), (103, "Discard All"), (105, "Cancel")],
                    )
                }
                FilePrompt::Conflict(id) => file_dialog(
                    format!(
                        "The file changed, moved, or disappeared. Reload discards your edits. Overwrite writes your edits to the file.\n{}",
                        self.files[id].path.display()
                    ),
                    &[
                        (102, "Reload from Disk"),
                        (103, "Overwrite"),
                        (104, "Save As…"),
                        (105, "Cancel"),
                    ],
                ),
            };
            body.push(dialog);
        } else if let Some((_, snapshot)) = self.recoverable.front() {
            let question = if self.recovery_after_crash {
                "Notes closed unexpectedly. Recover unsaved edits?"
            } else {
                "Recover unsaved edits?"
            };
            body.push(file_dialog(
                format!("{question}\n{}", snapshot.file.path.display()),
                &[(106, "Recover"), (107, "Discard"), (108, "Later")],
            ));
        }
        let mut root = panel(0, body).layout(Layout {
            height: Length::Fill,
            gap: 0.0,
            ..Default::default()
        });
        root.paint_surface = true;
        self.view.scene.replace(root)?;
        self.view.scene.set_stylesheet(":root { color: var(--text); font-weight: 400; } menu-item { font-weight: 400; } .header { background-color: var(--surface-raised); border-bottom: 1px solid var(--border); } .library-active { background-color: var(--surface-selected); color: var(--accent); border-bottom: 2px solid var(--accent); } .status { background-color: var(--surface-raised); border-top: 1px solid var(--border); } .note-surface { color: var(--surface-raised); } .note-selection { color: var(--surface-selected); } .note-accent { color: var(--accent); } .note-on-accent { color: var(--on-accent); } .status { color: var(--text-secondary); } .document-cover { background-color: var(--surface-raised); border: 1px solid var(--border); border-radius: 12px; color: var(--text-secondary); font-size: 15px; font-weight: 400; } .document-cover:hover { background-color: var(--surface-selected); border-color: var(--accent); color: var(--accent); } .document-preview { color: var(--text-secondary); font-family: monospace; font-size: 13px; line-height: 19px; font-weight: 400; } .document-cover:hover .document-preview { color: var(--accent); } .document-cover:hover .document-title { color: var(--accent); } .document-date { color: var(--text-secondary); font-size: 11px; } .document-cover:focus { outline: 2px solid var(--accent); } .document-title { font-size: 13px; line-height: 16px; font-weight: 400; } .document-menu { font-weight: 400; background-color: transparent; border-width: 0px; box-shadow: none; } .document-menu:hover { background-color: var(--surface-hover); } .library-search { border: 1px solid var(--border-strong); background-color: var(--surface-raised); border-radius: 6px; } .sort-dropdown { text-align: left; } text-input { text-align: left; }")?;
        if self.view.file_prompt.is_some() || !self.recoverable.is_empty() {
            self.view.scene.open_popup(ElementId(100))?;
            self.view
                .scene
                .set_focus(Some(ElementId(if self.view.file_prompt.is_some() {
                    105
                } else {
                    108
                })))?;
        }
        if self.view.rename_note.is_some() {
            self.view.scene.open_popup(ElementId(84))?;
            self.view.scene.set_focus(Some(ElementId(86)))?;
        }
        if self.view.pending_delete.is_some() {
            self.view.scene.open_popup(ElementId(89))?;
            self.view.scene.set_focus(Some(ElementId(92)))?;
        }
        Ok(())
    }
    fn paint_tabs(&mut self, list: &mut DisplayList) {
        let palette = paint::Palette::from_scene(&self.view.scene);
        let Some(area) = self.view.scene.bounds(STRIP) else {
            return;
        };
        paint::rect(list, area, palette.bg, area);
        let width = self.tab_width(area.width);
        self.view.tab_hits.clear();
        for (index, id) in self.book.tabs.iter().enumerate() {
            let r = Rect::new(
                area.x + index as f32 * width - self.view.tab_scroll,
                area.y,
                width,
                area.height,
            );
            let file = self.files.get(id);
            let base = DOCUMENT_TAB_BASE + index as u64 * 3;
            let close = Rect::new(r.x + r.width - 29.0, r.y + 6.0, 24.0, 26.0);
            let bookmark =
                file.map(|_| Rect::new(close.x - 24.0, close.y, close.width, close.height));
            self.view.tab_hits.push(TabHit {
                id: id.clone(),
                rect: r,
                close,
                bookmark,
            });
            if r.x + r.width < area.x || r.x > area.x + area.width {
                continue;
            }
            let active = self.view.page == Page::Editor && self.view.active.as_ref() == Some(id);
            paint::rect(
                list,
                r,
                if active {
                    palette.selected
                } else {
                    palette.surface
                },
                area,
            );
            paint::rect(
                list,
                Rect::new(r.x + r.width - 1.0, r.y, 1.0, r.height),
                palette.surface,
                area,
            );
            if active {
                paint::rect(
                    list,
                    Rect::new(r.x, r.y + r.height - 3.0, r.width, 3.0),
                    palette.accent,
                    area,
                );
            }
            let title = self.document_tab_title(id);
            let title = if file.is_some_and(|file| file.dirty(&self.tabs[id].doc.text)) {
                format!("• {title}")
            } else {
                title
            };
            let label_width = width - if file.is_some() { 87.0 } else { 63.0 };
            let count = ((label_width - 9.0) / 7.5).max(1.0) as usize;
            let title = if title.chars().count() > count {
                format!(
                    "{}…",
                    title
                        .chars()
                        .take(count.saturating_sub(1))
                        .collect::<String>()
                )
            } else {
                title
            };
            let mut style = paint::style(
                13.0,
                false,
                if active {
                    palette.accent
                } else {
                    palette.muted
                },
            );
            style.align = TextAlign::Center;
            paint::text(
                list,
                Rect::new(r.x + 28.0, r.y + 9.0, label_width, 22.0),
                title,
                style,
                area,
            );
            paint_icon(
                list,
                if file.is_some() {
                    "file-text"
                } else {
                    "notebook"
                },
                IconVariant::Outline,
                Rect::new(r.x + 8.0, r.y + 11.0, 16.0, 16.0),
                palette.muted,
                area,
            );
            let close_state = self.view.scene.interaction(ElementId(base + 1));
            if close_state.is_some_and(|state| state.hovered || state.focused) {
                paint::rect(list, close, palette.selected, area);
            }
            if self
                .view
                .scene
                .interaction(ElementId(base))
                .is_some_and(|state| state.focused)
            {
                paint::rect(
                    list,
                    Rect::new(r.x, r.y, r.width - 29.0, 2.0),
                    palette.accent,
                    area,
                );
            }
            if let Some((file, bookmark)) = file.zip(bookmark) {
                let bookmarked = self.bookmark_index(file).is_some();
                if self
                    .view
                    .scene
                    .interaction(ElementId(base + 2))
                    .is_some_and(|state| state.hovered || state.focused)
                {
                    paint::rect(list, bookmark, palette.selected, area);
                }
                paint_icon(
                    list,
                    "bookmark",
                    if bookmarked {
                        IconVariant::Filled
                    } else {
                        IconVariant::Outline
                    },
                    Rect::new(bookmark.x + 4.0, bookmark.y + 5.0, 16.0, 16.0),
                    palette.muted,
                    area,
                );
            }
            paint_icon(
                list,
                "x",
                IconVariant::Outline,
                Rect::new(close.x + 4.0, close.y + 5.0, 16.0, 16.0),
                palette.muted,
                area,
            );
        }
    }
}
impl Application for Notes {
    fn shutdown(&mut self) -> AppResult {
        if let Some(recovery) = &self.recovery {
            recovery.end_session()?;
        }
        Ok(())
    }

    fn activate_window(&mut self, id: u64) -> AppResult {
        self.switch_window(id)
    }

    fn open_window_files(&mut self, paths: &[PathBuf]) -> AppResult {
        for path in paths {
            self.open_cli_file(path)?;
        }
        Ok(())
    }

    fn window_removed(&mut self, id: u64) {
        self.forget_window(id);
    }

    fn document_generation(&self) -> u64 {
        self.generation
    }

    fn modifiers_changed(&mut self, modifiers: ModifiersState) {
        self.view.modifiers = modifiers;
    }
    fn scene(&mut self) -> &mut Scene {
        &mut self.view.scene
    }
    fn resize(&mut self, size: [f32; 2]) -> AppResult {
        let changed =
            ((self.view.size[0] - 32.0) / 220.0).floor() != ((size[0] - 32.0) / 220.0).floor();
        let width_changed = (self.view.size[0] - size[0]).abs() > 0.5 && !self.book.tabs.is_empty();
        self.view.size = size;
        if (changed && self.view.page == Page::Overview)
            || (width_changed && self.view.page != Page::Settings)
        {
            self.rebuild()?;
        }
        Ok(())
    }
    fn paint(&mut self, list: &mut DisplayList, measure: &mut dyn TextMeasurer) {
        if self.view.page != Page::Settings {
            self.paint_tabs(list);
        }
        if self.view.page == Page::Editor
            && let Some(area) = self.view.scene.bounds(EDITOR)
            && let Some(id) = self.view.active.clone()
            && let Some(tab) = self.tabs.get_mut(&id)
        {
            let normal = tab.vim.as_ref().is_some_and(|v| !v.insert_mode());
            self.view.metrics = paint::document(
                list,
                measure,
                &mut tab.doc,
                &self.settings,
                area,
                (self.view.focused, normal, &self.view.preedit),
                paint::Palette::from_scene(&self.view.scene),
            );
        }
    }
    fn effects(&mut self) -> Vec<Action> {
        std::mem::take(&mut self.view.effects)
    }
    fn close_requested(&mut self) -> AppResult<bool> {
        self.save();
        self.refresh_status()?;
        if self.save_error.is_some() {
            return Ok(false);
        }
        self.view.closing = true;
        self.view.save_all = false;
        self.close_progress()
    }

    fn take_exit_request(&mut self) -> bool {
        std::mem::take(&mut self.view.exit_requested)
    }

    fn next_checkpoint(&self) -> Option<Instant> {
        self.checkpoints
            .values()
            .copied()
            .chain(self.previews.deadline())
            .min()
    }

    fn checkpoint(&mut self, now: Instant) -> AppResult {
        if self
            .previews
            .deadline()
            .is_some_and(|deadline| deadline <= now)
            && self.previews.poll(now, &self.book.files)
        {
            self.generation = self.generation.wrapping_add(1);
            if self.view.page == Page::Overview {
                self.rebuild()?;
            }
        }
        let Some(recovery) = &self.recovery else {
            return Ok(());
        };
        let due: Vec<_> = self
            .checkpoints
            .iter()
            .filter(|(_, deadline)| **deadline <= now)
            .map(|(id, _)| id.clone())
            .collect();
        for id in due {
            let file = &self.files[&id];
            let text = &self.tabs[&id].doc.text;
            let result = if file.dirty(text) {
                recovery.checkpoint(&id, file, text)
            } else {
                recovery.clear(&id)
            };
            match result {
                Ok(()) => {
                    self.checkpoints.remove(&id);
                }
                Err(error) => {
                    self.checkpoints.insert(id, now + Duration::from_secs(5));
                    self.view.message = Some(format!("Recovery checkpoint failed: {error}"));
                }
            }
        }
        self.refresh_status()
    }

    fn ime_state(&self) -> Option<TextInputState> {
        if self.view.page != Page::Editor || !self.view.focused {
            return None;
        }
        let tab = self.current()?;
        Some(TextInputState {
            purpose: TextPurpose::Multiline,
            id: EDITOR,
            text: tab.doc.text.clone(),
            cursor: tab.doc.cursor,
            anchor: tab.doc.anchor,
            cursor_rectangle: self
                .view
                .metrics
                .caret(&tab.doc, self.settings.tab_width as usize),
        })
    }
    fn clipboard_error(&mut self, error: &dyn std::error::Error) {
        self.view.message = Some(format!("Clipboard: {error}"));
        let _ = self.refresh_status();
    }
    fn action(&mut self, action: Action) -> AppResult {
        match action {
            Action::Activated(id) => match id.0 {
                2 if self.view.page == Page::Settings => self.leave_settings()?,
                2 => {
                    let return_to_note =
                        self.view.page == Page::Overview && self.view.active.is_some();
                    self.view.page = if return_to_note {
                        Page::Editor
                    } else {
                        Page::Overview
                    };
                    self.view.focused = false;
                    self.view.message = None;
                    self.rebuild()?;
                    if return_to_note {
                        self.focus_editor()?;
                    }
                }
                4 => self.create()?,
                5 => {
                    if self.view.page != Page::Settings {
                        self.view.settings_return = Some(self.view.page);
                    }
                    self.view.page = Page::Settings;
                    self.view.focused = false;
                    self.view.message = None;
                    self.rebuild()?;
                }
                9 => self.pick_file()?,
                12 => {
                    self.save_current()?;
                }
                13 => self.import()?,
                15 => {
                    self.previews.start(&self.book.files);
                    self.view.page = Page::Overview;
                    self.view.focused = false;
                    self.view.message = None;
                    self.rebuild()?;
                }
                10 => self.export()?,
                11 => {
                    self.view.find_open = !self.view.find_open;
                    self.rebuild()?;
                    if self.view.find_open {
                        self.view.focused = false;
                        self.view.scene.set_focus(Some(FIND))?;
                    } else {
                        self.focus_editor()?;
                    }
                }
                37 => {
                    self.view.overview_page = self.view.overview_page.saturating_sub(1);
                    self.rebuild()?;
                }
                39 => {
                    self.view.overview_page += 1;
                    self.rebuild()?;
                }
                46 | 48 => {
                    self.settings.font_size = if id.0 == 46 {
                        self.settings.font_size.saturating_sub(1).max(10)
                    } else {
                        (self.settings.font_size + 1).min(40)
                    };
                    self.save();
                    self.rebuild()?;
                }
                52 | 54 => {
                    self.settings.tab_width = if id.0 == 52 {
                        self.settings.tab_width.saturating_sub(1).max(1)
                    } else {
                        (self.settings.tab_width + 1).min(16)
                    };
                    self.save();
                    self.rebuild()?;
                }
                102..=104 => self.resolve_file_prompt(id.0)?,
                105 => self.cancel_file_prompt()?,
                106..=108 => {
                    if let Some((snapshot_id, snapshot)) = self.recoverable.pop_front() {
                        match id.0 {
                            106 => self.recover_file(snapshot_id, snapshot)?,
                            107 => {
                                self.clear_recovery(&snapshot_id)?;
                            }
                            _ => {}
                        }
                    }
                    self.rebuild()?;
                }
                91 => {
                    if let Some(id) = self.view.pending_delete.take() {
                        self.delete_note(&id)?;
                    }
                }
                92 => {
                    self.view.pending_delete = None;
                    self.rebuild()?;
                }
                87 => self.finish_rename()?,
                88 => {
                    self.view.rename_note = None;
                    self.rebuild()?;
                }
                61 => {
                    self.save();
                    self.refresh_status()?;
                }
                73 => self.find(true)?,
                74 => self.find(false)?,
                75 => {
                    self.view.find_open = false;
                    self.rebuild()?;
                    self.focus_editor()?;
                }
                77 => {
                    let query = self.view.find_query.clone();
                    let replacement = self.view.replacement.clone();
                    if let Some(tab) = self.current_mut()
                        && tab.doc.selected() == query
                    {
                        tab.doc.insert(&replacement, false);
                    }
                    self.reset_vim_from_doc();
                    self.changed()?;
                    self.find(false)?;
                }
                78 => {
                    let query = self.view.find_query.clone();
                    let replacement = self.view.replacement.clone();
                    let count = self
                        .current_mut()
                        .map_or(0, |tab| tab.doc.replace_all(&query, &replacement));
                    self.reset_vim_from_doc();
                    self.view.message = Some(format!("Replaced {count} matches"));
                    self.changed()?;
                }
                value if value >= DOCUMENT_TAB_BASE => {
                    let offset = value - DOCUMENT_TAB_BASE;
                    if let Some(tab) = self.book.tabs.get((offset / 3) as usize).cloned() {
                        match offset % 3 {
                            0 => self.open(tab)?,
                            1 => self.close(&tab)?,
                            _ => self.bookmark_file(&tab)?,
                        }
                    }
                }
                value if (1000..1000 + PAGE_SIZE as u64).contains(&value) => {
                    if let Some(id) = self
                        .view
                        .results
                        .get(self.view.overview_page * PAGE_SIZE + (value - 1000) as usize)
                        .cloned()
                    {
                        self.open_library_entry(id)?;
                    }
                }
                _ => {}
            },
            Action::Selected(ElementId(16), 1) => self.import()?,
            Action::Selected(ElementId(16), 2) => {
                if let Some(id) = self
                    .view
                    .active
                    .clone()
                    .filter(|id| self.files.contains_key(id))
                {
                    self.save_file_as(&id)?;
                } else {
                    self.export()?;
                }
            }
            Action::Selected(ElementId(56), selected) => {
                self.view.overview_sort = match selected {
                    2 => OverviewSort::DateCreated,
                    3 => OverviewSort::TitleAscending,
                    _ => OverviewSort::DateModified,
                };
                self.view.overview_page = 0;
                self.rebuild()?;
            }
            Action::Selected(owner, operation)
                if (2000..2000 + PAGE_SIZE as u64).contains(&owner.0) =>
            {
                if let Some(id) = self
                    .view
                    .results
                    .get(self.view.overview_page * PAGE_SIZE + (owner.0 - 2000) as usize)
                    .cloned()
                {
                    match operation {
                        1 => {
                            self.view.rename_text = self
                                .book
                                .notes
                                .iter()
                                .find(|n| n.id == id)
                                .map(Note::title)
                                .unwrap_or_default();
                            self.view.pending_delete = None;
                            self.view.rename_note = Some(id);
                            self.rebuild()?;
                            self.view.scene.set_focus(Some(ElementId(86)))?;
                        }
                        2 => self.export_note(&id)?,
                        4 => {
                            self.book.files.retain(|entry| entry.id != id);
                            self.previews.start(&self.book.files);
                            self.save();
                            self.rebuild()?;
                        }
                        5 => self.locate_library_entry(&id)?,
                        3 => {
                            self.view.rename_note = None;
                            self.view.pending_delete = Some(id);
                            self.rebuild()?;
                            self.view.scene.set_focus(Some(ElementId(92)))?;
                        }
                        _ => {}
                    }
                }
            }
            Action::PopupClosed(ElementId(100)) => {
                if self.view.file_prompt.is_some() {
                    self.cancel_file_prompt()?;
                } else {
                    self.recoverable.pop_front();
                    self.rebuild()?;
                }
            }
            Action::PopupClosed(ElementId(84)) => {
                self.view.rename_note = None;
                self.rebuild()?;
            }
            Action::PopupClosed(ElementId(89)) => {
                self.view.pending_delete = None;
                self.rebuild()?;
            }
            Action::TextChanged(ElementId(86), text) => self.view.rename_text = text,
            Action::Submitted(ElementId(86), _) => self.finish_rename()?,
            Action::TextChanged(SEARCH, text) => {
                let edit = self.view.scene.widget(SEARCH).cloned();
                self.view.query = text;
                self.view.overview_page = 0;
                self.view.message = None;
                self.rebuild()?;
                if let Some(edit) = edit {
                    self.view.scene.set_widget(SEARCH, edit)?;
                    self.view.scene.set_focus(Some(SEARCH))?;
                }
            }
            Action::TextChanged(FIND, text) => {
                self.view.find_query = text;
                self.view.message = None;
                self.refresh_status()?;
            }
            Action::TextChanged(REPLACE, text) => {
                self.view.replacement = text;
            }
            Action::Submitted(FIND, _) => {
                self.find(self.view.search_backwards)?;
                if self.settings.vim {
                    self.focus_editor()?;
                }
            }
            Action::TextChanged(ElementId(80), text) => self.view.command = text,
            Action::Submitted(ElementId(80), _) => self.command()?,
            Action::Toggled(id, value) => {
                match id.0 {
                    42 => {
                        self.settings.vim = value;
                        for tab in self.tabs.values_mut() {
                            tab.doc.break_group();
                            tab.vim = value.then(|| {
                                VimEditor::with_store(
                                    &tab.doc.text,
                                    tab.doc.cursor,
                                    self.modal_store.clone(),
                                )
                            });
                        }
                    }
                    43 => self.settings.line_numbers = value,
                    44 => self.settings.relative_numbers = value,
                    49 => self.settings.spaces = value,
                    55 => self.settings.auto_indent = value,
                    56 => self.settings.highlight_line = value,
                    57 => self.settings.show_whitespace = value,
                    63 => {
                        self.settings.word_wrap = value;
                        for tab in self.tabs.values_mut() {
                            tab.doc.scroll_row = 0;
                            tab.doc.scroll_col = 0;
                        }
                    }
                    _ => {}
                }
                self.save();
                self.rebuild()?;
            }
            _ => {}
        }
        Ok(())
    }
    fn input(&mut self, input: &Input) -> AppResult<bool> {
        if self.view.file_prompt.is_some() || !self.recoverable.is_empty() {
            return Ok(false);
        }
        match input {
            Input::FocusLost => {
                self.view.dragging = false;
                self.view.scroll_drag = false;
                self.view.horizontal_drag = false;
                self.view.preedit.clear();
                self.view.modifiers = ModifiersState::empty();
                return Ok(false);
            }
            Input::Key { modifiers, .. } => {
                self.view.modifiers = ModifiersState::empty();
                if modifiers.shift {
                    self.view.modifiers |= ModifiersState::SHIFT;
                }
                if modifiers.control {
                    self.view.modifiers |= ModifiersState::CONTROL;
                }
                if modifiers.alt {
                    self.view.modifiers |= ModifiersState::ALT;
                }
                if modifiers.logo {
                    self.view.modifiers |= ModifiersState::SUPER;
                }
                if self.view.page == Page::Editor && self.view.focused {
                    return Ok(true);
                }
            }
            _ => {}
        }
        if let Input::Scroll { position, delta } = input
            && self
                .view
                .scene
                .bounds(STRIP)
                .is_some_and(|area| paint::contains(area, *position))
        {
            self.view.tab_scroll += if delta[0] != 0.0 { delta[0] } else { delta[1] };
            self.rebuild()?;
            return Ok(true);
        }
        if let Input::PointerDown(point) = input
            && let Some(area) = self.view.scene.bounds(STRIP)
            && paint::contains(area, *point)
        {
            if let Some(hit) = self
                .view
                .tab_hits
                .iter()
                .find(|hit| paint::contains(hit.rect, *point))
                .cloned()
            {
                if paint::contains(hit.close, *point) {
                    self.close(&hit.id)?;
                } else if hit
                    .bookmark
                    .is_some_and(|bookmark| paint::contains(bookmark, *point))
                {
                    self.bookmark_file(&hit.id)?;
                } else {
                    self.open(hit.id.clone())?;
                    self.view.tab_drag = Some((hit.id, point[0]));
                }
            }
            return Ok(true);
        }
        if self.view.page != Page::Editor {
            return Ok(false);
        }
        match input {
            Input::Preedit(text) => {
                if self.view.focused {
                    self.view.preedit = text.clone();
                    return Ok(true);
                }
            }
            Input::PointerDown(p) => {
                self.view.pointer = *p;
                let area = self.view.scene.bounds(EDITOR).unwrap_or_default();
                if !paint::contains(area, *p) {
                    self.view.focused = false;
                    return Ok(false);
                }
                self.focus_editor()?;
                self.view.message = None;
                self.view.preedit.clear();
                if !self.settings.word_wrap && p[1] > area.y + area.height - 10.0 {
                    self.view.horizontal_drag = true;
                    return Ok(true);
                }
                if p[0] > area.x + area.width - 10.0 {
                    self.view.scroll_drag = true;
                    return Ok(true);
                }
                let clicks = self
                    .view
                    .last_click
                    .filter(|(t, last, _)| {
                        t.elapsed().as_millis() < 400
                            && (last[0] - p[0]).abs() < 5.0
                            && (last[1] - p[1]).abs() < 5.0
                    })
                    .map_or(1, |(_, _, count)| count % 3 + 1);
                self.view.last_click = Some((Instant::now(), *p, clicks));
                let metrics = self.view.metrics;
                let width = self.settings.tab_width as usize;
                let extend = self.view.modifiers.shift_key();
                self.view.gutter_drag = p[0] < area.x + metrics.gutter;
                let gutter = self.view.gutter_drag;
                if let Some(tab) = self.current_mut() {
                    let byte = metrics.hit(&tab.doc, *p, width);
                    if gutter || clicks == 3 {
                        let row = tab.doc.row_at(byte);
                        tab.doc.select_line(row);
                    } else if clicks == 2 {
                        tab.doc.select_word(byte);
                    } else {
                        tab.doc.place(byte, extend);
                    }
                    if let Some(vim) = &mut tab.vim {
                        let _ = vim.mouse_selection(&tab.doc.text, tab.doc.cursor, tab.doc.anchor);
                    }
                }
                self.view.dragging = true;
                self.refresh_status()?;
                return Ok(true);
            }
            Input::PointerMove(p) => {
                self.view.pointer = *p;
                if let Some((id, start)) = self.view.tab_drag.clone() {
                    if (p[0] - start).abs() > 12.0
                        && let Some(target) = self
                            .view
                            .tab_hits
                            .iter()
                            .position(|h| paint::contains(h.rect, *p))
                        && let Some(from) = self.book.tabs.iter().position(|t| t == &id)
                        && target != from
                    {
                        let id = self.book.tabs.remove(from);
                        self.book.tabs.insert(target, id.clone());
                        self.view.tab_drag = Some((id, p[0]));
                        self.save();
                        self.rebuild()?;
                    }
                    return Ok(true);
                }
                if self.view.horizontal_drag {
                    let metrics = self.view.metrics;
                    let width = self.settings.tab_width as usize;
                    if let Some(tab) = self.current_mut() {
                        let longest = (tab.doc.scroll_row
                            ..(tab.doc.scroll_row + metrics.rows).min(tab.doc.lines()))
                            .map(|r| columns(tab.doc.line(r), width))
                            .max()
                            .unwrap_or(0);
                        let fraction = ((p[0] - metrics.area.x - metrics.gutter)
                            / (metrics.area.width - metrics.gutter))
                            .clamp(0.0, 1.0);
                        tab.doc.scroll_col =
                            (fraction * longest.saturating_sub(metrics.cols) as f32) as usize;
                    }
                    return Ok(true);
                }
                if self.view.scroll_drag {
                    let metrics = self.view.metrics;
                    let width = self.settings.tab_width as usize;
                    if let Some(tab) = self.current_mut() {
                        let fraction =
                            ((p[1] - metrics.area.y) / metrics.area.height).clamp(0.0, 1.0);
                        tab.doc.scroll_row = (fraction
                            * metrics
                                .total_rows(&tab.doc, width)
                                .saturating_sub(metrics.rows) as f32)
                            as usize;
                    }
                    return Ok(true);
                }
                if self.view.dragging {
                    let metrics = self.view.metrics;
                    let width = self.settings.tab_width as usize;
                    let gutter = self.view.gutter_drag;
                    if let Some(tab) = self.current_mut() {
                        if p[1] < metrics.area.y {
                            tab.doc.scroll_row = tab.doc.scroll_row.saturating_sub(1);
                        } else if p[1] > metrics.area.y + metrics.area.height {
                            tab.doc.scroll_row = (tab.doc.scroll_row + 1)
                                .min(metrics.total_rows(&tab.doc, width).saturating_sub(1));
                        }
                        let byte = metrics.hit(&tab.doc, *p, width);
                        let anchor = tab.doc.anchor;
                        if gutter {
                            let row = tab.doc.row_at(byte);
                            tab.doc.cursor = tab.doc.start(row + 1).max(tab.doc.end(row));
                            tab.doc.anchor = anchor;
                        } else {
                            tab.doc.place(byte, true);
                        }
                        if let Some(vim) = &mut tab.vim {
                            let _ =
                                vim.mouse_selection(&tab.doc.text, tab.doc.cursor, tab.doc.anchor);
                        }
                    }
                    self.refresh_status()?;
                    return Ok(true);
                }
            }
            Input::PointerUp(_) => {
                if self.view.dragging
                    || self.view.scroll_drag
                    || self.view.horizontal_drag
                    || self.view.tab_drag.is_some()
                {
                    self.view.dragging = false;
                    self.view.scroll_drag = false;
                    self.view.horizontal_drag = false;
                    self.view.tab_drag = None;
                    return Ok(true);
                }
            }
            Input::Scroll { position, delta } => {
                if self
                    .view
                    .scene
                    .bounds(EDITOR)
                    .is_some_and(|r| paint::contains(r, *position))
                {
                    let metrics = self.view.metrics;
                    let horizontal = self.view.modifiers.shift_key();
                    let width = self.settings.tab_width as usize;
                    if let Some(tab) = self.current_mut() {
                        if (horizontal || delta[0] != 0.0) && !metrics.wrap {
                            let amount = if delta[0] != 0.0 { delta[0] } else { delta[1] };
                            let longest = (tab.doc.scroll_row
                                ..(tab.doc.scroll_row + metrics.rows).min(tab.doc.lines()))
                                .map(|r| columns(tab.doc.line(r), width))
                                .max()
                                .unwrap_or(0);
                            tab.doc.scroll_col = tab
                                .doc
                                .scroll_col
                                .saturating_add_signed((amount / metrics.cell) as isize)
                                .min(longest.saturating_sub(metrics.cols));
                        } else {
                            let step = (delta[1] / metrics.line).round() as isize;
                            tab.doc.scroll_row = tab
                                .doc
                                .scroll_row
                                .saturating_add_signed(if step == 0 {
                                    delta[1].signum() as isize
                                } else {
                                    step
                                })
                                .min(
                                    metrics
                                        .total_rows(&tab.doc, width)
                                        .saturating_sub(metrics.rows),
                                );
                        }
                    }
                    return Ok(true);
                }
            }
            Input::PasteTo { target, text } if *target == EDITOR => {
                self.focus_editor()?;
                if let Some(tab) = self.current_mut() {
                    tab.doc.break_group();
                }
                self.text(text)?;
                if let Some(tab) = self.current_mut() {
                    tab.doc.break_group();
                }
                return Ok(true);
            }
            _ => {}
        }
        Ok(false)
    }
    fn key(&mut self, key: &NativeKey, modifiers: ModifiersState) -> AppResult<bool> {
        if self.view.file_prompt.is_some() || !self.recoverable.is_empty() {
            return Ok(false);
        }
        self.view.modifiers = modifiers;
        if self.view.page == Page::Settings && matches!(key, NativeKey::Named(NamedKey::Escape)) {
            self.leave_settings()?;
            return Ok(true);
        }
        let control = modifiers.control_key() || modifiers.super_key();
        let shift = modifiers.shift_key();
        if control {
            if let NativeKey::Character(c) = key {
                match c.to_lowercase().as_str() {
                    "n" | "t" => {
                        self.create()?;
                        return Ok(true);
                    }
                    "o" => {
                        self.pick_file()?;
                        return Ok(true);
                    }
                    "s" => {
                        if shift && self.view.page != Page::Editor {
                            return Ok(true);
                        }
                        if shift {
                            if let Some(id) = self
                                .view
                                .active
                                .clone()
                                .filter(|id| self.files.contains_key(id))
                            {
                                self.save_file_as(&id)?;
                            } else {
                                self.export()?;
                            }
                        } else {
                            self.save_current()?;
                        }
                        return Ok(true);
                    }
                    "w" => {
                        if self.view.page == Page::Editor
                            && let Some(id) = self.view.active.clone()
                        {
                            self.close(&id)?;
                        }
                        return Ok(true);
                    }
                    "f" | "h" if self.view.page == Page::Editor => {
                        self.view.find_open = true;
                        self.view.focused = false;
                        self.rebuild()?;
                        self.view
                            .scene
                            .set_focus(Some(if c.eq_ignore_ascii_case("h") {
                                REPLACE
                            } else {
                                FIND
                            }))?;
                        return Ok(true);
                    }
                    "+" | "=" | "-" | "0" => {
                        self.settings.font_size = match c.as_str() {
                            "-" => self.settings.font_size.saturating_sub(1).max(10),
                            "0" => 16,
                            _ => (self.settings.font_size + 1).min(40),
                        };
                        self.save();
                        self.refresh_status()?;
                        return Ok(true);
                    }
                    _ => {}
                }
            }
            if matches!(
                key,
                NativeKey::Named(NamedKey::Tab | NamedKey::PageDown | NamedKey::PageUp)
            ) {
                self.switch_tab(
                    if shift || matches!(key, NativeKey::Named(NamedKey::PageUp)) {
                        -1
                    } else {
                        1
                    },
                )?;
                return Ok(true);
            }
        }
        if self.view.page != Page::Editor {
            return Ok(false);
        }
        if matches!(key, NativeKey::Named(NamedKey::Escape)) && self.view.command_open {
            self.view.command_open = false;
            self.view.command.clear();
            self.rebuild()?;
            self.focus_editor()?;
            return Ok(true);
        }
        if matches!(key, NativeKey::Named(NamedKey::Escape)) && self.view.find_open {
            self.view.find_open = false;
            self.rebuild()?;
            self.focus_editor()?;
            return Ok(true);
        }
        if !self.view.focused {
            return Ok(false);
        }
        self.view.message = None;
        let vim = self.settings.vim;
        if control && let NativeKey::Character(c) = key {
            let c = c.to_lowercase();
            let clipboard = !vim || shift;
            if clipboard && matches!(c.as_str(), "c" | "x" | "v") {
                if c == "v" {
                    self.view.effects.push(Action::RequestPaste(EDITOR));
                } else if let Some(tab) = self.current() {
                    let selected = tab.doc.selected().to_owned();
                    if !selected.is_empty() {
                        self.view.effects.push(Action::Copy(selected));
                        if c == "x" {
                            let settings = self.settings.clone();
                            self.current_mut()
                                .unwrap()
                                .doc
                                .delete(false, false, &settings);
                            self.reset_vim_from_doc();
                            self.changed()?;
                        }
                    }
                }
                return Ok(true);
            }
            if !vim {
                match c.as_str() {
                    "a" => {
                        self.current_mut().unwrap().doc.select_all();
                        self.changed()?;
                        return Ok(true);
                    }
                    "z" | "y" => {
                        self.current_mut().unwrap().doc.undo(c == "y" || shift);
                        self.changed()?;
                        return Ok(true);
                    }
                    _ => {}
                }
            }
        }
        let settings = self.settings.clone();
        let rows = self.view.metrics.rows as isize;
        if vim
            && self
                .current()
                .and_then(|t| t.vim.as_ref())
                .is_some_and(|v| v.mode() == modalkit::env::vim::VimMode::Normal)
            && !control
            && let NativeKey::Character(c) = key
        {
            match c.as_str() {
                "/" | "?" => {
                    self.view.search_backwards = c == "?";
                    self.view.find_open = true;
                    self.view.focused = false;
                    self.rebuild()?;
                    self.view.scene.set_focus(Some(FIND))?;
                    return Ok(true);
                }
                "n" | "N" => {
                    self.find(self.view.search_backwards ^ (c == "N"))?;
                    return Ok(true);
                }
                ":" => {
                    self.view.command_open = true;
                    self.view.focused = false;
                    self.rebuild()?;
                    self.view.scene.set_focus(Some(ElementId(80)))?;
                    return Ok(true);
                }
                _ => {}
            }
        }
        if vim {
            // Indentation policies apply in Insert mode; modal motions/operators remain modalkit's.
            let insert = self
                .current()
                .and_then(|t| t.vim.as_ref())
                .is_some_and(VimEditor::insert_mode);
            if insert && matches!(key, NativeKey::Named(NamedKey::Tab | NamedKey::Enter)) {
                let selected = !self.current().unwrap().doc.selection().is_empty();
                if shift || selected {
                    let tab = self.current_mut().unwrap();
                    if matches!(key, NativeKey::Named(NamedKey::Tab)) {
                        tab.doc.indent(shift, &settings);
                    } else {
                        tab.doc.newline(&settings);
                    }
                    self.reset_vim_from_doc();
                    self.changed()?;
                } else {
                    let doc = &self.current().unwrap().doc;
                    let newline = matches!(key, NativeKey::Named(NamedKey::Enter));
                    let value = if newline {
                        let indent: String = if settings.auto_indent {
                            doc.text[doc.start(doc.row())..doc.cursor]
                                .chars()
                                .take_while(|c| matches!(c, ' ' | '\t'))
                                .collect()
                        } else {
                            String::new()
                        };
                        format!("\n{indent}")
                    } else if settings.spaces {
                        " ".repeat(
                            settings.tab_width as usize
                                - doc.column(settings.tab_width as usize)
                                    % settings.tab_width as usize,
                        )
                    } else {
                        "\t".into()
                    };
                    let mut result = Ok(());
                    for c in value.chars() {
                        let code = match c {
                            '\n' => KeyCode::Enter,
                            '\t' => KeyCode::Tab,
                            _ => KeyCode::Char(c),
                        };
                        if let Err(error) = self
                            .current_mut()
                            .unwrap()
                            .vim
                            .as_mut()
                            .unwrap()
                            .key(code, ModalModifiers::NONE)
                        {
                            result = Err(error);
                            break;
                        }
                    }
                    self.sync_vim(result)?;
                }
                return Ok(true);
            }
            if insert && !self.current().unwrap().doc.selection().is_empty() {
                if let NativeKey::Character(c) = key
                    && !control
                {
                    self.current_mut().unwrap().doc.insert(c, true);
                    self.reset_vim_from_doc();
                    self.changed()?;
                    return Ok(true);
                }
                if matches!(
                    key,
                    NativeKey::Named(NamedKey::Backspace | NamedKey::Delete)
                ) {
                    self.current_mut()
                        .unwrap()
                        .doc
                        .delete(false, false, &settings);
                    self.reset_vim_from_doc();
                    self.changed()?;
                    return Ok(true);
                }
            }
            let code = match key {
                NativeKey::Character(c) if c.chars().count() == 1 => {
                    KeyCode::Char(c.chars().next().unwrap())
                }
                NativeKey::Named(n) => match n {
                    NamedKey::Escape => KeyCode::Esc,
                    NamedKey::Enter => KeyCode::Enter,
                    NamedKey::Tab => KeyCode::Tab,
                    NamedKey::Space => KeyCode::Char(' '),
                    NamedKey::ArrowLeft => KeyCode::Left,
                    NamedKey::ArrowRight => KeyCode::Right,
                    NamedKey::ArrowUp => KeyCode::Up,
                    NamedKey::ArrowDown => KeyCode::Down,
                    NamedKey::Home => KeyCode::Home,
                    NamedKey::End => KeyCode::End,
                    NamedKey::Backspace => KeyCode::Backspace,
                    NamedKey::Delete => KeyCode::Delete,
                    NamedKey::PageUp => KeyCode::PageUp,
                    NamedKey::PageDown => KeyCode::PageDown,
                    _ => return Ok(true),
                },
                _ => return Ok(true),
            };
            let mut mods = ModalModifiers::NONE;
            if control {
                mods |= ModalModifiers::CONTROL;
            }
            if shift {
                mods |= ModalModifiers::SHIFT;
            }
            if modifiers.alt_key() {
                mods |= ModalModifiers::ALT;
            }
            let result = self
                .current_mut()
                .unwrap()
                .vim
                .as_mut()
                .unwrap()
                .key(code, mods);
            self.sync_vim(result)?;
            return Ok(true);
        }
        let metrics = self.view.metrics;
        let doc = &mut self.current_mut().unwrap().doc;
        match key {
            NativeKey::Named(n) => match n {
                NamedKey::Enter => doc.newline(&settings),
                NamedKey::Tab => doc.indent(shift, &settings),
                NamedKey::Backspace => doc.delete(false, control, &settings),
                NamedKey::Delete => doc.delete(true, control, &settings),
                NamedKey::ArrowLeft => doc.horizontal(false, control, shift),
                NamedKey::ArrowRight => doc.horizontal(true, control, shift),
                NamedKey::ArrowUp => metrics.vertical(doc, -1, shift, settings.tab_width as usize),
                NamedKey::ArrowDown => metrics.vertical(doc, 1, shift, settings.tab_width as usize),
                NamedKey::PageUp => {
                    metrics.vertical(doc, -rows, shift, settings.tab_width as usize)
                }
                NamedKey::PageDown => {
                    metrics.vertical(doc, rows, shift, settings.tab_width as usize)
                }
                NamedKey::Home => doc.home(false, control, shift),
                NamedKey::End => doc.home(true, control, shift),
                NamedKey::Insert => doc.overwrite = !doc.overwrite,
                NamedKey::Escape => {
                    doc.anchor = doc.cursor;
                }
                NamedKey::Space => return Ok(false),
                _ => return Ok(false),
            },
            NativeKey::Character(_) => return Ok(control || modifiers.alt_key()),
            _ => return Ok(false),
        }
        self.changed()?;
        Ok(true)
    }
    fn text(&mut self, text: &str) -> AppResult<bool> {
        if self.view.file_prompt.is_some() || !self.recoverable.is_empty() {
            return Ok(false);
        }
        if self.view.page != Page::Editor || !self.view.focused {
            return Ok(false);
        }
        self.view.preedit.clear();
        self.view.message = None;
        let result = if self.settings.vim {
            self.current_mut()
                .unwrap()
                .vim
                .as_mut()
                .unwrap()
                .paste(text)
        } else {
            self.current_mut().unwrap().doc.insert(text, true);
            Ok(())
        };
        if self.settings.vim {
            self.sync_vim(result)?;
        } else {
            self.changed()?;
        }
        Ok(true)
    }
}

#[cfg(test)]
mod tests;
