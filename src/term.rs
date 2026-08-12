use vte::{Params, Perform};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Cell {
    pub ch: char,
    pub fg: [u8; 3],
    pub bg: [u8; 3],
    pub bold: bool,
}

impl Default for Cell {
    fn default() -> Self {
        Cell {
            ch: ' ',
            fg: [205, 214, 244],
            bg: [30, 30, 46],
            bold: false,
        }
    }
}

pub struct Term {
    pub cols: usize,
    pub rows: usize,
    pub grid: Vec<Cell>,
    pub cursor_x: usize,
    pub cursor_y: usize,
    pub fg: [u8; 3],
    pub bg: [u8; 3],
    pub default_fg: [u8; 3],
    pub default_bg: [u8; 3],
    pub bold: bool,
    pub palette: [[u8; 3]; 16],
    pub dirty: bool,
    // Bytes queued to send back to the PTY in response to terminal queries
    // (device attributes, cursor position reports, status). Drained by the
    // main loop after each parse tick.
    pub response_queue: Vec<u8>,
}

impl Term {
    pub fn new(cols: usize, rows: usize, default_fg: [u8; 3], default_bg: [u8; 3], palette: [[u8; 3]; 16]) -> Self {
        let grid = vec![
            Cell {
                ch: ' ',
                fg: default_fg,
                bg: default_bg,
                bold: false
            };
            cols * rows
        ];
        Term {
            cols,
            rows,
            grid,
            cursor_x: 0,
            cursor_y: 0,
            fg: default_fg,
            bg: default_bg,
            default_fg,
            default_bg,
            bold: false,
            palette,
            dirty: true,
            response_queue: Vec::new(),
        }
    }

    pub fn resize(&mut self, cols: usize, rows: usize) {
        if cols == self.cols && rows == self.rows {
            return;
        }
        let mut new_grid = vec![
            Cell {
                ch: ' ',
                fg: self.default_fg,
                bg: self.default_bg,
                bold: false
            };
            cols * rows
        ];
        for y in 0..rows.min(self.rows) {
            for x in 0..cols.min(self.cols) {
                new_grid[y * cols + x] = self.grid[y * self.cols + x];
            }
        }
        self.grid = new_grid;
        self.cols = cols;
        self.rows = rows;
        self.cursor_x = self.cursor_x.min(cols.saturating_sub(1));
        self.cursor_y = self.cursor_y.min(rows.saturating_sub(1));
        self.dirty = true;
    }

    fn idx(&self, x: usize, y: usize) -> usize {
        y * self.cols + x
    }

    fn scroll_up_one(&mut self) {
        // shift everything up one row, clear bottom row
        for y in 1..self.rows {
            for x in 0..self.cols {
                let src = self.idx(x, y);
                let dst = self.idx(x, y - 1);
                self.grid[dst] = self.grid[src];
            }
        }
        let last = self.rows - 1;
        for x in 0..self.cols {
            let i = self.idx(x, last);
            self.grid[i] = Cell {
                ch: ' ',
                fg: self.default_fg,
                bg: self.default_bg,
                bold: false,
            };
        }
    }

    fn newline(&mut self) {
        if self.cursor_y + 1 >= self.rows {
            self.scroll_up_one();
        } else {
            self.cursor_y += 1;
        }
    }

    fn put_char(&mut self, c: char) {
        if self.cursor_x >= self.cols {
            self.cursor_x = 0;
            self.newline();
        }
        let i = self.idx(self.cursor_x, self.cursor_y);
        self.grid[i] = Cell {
            ch: c,
            fg: self.fg,
            bg: self.bg,
            bold: self.bold,
        };
        self.cursor_x += 1;
        self.dirty = true;
    }

    fn erase_in_display(&mut self, mode: u16) {
        match mode {
            0 => {
                // cursor to end
                let start = self.idx(self.cursor_x, self.cursor_y);
                for cell in self.grid[start..].iter_mut() {
                    *cell = Cell {
                        ch: ' ',
                        fg: self.default_fg,
                        bg: self.default_bg,
                        bold: false,
                    };
                }
            }
            1 => {
                let end = self.idx(self.cursor_x, self.cursor_y).min(self.grid.len() - 1);
                for cell in self.grid[..=end].iter_mut() {
                    *cell = Cell {
                        ch: ' ',
                        fg: self.default_fg,
                        bg: self.default_bg,
                        bold: false,
                    };
                }
            }
            2 | 3 => {
                for cell in self.grid.iter_mut() {
                    *cell = Cell {
                        ch: ' ',
                        fg: self.default_fg,
                        bg: self.default_bg,
                        bold: false,
                    };
                }
            }
            _ => {}
        }
        self.dirty = true;
    }

    fn erase_in_line(&mut self, mode: u16) {
        let y = self.cursor_y;
        match mode {
            0 => {
                for x in self.cursor_x..self.cols {
                    let i = self.idx(x, y);
                    self.grid[i] = Cell {
                        ch: ' ',
                        fg: self.default_fg,
                        bg: self.default_bg,
                        bold: false,
                    };
                }
            }
            1 => {
                for x in 0..=self.cursor_x.min(self.cols - 1) {
                    let i = self.idx(x, y);
                    self.grid[i] = Cell {
                        ch: ' ',
                        fg: self.default_fg,
                        bg: self.default_bg,
                        bold: false,
                    };
                }
            }
            2 => {
                for x in 0..self.cols {
                    let i = self.idx(x, y);
                    self.grid[i] = Cell {
                        ch: ' ',
                        fg: self.default_fg,
                        bg: self.default_bg,
                        bold: false,
                    };
                }
            }
            _ => {}
        }
        self.dirty = true;
    }

