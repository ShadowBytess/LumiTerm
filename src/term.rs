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
    // Mouse text selection, stored as ((col,row), (col,row)) for the click
    // point and current drag point — unordered (start may be after end in
    // reading order); use selection_range() to get it normalized.
    pub selection: Option<((usize, usize), (usize, usize))>,
    // Whether the running program has requested bracketed-paste mode
    // (DECSET 2004). If set, pasted text gets wrapped in ESC[200~ / ESC[201~
    // so shells/editors can tell pasted input apart from typed input.
    pub bracketed_paste: bool,
    // Rows that have scrolled off the top of the visible grid, oldest at
    // the front. Capped at MAX_SCROLLBACK_LINES.
    pub scrollback: std::collections::VecDeque<Vec<Cell>>,
    // How many lines back into scrollback the current view is scrolled.
    // 0 means viewing the live grid (normal state).
    pub scroll_offset: usize,
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
            selection: None,
            bracketed_paste: false,
            scrollback: std::collections::VecDeque::new(),
            scroll_offset: 0,
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

    /// Returns the selection as (start, end) in reading order — start is
    /// always at or before end, regardless of which direction the user
    /// dragged.
    pub fn selection_range(&self) -> Option<((usize, usize), (usize, usize))> {
        self.selection.map(|(a, b)| {
            let a_key = (a.1, a.0); // (row, col) for comparison
            let b_key = (b.1, b.0);
            if a_key <= b_key {
                (a, b)
            } else {
                (b, a)
            }
        })
    }

    /// Extracts the plain text of the current selection, trimming trailing
    /// whitespace from each line (since blank cells are stored as spaces).
    pub fn selected_text(&self) -> Option<String> {
        let (start, end) = self.selection_range()?;
        let mut lines = Vec::new();
        for row in start.1..=end.1 {
            let row_start_col = if row == start.1 { start.0 } else { 0 };
            let row_end_col = if row == end.1 { end.0 } else { self.cols.saturating_sub(1) };
            let mut line = String::new();
            for col in row_start_col..=row_end_col.min(self.cols.saturating_sub(1)) {
                let idx = row * self.cols + col;
                if idx < self.grid.len() {
                    line.push(self.grid[idx].ch);
                }
            }
            lines.push(line.trim_end().to_string());
        }
        Some(lines.join("\n"))
    }

    pub fn clear_selection(&mut self) {
        self.selection = None;
        self.dirty = true;
    }

    pub fn scroll_up(&mut self, lines: usize) {
        self.scroll_offset = (self.scroll_offset + lines).min(self.scrollback.len());
        self.dirty = true;
    }

    pub fn scroll_down(&mut self, lines: usize) {
        self.scroll_offset = self.scroll_offset.saturating_sub(lines);
        self.dirty = true;
    }

    pub fn scroll_reset(&mut self) {
        if self.scroll_offset != 0 {
            self.scroll_offset = 0;
            self.dirty = true;
        }
    }

    /// Resolves a (col, row) position in the *currently displayed* view —
    /// which may be the live grid or scrolled back into history — to the
    /// cell that should actually be drawn there.
    pub fn visible_cell(&self, col: usize, row: usize) -> Cell {
        let blank = Cell {
            ch: ' ',
            fg: self.default_fg,
            bg: self.default_bg,
            bold: false,
        };
        if self.scroll_offset == 0 {
            let idx = row * self.cols + col;
            return self.grid.get(idx).copied().unwrap_or(blank);
        }
        let sb_len = self.scrollback.len();
        let start = sb_len.saturating_sub(self.scroll_offset);
        let combined_index = start + row;
        if combined_index < sb_len {
            self.scrollback[combined_index].get(col).copied().unwrap_or(blank)
        } else {
            let grid_row = combined_index - sb_len;
            if grid_row < self.rows {
                self.grid.get(grid_row * self.cols + col).copied().unwrap_or(blank)
            } else {
                blank
            }
        }
    }

    fn scroll_up_one(&mut self) {
        const MAX_SCROLLBACK_LINES: usize = 5000;
        // capture the top row before it gets overwritten by the shift below
        let top_row: Vec<Cell> = (0..self.cols).map(|x| self.grid[x]).collect();
        self.scrollback.push_back(top_row);
        while self.scrollback.len() > MAX_SCROLLBACK_LINES {
            self.scrollback.pop_front();
        }

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

    fn csi_dispatch(&mut self, params: &Params, intermediates: &[u8], _ignore: bool, c: char) {
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
            'h' | 'l' => {
                // DECSET ('h') / DECRST ('l') private mode set/reset —
                // only handling bracketed paste (2004) for now. Other
                // private modes (cursor visibility, alt screen, etc.) are
                // intentionally ignored rather than silently mishandled.
                if intermediates.contains(&b'?') {
                    if get(0, 0) == 2004 {
                        self.bracketed_paste = c == 'h';
                    }
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
