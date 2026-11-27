//! A ratatui [`Buffer`] as an SVG "terminal window", in the style of the esp32-logcat manual
//! (Rich/Textual's export): a dark rounded frame with a centred title and the three window
//! buttons, a monospaced grid of 12.2 × 24.4 px cells, one `<text>` per run of cells with the
//! same style, each clipped to its line.
//!
//! Deterministic: no timestamp, no hash of the content, CSS classes numbered by first use, and
//! every coordinate computed in tenths of a pixel with integers (so no float formatting).
//!
//! # Colours
//! The terminal's named colours, as a dark theme shows them (Rich's MONOKAI for the ANSI
//! colours, Textual's background and foreground, as in the esp32-logcat images):
//!
//! | ratatui                     | hex       | used for (in this UI)                 |
//! |-----------------------------|-----------|---------------------------------------|
//! | background (`Reset`)        | `#292929` | the window                            |
//! | foreground (`Reset`)        | `#c5c8c6` | plain text                            |
//! | foreground, bold            | `#e0e0e0` | titles, keys, chip names              |
//! | `Black`                     | `#1a1a1a` |                                       |
//! | `Red` / `LightRed`          | `#f4005f` / `#ff4f8b` | `✕`, FAIL               |
//! | `Green` / `LightGreen`      | `#98e024` / `#b6f05a` | `✓`, PASS               |
//! | `Yellow` / `LightYellow`    | `#fd971f` / `#e0d561` | `○`, `?`, findings `!`  |
//! | `Blue` / `LightBlue`        | `#9d65ff` / `#b994ff` | the shell prompt        |
//! | `Magenta` / `LightMagenta`  | `#d670d6` / `#e8a0e8` |                         |
//! | `Cyan` / `LightCyan`        | `#58d1eb` / `#8ee3f5` | `▸ ●`, spinner          |
//! | `Gray`                      | `#c4c5b5` |                                       |
//! | `DarkGray`                  | `#8a8d8f` | rules, labels, reasons                |
//! | `White`                     | `#f6f6ef` |                                       |
//! | `Indexed(0..=15)`           | the 16 above, in ANSI order               |
//! | `Indexed(16..=255)`         | the xterm 6×6×6 cube and gray ramp        |
//! | `Rgb(r, g, b)`              | `#rrggbb` |                                       |

use ratatui::buffer::Buffer;
use ratatui::style::{Color, Modifier};

/// Window background.
const BACKGROUND: &str = "#292929";
/// Default foreground.
const FOREGROUND: &str = "#c5c8c6";
/// Default foreground in bold.
const FOREGROUND_BOLD: &str = "#e0e0e0";

/// Cell width, in tenths of a pixel (12.2 px).
const CELL_W: u32 = 122;
/// Line height, in tenths of a pixel (24.4 px).
const CELL_H: u32 = 244;
/// Baseline of the first line inside the grid (20 px).
const BASELINE: u32 = 200;
/// Top of the first line's clip box (1.5 px).
const CLIP_TOP: u32 = 15;
/// Left of the grid inside the window, in px.
const GRID_X: u32 = 9;
/// Top of the grid inside the window, in px (below the title bar).
const GRID_Y: u32 = 41;
/// Window width beyond the grid, in tenths (18 px).
const PAD_W: u32 = 180;
/// Window height beyond the grid, in tenths (50 px).
const PAD_H: u32 = 500;

/// The 16 ANSI colours, in `Indexed(0..=15)` order.
const ANSI: [&str; 16] = [
    "#1a1a1a", "#f4005f", "#98e024", "#fd971f", "#9d65ff", "#d670d6", "#58d1eb", "#c4c5b5",
    "#8a8d8f", "#ff4f8b", "#b6f05a", "#e0d561", "#b994ff", "#e8a0e8", "#8ee3f5", "#f6f6ef",
];

/// `tenths` of a pixel as SVG text: `122` → `12.2`, `90` → `9`.
fn px(tenths: u32) -> String {
    if tenths.is_multiple_of(10) {
        (tenths / 10).to_string()
    } else {
        format!("{}.{}", tenths / 10, tenths % 10)
    }
}

/// Tenths of a pixel rounded up to whole pixels.
fn ceil_px(tenths: u32) -> u32 {
    tenths.div_ceil(10)
}

