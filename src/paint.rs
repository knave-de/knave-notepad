//! Application surfaces use Knave UI display commands and the renderer's font metrics.
use crate::{
    editor::{Document, cells},
    store::Settings,
};
use knave_ui::toolkit::TextMeasurer;
use knave_ui::{
    Color, DisplayCommand, DisplayList, Rect, ShapePaint, TextAlign, TextStyle, TextWrap,
    Transform2D,
};
use unicode_segmentation::UnicodeSegmentation;
/// Colors resolved from the scene, including desktop and user overrides.
#[derive(Clone, Copy)]
pub struct Palette {
    pub bg: Color,
    pub fg: Color,
    pub muted: Color,
    pub accent: Color,
    pub on_accent: Color,
    pub surface: Color,
    pub selected: Color,
}
impl Palette {
    pub fn from_scene(scene: &knave_ui::toolkit::Scene) -> Self {
        let style = scene
            .computed_style(knave_ui::toolkit::ElementId(0))
            .unwrap()
            .style;
        let role = |id| {
            scene
                .computed_style(knave_ui::toolkit::ElementId(id))
                .unwrap()
                .style
                .text
                .color
        };
        Self {
            bg: style
                .background
                .and_then(|p| p.fill)
                .unwrap_or(Color::BACKGROUND),
            fg: style.text.color,
            muted: scene
                .computed_style(knave_ui::toolkit::ElementId(21))
                .unwrap()
                .style
                .text
                .color,
            accent: role(90003),
            on_accent: role(90004),
            surface: role(90001),
            selected: role(90002),
        }
    }
}
pub fn rect(list: &mut DisplayList, bounds: Rect, color: Color, clip: Rect) {
    if bounds.width <= 0.0 || bounds.height <= 0.0 {
        return;
    }
    list.commands.push(DisplayCommand::Shape {
        bounds,
        paint: ShapePaint::fill(color),
        transform: Transform2D::IDENTITY,
        clip: Some(clip),
    });
}
pub fn text(
    list: &mut DisplayList,
    bounds: Rect,
    value: impl Into<String>,
    style: TextStyle,
    clip: Rect,
) {
    list.commands.push(DisplayCommand::Text {
        bounds,
        text: value.into(),
        style,
        transform: Transform2D::IDENTITY,
        clip: Some(clip),
    });
}
pub fn style(size: f32, mono: bool, color: Color) -> TextStyle {
    TextStyle {
        font_family: if mono { "monospace" } else { "sans-serif" }.into(),
        font_size: size,
        line_height: (size * 1.45).round(),
        wrap: TextWrap::None,
        align: TextAlign::Start,
        color,
        ..Default::default()
    }
}
pub fn contains(r: Rect, p: [f32; 2]) -> bool {
    p[0] >= r.x && p[0] < r.x + r.width && p[1] >= r.y && p[1] < r.y + r.height
}
#[derive(Clone, Copy)]
pub struct Metrics {
    pub area: Rect,
    pub gutter: f32,
    pub cell: f32,
    pub line: f32,
    pub rows: usize,
    pub cols: usize,
    pub wrap: bool,
}
impl Default for Metrics {
    fn default() -> Self {
        Self {
            area: Rect::default(),
            gutter: 50.0,
            cell: 9.0,
            line: 23.0,
            rows: 24,
            cols: 80,
            wrap: false,
        }
    }
}
// Each visual row retains logical columns for tab expansion and byte hit testing.
#[derive(Debug)]
struct VisualRow {
    row: usize,
    start: usize,
    end: usize,
}
impl Metrics {
    fn visual_rows(&self, doc: &Document, width: usize) -> Vec<VisualRow> {
        let mut rows = Vec::new();
        for row in 0..doc.lines() {
            if !self.wrap {
                rows.push(VisualRow {
                    row,
                    start: 0,
                    end: usize::MAX,
                });
                continue;
            }
            let mut start = 0;
            let mut col = 0;
            let mut boundary = None;
            let glyphs: Vec<_> = doc.line(row).graphemes(true).collect();
            let mut index = 0;
            let mut segment_index = 0;
            while index < glyphs.len() {
                let g = glyphs[index];
                let next = col + cells(g, col, width);
                if self.wrap && next > start + self.cols && index > segment_index {
                    let (split_index, split_col) = boundary
                        .filter(|(i, _)| *i > segment_index)
                        .unwrap_or((index, col));
                    rows.push(VisualRow {
                        row,
                        start,
                        end: split_col,
                    });
                    start = split_col;
                    col = split_col;
                    index = split_index;
                    segment_index = index;
                    boundary = None;
                    continue;
                }
                col = next;
                index += 1;
                if g.chars().all(char::is_whitespace) {
                    boundary = Some((index, col));
                }
            }
            rows.push(VisualRow {
                row,
                start,
                end: col,
            });
        }
        rows
    }
    pub fn total_rows(&self, doc: &Document, width: usize) -> usize {
        if self.wrap {
            self.visual_rows(doc, width).len()
        } else {
            doc.lines()
        }
    }
    fn position(&self, doc: &Document, width: usize) -> (usize, usize) {
        let col = doc.column(width);
        if !self.wrap {
            return (doc.row(), col);
        }
        let rows = self.visual_rows(doc, width);
        let index = rows
            .iter()
            .rposition(|r| r.row == doc.row() && r.start <= col)
            .unwrap_or(0);
        (index, col - rows[index].start)
    }
    pub fn vertical(&self, doc: &mut Document, delta: isize, extend: bool, width: usize) {
        if !self.wrap {
            doc.vertical(delta, extend, width);
            return;
        }
        let rows = self.visual_rows(doc, width);
        let (row, col) = self.position(doc, width);
        let target = &rows[row.saturating_add_signed(delta).min(rows.len() - 1)];
        let byte = doc.byte_at_column(target.row, (target.start + col).min(target.end), width);
        doc.place(byte, extend);
    }
    pub fn reveal(&self, doc: &mut Document, width: usize) {
        if !self.wrap {
            doc.reveal(self.rows, self.cols, width);
            return;
        }
        doc.scroll_col = 0;
        let (row, _) = self.position(doc, width);
        if row < doc.scroll_row {
            doc.scroll_row = row;
        }
        if row >= doc.scroll_row + self.rows {
            doc.scroll_row = row + 1 - self.rows;
        }
    }

