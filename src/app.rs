use crate::{
    AppResult,
    editor::{Document, columns},
    host::Application,
    paint::{self, Metrics},
    store::{Note, Notebook, Settings, Storage, now},
    vim::VimEditor,
};
use crossterm::event::{KeyCode, KeyModifiers as ModalModifiers};
use knave_ui::{Color, DisplayList, Rect, TextAlign, toolkit::*};
use std::{cell::RefCell, collections::HashMap, rc::Rc, time::Instant};
use winit::keyboard::{Key as NativeKey, ModifiersState, NamedKey};

const EDITOR: ElementId = ElementId(20);
const STRIP: ElementId = ElementId(19);
const STATUS: ElementId = ElementId(21);
const SEARCH: ElementId = ElementId(30);
const FIND: ElementId = ElementId(70);
const REPLACE: ElementId = ElementId(71);
const PAGE_SIZE: usize = 60;
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
}
pub struct Notes {
    scene: Scene,
    storage: Storage,
    book: Notebook,
    settings: Settings,
    page: Page,
    active: Option<String>,
    tabs: HashMap<String, Tab>,
    modal_store:
        Rc<RefCell<modalkit::editing::store::Store<modalkit::editing::application::EmptyInfo>>>,
    size: [f32; 2],
    query: String,
    results: Vec<String>,
    overview_page: usize,
    metrics: Metrics,
    focused: bool,
    dragging: bool,
    gutter_drag: bool,
    scroll_drag: bool,
    horizontal_drag: bool,
    pointer: [f32; 2],
    modifiers: ModifiersState,
    last_click: Option<(Instant, [f32; 2], u8)>,
    tab_scroll: f32,
    tab_hits: Vec<TabHit>,
    tab_drag: Option<(String, f32)>,
    reveal_tab: bool,
    save_error: Option<String>,
    message: Option<String>,
    effects: Vec<Action>,
    preedit: String,
    find_open: bool,
    find_query: String,
    replacement: String,
    command_open: bool,
    command: String,
    search_backwards: bool,
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
fn row(id: u64, children: Vec<Element>) -> Element {
    let mut e = Element::new(id, Widget::Panel)
        .layout(Layout {
            flow: Flow::Row,
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
impl Notes {
    pub fn new(storage: Storage) -> AppResult<Self> {
        let (book, settings) = storage.load()?;
        let mut app = Self {
            scene: Scene::new(panel(0, vec![]))?,
            storage,
            book,
            settings,
            page: Page::Overview,
            active: None,
            tabs: HashMap::new(),
            modal_store: Rc::new(RefCell::new(Default::default())),
            size: [1000.0, 720.0],
            query: String::new(),
            results: vec![],
            overview_page: 0,
            metrics: Metrics::default(),
            focused: false,
            dragging: false,
            gutter_drag: false,
            scroll_drag: false,
            horizontal_drag: false,
            pointer: [0.0; 2],
            modifiers: ModifiersState::empty(),
            last_click: None,
            tab_scroll: 0.0,
            tab_hits: vec![],
            tab_drag: None,
            reveal_tab: true,
            save_error: None,
            message: None,
            effects: vec![],
            preedit: String::new(),
            find_open: false,
            find_query: String::new(),
            replacement: String::new(),
            command_open: false,
            command: String::new(),
            search_backwards: false,
        };
        app.rebuild()?;
        Ok(app)
    }
    fn save(&mut self) {
        self.save_error = self
            .storage
            .save(&self.book, &self.settings)
            .err()
            .map(|e| format!("Save failed: {e}. Retry with Ctrl+S."));
    }
    fn current(&self) -> Option<&Tab> {
        self.active.as_ref().and_then(|id| self.tabs.get(id))
    }
    fn current_mut(&mut self) -> Option<&mut Tab> {
        self.active.as_ref().and_then(|id| self.tabs.get_mut(id))
    }
    fn status(&self) -> String {
        if let Some(error) = &self.save_error {
            return error.clone();
        }
        if let Some(message) = &self.message {
            return message.clone();
        }
        if self.page != Page::Editor {
            return format!("{} documents", self.book.notes.len());
        }
        if let Some(tab) = self.current() {
            let doc = &tab.doc;
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
            let selected = if doc.selection().is_empty() {
                String::new()
            } else {
                format!(" · {} selected", doc.selected().chars().count())
            };
            format!(
                "{}    UTF-8 · LF    {indent}: {}    Ln {}, Col {}{selected}    {mode}",
                if self.save_error.is_none() {
                    "Saved"
                } else {
                    "Unsaved"
                },
                self.settings.tab_width,
                doc.row() + 1,
                doc.column(self.settings.tab_width as usize) + 1
            )
        } else {
            String::new()
        }
    }
    fn refresh_status(&mut self) -> AppResult {
        self.scene.set_text(STATUS, self.status())?;
        if self.scene.widget(ElementId(61)).is_some() {
            self.scene
                .set_visible(ElementId(61), self.save_error.is_some())?;
        }
        Ok(())
    }
    fn ensure_tab(&mut self, id: &str) -> AppResult {
        if !self.tabs.contains_key(id) {
            let note = self
                .book
                .notes
                .iter()
                .find(|n| n.id == id)
                .ok_or("Missing note")?;
            let doc = Document::new(&note.text);
            let vim = self
                .settings
                .vim
                .then(|| VimEditor::with_store(&doc.text, 0, self.modal_store.clone()));
            self.tabs.insert(id.into(), Tab { doc, vim });
        }
        Ok(())
    }
    fn focus_editor(&mut self) -> AppResult {
        self.focused = true;
        self.scene.set_focus(Some(EDITOR))?;
        Ok(())
    }
    fn open(&mut self, id: String) -> AppResult {
        self.ensure_tab(&id)?;
        if !self.book.tabs.contains(&id) {
            self.book.tabs.push(id.clone());
        }
        self.active = Some(id);
        self.page = Page::Editor;
        self.message = None;
        self.reveal_tab = true;
        self.preedit.clear();
        self.save();
        self.rebuild()?;
        self.focus_editor()
    }
    fn create(&mut self) -> AppResult {
        let mut note = Note::new();
        let number = self.book.notes.len() + 1;
        note.name = Some(format!("Untitled Document {number}"));
        let id = note.id.clone();
        self.book.notes.push(note);
        self.open(id)
    }
    fn close(&mut self, id: &str) -> AppResult {
        let index = self.book.tabs.iter().position(|t| t == id).unwrap_or(0);
        self.book.tabs.retain(|t| t != id);
        self.tabs.remove(id);
        if self.active.as_deref() == Some(id) {
            self.active = None;
            if let Some(next) = self
                .book
                .tabs
                .get(index.min(self.book.tabs.len().saturating_sub(1)))
                .cloned()
            {
                return self.open(next);
            }
            self.page = Page::Overview;
            self.focused = false;
        }
        self.save();
        self.rebuild()
    }
    fn changed(&mut self) -> AppResult {
        let mut changed = false;
        if let Some(id) = &self.active
            && let Some(tab) = self.tabs.get(id)
            && let Some(note) = self.book.notes.iter_mut().find(|n| &n.id == id)
            && note.text != tab.doc.text
        {
            note.text = tab.doc.text.clone();
            if note
                .name
                .as_ref()
                .is_some_and(|name| name.starts_with("Untitled Document "))
                && !note.text.trim().is_empty()
            {
                note.name = None;
            }
            note.updated = now();
            changed = true;
        }
        if changed {
            self.save();
        }
        let metrics = self.metrics;
        let width = self.settings.tab_width as usize;
        if let Some(tab) = self.current_mut() {
            tab.doc.reveal(metrics.rows, metrics.cols, width);
        }
        self.refresh_status()
    }
    fn sync_vim(&mut self, result: Result<(), String>) -> AppResult {
        self.message = result.err();
        if let Some(tab) = self.current_mut()
            && let Some(vim) = &mut tab.vim
        {
            let text = vim.text();
            let (cursor, anchor) = vim.positions();
            tab.doc.set_external(text, cursor, anchor);
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
            self.message = Some(format!("Could not apply modal edit: {error}"));
        }
    }
    fn switch_tab(&mut self, delta: isize) -> AppResult {
        if self.book.tabs.is_empty() {
            return Ok(());
        }
        let index = self
            .active
            .as_ref()
            .and_then(|id| self.book.tabs.iter().position(|t| t == id))
            .unwrap_or(0);
        let index = (index as isize + delta).rem_euclid(self.book.tabs.len() as isize) as usize;
        self.open(self.book.tabs[index].clone())
    }
    fn find(&mut self, backwards: bool) -> AppResult {
        let query = self.find_query.clone();
        let found = self
            .current_mut()
            .is_some_and(|tab| tab.doc.find(&query, backwards));
        self.message = (!found).then(|| "No match".into());
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
            .set_title("Open text document")
            .pick_file()
        else {
            return Ok(());
        };
        match std::fs::read_to_string(&path) {
            Ok(text) => {
                let mut note = Note::new();
                note.text = crate::editor::normalize(&text);
                note.name = path.file_name().map(|s| s.to_string_lossy().into_owned());
                let id = note.id.clone();
                self.book.notes.push(note);
                self.open(id)?;
            }
            Err(error) => {
                self.message = Some(format!("Could not open document: {error}"));
                self.refresh_status()?;
            }
        }
        Ok(())
    }
    fn export(&mut self) -> AppResult {
        let Some(id) = &self.active else {
            return Ok(());
        };
        let Some(note) = self.book.notes.iter().find(|n| &n.id == id) else {
            return Ok(());
        };
        let Some(path) = rfd::FileDialog::new()
            .set_title("Export text document")
            .set_file_name(format!("{}.txt", note.title()))
            .save_file()
        else {
            return Ok(());
        };
        match crate::store::export_text(&path, &note.text) {
            Ok(()) => self.message = Some(format!("Exported to {}", path.display())),
            Err(e) => self.message = Some(format!("Export failed: {e}")),
        };
        self.refresh_status()
    }
    fn command(&mut self) -> AppResult {
        let command = self.command.trim().trim_start_matches(':').to_owned();
        self.command_open = false;
        self.command.clear();
        self.rebuild()?;
        self.focus_editor()?;
        match command.as_str() {
            "w" => {
                self.save();
                self.refresh_status()?;
            }
            "q" => {
                if let Some(id) = self.active.clone() {
                    self.close(&id)?;
                }
            }
            "wq" | "x" => {
                self.save();
                if self.save_error.is_none()
                    && let Some(id) = self.active.clone()
                {
                    self.close(&id)?;
                }
            }
            "e" => self.import()?,
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
                self.message = error;
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
                self.message = Some(format!("Unknown command: {command}"));
                self.refresh_status()?;
            }
        }
        Ok(())
    }
    fn rebuild(&mut self) -> AppResult {
        let editing = self.page == Page::Editor;
        let heading = if self.page == Page::Settings {
            "Preferences"
        } else {
            "Notes"
        };
        let mut title = label(3, heading);
        title.style.text.align = TextAlign::Center;
        title.style.text.weight = 600;
        title.style.text.wrap = knave_ui::TextWrap::None;
        let mut header = row(
            1,
            vec![
                button(2, if editing { "Library" } else { "Notes" }),
                compact(4, "+"),
                title,
                button(9, "Open…"),
                button(10, "Export…").enabled(editing),
                button(11, "Find").enabled(editing),
                button(5, "Settings"),
            ],
        )
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
        if editing {
            let mut strip = panel(STRIP.0, vec![]).layout(Layout {
                height: Length::Px(38.0),
                ..Default::default()
            });
            strip.paint_surface = false;
            body.push(strip);
            if self.find_open {
                body.push(
                    row(
                        72,
                        vec![
                            field(FIND.0, &self.find_query)?,
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
                            field(REPLACE.0, &self.replacement)?,
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
            if self.command_open {
                body.push(
                    row(
                        81,
                        vec![
                            {
                                let mut e = label(82, ":");
                                e.layout.width = Length::Px(16.0);
                                e
                            },
                            field(80, &self.command)?,
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
        } else if self.page == Page::Overview {
            let mut search = field(SEARCH.0, &self.query)?;
            search.layout.width = Length::Px(260.0);
            body.push(
                row(
                    31,
                    vec![
                        label(32, "All documents"),
                        {
                            let mut e = label(33, "Search");
                            e.layout.width = Length::Px(48.0);
                            e
                        },
                        search,
                    ],
                )
                .layout(Layout {
                    flow: Flow::Row,
                    height: Length::Px(52.0),
                    padding: 10.0,
                    ..Default::default()
                }),
            );
            let query = self.query.to_lowercase();
            let mut notes: Vec<_> = self
                .book
                .notes
                .iter()
                .filter(|n| {
                    n.title().to_lowercase().contains(&query)
                        || n.text.to_lowercase().contains(&query)
                })
                .collect();
            notes.sort_by(|a, b| b.updated.cmp(&a.updated).then_with(|| a.id.cmp(&b.id)));
            self.results = notes.iter().map(|n| n.id.clone()).collect();
            self.overview_page = self
                .overview_page
                .min(notes.len().saturating_sub(1) / PAGE_SIZE);
            let cols = ((self.size[0] - 32.0) / 170.0).floor().max(1.0) as usize;
            let cards = notes
                .iter()
                .enumerate()
                .skip(self.overview_page * PAGE_SIZE)
                .take(PAGE_SIZE)
                .map(|(index, _)| {
                    let mut e = button(1000 + (index % PAGE_SIZE) as u64, "")
                        .class("document")
                        .layout(Layout {
                            width: Length::Fill,
                            height: Length::Px(244.0),
                            ..Default::default()
                        });
                    e.paint_surface = false;
                    e
                })
                .collect::<Vec<_>>();
            let grid = panel(35, cards).layout(Layout {
                flow: Flow::Grid { columns: cols },
                padding: 16.0,
                gap: 12.0,
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
            if self.results.is_empty() {
                self.message = Some(
                    if self.query.is_empty() {
                        "No documents yet. Choose + to start writing."
                    } else {
                        "No matching documents."
                    }
                    .into(),
                );
            }
            if self.results.len() > PAGE_SIZE {
                body.push(row(
                    36,
                    vec![
                        button(37, "Previous").enabled(self.overview_page > 0),
                        label(38, format!("Page {}", self.overview_page + 1)),
                        button(39, "Next")
                            .enabled((self.overview_page + 1) * PAGE_SIZE < self.results.len()),
                    ],
                ));
            }
        } else {
            let settings = vec![
                label(40, "Editor"),
                row(
                    41,
                    vec![
                        label(45, "Font size"),
                        compact(46, "−").enabled(self.settings.font_size > 10),
                        label(47, format!("{} px", self.settings.font_size)),
                        compact(48, "+").enabled(self.settings.font_size < 40),
                    ],
                ),
                toggle(42, "Vim motions (modalkit)", self.settings.vim),
                toggle(43, "Show line numbers", self.settings.line_numbers),
                toggle(44, "Relative line numbers", self.settings.relative_numbers),
                toggle(49, "Insert spaces instead of tabs", self.settings.spaces),
                row(
                    50,
                    vec![
                        label(51, "Tab width"),
                        compact(52, "−").enabled(self.settings.tab_width > 1),
                        label(53, self.settings.tab_width.to_string()),
                        compact(54, "+").enabled(self.settings.tab_width < 16),
                    ],
                ),
                toggle(55, "Automatic indentation", self.settings.auto_indent),
                toggle(56, "Highlight current line", self.settings.highlight_line),
                toggle(57, "Show spaces and tabs", self.settings.show_whitespace),
                label(
                    58,
                    "Tab indents. Shift+Tab unindents. Ctrl+Tab switches documents.",
                ),
            ];
            let settings = panel(59, settings).layout(Layout {
                width: Length::Px(640.0),
                padding: 24.0,
                gap: 12.0,
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
        let mut root = panel(0, body).layout(Layout {
            height: Length::Fill,
            gap: 0.0,
            ..Default::default()
        });
        root.paint_surface = true;
        self.scene.replace(root)?;
        self.scene.set_stylesheet(":root { background-color: #1c1d26; color: #dee1e9; } button { background-color: #2b2c32; border-radius: 3px; color: #dee1e9; } button:hover { background-color: #3b3d48; } .header { background-color: #24252b; } .status { background-color: #232530; } text-input { background-color: #181a22; text-align: left; }")?;
        Ok(())
    }
    fn paint_tabs(&mut self, list: &mut DisplayList) {
        let Some(area) = self.scene.bounds(STRIP) else {
            return;
        };
        paint::rect(list, area, Color::rgba(35, 36, 41, 255), area);
        let width = (area.width / self.book.tabs.len().max(1) as f32).max(155.0);
        let total = width * self.book.tabs.len() as f32;
        if self.reveal_tab
            && let Some(index) = self
                .active
                .as_ref()
                .and_then(|id| self.book.tabs.iter().position(|t| t == id))
        {
            let x = index as f32 * width;
            if x < self.tab_scroll {
                self.tab_scroll = x;
            }
            if x + width > self.tab_scroll + area.width {
                self.tab_scroll = x + width - area.width;
            }
            self.reveal_tab = false;
        }
        self.tab_scroll = self.tab_scroll.clamp(0.0, (total - area.width).max(0.0));
        self.tab_hits.clear();
        for (index, id) in self.book.tabs.iter().enumerate() {
            let r = Rect::new(
                area.x + index as f32 * width - self.tab_scroll,
                area.y,
                width,
                area.height,
            );
            let close = Rect::new(r.x + r.width - 29.0, r.y + 6.0, 24.0, 26.0);
            self.tab_hits.push(TabHit {
                id: id.clone(),
                rect: r,
                close,
            });
            if r.x + r.width < area.x || r.x > area.x + area.width {
                continue;
            }
            let active = self.active.as_ref() == Some(id);
            paint::rect(
                list,
                r,
                if active {
                    Color::rgba(46, 48, 56, 255)
                } else {
                    Color::rgba(37, 38, 43, 255)
                },
                area,
            );
            paint::rect(
                list,
                Rect::new(r.x + r.width - 1.0, r.y, 1.0, r.height),
                Color::rgba(28, 29, 34, 255),
                area,
            );
            if active {
                paint::rect(
                    list,
                    Rect::new(r.x, r.y + r.height - 3.0, r.width, 3.0),
                    paint::ACCENT,
                    area,
                );
            }
            let title = self
                .book
                .notes
                .iter()
                .find(|n| &n.id == id)
                .map(Note::title)
                .unwrap_or_default();
            let count = ((width - 50.0) / 7.5).max(1.0) as usize;
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
            let mut style = paint::style(13.0, false, paint::FG);
            style.align = TextAlign::Center;
            paint::text(
                list,
                Rect::new(r.x + 8.0, r.y + 9.0, r.width - 43.0, 22.0),
                title,
                style,
                area,
            );
            paint::text(
                list,
                Rect::new(close.x + 7.0, close.y + 3.0, 16.0, 20.0),
                "×",
                paint::style(15.0, false, paint::MUTED),
                area,
            );
        }
    }
    fn paint_library(&self, list: &mut DisplayList) {
        let Some(view) = self.scene.bounds(ElementId(34)) else {
            return;
        };
        for (slot, id) in self
            .results
            .iter()
            .skip(self.overview_page * PAGE_SIZE)
            .take(PAGE_SIZE)
            .enumerate()
        {
            let Some(bounds) = self.scene.bounds(ElementId(1000 + slot as u64)) else {
                continue;
            };
            let Some(clip) = bounds.intersection(view) else {
                continue;
            };
            let Some(note) = self.book.notes.iter().find(|n| &n.id == id) else {
                continue;
            };
            let w = 116.0;
            let h = 158.0;
            let page = Rect::new(bounds.x + (bounds.width - w) / 2.0, bounds.y + 8.0, w, h);
            if self.scene.focus() == Some(ElementId(1000 + slot as u64)) {
                paint::rect(
                    list,
                    Rect::new(page.x - 3.0, page.y - 3.0, w + 6.0, h + 6.0),
                    paint::ACCENT,
                    clip,
                );
            }
            paint::rect(
                list,
                Rect::new(page.x + 3.0, page.y + 4.0, w, h),
                Color::rgba(13, 14, 21, 255),
                clip,
            );
            paint::rect(list, page, Color::rgba(238, 238, 232, 255), clip);
            let inner = Rect::new(page.x + 10.0, page.y + 12.0, w - 20.0, h - 22.0);
            let pageclip = inner.intersection(clip).unwrap_or_default();
            for (index, line) in note.text.lines().take(17).enumerate() {
                let value = line
                    .replace('\t', "    ")
                    .chars()
                    .take(30)
                    .collect::<String>();
                paint::text(
                    list,
                    Rect::new(inner.x, inner.y + index as f32 * 7.5, inner.width, 8.0),
                    value,
                    paint::style(4.8, true, Color::rgba(61, 65, 73, 255)),
                    pageclip,
                );
            }
            let mut title = paint::style(13.0, false, paint::FG);
            title.wrap = knave_ui::TextWrap::WordOrGlyph;
            title.align = TextAlign::Center;
            paint::text(
                list,
                Rect::new(bounds.x + 4.0, page.y + h + 12.0, bounds.width - 8.0, 40.0),
                note.title(),
                title,
                clip,
            );
            let lines = note.text.bytes().filter(|b| *b == b'\n').count() + 1;
            let mut meta = paint::style(11.0, false, paint::MUTED);
            meta.align = TextAlign::Center;
            paint::text(
                list,
                Rect::new(bounds.x, page.y + h + 51.0, bounds.width, 16.0),
                format!("{lines} lines"),
                meta,
                clip,
            );
        }
    }
}
impl Application for Notes {
    fn modifiers_changed(&mut self, modifiers: ModifiersState) {
        self.modifiers = modifiers;
    }
    fn scene(&mut self) -> &mut Scene {
        &mut self.scene
    }
    fn resize(&mut self, size: [f32; 2]) -> AppResult {
        let changed = ((self.size[0] - 32.0) / 170.0).floor() != ((size[0] - 32.0) / 170.0).floor();
        self.size = size;
        if changed && self.page == Page::Overview {
            self.rebuild()?;
        }
        Ok(())
    }
    fn paint(&mut self, list: &mut DisplayList, measure: &mut dyn TextMeasurer) {
        if self.page == Page::Overview {
            self.paint_library(list);
        } else if self.page == Page::Editor {
            self.paint_tabs(list);
            if let Some(area) = self.scene.bounds(EDITOR)
                && let Some(tab) = self.current()
            {
                let normal = tab.vim.as_ref().is_some_and(|v| !v.insert_mode());
                self.metrics = paint::document(
                    list,
                    measure,
                    &tab.doc,
                    &self.settings,
                    area,
                    (self.focused, normal, &self.preedit),
                );
            }
        }
    }
    fn effects(&mut self) -> Vec<Action> {
        std::mem::take(&mut self.effects)
    }
    fn close_requested(&mut self) -> AppResult<bool> {
        self.save();
        self.refresh_status()?;
        Ok(self.save_error.is_none())
    }
    fn ime_state(&self) -> Option<TextInputState> {
        if self.page != Page::Editor || !self.focused {
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
                .metrics
                .caret(&tab.doc, self.settings.tab_width as usize),
        })
    }
    fn clipboard_error(&mut self, error: &dyn std::error::Error) {
        self.message = Some(format!("Clipboard: {error}"));
        let _ = self.refresh_status();
    }
    fn action(&mut self, action: Action) -> AppResult {
        match action {
            Action::Activated(id) => match id.0 {
                2 => {
                    self.page = Page::Overview;
                    self.focused = false;
                    self.message = None;
                    self.rebuild()?;
                }
                4 => self.create()?,
                5 => {
                    self.page = Page::Settings;
                    self.focused = false;
                    self.message = None;
                    self.rebuild()?;
                }
                9 => self.import()?,
                10 => self.export()?,
                11 => {
                    self.find_open = !self.find_open;
                    self.rebuild()?;
                    if self.find_open {
                        self.focused = false;
                        self.scene.set_focus(Some(FIND))?;
                    } else {
                        self.focus_editor()?;
                    }
                }
                37 => {
                    self.overview_page = self.overview_page.saturating_sub(1);
                    self.rebuild()?;
                }
                39 => {
                    self.overview_page += 1;
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
                61 => {
                    self.save();
                    self.refresh_status()?;
                }
                73 => self.find(true)?,
                74 => self.find(false)?,
                75 => {
                    self.find_open = false;
                    self.rebuild()?;
                    self.focus_editor()?;
                }
                77 => {
                    let query = self.find_query.clone();
                    let replacement = self.replacement.clone();
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
                    let query = self.find_query.clone();
                    let replacement = self.replacement.clone();
                    let count = self
                        .current_mut()
                        .map_or(0, |tab| tab.doc.replace_all(&query, &replacement));
                    self.reset_vim_from_doc();
                    self.message = Some(format!("Replaced {count} matches"));
                    self.changed()?;
                }
                value if (1000..1000 + PAGE_SIZE as u64).contains(&value) => {
                    if let Some(id) = self
                        .results
                        .get(self.overview_page * PAGE_SIZE + (value - 1000) as usize)
                        .cloned()
                    {
                        self.open(id)?;
                    }
                }
                _ => {}
            },
            Action::TextChanged(SEARCH, text) => {
                let edit = self.scene.widget(SEARCH).cloned();
                self.query = text;
                self.overview_page = 0;
                self.message = None;
                self.rebuild()?;
                if let Some(edit) = edit {
                    self.scene.set_widget(SEARCH, edit)?;
                    self.scene.set_focus(Some(SEARCH))?;
                }
            }
            Action::TextChanged(FIND, text) => {
                self.find_query = text;
                self.message = None;
                self.refresh_status()?;
            }
            Action::TextChanged(REPLACE, text) => {
                self.replacement = text;
            }
            Action::Submitted(FIND, _) => {
                self.find(self.search_backwards)?;
                if self.settings.vim {
                    self.focus_editor()?;
                }
            }
            Action::TextChanged(ElementId(80), text) => self.command = text,
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
        match input {
            Input::FocusLost => {
                self.dragging = false;
                self.scroll_drag = false;
                self.horizontal_drag = false;
                self.preedit.clear();
                self.modifiers = ModifiersState::empty();
                return Ok(false);
            }
            Input::Key { modifiers, .. } => {
                self.modifiers = ModifiersState::empty();
                if modifiers.shift {
                    self.modifiers |= ModifiersState::SHIFT;
                }
                if modifiers.control {
                    self.modifiers |= ModifiersState::CONTROL;
                }
                if modifiers.alt {
                    self.modifiers |= ModifiersState::ALT;
                }
                if modifiers.logo {
                    self.modifiers |= ModifiersState::SUPER;
                }
                if self.page == Page::Editor && self.focused {
                    return Ok(true);
                }
            }
            _ => {}
        }
        if self.page != Page::Editor {
            return Ok(false);
        }
        match input {
            Input::Preedit(text) => {
                if self.focused {
                    self.preedit = text.clone();
                    return Ok(true);
                }
            }
            Input::PointerDown(p) => {
                self.pointer = *p;
                if let Some(area) = self.scene.bounds(STRIP)
                    && paint::contains(area, *p)
                {
                    if let Some(hit) = self
                        .tab_hits
                        .iter()
                        .find(|h| paint::contains(h.rect, *p))
                        .cloned()
                    {
                        if paint::contains(hit.close, *p) {
                            self.close(&hit.id)?;
                        } else {
                            self.open(hit.id.clone())?;
                            self.tab_drag = Some((hit.id, p[0]));
                        }
                    }
                    return Ok(true);
                }
                let area = self.scene.bounds(EDITOR).unwrap_or_default();
                if !paint::contains(area, *p) {
                    self.focused = false;
                    return Ok(false);
                }
                self.focus_editor()?;
                self.message = None;
                self.preedit.clear();
                if p[1] > area.y + area.height - 10.0 {
                    self.horizontal_drag = true;
                    return Ok(true);
                }
                if p[0] > area.x + area.width - 10.0 {
                    self.scroll_drag = true;
                    return Ok(true);
                }
                let clicks = self
                    .last_click
                    .filter(|(t, last, _)| {
                        t.elapsed().as_millis() < 400
                            && (last[0] - p[0]).abs() < 5.0
                            && (last[1] - p[1]).abs() < 5.0
                    })
                    .map_or(1, |(_, _, count)| count % 3 + 1);
                self.last_click = Some((Instant::now(), *p, clicks));
                let metrics = self.metrics;
                let width = self.settings.tab_width as usize;
                let extend = self.modifiers.shift_key();
                self.gutter_drag = p[0] < area.x + metrics.gutter;
                let gutter = self.gutter_drag;
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
                self.dragging = true;
                self.refresh_status()?;
                return Ok(true);
            }
            Input::PointerMove(p) => {
                self.pointer = *p;
                if let Some((id, start)) = self.tab_drag.clone() {
                    if (p[0] - start).abs() > 12.0
                        && let Some(target) = self
                            .tab_hits
                            .iter()
                            .position(|h| paint::contains(h.rect, *p))
                        && let Some(from) = self.book.tabs.iter().position(|t| t == &id)
                        && target != from
                    {
                        let id = self.book.tabs.remove(from);
                        self.book.tabs.insert(target, id.clone());
                        self.tab_drag = Some((id, p[0]));
                        self.save();
                    }
                    return Ok(true);
                }
                if self.horizontal_drag {
                    let metrics = self.metrics;
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
                if self.scroll_drag {
                    let metrics = self.metrics;
                    if let Some(tab) = self.current_mut() {
                        let fraction =
                            ((p[1] - metrics.area.y) / metrics.area.height).clamp(0.0, 1.0);
                        tab.doc.scroll_row = (fraction
                            * tab.doc.lines().saturating_sub(metrics.rows) as f32)
                            as usize;
                    }
                    return Ok(true);
                }
                if self.dragging {
                    let metrics = self.metrics;
                    let width = self.settings.tab_width as usize;
                    let gutter = self.gutter_drag;
                    if let Some(tab) = self.current_mut() {
                        if p[1] < metrics.area.y {
                            tab.doc.scroll_row = tab.doc.scroll_row.saturating_sub(1);
                        } else if p[1] > metrics.area.y + metrics.area.height {
                            tab.doc.scroll_row = (tab.doc.scroll_row + 1).min(tab.doc.lines() - 1);
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
                if self.dragging
                    || self.scroll_drag
                    || self.horizontal_drag
                    || self.tab_drag.is_some()
                {
                    self.dragging = false;
                    self.scroll_drag = false;
                    self.horizontal_drag = false;
                    self.tab_drag = None;
                    return Ok(true);
                }
            }
            Input::Scroll { position, delta } => {
                if let Some(area) = self.scene.bounds(STRIP)
                    && paint::contains(area, *position)
                {
                    self.tab_scroll += if delta[0] != 0.0 { delta[0] } else { delta[1] };
                    return Ok(true);
                }
                if self
                    .scene
                    .bounds(EDITOR)
                    .is_some_and(|r| paint::contains(r, *position))
                {
                    let metrics = self.metrics;
                    let horizontal = self.modifiers.shift_key();
                    let width = self.settings.tab_width as usize;
                    if let Some(tab) = self.current_mut() {
                        if horizontal || delta[0] != 0.0 {
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
                                .min(tab.doc.lines().saturating_sub(metrics.rows));
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
        self.modifiers = modifiers;
        let control = modifiers.control_key() || modifiers.super_key();
        let shift = modifiers.shift_key();
        if control {
            if let NativeKey::Character(c) = key {
                match c.to_lowercase().as_str() {
                    "n" => {
                        self.create()?;
                        return Ok(true);
                    }
                    "o" => {
                        self.import()?;
                        return Ok(true);
                    }
                    "s" => {
                        if shift {
                            self.export()?;
                        } else {
                            self.save();
                            self.refresh_status()?;
                        }
                        return Ok(true);
                    }
                    "w" => {
                        if self.page == Page::Editor
                            && let Some(id) = self.active.clone()
                        {
                            self.close(&id)?;
                        }
                        return Ok(true);
                    }
                    "f" | "h" if self.page == Page::Editor => {
                        self.find_open = true;
                        self.focused = false;
                        self.rebuild()?;
                        self.scene.set_focus(Some(if c.eq_ignore_ascii_case("h") {
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
        if self.page != Page::Editor {
            return Ok(false);
        }
        if matches!(key, NativeKey::Named(NamedKey::Escape)) && self.command_open {
            self.command_open = false;
            self.command.clear();
            self.rebuild()?;
            self.focus_editor()?;
            return Ok(true);
        }
        if matches!(key, NativeKey::Named(NamedKey::Escape)) && self.find_open {
            self.find_open = false;
            self.rebuild()?;
            self.focus_editor()?;
            return Ok(true);
        }
        if !self.focused {
            return Ok(false);
        }
        self.message = None;
        let vim = self.settings.vim;
        if control && let NativeKey::Character(c) = key {
            let c = c.to_lowercase();
            let clipboard = !vim || shift;
            if clipboard && matches!(c.as_str(), "c" | "x" | "v") {
                if c == "v" {
                    self.effects.push(Action::RequestPaste(EDITOR));
                } else if let Some(tab) = self.current() {
                    let selected = tab.doc.selected().to_owned();
                    if !selected.is_empty() {
                        self.effects.push(Action::Copy(selected));
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
        let rows = self.metrics.rows as isize;
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
                    self.search_backwards = c == "?";
                    self.find_open = true;
                    self.focused = false;
                    self.rebuild()?;
                    self.scene.set_focus(Some(FIND))?;
                    return Ok(true);
                }
                "n" | "N" => {
                    self.find(self.search_backwards ^ (c == "N"))?;
                    return Ok(true);
                }
                ":" => {
                    self.command_open = true;
                    self.focused = false;
                    self.rebuild()?;
                    self.scene.set_focus(Some(ElementId(80)))?;
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
        let doc = &mut self.current_mut().unwrap().doc;
        match key {
            NativeKey::Named(n) => match n {
                NamedKey::Enter => doc.newline(&settings),
                NamedKey::Tab => doc.indent(shift, &settings),
                NamedKey::Backspace => doc.delete(false, control, &settings),
                NamedKey::Delete => doc.delete(true, control, &settings),
                NamedKey::ArrowLeft => doc.horizontal(false, control, shift),
                NamedKey::ArrowRight => doc.horizontal(true, control, shift),
                NamedKey::ArrowUp => doc.vertical(-1, shift, settings.tab_width as usize),
                NamedKey::ArrowDown => doc.vertical(1, shift, settings.tab_width as usize),
                NamedKey::PageUp => doc.vertical(-rows, shift, settings.tab_width as usize),
                NamedKey::PageDown => doc.vertical(rows, shift, settings.tab_width as usize),
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
        if self.page != Page::Editor || !self.focused {
            return Ok(false);
        }
        self.preedit.clear();
        self.message = None;
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
