use super::*;

/// UI and navigation state owned by one window. Document text and history live in Notes.
pub(super) struct WindowView {
    pub(super) tab_order: Vec<String>,
    pub(super) positions: HashMap<String, ViewPosition>,
    pub(super) seen_generation: u64,
    pub(super) scene: Scene,
    pub(super) page: Page,
    pub(super) settings_return: Option<Page>,
    pub(super) active: Option<String>,
    pub(super) file_prompt: Option<FilePrompt>,
    pub(super) closing: bool,
    pub(super) save_all: bool,
    pub(super) exit_requested: bool,
    pub(super) close_after_save: Option<String>,
    pub(super) size: [f32; 2],
    pub(super) query: String,
    pub(super) results: Vec<String>,
    pub(super) overview_page: usize,
    pub(super) overview_sort: OverviewSort,
    pub(super) metrics: Metrics,
    pub(super) focused: bool,
    pub(super) dragging: bool,
    pub(super) gutter_drag: bool,
    pub(super) scroll_drag: bool,
    pub(super) horizontal_drag: bool,
    pub(super) pointer: [f32; 2],
    pub(super) modifiers: ModifiersState,
    pub(super) last_click: Option<(Instant, [f32; 2], u8)>,
    pub(super) tab_scroll: f32,
    pub(super) tab_hits: Vec<TabHit>,
    pub(super) tab_drag: Option<(String, f32)>,
    pub(super) reveal_tab: bool,
    pub(super) message: Option<String>,
    pub(super) effects: Vec<Action>,
    pub(super) preedit: String,
    pub(super) find_open: bool,
    pub(super) find_query: String,
    pub(super) replacement: String,
    pub(super) command_open: bool,
    pub(super) command: String,
    pub(super) search_backwards: bool,
    pub(super) pending_delete: Option<String>,
    pub(super) rename_note: Option<String>,
    pub(super) rename_text: String,
}

impl WindowView {
    pub(super) fn new() -> AppResult<Self> {
        Ok(Self {
            tab_order: Vec::new(),
            positions: HashMap::new(),
            seen_generation: 0,
            scene: Scene::new(panel(0, vec![]))?,
            page: Page::Overview,
            settings_return: None,
            active: None,
            file_prompt: None,
            closing: false,
            save_all: false,
            exit_requested: false,
            close_after_save: None,
            size: [1000.0, 720.0],
            query: String::new(),
            results: vec![],
            overview_page: 0,
            overview_sort: OverviewSort::default(),
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
            message: None,
            effects: vec![],
            preedit: String::new(),
            find_open: false,
            find_query: String::new(),
            replacement: String::new(),
            command_open: false,
            command: String::new(),
            search_backwards: false,
            pending_delete: None,
            rename_note: None,
            rename_text: String::new(),
        })
    }
}

#[derive(Clone, Copy, Default)]
pub(super) struct ViewPosition {
    pub(super) cursor: usize,
    pub(super) anchor: usize,
    pub(super) scroll_row: usize,
    pub(super) scroll_col: usize,
}

impl ViewPosition {
    pub(super) fn capture(doc: &Document) -> Self {
        Self {
            cursor: doc.cursor,
            anchor: doc.anchor,
            scroll_row: doc.scroll_row,
            scroll_col: doc.scroll_col,
        }
    }

    pub(super) fn restore(self, tab: &mut Tab) {
        tab.doc
            .restore_view(self.cursor, self.anchor, self.scroll_row, self.scroll_col);
        if let Some(vim) = &mut tab.vim {
            vim.place(&tab.doc.text, tab.doc.cursor);
        }
    }
}