    fn apply_sgr(&mut self, params: &Params) {
        let mut it = params.iter();
        let mut codes: Vec<u16> = Vec::new();
        for p in it.by_ref() {
            if let Some(&v) = p.first() {
                codes.push(v);
            } else {
                codes.push(0);
            }
        }
        if codes.is_empty() {
            codes.push(0);
        }
        let mut i = 0;
        while i < codes.len() {
            let code = codes[i];
            match code {
                0 => {
                    self.fg = self.default_fg;
                    self.bg = self.default_bg;
                    self.bold = false;
                }
                1 => self.bold = true,
                22 => self.bold = false,
                39 => self.fg = self.default_fg,
                49 => self.bg = self.default_bg,
                30..=37 => self.fg = self.palette[(code - 30) as usize],
                40..=47 => self.bg = self.palette[(code - 40) as usize],
                90..=97 => self.fg = self.palette[(code - 90 + 8) as usize],
                100..=107 => self.bg = self.palette[(code - 100 + 8) as usize],
                38 | 48 => {
                    // extended color: 38;5;N or 38;2;R;G;B
                    if i + 1 < codes.len() {
                        let mode = codes[i + 1];
                        if mode == 5 && i + 2 < codes.len() {
                            let idx = codes[i + 2] as usize;
                            let color = if idx < 16 {
                                self.palette[idx]
                            } else if idx < 232 {
                                // xterm's 6x6x6 color cube uses these fixed
                                // levels per component, not an even step —
                                // using a flat multiplier here previously
                                // produced near-black colors for most of
                                // the cube.
                                const LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];
                                let idx = idx - 16;
                                let r = LEVELS[(idx / 36) % 6];
                                let g = LEVELS[(idx / 6) % 6];
                                let b = LEVELS[idx % 6];
                                [r, g, b]
                            } else {
                                let v = (idx - 232) as u8 * 10 + 8;
                                [v, v, v]
                            };
                            if code == 38 {
                                self.fg = color;
                            } else {
                                self.bg = color;
                            }
                            i += 2;
                        } else if mode == 2 && i + 4 < codes.len() {
                            let color = [codes[i + 2] as u8, codes[i + 3] as u8, codes[i + 4] as u8];
                            if code == 38 {
                                self.fg = color;
                            } else {
                                self.bg = color;
                            }
                            i += 4;
                        }
                    }
                }
                _ => {}
            }
            i += 1;
        }
    }
}

impl Perform for Term {
    fn print(&mut self, c: char) {
        self.put_char(c);
    }

    fn execute(&mut self, byte: u8) {
        match byte {
            b'\n' => {
                self.cursor_x = 0;
                self.newline();
            }
            b'\r' => {
                self.cursor_x = 0;
            }
            0x08 => {
                // backspace
                if self.cursor_x > 0 {
                    self.cursor_x -= 1;
                }
            }
            b'\t' => {
                let next_tab = (self.cursor_x / 8 + 1) * 8;
                self.cursor_x = next_tab.min(self.cols - 1);
            }
            0x07 => {} // bell, ignore
            _ => {}
        }
        self.dirty = true;
    }

    fn csi_dispatch(&mut self, params: &Params, _intermediates: &[u8], _ignore: bool, c: char) {
        let get = |n: usize, default: u16| -> u16 {
            params.iter().nth(n).and_then(|p| p.first().copied()).unwrap_or(default)
        };
        match c {
            'A' => {
                let n = get(0, 1).max(1) as usize;
                self.cursor_y = self.cursor_y.saturating_sub(n);
            }
            'B' => {
                let n = get(0, 1).max(1) as usize;
                self.cursor_y = (self.cursor_y + n).min(self.rows - 1);
            }
            'C' => {
                let n = get(0, 1).max(1) as usize;
                self.cursor_x = (self.cursor_x + n).min(self.cols - 1);
            }
            'D' => {
                let n = get(0, 1).max(1) as usize;
                self.cursor_x = self.cursor_x.saturating_sub(n);
            }
            'H' | 'f' => {
                let row = get(0, 1).max(1) as usize - 1;
                let col = get(1, 1).max(1) as usize - 1;
                self.cursor_y = row.min(self.rows - 1);
                self.cursor_x = col.min(self.cols - 1);
            }
            'J' => self.erase_in_display(get(0, 0)),
            'K' => self.erase_in_line(get(0, 0)),
            'm' => self.apply_sgr(params),
            'c' => {
                // Primary Device Attributes query (fish, and others, send
                // this at startup and wait for a reply). Claim basic VT102
                // support so shells stop waiting and enable normal features.
                self.response_queue.extend_from_slice(b"\x1b[?6c");
            }
            'n' => {
                // Device Status Report.
                match get(0, 0) {
                    5 => {
                        // "are you OK?" -> "yes, OK"
                        self.response_queue.extend_from_slice(b"\x1b[0n");
                    }
                    6 => {
                        // cursor position report, 1-indexed
                        let report = format!("\x1b[{};{}R", self.cursor_y + 1, self.cursor_x + 1);
                        self.response_queue.extend_from_slice(report.as_bytes());
                    }
                    _ => {}
                }
            }
            _ => {}
        }
        self.dirty = true;
    }

    fn esc_dispatch(&mut self, _intermediates: &[u8], _ignore: bool, byte: u8) {
        if byte == b'c' {
            // RIS - full reset
            for cell in self.grid.iter_mut() {
                *cell = Cell {
                    ch: ' ',
                    fg: self.default_fg,
                    bg: self.default_bg,
                    bold: false,
                };
            }
            self.cursor_x = 0;
            self.cursor_y = 0;
            self.fg = self.default_fg;
            self.bg = self.default_bg;
            self.dirty = true;
        }
    }
}
