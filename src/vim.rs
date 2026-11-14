//! Modalkit owns modal state, motions, operators, registers, and undo history.
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use modalkit::{
    actions::{Action, EditAction, Editable, EditorAction, InsertTextAction},
    editing::{
        application::EmptyInfo,
        buffer::{CursorGroupId, EditBuffer},
        context::EditContextBuilder,
        cursor::{Cursor, CursorGroup, CursorState},
        store::Store,
    },
    env::vim::{
        VimMode,
        keybindings::{VimMachine, default_vim_keys},
    },
    key::TerminalKey,
    keybindings::BindingMachine,
    prelude::*,
};
use std::{cell::RefCell, rc::Rc};

pub struct VimEditor {
    machine: VimMachine<TerminalKey>,
    buffer: EditBuffer<EmptyInfo>,
    group: CursorGroupId,
    store: Rc<RefCell<Store<EmptyInfo>>>,
}
impl VimEditor {
    #[cfg(test)]
    pub fn new(text: &str, byte: usize) -> Self {
        Self::with_store(text, byte, Rc::new(RefCell::new(Store::default())))
    }
    pub fn with_store(text: &str, byte: usize, store: Rc<RefCell<Store<EmptyInfo>>>) -> Self {
        let mut buffer = EditBuffer::from_str(String::new(), &format!("{text}\n"));
        let group = buffer.create_group();
        buffer.set_leader(group, cursor_from_byte(text, byte));
        Self {
            machine: default_vim_keys(),
            buffer,
            group,
            store,
        }
    }
    pub fn mode(&self) -> VimMode {
        self.machine.mode()
    }
    pub fn text(&self) -> String {
        let text = self.buffer.get_text();
        text.strip_suffix('\n').unwrap_or(&text).to_owned()
    }
    pub fn place(&mut self, text: &str, byte: usize) {
        self.buffer
            .set_leader(self.group, cursor_from_byte(text, byte));
    }
    pub fn mouse_selection(
        &mut self,
        text: &str,
        cursor: usize,
        anchor: usize,
    ) -> Result<(), String> {
        if cursor == anchor {
            if self.mode() == VimMode::Visual {
                self.key(KeyCode::Esc, KeyModifiers::NONE)?;
            }
            self.buffer.set_group(
                self.group,
                CursorGroup::new(cursor_from_byte(text, cursor).into(), vec![]),
            );
        } else if self.insert_mode() {
            self.place(text, cursor);
        } else {
            if self.mode() != VimMode::Visual {
                self.key(KeyCode::Char('v'), KeyModifiers::NONE)?;
            }
            let previous =
                |end: usize| text[..end].char_indices().next_back().map_or(0, |(i, _)| i);
            let c = if cursor > anchor {
                previous(cursor)
            } else {
                cursor
            };
            let a = if anchor > cursor {
                previous(anchor)
            } else {
                anchor
            };
            self.buffer.set_group(
                self.group,
                CursorGroup::new(
                    CursorState::Selection(
                        cursor_from_byte(text, c),
                        cursor_from_byte(text, a),
                        TargetShape::CharWise,
                    ),
                    vec![],
                ),
            );
        }
        Ok(())
    }
    pub fn key(&mut self, code: KeyCode, modifiers: KeyModifiers) -> Result<(), String> {
        self.machine
            .input_key(KeyEvent::new(code, modifiers).into());
        let viewport = ViewportContext {
            dimensions: (100, 30),
            ..Default::default()
        };
        let mut error = None;
        while let Some((action, ctx)) = self.machine.pop() {
            match action {
                Action::Editor(action) => {
                    if let Err(e) = self.buffer.editor_command(
                        &action,
                        &(self.group, &viewport, &ctx),
                        &mut self.store.borrow_mut(),
                    ) {
                        error = Some(e.to_string());
                    }
                }
                Action::NoOp => {}
                Action::Repeat(sequence) => self.machine.repeat(sequence, Some(ctx)),
                // These require additional app UI; never send their keys to the plain editor.
                _ => {
                    error = Some("Search, command-line and macro UI are not available yet; press Escape to return.".into());
                }
            }
        }
        error.map_or(Ok(()), Err)
    }
    pub fn paste(&mut self, text: &str) -> Result<(), String> {
        if self.mode() != VimMode::Insert {
            return Err("Enter Insert mode (i) to paste text.".into());
        }
        let viewport = ViewportContext {
            dimensions: (100, 30),
            ..Default::default()
        };
        let ctx = self.machine.context();
        let action = EditorAction::InsertText(InsertTextAction::Transcribe(
            text.into(),
            MoveDir1D::Previous,
            Count::Exact(1),
        ));
        self.buffer
            .editor_command(
                &action,
                &(self.group, &viewport, &ctx),
                &mut self.store.borrow_mut(),
            )
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
    pub fn positions(&mut self) -> (usize, usize) {
        let text = self.text();
        let leader = self.buffer.get_leader(self.group);
        let cursor = byte_from_cursor(&text, &leader);
        if let Some((start, end, shape)) = self.buffer.get_leader_selection(self.group) {
            let (mut a, mut b) = (
                byte_from_cursor(&text, &start),
                byte_from_cursor(&text, &end),
            );
            if shape == TargetShape::LineWise {
                a = text[..a].rfind('\n').map_or(0, |i| i + 1);
                b = text[b..].find('\n').map_or(text.len(), |i| b + i + 1);
            } else if b < text.len() {
                b += text[b..].chars().next().unwrap().len_utf8();
            }
            if cursor <= a { (a, b) } else { (b, a) }
        } else {
            (cursor, cursor)
        }
    }
    pub fn insert_mode(&self) -> bool {
        self.mode() == VimMode::Insert
    }
    pub fn apply_external(&mut self, text: &str, cursor: usize) -> Result<(), String> {
        let old = self.text();
        if old == text {
            self.place(text, cursor);
            return Ok(());
        }
        let mut prefix = old
            .bytes()
            .zip(text.bytes())
            .take_while(|(a, b)| a == b)
            .count();
        while !old.is_char_boundary(prefix) || !text.is_char_boundary(prefix) {
            prefix -= 1;
        }
        let mut suffix = old[prefix..]
            .bytes()
            .rev()
            .zip(text[prefix..].bytes().rev())
            .take_while(|(a, b)| a == b)
            .count();
        while !old.is_char_boundary(old.len() - suffix)
            || !text.is_char_boundary(text.len() - suffix)
        {
            suffix -= 1;
        }
        let old_end = old.len() - suffix;
        let viewport = ViewportContext {
            dimensions: (100, 30),
            ..Default::default()
        };
        let ctx = EditContextBuilder::default()
            .insert_style(Some(InsertStyle::Insert))
            .build();
        if old_end > prefix {
            let end = old[..old_end].char_indices().next_back().unwrap().0;
            self.buffer.set_group(
                self.group,
                CursorGroup::new(
                    CursorState::Selection(
                        cursor_from_byte(&old, end),
                        cursor_from_byte(&old, prefix),
                        TargetShape::CharWise,
                    ),
                    vec![],
                ),
            );
            let action = EditorAction::Edit(
                EditAction::Delete(Register::Blackhole.into(), Specifier::Contextual).into(),
                EditTarget::Selection,
            );
            self.buffer
                .editor_command(
                    &action,
                    &(self.group, &viewport, &ctx),
                    &mut self.store.borrow_mut(),
                )
                .map_err(|e| e.to_string())?;
        }
        self.buffer
            .set_leader(self.group, cursor_from_byte(&self.text(), prefix));
        let insert = &text[prefix..text.len() - suffix];
        if !insert.is_empty() {
            let action = EditorAction::InsertText(InsertTextAction::Transcribe(
                insert.into(),
                MoveDir1D::Previous,
                Count::Exact(1),
            ));
            self.buffer
                .editor_command(
                    &action,
                    &(self.group, &viewport, &ctx),
                    &mut self.store.borrow_mut(),
                )
                .map_err(|e| e.to_string())?;
        }
        self.buffer
            .set_leader(self.group, cursor_from_byte(text, cursor));
        Ok(())
    }
}
fn cursor_from_byte(text: &str, byte: usize) -> Cursor {
    let mut byte = byte.min(text.len());
    while !text.is_char_boundary(byte) {
        byte -= 1;
    }
    let prefix = &text[..byte];
    Cursor::new(
        prefix.bytes().filter(|b| *b == b'\n').count(),
        prefix.rsplit('\n').next().unwrap_or("").chars().count(),
    )
}
fn byte_from_cursor(text: &str, cursor: &Cursor) -> usize {
    let start: usize = text
        .split_inclusive('\n')
        .take(cursor.get_y())
        .map(str::len)
        .sum();
    let column: usize = text[start.min(text.len())..]
        .split('\n')
        .next()
        .unwrap_or("")
        .chars()
        .take(cursor.get_x())
        .map(char::len_utf8)
        .sum();
    (start + column).min(text.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn keys(vim: &mut VimEditor, text: &str) {
        for c in text.chars() {
            vim.key(KeyCode::Char(c), KeyModifiers::NONE).unwrap();
        }
    }
    #[test]
    fn modalkit_insert_motion_delete_undo_and_redo() {
        let mut vim = VimEditor::new("", 0);
        keys(&mut vim, "ione two");
        vim.key(KeyCode::Esc, KeyModifiers::NONE).unwrap();
        keys(&mut vim, "0dw");
        assert_eq!(vim.text(), "two");
        keys(&mut vim, "u");
        assert_eq!(vim.text(), "one two");
        vim.key(KeyCode::Char('r'), KeyModifiers::CONTROL).unwrap();
        assert_eq!(vim.text(), "two");
    }
    #[test]
    fn unicode_cursor_and_trailing_newlines_round_trip() {
        for text in ["", "हेलो\nworld", "hello\n\n"] {
            let mut vim = VimEditor::new(text, text.len());
            assert_eq!(vim.text(), text);
            assert_eq!(vim.positions().0, text.len());
        }
    }
    #[test]
    fn visual_delete_and_register_paste() {
        let mut vim = VimEditor::new("hello world", 0);
        keys(&mut vim, "vld");
        assert_eq!(vim.text(), "llo world");
        keys(&mut vim, "P");
        assert_eq!(vim.text(), "hello world");
    }
}