/// The hex of a colour; `None` for `Reset` (the default of the layer).
fn hex(color: Color) -> Option<String> {
    let ansi = |index: usize| ANSI.get(index).map(|hex| (*hex).to_owned());
    match color {
        Color::Reset => None,
        Color::Black => ansi(0),
        Color::Red => ansi(1),
        Color::Green => ansi(2),
        Color::Yellow => ansi(3),
        Color::Blue => ansi(4),
        Color::Magenta => ansi(5),
        Color::Cyan => ansi(6),
        Color::Gray => ansi(7),
        Color::DarkGray => ansi(8),
        Color::LightRed => ansi(9),
        Color::LightGreen => ansi(10),
        Color::LightYellow => ansi(11),
        Color::LightBlue => ansi(12),
        Color::LightMagenta => ansi(13),
        Color::LightCyan => ansi(14),
        Color::White => ansi(15),
        Color::Rgb(r, g, b) => Some(format!("#{r:02x}{g:02x}{b:02x}")),
        Color::Indexed(index) => Some(indexed(index)),
    }
}

/// xterm's 256-colour palette.
fn indexed(index: u8) -> String {
    match index {
        0..=15 => ANSI
            .get(usize::from(index))
            .map_or_else(|| FOREGROUND.to_owned(), |hex| (*hex).to_owned()),
        16..=231 => {
            let level = |value: u8| if value == 0 { 0 } else { 55 + value * 40 };
            let cube = index - 16;
            let (r, g, b) = (cube / 36, cube / 6 % 6, cube % 6);
            format!("#{:02x}{:02x}{:02x}", level(r), level(g), level(b))
        }
        232..=255 => {
            let gray = 8 + (index - 232) * 10;
            format!("#{gray:02x}{gray:02x}{gray:02x}")
        }
    }
}

/// How a run of cells is drawn.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Look {
    fill: String,
    background: Option<String>,
    bold: bool,
    italic: bool,
    underline: bool,
}

impl Look {
    fn css(&self) -> String {
        let mut css = format!("fill: {}", self.fill);
        if self.bold {
            css.push_str(";font-weight: bold");
        }
        if self.italic {
            css.push_str(";font-style: italic");
        }
        if self.underline {
            css.push_str(";text-decoration: underline");
        }
        css
    }

    /// Same text style (the background is drawn apart).
    fn same_text(&self, other: &Look) -> bool {
        (&self.fill, self.bold, self.italic, self.underline)
            == (&other.fill, other.bold, other.italic, other.underline)
    }
}

fn look_of(cell: &ratatui::buffer::Cell) -> Look {
    let modifier = cell.modifier;
    let bold = modifier.contains(Modifier::BOLD);
    let (mut fg, mut bg) = (hex(cell.fg), hex(cell.bg));
    if modifier.contains(Modifier::REVERSED) {
        let fg_default = if bold { FOREGROUND_BOLD } else { FOREGROUND };
        let new_fg = bg.unwrap_or_else(|| BACKGROUND.to_owned());
        let new_bg = fg.unwrap_or_else(|| fg_default.to_owned());
        fg = Some(new_fg);
        bg = Some(new_bg);
    }
    let fill = fg.unwrap_or_else(|| {
        if bold {
            FOREGROUND_BOLD.to_owned()
        } else {
            FOREGROUND.to_owned()
        }
    });
    Look {
        fill,
        background: bg,
        bold,
        italic: modifier.contains(Modifier::ITALIC),
        underline: modifier.contains(Modifier::UNDERLINED),
    }
}

/// Text for an SVG `<text>`: XML-escaped, spaces as `&#160;` (SVG collapses plain spaces), and
/// any control character as `?` (the buffer never holds one; this is only a backstop).
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            ' ' => out.push_str("&#160;"),
            c if c.is_control() => out.push('?'),
            c => out.push(c),
        }
    }
    out
}

/// One run of cells on a row.
struct Run {
    column: u32,
    cells: u32,
    look: Look,
    text: String,
}

/// The runs of one row: consecutive cells with the same look.
fn runs(buffer: &Buffer, row: u16) -> Vec<Run> {
    let mut runs: Vec<Run> = Vec::new();
    for column in 0..buffer.area.width {
        let Some(cell) = buffer.cell((buffer.area.x + column, buffer.area.y + row)) else {
            continue;
        };
        let look = look_of(cell);
        let symbol = cell.symbol();
        match runs.last_mut() {
            Some(run) if run.look == look => {
                run.cells += 1;
                run.text.push_str(symbol);
            }
            _ => runs.push(Run {
                column: u32::from(column),
                cells: 1,
                look,
                text: symbol.to_owned(),
            }),
        }
    }
    runs
}

