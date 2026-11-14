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
pub const BG: Color = Color::rgba(28, 29, 38, 255);
pub const FG: Color = Color::rgba(222, 225, 233, 255);
pub const MUTED: Color = Color::rgba(140, 145, 164, 255);
pub const ACCENT: Color = Color::rgba(106, 164, 229, 255);
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
        }
    }
}
impl Metrics {
    pub fn caret(&self, doc: &Document, width: usize) -> Rect {
        Rect::new(
            self.area.x
                + self.gutter
                + 8.0
                + (doc.column(width) as f32 - doc.scroll_col as f32) * self.cell,
            self.area.y + 4.0 + (doc.row() as f32 - doc.scroll_row as f32) * self.line,
            2.0,
            self.line,
        )
    }
    pub fn hit(&self, doc: &Document, p: [f32; 2], width: usize) -> usize {
        let row = (doc.scroll_row as isize
            + ((p[1] - self.area.y - 4.0) / self.line).floor() as isize)
            .max(0) as usize;
        let col = (doc.scroll_col as f32 + (p[0] - self.area.x - self.gutter - 8.0) / self.cell)
            .round()
            .max(0.0) as usize;
        doc.byte_at_column(row.min(doc.lines() - 1), col, width)
    }
}
pub fn document(
    list: &mut DisplayList,
    measure: &mut dyn TextMeasurer,
    doc: &Document,
    settings: &Settings,
    area: Rect,
    state: (bool, bool, &str),
) -> Metrics {
    let (focused, normal, preedit) = state;
    let ts = style(settings.font_size as f32, true, FG);
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
    };
    rect(list, area, BG, area);
    if gutter > 0.0 {
        rect(
            list,
            Rect::new(area.x, area.y, gutter, area.height),
            Color::rgba(25, 26, 34, 255),
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
    for row in doc.scroll_row..(doc.scroll_row + metrics.rows + 2).min(doc.lines()) {
        let y = area.y + 4.0 + (row - doc.scroll_row) as f32 * line;
        if settings.highlight_line && row == doc.row() {
            rect(
                list,
                Rect::new(content.x, y, content.width, line),
                Color::rgba(35, 41, 55, 255),
                content,
            );
        }
        if settings.line_numbers {
            let number = if settings.relative_numbers && row != doc.row() {
                row.abs_diff(doc.row())
            } else {
                row + 1
            };
            let mut ns = ts.clone();
            ns.color = if row == doc.row() { FG } else { MUTED };
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
            if endcol >= doc.scroll_col && col <= doc.scroll_col + metrics.cols + 2 {
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
                            content.x + 8.0 + (col as f32 - doc.scroll_col as f32) * cell,
                            y,
                            count.max(1) as f32 * cell,
                            line,
                        ),
                        Color::rgba(55, 83, 121, 255),
                        content,
                    );
                }
                if settings.show_whitespace && matches!(g, " " | "\t") {
                    text(
                        list,
                        Rect::new(
                            content.x + 8.0 + (col as f32 - doc.scroll_col as f32) * cell,
                            y,
                            count as f32 * cell,
                            line,
                        ),
                        if g == "\t" { "→" } else { "·" },
                        style(settings.font_size as f32, true, MUTED),
                        content,
                    );
                }
            }
            col = endcol;
            if col > doc.scroll_col + metrics.cols + 3 {
                break;
            }
        }
        if selection.start <= doc.end(row) && selection.end > doc.end(row) {
            rect(
                list,
                Rect::new(
                    content.x + 8.0 + (col as f32 - doc.scroll_col as f32) * cell,
                    y,
                    cell,
                    line,
                ),
                Color::rgba(55, 83, 121, 255),
                content,
            );
        }
        text(
            list,
            Rect::new(
                content.x + 8.0 + (draw_col as f32 - doc.scroll_col as f32) * cell,
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
                Color::rgba(165, 186, 211, 100),
                content,
            );
        } else {
            rect(list, caret, ACCENT, content);
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
                ACCENT,
                content,
            );
        }
    }
    // Vertical scrollbar and horizontal scroll position share the document viewport.
    if doc.lines() > metrics.rows {
        let h = (area.height * metrics.rows as f32 / doc.lines() as f32).max(22.0);
        let y = area.y
            + (area.height - h)
                * (doc.scroll_row as f32 / (doc.lines() - metrics.rows) as f32).min(1.0);
        rect(
            list,
            Rect::new(area.x + area.width - 8.0, y, 6.0, h),
            Color::rgba(85, 91, 108, 255),
            area,
        );
    }
    if doc.scroll_col > 0
        || (doc.scroll_row..(doc.scroll_row + metrics.rows).min(doc.lines()))
            .any(|r| crate::editor::columns(doc.line(r), width) > metrics.cols)
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
            Color::rgba(85, 91, 108, 255),
            area,
        );
    }
    metrics
}