    pub fn caret(&self, doc: &Document, width: usize) -> Rect {
        let (row, col) = self.position(doc, width);
        Rect::new(
            self.area.x
                + self.gutter
                + 8.0
                + (col as f32
                    - if self.wrap {
                        0.0
                    } else {
                        doc.scroll_col as f32
                    })
                    * self.cell,
            self.area.y + 4.0 + (row as f32 - doc.scroll_row as f32) * self.line,
            2.0,
            self.line,
        )
    }
    pub fn hit(&self, doc: &Document, p: [f32; 2], width: usize) -> usize {
        let row = (doc.scroll_row as isize
            + ((p[1] - self.area.y - 4.0) / self.line).floor() as isize)
            .max(0) as usize;
        let col = ((if self.wrap {
            0.0
        } else {
            doc.scroll_col as f32
        }) + (p[0] - self.area.x - self.gutter - 8.0) / self.cell)
            .round()
            .max(0.0) as usize;
        if self.wrap {
            let rows = self.visual_rows(doc, width);
            let visual = &rows[row.min(rows.len() - 1)];
            return doc.byte_at_column(visual.row, (visual.start + col).min(visual.end), width);
        }
        doc.byte_at_column(row.min(doc.lines() - 1), col, width)
    }
}
pub fn document(
    list: &mut DisplayList,
    measure: &mut dyn TextMeasurer,
    doc: &mut Document,
    settings: &Settings,
    area: Rect,
    state: (bool, bool, &str),
    palette: Palette,
) -> Metrics {
    let (focused, normal, preedit) = state;
    let ts = style(settings.font_size as f32, true, palette.fg);
    let cell = measure.measure("0000000000", &ts, 10000.0).size[0] / 10.0;
    let cell = cell.max(1.0);
    let line = ts.line_height;
    let gutter = if settings.line_numbers {
        ((doc.lines().to_string().len().max(3) + 2) as f32 * cell).ceil()
    } else {
        0.0
    };
    let metrics = Metrics {
        area,
        gutter,
        cell,
        line,
        rows: ((area.height - 12.0) / line).floor().max(1.0) as usize,
        cols: ((area.width - gutter - 24.0) / cell).floor().max(1.0) as usize,
        wrap: settings.word_wrap,
    };
    rect(list, area, palette.bg, area);
    if gutter > 0.0 {
        rect(
            list,
            Rect::new(area.x, area.y, gutter, area.height),
            palette.surface,
            area,
        );
    }
    let content = Rect::new(
        area.x + gutter,
        area.y,
        (area.width - gutter - 10.0).max(0.0),
        area.height - 10.0,
    );
    let selection = doc.selection();
    let width = settings.tab_width as usize;
    let visual_rows = metrics.visual_rows(doc, width);
    doc.scroll_row = doc.scroll_row.min(visual_rows.len().saturating_sub(1));
    if metrics.wrap {
        doc.scroll_col = 0;
    }
    for (visual_index, visual) in visual_rows
        .iter()
        .enumerate()
        .skip(doc.scroll_row)
        .take(metrics.rows + 2)
    {
        let row = visual.row;
        let scroll_col = if metrics.wrap {
            visual.start
        } else {
            doc.scroll_col
        };
        let y = area.y + 4.0 + (visual_index - doc.scroll_row) as f32 * line;
        if settings.highlight_line && row == doc.row() {
            rect(
                list,
                Rect::new(content.x, y, content.width, line),
                palette.surface,
                content,
            );
        }
        if settings.line_numbers && visual.start == 0 {
            let number = if settings.relative_numbers && row != doc.row() {
                row.abs_diff(doc.row())
            } else {
                row + 1
            };
            let mut ns = ts.clone();
            ns.color = if row == doc.row() {
                palette.fg
            } else {
                palette.muted
            };
            ns.align = TextAlign::End;
            text(
                list,
                Rect::new(area.x + 2.0, y, gutter - 12.0, line),
                number.to_string(),
                ns,
                Rect::new(area.x, area.y, gutter, area.height),
            );
        }
        let start = doc.start(row);
        let mut col = 0;
        let mut displayed = String::new();
        let mut draw_col = 0;
        let mut begun = false;
        for (byte, g) in doc.line(row).grapheme_indices(true) {
            let count = cells(g, col, width);
            let endcol = col + count;
            if endcol > scroll_col
                && (!metrics.wrap || col < visual.end)
                && col <= scroll_col + metrics.cols + 2
            {
                if !begun {
                    draw_col = col;
                    begun = true;
                }
                if g == "\t" {
                    displayed.push_str(&" ".repeat(count));
                } else {
                    displayed.push_str(g);
                }
                if start + byte < selection.end && start + byte + g.len() > selection.start {
                    rect(
                        list,
                        Rect::new(
                            content.x + 8.0 + (col as f32 - scroll_col as f32) * cell,
                            y,
                            count.max(1) as f32 * cell,
                            line,
                        ),
                        palette.selected,
                        content,
                    );
                }
                if settings.show_whitespace && matches!(g, " " | "\t") {
                    text(
                        list,
                        Rect::new(
                            content.x + 8.0 + (col as f32 - scroll_col as f32) * cell,
                            y,
                            count as f32 * cell,
                            line,
                        ),
                        if g == "\t" { "→" } else { "·" },
                        style(settings.font_size as f32, true, palette.muted),
                        content,
                    );
                }
            }
            col = endcol;
            if col > scroll_col + metrics.cols + 3 {
                break;
            }
        }
        if (!metrics.wrap || visual.end == crate::editor::columns(doc.line(row), width))
            && selection.start <= doc.end(row)
            && selection.end > doc.end(row)
        {
            rect(
                list,
                Rect::new(
                    content.x + 8.0 + (col as f32 - scroll_col as f32) * cell,
                    y,
                    cell,
                    line,
                ),
                palette.selected,
                content,
            );
        }
        text(
            list,
            Rect::new(
                content.x + 8.0 + (draw_col as f32 - scroll_col as f32) * cell,
                y,
                (metrics.cols + 5) as f32 * cell,
                line,
            ),
            displayed,
            ts.clone(),
            content,
        );
    }
    if focused {
        let caret = metrics.caret(doc, width);
        if normal {
            rect(
                list,
                Rect::new(caret.x, caret.y, cell, line),
                palette.accent,
                content,
            );
            let glyph = doc.text[doc.cursor..].graphemes(true).next().unwrap_or(" ");
            if glyph != "\n" && glyph != "\t" {
                text(
                    list,
                    Rect::new(caret.x, caret.y, cell * 2.0, line),
                    glyph,
                    style(settings.font_size as f32, true, palette.on_accent),
                    content,
                );
            }
        } else {
            rect(list, caret, palette.accent, content);
        }
        if !preedit.is_empty() {
            text(
                list,
                Rect::new(caret.x, caret.y, content.width, line),
                preedit,
                ts,
                content,
            );
            rect(
                list,
                Rect::new(
                    caret.x,
                    caret.y + line - 2.0,
                    preedit.graphemes(true).count() as f32 * cell,
                    1.0,
                ),
                palette.accent,
                content,
            );
        }
    }
    // Vertical scrollbar and horizontal scroll position share the document viewport.
    let total_rows = visual_rows.len();
    if total_rows > metrics.rows {
        let h = (area.height * metrics.rows as f32 / total_rows as f32).max(22.0);
        let y = area.y
            + (area.height - h)
                * (doc.scroll_row as f32 / (total_rows - metrics.rows) as f32).min(1.0);
        rect(
            list,
            Rect::new(area.x + area.width - 8.0, y, 6.0, h),
            palette.muted,
            area,
        );
    }
    if !metrics.wrap
        && (doc.scroll_col > 0
            || (doc.scroll_row..(doc.scroll_row + metrics.rows).min(doc.lines()))
                .any(|r| crate::editor::columns(doc.line(r), width) > metrics.cols))
    {
        let longest = (doc.scroll_row..(doc.scroll_row + metrics.rows).min(doc.lines()))
            .map(|r| crate::editor::columns(doc.line(r), width))
            .max()
            .unwrap_or(metrics.cols)
            .max(metrics.cols + doc.scroll_col);
        let w = (content.width * metrics.cols as f32 / longest as f32).max(22.0);
        let x = content.x
            + (content.width - w) * doc.scroll_col as f32 / (longest - metrics.cols).max(1) as f32;
        rect(
            list,
            Rect::new(x, area.y + area.height - 7.0, w, 5.0),
            palette.muted,
            area,
        );
    }
    metrics
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrapping_preserves_words_and_maps_caret_and_hits() {
        let mut doc = Document::new("hello world abcdefghijk\n\t界é".into());
        let metrics = Metrics {
            wrap: true,
            cols: 8,
            rows: 2,
            gutter: 0.0,
            cell: 10.0,
            line: 20.0,
            ..Default::default()
        };
        let rows = metrics.visual_rows(&doc, 4);
        assert_eq!(
            rows.iter()
                .map(|r| (r.row, r.start, r.end))
                .collect::<Vec<_>>(),
            vec![(0, 0, 6), (0, 6, 12), (0, 12, 20), (0, 20, 23), (1, 0, 7)]
        );
        doc.place(8, false);
        assert_eq!(metrics.position(&doc, 4), (1, 2));
        assert_eq!(metrics.hit(&doc, [28.0, 24.0], 4), 8);
        doc.place(23, false);
        metrics.reveal(&mut doc, 4);
        assert_eq!(doc.scroll_row, 2);
        assert_eq!(doc.scroll_col, 0);
        assert_eq!(metrics.caret(&doc, 4).y, 24.0);
        assert_eq!(doc.text, "hello world abcdefghijk\n\t界é");
    }
}
