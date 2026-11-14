//! Document editing, independent of widgets and rendering. All offsets are UTF-8 bytes.
use crate::store::Settings;
use std::{ops::Range, time::Instant};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

#[derive(Clone, Debug)]
struct State {
    text: String,
    cursor: usize,
    anchor: usize,
}
#[derive(Default)]
pub struct Document {
    pub text: String,
    pub cursor: usize,
    pub anchor: usize,
    pub scroll_row: usize,
    pub scroll_col: usize,
    pub overwrite: bool,
    pub revision: u64,
    starts: Vec<usize>,
    undo: Vec<State>,
    redo: Vec<State>,
    typing: Option<Instant>,
    preferred: Option<usize>,
}
impl Document {
    pub fn new(text: &str) -> Self {
        let mut doc = Self {
            text: normalize(text),
            ..Default::default()
        };
        doc.reindex();
        doc
    }
    fn state(&self) -> State {
        State {
            text: self.text.clone(),
            cursor: self.cursor,
            anchor: self.anchor,
        }
    }
    fn restore(&mut self, state: State) {
        self.text = state.text;
        self.cursor = state.cursor;
        self.anchor = state.anchor;
        self.typing = None;
        self.preferred = None;
        self.reindex();
        self.revision += 1;
    }
    fn checkpoint(&mut self, typing: bool) {
        let grouped = typing && self.typing.is_some_and(|t| t.elapsed().as_millis() < 750);
        if !grouped {
            self.undo.push(self.state());
            while self.undo.len() > 1
                && (self.undo.len() > 200
                    || self
                        .undo
                        .iter()
                        .map(|state| state.text.len())
                        .sum::<usize>()
                        > 32 * 1024 * 1024)
            {
                self.undo.remove(0);
            }
        }
        self.redo.clear();
        self.typing = typing.then(Instant::now);
    }
    pub fn break_group(&mut self) {
        self.typing = None;
    }
    pub fn undo(&mut self, redo: bool) {
        let state = if redo {
            self.redo.pop()
        } else {
            self.undo.pop()
        };
        if let Some(state) = state {
            if redo {
                self.undo.push(self.state());
            } else {
                self.redo.push(self.state());
            }
            self.restore(state);
        }
    }
    pub fn set_external(&mut self, text: String, cursor: usize, anchor: usize) {
        if self.text != text {
            self.text = text;
            self.reindex();
            self.revision += 1;
        }
        self.cursor = self.boundary(cursor);
        self.anchor = self.boundary(anchor);
    }
    fn boundary(&self, mut byte: usize) -> usize {
        byte = byte.min(self.text.len());
        while !self.text.is_char_boundary(byte) {
            byte -= 1;
        }
        byte
    }
    fn reindex(&mut self) {
        self.starts.clear();
        self.starts.push(0);
        self.starts
            .extend(self.text.match_indices('\n').map(|(i, _)| i + 1));
    }
    pub fn lines(&self) -> usize {
        self.starts.len()
    }
    pub fn row_at(&self, byte: usize) -> usize {
        self.starts
            .partition_point(|s| *s <= byte.min(self.text.len()))
            .saturating_sub(1)
    }
    pub fn row(&self) -> usize {
        self.row_at(self.cursor)
    }
    pub fn start(&self, row: usize) -> usize {
        *self.starts.get(row).unwrap_or(&self.text.len())
    }
    pub fn end(&self, row: usize) -> usize {
        self.starts.get(row + 1).map_or(self.text.len(), |s| s - 1)
    }
    pub fn line(&self, row: usize) -> &str {
        &self.text[self.start(row)..self.end(row)]
    }
    pub fn selection(&self) -> Range<usize> {
        self.anchor.min(self.cursor)..self.anchor.max(self.cursor)
    }
    pub fn selected(&self) -> &str {
        &self.text[self.selection()]
    }
    pub fn column(&self, width: usize) -> usize {
        columns(&self.text[self.start(self.row())..self.cursor], width)
    }
    pub fn place(&mut self, byte: usize, extend: bool) {
        self.cursor = self.boundary(byte);
        if !extend {
            self.anchor = self.cursor;
        }
        self.preferred = None;
        self.break_group();
    }
    pub fn byte_at_column(&self, row: usize, target: usize, width: usize) -> usize {
        let mut col = 0;
        for (byte, g) in self.line(row).grapheme_indices(true) {
            let next = col + cells(g, col, width);
            if target < next {
                return self.start(row)
                    + if target - col >= (next - col).div_ceil(2) {
                        byte + g.len()
                    } else {
                        byte
                    };
            }
            col = next;
        }
        self.end(row)
    }
    pub fn insert(&mut self, text: &str, typing: bool) {
        let text = normalize(text);
        if text.is_empty() {
            return;
        }
        self.checkpoint(typing && !text.contains('\n'));
        let mut range = self.selection();
        if self.overwrite && range.is_empty() && !text.contains('\n') {
            range.end = self.text[self.cursor..]
                .graphemes(true)
                .take(text.graphemes(true).count())
                .take_while(|g| *g != "\n")
                .map(str::len)
                .sum::<usize>()
                + self.cursor;
        }
        self.text.replace_range(range.clone(), &text);
        self.cursor = range.start + text.len();
        self.anchor = self.cursor;
        self.preferred = None;
        self.reindex();
        self.revision += 1;
        if text.ends_with(char::is_whitespace) {
            self.break_group();
        }
    }
    pub fn delete(&mut self, forward: bool, word: bool, settings: &Settings) {
        if self.selection().is_empty() {
            let next = if word {
                self.word_boundary(forward)
            } else if !forward
                && settings.spaces
                && self.cursor > self.start(self.row())
                && self.text[self.start(self.row())..self.cursor]
                    .chars()
                    .all(|c| c == ' ')
            {
                let col = self.column(settings.tab_width as usize);
                self.cursor - ((col - 1) % settings.tab_width as usize + 1)
            } else {
                self.adjacent(forward)
            };
            if next == self.cursor {
                return;
            }
            self.anchor = next;
        }
        self.checkpoint(false);
        let range = self.selection();
        self.text.replace_range(range.clone(), "");
        self.cursor = range.start;
        self.anchor = self.cursor;
        self.reindex();
        self.revision += 1;
        self.preferred = None;
    }
    fn adjacent(&self, forward: bool) -> usize {
        if forward {
            self.cursor
                + self.text[self.cursor..]
                    .graphemes(true)
                    .next()
                    .map_or(0, str::len)
        } else {
            self.text[..self.cursor]
                .grapheme_indices(true)
                .next_back()
                .map_or(0, |(i, _)| i)
        }
    }
    pub fn word_boundary(&self, forward: bool) -> usize {
        let class = |g: &str| {
            if g.chars().all(char::is_whitespace) {
                0
            } else if g.chars().any(|c| c.is_alphanumeric() || c == '_') {
                1
            } else {
                2
            }
        };
        if forward {
            let mut iter = self.text[self.cursor..].grapheme_indices(true).peekable();
            let Some((_, first)) = iter.next() else {
                return self.cursor;
            };
            let kind = class(first);
            while iter.peek().is_some_and(|(_, g)| class(g) == kind) {
                iter.next();
            }
            while iter.peek().is_some_and(|(_, g)| class(g) == 0) {
                iter.next();
            }
            iter.peek()
                .map_or(self.text.len(), |(i, _)| self.cursor + i)
        } else {
            let mut iter = self.text[..self.cursor]
                .grapheme_indices(true)
                .rev()
                .peekable();
            while iter.peek().is_some_and(|(_, g)| class(g) == 0) {
                iter.next();
            }
            let Some((mut result, g)) = iter.next() else {
                return 0;
            };
            let kind = class(g);
            while iter.peek().is_some_and(|(_, g)| class(g) == kind) {
                result = iter.next().unwrap().0;
            }
            result
        }
    }
    pub fn horizontal(&mut self, forward: bool, word: bool, extend: bool) {
        let range = self.selection();
        let next = if !extend && !range.is_empty() && !word {
            if forward { range.end } else { range.start }
        } else if word {
            self.word_boundary(forward)
        } else {
            self.adjacent(forward)
        };
        self.place(next, extend);
    }
    pub fn vertical(&mut self, delta: isize, extend: bool, width: usize) {
        let col = self.preferred.unwrap_or_else(|| self.column(width));
        let row = self
            .row()
            .saturating_add_signed(delta)
            .min(self.lines() - 1);
        let next = self.byte_at_column(row, col, width);
        self.place(next, extend);
        self.preferred = Some(col);
    }
    pub fn home(&mut self, end: bool, whole: bool, extend: bool) {
        let next = if whole {
            if end { self.text.len() } else { 0 }
        } else if end {
            self.end(self.row())
        } else {
            let start = self.start(self.row());
            let indent = self.line(self.row()).len()
                - self.line(self.row()).trim_start_matches([' ', '\t']).len();
            if self.cursor == start + indent {
                start
            } else {
                start + indent
            }
        };
        self.place(next, extend);
    }
    pub fn newline(&mut self, settings: &Settings) {
        let prefix = &self.text[self.start(self.row())..self.cursor];
        let indent: String = if settings.auto_indent {
            prefix
                .chars()
                .take_while(|c| matches!(c, ' ' | '\t'))
                .collect()
        } else {
            String::new()
        };
        self.insert(&format!("\n{indent}"), false);
    }
    pub fn indent(&mut self, outdent: bool, settings: &Settings) {
        let width = settings.tab_width as usize;
        if self.selection().is_empty() && !outdent {
            let indent = if settings.spaces {
                " ".repeat(width - self.column(width) % width)
            } else {
                "\t".into()
            };
            self.insert(&indent, false);
            return;
        }
        let range = self.selection();
        let first = self.row_at(range.start);
        let last = self.row_at(
            if range.end > range.start && self.text[..range.end].ends_with('\n') {
                range.end - 1
            } else {
                range.end
            },
        );
        self.checkpoint(false);
        let mut anchor = self.anchor;
        let mut cursor = self.cursor;
        for row in (first..=last).rev() {
            let start = self.start(row);
            let remove = if outdent {
                if self.line(row).starts_with('\t') {
                    1
                } else {
                    self.line(row)
                        .chars()
                        .take(width)
                        .take_while(|c| *c == ' ')
                        .count()
                }
            } else {
                0
            };
            let add = if outdent {
                String::new()
            } else if settings.spaces {
                " ".repeat(width)
            } else {
                "\t".into()
            };
            self.text.replace_range(start..start + remove, &add);
            let adjust = |byte: usize| {
                if byte >= start {
                    start + byte.saturating_sub(start + remove) + add.len()
                } else {
                    byte
                }
            };
            anchor = adjust(anchor);
            cursor = adjust(cursor);
        }
        self.reindex();
        self.cursor = self.boundary(cursor);
        self.anchor = self.boundary(anchor);
        self.revision += 1;
    }
    pub fn select_all(&mut self) {
        self.anchor = 0;
        self.cursor = self.text.len();
        self.break_group();
    }
    pub fn select_word(&mut self, byte: usize) {
        self.place(byte, false);
        let class = |c: char| c.is_alphanumeric() || c == '_';
        let start = self.text[..self.cursor]
            .char_indices()
            .rev()
            .take_while(|(_, c)| class(*c))
            .last()
            .map_or(self.cursor, |(i, _)| i);
        let end = self.cursor
            + self.text[self.cursor..]
                .chars()
                .take_while(|c| class(*c))
                .map(char::len_utf8)
                .sum::<usize>();
        self.anchor = start;
        self.cursor = end;
    }
    pub fn select_line(&mut self, row: usize) {
        self.anchor = self.start(row);
        self.cursor = self.starts.get(row + 1).copied().unwrap_or(self.text.len());
    }
    pub fn find(&mut self, query: &str, backwards: bool) -> bool {
        if query.is_empty() {
            return false;
        }
        let found = if backwards {
            self.text[..self.selection().start]
                .rfind(query)
                .or_else(|| self.text.rfind(query))
        } else {
            self.text[self.selection().end..]
                .find(query)
                .map(|i| self.selection().end + i)
                .or_else(|| self.text.find(query))
        };
        if let Some(start) = found {
            self.anchor = start;
            self.cursor = start + query.len();
            self.break_group();
            true
        } else {
            false
        }
    }
    pub fn replace_all(&mut self, query: &str, replacement: &str) -> usize {
        if query.is_empty() {
            return 0;
        }
        let count = self.text.matches(query).count();
        if count == 0 {
            return 0;
        }
        self.checkpoint(false);
        self.text = self.text.replace(query, &normalize(replacement));
        self.cursor = self.cursor.min(self.text.len());
        self.cursor = self.boundary(self.cursor);
        self.anchor = self.cursor;
        self.reindex();
        self.revision += 1;
        count
    }
    pub fn reveal(&mut self, rows: usize, cols: usize, width: usize) {
        let row = self.row();
        let col = self.column(width);
        if row < self.scroll_row {
            self.scroll_row = row;
        } else if row >= self.scroll_row + rows.max(1) {
            self.scroll_row = row + 1 - rows.max(1);
        }
        if col < self.scroll_col {
            self.scroll_col = col;
        } else if col >= self.scroll_col + cols.max(1) {
            self.scroll_col = col + 1 - cols.max(1);
        }
    }
}
pub fn normalize(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}
pub fn cells(g: &str, col: usize, width: usize) -> usize {
    if g == "\t" {
        width.max(1) - col % width.max(1)
    } else {
        UnicodeWidthStr::width(g)
    }
}
pub fn columns(text: &str, width: usize) -> usize {
    text.graphemes(true)
        .fold(0, |col, g| col + cells(g, col, width))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn newlines_indentation_and_undo() {
        let s = Settings::default();
        let mut d = Document::new("    hello");
        d.place(d.text.len(), false);
        d.newline(&s);
        assert_eq!(d.text, "    hello\n    ");
        d.insert("world", true);
        d.undo(false);
        assert_eq!(d.text, "    hello\n    ");
        d.undo(false);
        assert_eq!(d.text, "    hello");
        d.undo(true);
        assert_eq!(d.lines(), 2);
    }
    #[test]
    fn tabs_spaces_selection_indent_and_outdent() {
        let mut d = Document::new("a\nb\n");
        d.select_all();
        d.indent(false, &Settings::default());
        assert_eq!(d.text, "    a\n    b\n");
        d.indent(true, &Settings::default());
        assert_eq!(d.text, "a\nb\n");
        let s = Settings {
            spaces: false,
            ..Default::default()
        };
        d.place(0, false);
        d.indent(false, &s);
        assert!(d.text.starts_with('\t'));
    }
    #[test]
    fn unicode_graphemes_vertical_goal_and_crlf() {
        let mut d = Document::new("a👩‍💻e\u{301}\r\nx\nabcdef");
        assert_eq!(d.lines(), 3);
        d.place(1, false);
        d.horizontal(true, false, false);
        assert_eq!(&d.text[1..d.cursor], "👩‍💻");
        d.horizontal(true, false, false);
        assert!(d.text[..d.cursor].ends_with("e\u{301}"));
        d.vertical(1, false, 4);
        assert_eq!(d.column(4), 1);
        d.vertical(1, false, 4);
        assert_eq!(d.column(4), 4);
    }
    #[test]
    fn large_documents_and_long_lines_have_no_widget_limit() {
        let text = "x".repeat(100_000) + "\n" + &"line\n".repeat(20_000);
        let mut d = Document::new(&text);
        d.place(d.text.len(), false);
        d.insert("tail", false);
        assert_eq!(d.lines(), 20_002);
        assert!(d.text.ends_with("tail"));
        d.undo(false);
        assert_eq!(d.text, text);
    }
    #[test]
    fn find_replace_delete_and_word_movement() {
        let mut d = Document::new("hello world\nhello");
        assert!(d.find("hello", false));
        assert_eq!(d.selected(), "hello");
        assert_eq!(d.replace_all("hello", "hi"), 2);
        assert_eq!(d.text, "hi world\nhi");
        d.place(0, false);
        d.horizontal(true, true, false);
        assert_eq!(d.cursor, 3);
        d.delete(false, true, &Settings::default());
        assert_eq!(d.text, "world\nhi");
    }
}