/// `buffer` as a standalone SVG document with `title` in the window's title bar. `comment` goes
/// in an XML comment at the top (where the image came from); `--` in it is replaced.
pub fn to_svg(buffer: &Buffer, title: &str, comment: &str) -> String {
    let columns = u32::from(buffer.area.width);
    let rows = u32::from(buffer.area.height);
    let grid_w = columns * CELL_W;
    let grid_h = rows * CELL_H;
    let width = ceil_px(grid_w + PAD_W);
    let height = ceil_px(grid_h + PAD_H);

    let mut looks: Vec<Look> = Vec::new();
    let mut class_of = |look: &Look| -> usize {
        match looks.iter().position(|known| known.same_text(look)) {
            Some(index) => index + 1,
            None => {
                looks.push(look.clone());
                looks.len()
            }
        }
    };

    let mut backgrounds = String::new();
    let mut texts = String::new();
    for row in 0..buffer.area.height {
        let line = u32::from(row);
        for run in runs(buffer, row) {
            let x = px(run.column * CELL_W);
            let length = px(run.cells * CELL_W);
            if let Some(background) = &run.look.background {
                backgrounds.push_str(&format!(
                    "<rect fill=\"{background}\" x=\"{x}\" y=\"{}\" width=\"{length}\" \
                     height=\"24.65\" shape-rendering=\"crispEdges\"/>",
                    px(CLIP_TOP + line * CELL_H)
                ));
            }
            if run.text.trim().is_empty() && !run.look.underline {
                continue;
            }
            let class = class_of(&run.look);
            texts.push_str(&format!(
                "<text class=\"dkt-r{class}\" x=\"{x}\" y=\"{}\" textLength=\"{length}\" \
                 clip-path=\"url(#dkt-line-{line})\">{}</text>",
                px(BASELINE + line * CELL_H),
                escape(&run.text)
            ));
        }
        texts.push('\n');
    }

    let styles: Vec<String> = looks
        .iter()
        .enumerate()
        .map(|(index, look)| format!(".dkt-r{} {{ {} }}", index + 1, look.css()))
        .collect();
    let clips: Vec<String> = (0..rows)
        .map(|line| {
            format!(
                "    <clipPath id=\"dkt-line-{line}\">\n    <rect x=\"0\" y=\"{}\" width=\"{}\" \
                 height=\"24.65\"/>\n            </clipPath>",
                px(CLIP_TOP + line * CELL_H),
                px(grid_w + 10)
            )
        })
        .collect();

    let mut svg = String::new();
    svg.push_str(&format!(
        "<svg class=\"rich-terminal\" viewBox=\"0 0 {width} {height}\" \
         xmlns=\"http://www.w3.org/2000/svg\">\n"
    ));
    svg.push_str(&format!("    <!-- {} -->\n", escape_comment(comment)));
    svg.push_str(FONT_STYLE);
    svg.push_str(&format!("    {}\n    </style>\n\n", styles.join("\n")));
    svg.push_str("    <defs>\n");
    svg.push_str(&format!(
        "    <clipPath id=\"dkt-clip-terminal\">\n      <rect x=\"0\" y=\"0\" width=\"{}\" \
         height=\"{}\" />\n    </clipPath>\n",
        px(grid_w.saturating_sub(10)),
        px(grid_h.saturating_sub(10))
    ));
    svg.push_str(&clips.join("\n"));
    svg.push_str("\n    </defs>\n\n");
    svg.push_str(&format!(
        "    <rect fill=\"{BACKGROUND}\" stroke=\"rgba(255,255,255,0.35)\" stroke-width=\"1\" \
         x=\"1\" y=\"1\" width=\"{}\" height=\"{}\" rx=\"8\"/><text class=\"dkt-title\" \
         fill=\"{FOREGROUND}\" text-anchor=\"middle\" x=\"{}\" y=\"27\">{}</text>\n",
        width.saturating_sub(2),
        height.saturating_sub(2),
        width / 2,
        escape(title)
    ));
    svg.push_str(
        "            <g transform=\"translate(26,22)\">\n            \
         <circle cx=\"0\" cy=\"0\" r=\"7\" fill=\"#ff5f57\"/>\n            \
         <circle cx=\"22\" cy=\"0\" r=\"7\" fill=\"#febc2e\"/>\n            \
         <circle cx=\"44\" cy=\"0\" r=\"7\" fill=\"#28c840\"/>\n            </g>\n\n",
    );
    svg.push_str(&format!(
        "    <g transform=\"translate({GRID_X}, {GRID_Y})\" \
         clip-path=\"url(#dkt-clip-terminal)\">\n    {backgrounds}\n    \
         <g class=\"dkt-matrix\">\n    {texts}    </g>\n    </g>\n</svg>\n"
    ));
    svg
}

