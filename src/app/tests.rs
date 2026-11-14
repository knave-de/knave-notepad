use super::*;
fn app() -> (tempfile::TempDir, Notes) {
    let dir = tempfile::tempdir().unwrap();
    let storage = Storage::open(dir.path().join("data"), dir.path().join("config")).unwrap();
    let app = Notes::new(storage).unwrap();
    (dir, app)
}
fn key(app: &mut Notes, key: NamedKey, modifiers: ModifiersState) {
    assert!(app.key(&NativeKey::Named(key), modifiers).unwrap());
}
fn shortcut(app: &mut Notes, key: &str) {
    assert!(
        app.key(&NativeKey::Character(key.into()), ModifiersState::CONTROL)
            .unwrap()
    );
}
#[test]
fn actual_editor_hooks_indentation_selection_undo_and_restart() {
    let (dir, mut app) = app();
    app.create().unwrap();
    app.text("fn main() {").unwrap();
    key(&mut app, NamedKey::Enter, ModifiersState::empty());
    key(&mut app, NamedKey::Tab, ModifiersState::empty());
    app.text("hello").unwrap();
    key(&mut app, NamedKey::Enter, ModifiersState::empty());
    app.text("world").unwrap();
    assert_eq!(
        app.current().unwrap().doc.text,
        "fn main() {\n    hello\n    world"
    );
    key(&mut app, NamedKey::Home, ModifiersState::empty());
    key(&mut app, NamedKey::End, ModifiersState::SHIFT);
    assert_eq!(app.current().unwrap().doc.selected(), "world");
    shortcut(&mut app, "x");
    assert_eq!(app.effects().len(), 1);
    assert!(app.current().unwrap().doc.text.ends_with("\n    "));
    shortcut(&mut app, "z");
    assert!(app.current().unwrap().doc.text.ends_with("world"));
    shortcut(&mut app, "y");
    assert!(app.current().unwrap().doc.text.ends_with("\n    "));
    app.input(&Input::PasteTo {
        target: EDITOR,
        text: "replaced\r\n    next".into(),
    })
    .unwrap();
    assert!(
        app.current()
            .unwrap()
            .doc
            .text
            .ends_with("replaced\n    next")
    );
    let first = app.active.clone().unwrap();
    app.create().unwrap();
    app.text("second").unwrap();
    app.open(first.clone()).unwrap();
    assert!(app.current().unwrap().doc.text.starts_with("fn main"));
    app.action(Action::Activated(ElementId(2))).unwrap();
    app.action(Action::TextChanged(SEARCH, "replaced".into()))
        .unwrap();
    assert_eq!(app.results, vec![first]);
    app.action(Action::Activated(ElementId(5))).unwrap();
    for id in [42, 44, 57] {
        app.action(Action::Toggled(ElementId(id), true)).unwrap();
    }
    app.action(Action::Activated(ElementId(54))).unwrap();
    assert!(app.save_error.is_none());
    drop(app);
    let storage = Storage::open(dir.path().join("data"), dir.path().join("config")).unwrap();
    let app = Notes::new(storage).unwrap();
    assert_eq!(app.book.notes.len(), 2);
    assert_eq!(app.book.tabs.len(), 2);
    assert!(app.settings.relative_numbers);
    assert!(app.settings.show_whitespace);
    assert_eq!(app.settings.tab_width, 5);
}
#[test]
fn modal_insert_newlines_indentation_and_undo_share_one_history() {
    let (_, mut app) = app();
    app.settings.vim = true;
    app.create().unwrap();
    for c in ["i", "h", "i"] {
        app.key(&NativeKey::Character(c.into()), ModifiersState::empty())
            .unwrap();
    }
    key(&mut app, NamedKey::Enter, ModifiersState::empty());
    key(&mut app, NamedKey::Tab, ModifiersState::empty());
    app.key(&NativeKey::Character("x".into()), ModifiersState::empty())
        .unwrap();
    assert_eq!(app.current().unwrap().doc.text, "hi\n    x");
    key(&mut app, NamedKey::Escape, ModifiersState::empty());
    app.key(&NativeKey::Character("u".into()), ModifiersState::empty())
        .unwrap();
    assert_eq!(app.current().unwrap().doc.text, "");
    app.key(&NativeKey::Character("r".into()), ModifiersState::CONTROL)
        .unwrap();
    assert_eq!(app.current().unwrap().doc.text, "hi\n    x");
}
#[test]
fn tabs_close_without_deleting_and_cycle_restores_cursor() {
    let (_, mut app) = app();
    app.create().unwrap();
    app.text("first").unwrap();
    let id = app.active.clone().unwrap();
    app.create().unwrap();
    app.text("second").unwrap();
    app.switch_tab(-1).unwrap();
    assert_eq!(app.active.as_ref(), Some(&id));
    assert_eq!(app.current().unwrap().doc.cursor, 5);
    app.close(&id).unwrap();
    assert_eq!(app.book.notes.len(), 2);
    assert_eq!(app.book.tabs.len(), 1);
}
#[test]
fn save_failure_keeps_editor_and_blocks_close_until_retry() {
    let (dir, mut app) = app();
    app.create().unwrap();
    let temp = dir.path().join("data/notebook.json.tmp");
    std::fs::create_dir(&temp).unwrap();
    app.text("still here").unwrap();
    assert!(!app.close_requested().unwrap());
    assert_eq!(app.current().unwrap().doc.text, "still here");
    std::fs::remove_dir(temp).unwrap();
    assert!(app.close_requested().unwrap());
    assert_eq!(app.storage.load().unwrap().0.notes[0].text, "still here");
}
#[test]
fn find_replace_and_large_document_use_the_full_buffer() {
    let (_, mut app) = app();
    app.create().unwrap();
    let text = "line\n".repeat(10_000) + "needle";
    app.text(&text).unwrap();
    assert_eq!(app.current().unwrap().doc.lines(), 10_001);
    app.find_query = "needle".into();
    app.find(false).unwrap();
    assert_eq!(app.current().unwrap().doc.selected(), "needle");
    app.replacement = "thread".into();
    app.action(Action::Activated(ElementId(77))).unwrap();
    assert!(app.book.notes[0].text.ends_with("thread"));
}
struct Measure;
impl TextMeasurer for Measure {
    fn measure(&mut self, text: &str, style: &knave_ui::TextStyle, _: f32) -> TextLayout {
        TextLayout {
            size: [
                text.chars().count() as f32 * style.font_size * 0.6,
                style.line_height,
            ],
            carets: vec![],
        }
    }
    fn measure_spans(
        &mut self,
        spans: &[knave_ui::TextSpan],
        style: &knave_ui::TextStyle,
        width: f32,
    ) -> TextLayout {
        self.measure(
            &spans.iter().map(|s| s.text.as_str()).collect::<String>(),
            style,
            width,
        )
    }
}
#[test]
fn renderer_virtualizes_document_and_relative_gutter_tracks_cursor() {
    let (_, mut app) = app();
    app.create().unwrap();
    app.text(&"text\n".repeat(10_000)).unwrap();
    app.settings.relative_numbers = true;
    app.current_mut().unwrap().doc.place(50, false);
    app.current_mut().unwrap().doc.scroll_row = 8;
    let mut measure = Measure;
    let mut list = app
        .scene
        .display_list([1000.0, 720.0], &mut measure)
        .clone();
    let base = list.commands.len();
    app.paint(&mut list, &mut measure);
    assert!(list.commands.len() - base < 300);
    assert!(app.metrics.area.height > 500.0);
    let numbers = list
        .commands
        .iter()
        .skip(base)
        .filter_map(|c| {
            if let knave_ui::DisplayCommand::Text { text, style, .. } = c {
                (style.align == TextAlign::End).then_some(text.as_str())
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    assert_eq!(&numbers[..5], &["2", "1", "11", "1", "2"]);
}

#[test]
fn modal_registers_cross_tabs_and_dot_repeats_edits() {
    let (_, mut app) = app();
    app.settings.vim = true;
    app.create().unwrap();
    app.key(&NativeKey::Character("i".into()), ModifiersState::empty())
        .unwrap();
    app.text("first line").unwrap();
    key(&mut app, NamedKey::Escape, ModifiersState::empty());
    for c in ["y", "y"] {
        app.key(&NativeKey::Character(c.into()), ModifiersState::empty())
            .unwrap();
    }
    app.create().unwrap();
    app.key(&NativeKey::Character("p".into()), ModifiersState::empty())
        .unwrap();
    assert!(app.current().unwrap().doc.text.contains("first line"));
    app.key(&NativeKey::Character("x".into()), ModifiersState::empty())
        .unwrap();
    let before = app.current().unwrap().doc.text.len();
    app.key(&NativeKey::Character(".".into()), ModifiersState::empty())
        .unwrap();
    assert!(app.current().unwrap().doc.text.len() < before);
}
#[test]
fn modal_command_bar_sets_persistent_editor_preferences() {
    let (_, mut app) = app();
    app.settings.vim = true;
    app.create().unwrap();
    app.key(&NativeKey::Character(":".into()), ModifiersState::empty())
        .unwrap();
    assert!(app.command_open);
    app.action(Action::TextChanged(
        ElementId(80),
        "set rnu noexpandtab ts=8".into(),
    ))
    .unwrap();
    app.action(Action::Submitted(ElementId(80), String::new()))
        .unwrap();
    assert!(app.settings.relative_numbers);
    assert!(!app.settings.spaces);
    assert_eq!(app.settings.tab_width, 8);
    assert!(app.focused);
    assert!(!app.command_open);
}
#[test]
fn mouse_selection_and_scrollbars_operate_on_the_editor_viewport() {
    let (_, mut app) = app();
    app.create().unwrap();
    app.text(&("x".repeat(240) + "\n" + &"row\n".repeat(100)))
        .unwrap();
    app.current_mut().unwrap().doc.place(0, false);
    app.current_mut().unwrap().doc.scroll_row = 0;
    app.current_mut().unwrap().doc.scroll_col = 0;
    let mut measure = Measure;
    let mut list = app
        .scene
        .display_list([1000.0, 720.0], &mut measure)
        .clone();
    app.paint(&mut list, &mut measure);
    let m = app.metrics;
    let p = [m.area.x + m.gutter + 8.0, m.area.y + 8.0];
    app.input(&Input::PointerDown(p)).unwrap();
    app.input(&Input::PointerMove([p[0] + m.cell * 4.0, p[1]]))
        .unwrap();
    app.input(&Input::PointerUp(p)).unwrap();
    assert_eq!(app.current().unwrap().doc.selected(), "xxxx");
    app.modifiers_changed(ModifiersState::SHIFT);
    app.modifiers_changed(ModifiersState::empty());
    app.last_click = None; // A separate click, outside the double-click gesture.
    app.input(&Input::PointerDown(p)).unwrap();
    app.input(&Input::PointerUp(p)).unwrap();
    assert!(app.current().unwrap().doc.selection().is_empty());
    app.input(&Input::PointerDown([
        m.area.x + 300.0,
        m.area.y + m.area.height - 5.0,
    ]))
    .unwrap();
    app.input(&Input::PointerMove([
        m.area.x + 700.0,
        m.area.y + m.area.height - 5.0,
    ]))
    .unwrap();
    app.input(&Input::PointerUp(p)).unwrap();
    assert!(app.current().unwrap().doc.scroll_col > 0);
    app.input(&Input::PointerDown([
        m.area.x + m.area.width - 4.0,
        m.area.y + 100.0,
    ]))
    .unwrap();
    app.input(&Input::PointerMove([
        m.area.x + m.area.width - 4.0,
        m.area.y + 400.0,
    ]))
    .unwrap();
    app.input(&Input::PointerUp(p)).unwrap();
    assert!(app.current().unwrap().doc.scroll_row > 0);
}
#[test]
fn continuous_tab_strip_reorders_and_closes_with_pointer_input() {
    let (_, mut app) = app();
    for _ in 0..8 {
        app.create().unwrap();
    }
    let first = app.book.tabs[0].clone();
    app.open(first.clone()).unwrap();
    let mut measure = Measure;
    let mut list = app.scene.display_list([640.0, 480.0], &mut measure).clone();
    app.paint(&mut list, &mut measure);
    assert_eq!(app.tab_hits.len(), 8);
    let p = [app.tab_hits[0].rect.x + 40.0, app.tab_hits[0].rect.y + 15.0];
    let target = [app.tab_hits[1].rect.x + 40.0, p[1]];
    app.input(&Input::PointerDown(p)).unwrap();
    app.input(&Input::PointerMove(target)).unwrap();
    app.input(&Input::PointerUp(target)).unwrap();
    assert_eq!(app.book.tabs[1], first);
    let mut list = app.scene.display_list([640.0, 480.0], &mut measure).clone();
    app.paint(&mut list, &mut measure);
    let close = app.tab_hits[1].close;
    app.input(&Input::PointerDown([close.x + 8.0, close.y + 8.0]))
        .unwrap();
    assert_eq!(app.book.tabs.len(), 7);
    assert_eq!(app.book.notes.len(), 8);
}
