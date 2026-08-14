use crate::term::Term;
use fontdue::{Font, FontSettings};
use std::collections::HashMap;

pub struct Renderer {
    font: Font,
    font_size: f32,
    pub cell_w: usize,
    pub cell_h: usize,
    glyph_cache: HashMap<char, (fontdue::Metrics, Vec<u8>)>,
}

impl Renderer {
    pub fn new(font_bytes: &[u8], font_size: f32) -> Renderer {
        let font = Font::from_bytes(font_bytes, FontSettings::default())
            .expect("failed to parse font file (check font.path in config)");

        // measure a representative glyph to determine fixed cell size
        let (metrics, _) = font.rasterize('M', font_size);
        let cell_w = metrics.advance_width.ceil().max(1.0) as usize;
        let line_metrics = font.horizontal_line_metrics(font_size).unwrap();
        let cell_h = (line_metrics.ascent - line_metrics.descent + line_metrics.line_gap).ceil() as usize;

        Renderer {
            font,
            font_size,
            cell_w: cell_w.max(6),
            cell_h: cell_h.max(10),
            glyph_cache: HashMap::new(),
        }
    }

    fn glyph(&mut self, c: char) -> &(fontdue::Metrics, Vec<u8>) {
        self.glyph_cache
            .entry(c)
            .or_insert_with(|| self.font.rasterize(c, self.font_size))
    }

    /// Render the terminal grid into an RGBA-in-u32 buffer (0x00RRGGBB packed, softbuffer style: 0RGB per pixel as u32).
    pub fn render(&mut self, term: &Term, buffer: &mut [u32], width: usize, height: usize, padding: usize) {
        // clear background with term default bg
        let bg = term.default_bg;
        let bg_px = pack(bg[0], bg[1], bg[2]);
        for px in buffer.iter_mut() {
            *px = bg_px;
        }

        let cell_w = self.cell_w;
        let cell_h = self.cell_h;
        let ascent = self.font.horizontal_line_metrics(self.font_size).unwrap().ascent;
        let selection = term.selection_range();
        const SELECTION_HIGHLIGHT: [u8; 3] = [137, 180, 250]; // soft blue overlay

        for y in 0..term.rows {
            for x in 0..term.cols {
                let cell = term.visible_cell(x, y);
                let px0 = padding + x * cell_w;
                let py0 = padding + y * cell_h;
                if px0 + cell_w > width || py0 + cell_h > height {
                    continue;
                }

                let selected = match selection {
                    Some((start, end)) => {
                        let pos = (y, x);
                        let start_key = (start.1, start.0);
                        let end_key = (end.1, end.0);
                        pos >= start_key && pos <= end_key
                    }
                    None => false,
                };

                // background: cell's own color, blended with a highlight
                // overlay if this cell is part of the current selection.
                let effective_bg = if selected {
                    blend(SELECTION_HIGHLIGHT, cell.bg, 90)
                } else {
                    cell.bg
                };

                if effective_bg != term.default_bg || selected {
                    let bgpx = pack(effective_bg[0], effective_bg[1], effective_bg[2]);
                    for yy in 0..cell_h {
                        let row_start = (py0 + yy) * width + px0;
                        for xx in 0..cell_w {
                            if row_start + xx < buffer.len() {
                                buffer[row_start + xx] = bgpx;
                            }
                        }
                    }
                }

                if cell.ch != ' ' && cell.ch != '\0' {
                    let (metrics, bitmap) = self.glyph(cell.ch).clone();
                    let fg = cell.fg;
                    let glyph_x = px0 as i32 + metrics.xmin;
                    let glyph_y = py0 as i32 + ascent as i32 - metrics.height as i32 - metrics.ymin;

                    for gy in 0..metrics.height {
                        for gx in 0..metrics.width {
                            let coverage = bitmap[gy * metrics.width + gx];
                            if coverage == 0 {
                                continue;
                            }
                            let px = glyph_x + gx as i32;
                            let py = glyph_y + gy as i32;
                            if px < 0 || py < 0 || px as usize >= width || py as usize >= height {
                                continue;
                            }
                            let idx = py as usize * width + px as usize;
                            let blended = blend(fg, effective_bg, coverage);
                            buffer[idx] = pack(blended[0], blended[1], blended[2]);
                        }
                    }
                }
            }
        }

        // draw cursor as a block outline / solid at cursor position —
        // skipped while scrolled back, since the live cursor position
        // isn't part of the history view being shown.
        if term.scroll_offset == 0 {
            let cx = padding + term.cursor_x * cell_w;
            let cy = padding + term.cursor_y * cell_h;
            let cursor_color = pack(245, 224, 220);
            for yy in 0..cell_h {
                let row = cy + yy;
                if row >= height {
                    continue;
                }
                for side in [0usize, cell_w.saturating_sub(1)] {
                    let col = cx + side;
                    if col < width {
                        buffer[row * width + col] = cursor_color;
                    }
                }
            }
            for xx in 0..cell_w {
                for side in [0usize, cell_h.saturating_sub(1)] {
                    let row = cy + side;
                    let col = cx + xx;
                    if row < height && col < width {
                        buffer[row * width + col] = cursor_color;
                    }
                }
            }
        }
    }
}

fn pack(r: u8, g: u8, b: u8) -> u32 {
    ((r as u32) << 16) | ((g as u32) << 8) | (b as u32)
}

fn blend(fg: [u8; 3], bg: [u8; 3], coverage: u8) -> [u8; 3] {
    let a = coverage as f32 / 255.0;
    [
        (fg[0] as f32 * a + bg[0] as f32 * (1.0 - a)) as u8,
        (fg[1] as f32 * a + bg[1] as f32 * (1.0 - a)) as u8,
        (fg[2] as f32 * a + bg[2] as f32 * (1.0 - a)) as u8,
    ]
}