/// Text for an XML comment: no control character and no `--` (which would end it early).
fn escape_comment(text: &str) -> String {
    let mut out: String = text
        .chars()
        .map(|c| if c.is_control() { '?' } else { c })
        .collect();
    while out.contains("--") {
        out = out.replace("--", "- -");
    }
    out
}

/// The fonts and the fixed classes, as in the esp32-logcat images.
const FONT_STYLE: &str = r#"    <style>

    @font-face {
        font-family: "Fira Code";
        src: local("FiraCode-Regular"),
                url("https://cdnjs.cloudflare.com/ajax/libs/firacode/6.2.0/woff2/FiraCode-Regular.woff2") format("woff2"),
                url("https://cdnjs.cloudflare.com/ajax/libs/firacode/6.2.0/woff/FiraCode-Regular.woff") format("woff");
        font-style: normal;
        font-weight: 400;
    }
    @font-face {
        font-family: "Fira Code";
        src: local("FiraCode-Bold"),
                url("https://cdnjs.cloudflare.com/ajax/libs/firacode/6.2.0/woff2/FiraCode-Bold.woff2") format("woff2"),
                url("https://cdnjs.cloudflare.com/ajax/libs/firacode/6.2.0/woff/FiraCode-Bold.woff") format("woff");
        font-style: bold;
        font-weight: 700;
    }

    .dkt-matrix {
        font-family: Fira Code, monospace;
        font-size: 20px;
        line-height: 24.4px;
        font-variant-east-asian: full-width;
    }

    .dkt-title {
        font-size: 18px;
        font-weight: bold;
        font-family: arial;
    }

"#;

#[cfg(test)]
mod tests {
    use ratatui::layout::Rect;
    use ratatui::style::Style;

    use super::*;

    #[test]
    fn coordinates_are_exact_tenths() {
        assert_eq!(px(122), "12.2");
        assert_eq!(px(90), "9");
        assert_eq!(px(80 * CELL_W), "976");
        assert_eq!(ceil_px(80 * CELL_W + PAD_W), 994);
    }

    #[test]
    fn palette() {
        assert_eq!(hex(Color::Reset), None);
        assert_eq!(hex(Color::Green).as_deref(), Some("#98e024"));
        assert_eq!(hex(Color::DarkGray).as_deref(), Some("#8a8d8f"));
        assert_eq!(hex(Color::Rgb(1, 2, 255)).as_deref(), Some("#0102ff"));
        assert_eq!(indexed(16), "#000000");
        assert_eq!(indexed(231), "#ffffff");
        assert_eq!(indexed(232), "#080808");
        assert_eq!(indexed(9), "#ff4f8b");
    }

    #[test]
    fn text_is_escaped_and_styled_by_run() {
        let mut buffer = Buffer::empty(Rect::new(0, 0, 12, 2));
        buffer.set_string(0, 0, "a<b & \"c\"", Style::new().fg(Color::Red));
        buffer.set_string(0, 1, "ok", Style::new().add_modifier(Modifier::BOLD));
        let svg = to_svg(&buffer, "t <x>", "from -- here");
        assert!(
            svg.contains(">a&lt;b&#160;&amp;&#160;&quot;c&quot;</text>"),
            "{svg}"
        );
        assert!(svg.contains(".dkt-r1 { fill: #f4005f }"));
        assert!(svg.contains(".dkt-r2 { fill: #e0e0e0;font-weight: bold }"));
        assert!(svg.contains(">t&#160;&lt;x&gt;</text>"));
        assert!(svg.contains("<!-- from - - here -->"));
        assert!(svg.contains("viewBox=\"0 0 165 99\""), "{svg}");
        assert_eq!(svg, to_svg(&buffer, "t <x>", "from -- here"));
    }
}
