// lumiterm's own config syntax. Looks like:
//
// window {
//     width: 1000
//     height: 650
//     opacity: 0.95
//     padding: 8
// }
//
// font {
//     path: "/usr/share/fonts/TTF/JetBrainsMonoNerdFont-Regular.ttf"
//     size: 14
// }
//
// colors {
//     background: #1e1e2e
//     foreground: #cdd6f4
//     black: #45475a
//     ...
// }
//
// shell {
//     program: "/bin/bash"
// }
//
// This is a hand-rolled recursive-descent parser: no TOML/serde involved,
// by design, since the whole point is a custom syntax.

use std::collections::HashMap;
use std::fs;
use std::path::Path;

#[derive(Debug, Clone)]
pub enum Value {
    Str(String),
    Num(f64),
    Section(HashMap<String, Value>),
}

struct Parser {
    chars: Vec<char>,
    pos: usize,
}

impl Parser {
    fn new(input: &str) -> Self {
        Parser {
            chars: input.chars().collect(),
            pos: 0,
        }
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn advance(&mut self) -> Option<char> {
        let c = self.peek();
        self.pos += 1;
        c
    }

    fn skip_ws_only(&mut self) {
        while let Some(c) = self.peek() {
            if c.is_whitespace() {
                self.pos += 1;
            } else {
                break;
            }
        }
    }

    fn skip_ws_and_comments(&mut self) {
        loop {
            while let Some(c) = self.peek() {
                if c.is_whitespace() {
                    self.pos += 1;
                } else {
                    break;
                }
            }
            if self.peek() == Some('#') {
                while let Some(c) = self.peek() {
                    if c == '\n' {
                        break;
                    }
                    self.pos += 1;
                }
            } else {
                break;
            }
        }
    }

    fn parse_ident(&mut self) -> String {
        let mut s = String::new();
        while let Some(c) = self.peek() {
            if c.is_alphanumeric() || c == '_' || c == '-' {
                s.push(c);
                self.pos += 1;
            } else {
                break;
            }
        }
        s
    }

    fn parse_string(&mut self) -> String {
        // assumes current char is opening quote
        self.advance(); // consume "
        let mut s = String::new();
        while let Some(c) = self.advance() {
            if c == '"' {
                break;
            }
            if c == '\\' {
                if let Some(next) = self.advance() {
                    s.push(next);
                }
            } else {
                s.push(c);
            }
        }
        s
    }

    fn parse_bare_value(&mut self) -> String {
        let mut s = String::new();
        while let Some(c) = self.peek() {
            if c == '\n' || c == '}' {
                break;
            }
            s.push(c);
            self.pos += 1;
        }
        s.trim().to_string()
    }

    fn parse_value(&mut self) -> Value {
        // NOTE: whitespace-only skip here, deliberately not comment-aware —
        // '#' is legitimately how a value starts (hex colors), so treating
        // it as a comment marker here would eat the whole color. Comments
        // are only recognized between statements, via skip_ws_and_comments
        // in parse_block.
        self.skip_ws_only();
        match self.peek() {
            Some('"') => Value::Str(self.parse_string()),
            Some('#') => {
                // hex color like #1e1e2e -- treat as string
                let raw = self.parse_bare_value();
                Value::Str(raw.trim().to_string())
            }
            _ => {
                let raw = self.parse_bare_value();
                if let Ok(n) = raw.parse::<f64>() {
                    Value::Num(n)
                } else {
                    Value::Str(raw)
                }
            }
        }
    }

    fn parse_block(&mut self) -> HashMap<String, Value> {
        let mut map = HashMap::new();
        loop {
            self.skip_ws_and_comments();
            match self.peek() {
                None | Some('}') => break,
                _ => {}
            }
            let key = self.parse_ident();
            if key.is_empty() {
                // avoid infinite loop on unexpected char
                self.pos += 1;
                continue;
            }
            self.skip_ws_and_comments();
            match self.peek() {
                Some('{') => {
                    self.advance(); // consume {
                    let inner = self.parse_block();
                    self.skip_ws_and_comments();
                    if self.peek() == Some('}') {
                        self.advance();
                    }
                    map.insert(key, Value::Section(inner));
                }
                Some(':') => {
                    self.advance(); // consume :
                    let val = self.parse_value();
                    map.insert(key, val);
                }
                _ => {
                    // malformed line, skip to end of line
                    while let Some(c) = self.peek() {
                        if c == '\n' {
                            break;
                        }
                        self.pos += 1;
                    }
                }
            }
        }
        map
    }
}

pub fn parse(input: &str) -> HashMap<String, Value> {
    let mut p = Parser::new(input);
    p.parse_block()
}

// ---- typed config extracted from the raw Value tree ----

#[derive(Debug, Clone)]
pub struct Config {
    pub width: u32,
    pub height: u32,
    // Parsed from config but not yet wired to actual rendering. Real window
    // transparency needs alpha-aware compositing support that varies across
    // X11/Wayland backends in softbuffer — rather than half-implement
    // something that might silently no-op on your setup, this is left
    // explicit until it's done properly. See README "known rough edges".
    #[allow(dead_code)]
    pub opacity: f32,
    pub padding: u32,
    pub font_path: String,
    pub font_size: f32,
    pub shell: String,
    pub colors: HashMap<String, [u8; 3]>,
}

fn get_section<'a>(map: &'a HashMap<String, Value>, name: &str) -> Option<&'a HashMap<String, Value>> {
    match map.get(name) {
        Some(Value::Section(s)) => Some(s),
        _ => None,
    }
}

fn get_num(map: &HashMap<String, Value>, key: &str, default: f64) -> f64 {
    match map.get(key) {
        Some(Value::Num(n)) => *n,
        Some(Value::Str(s)) => s.parse().unwrap_or(default),
        _ => default,
    }
}

fn get_str(map: &HashMap<String, Value>, key: &str, default: &str) -> String {
    match map.get(key) {
        Some(Value::Str(s)) => s.clone(),
        Some(Value::Num(n)) => n.to_string(),
        _ => default.to_string(),
    }
}

pub fn hex_to_rgb(s: &str) -> [u8; 3] {
    let s = s.trim_start_matches('#');
    if s.len() >= 6 {
        let r = u8::from_str_radix(&s[0..2], 16).unwrap_or(0);
        let g = u8::from_str_radix(&s[2..4], 16).unwrap_or(0);
        let b = u8::from_str_radix(&s[4..6], 16).unwrap_or(0);
        [r, g, b]
    } else {
        [0, 0, 0]
    }
}

impl Config {
    pub fn load(path: &Path) -> Config {
        let text = fs::read_to_string(path).unwrap_or_default();
        let tree = parse(&text);

        let window = get_section(&tree, "window").cloned().unwrap_or_default();
        let font = get_section(&tree, "font").cloned().unwrap_or_default();
        let shell = get_section(&tree, "shell").cloned().unwrap_or_default();
        let colors_section = get_section(&tree, "colors").cloned().unwrap_or_default();

        let mut colors = HashMap::new();
        for (k, v) in colors_section.iter() {
            if let Value::Str(s) = v {
                colors.insert(k.clone(), hex_to_rgb(s));
            }
        }
        // sensible fallback palette (Catppuccin-ish) if config didn't specify
        let defaults: [(&str, &str); 18] = [
            ("background", "#1e1e2e"),
            ("foreground", "#cdd6f4"),
            ("cursor", "#f5e0dc"),
            ("black", "#45475a"),
            ("red", "#f38ba8"),
            ("green", "#a6e3a1"),
            ("yellow", "#f9e2af"),
            ("blue", "#89b4fa"),
            ("magenta", "#f5c2e7"),
            ("cyan", "#94e2d5"),
            ("white", "#bac2de"),
            ("bright_black", "#585b70"),
            ("bright_red", "#f38ba8"),
            ("bright_green", "#a6e3a1"),
            ("bright_yellow", "#f9e2af"),
            ("bright_blue", "#89b4fa"),
            ("bright_magenta", "#f5c2e7"),
            ("bright_white", "#a6adc8"),
        ];
        for (k, v) in defaults.iter() {
            colors.entry(k.to_string()).or_insert_with(|| hex_to_rgb(v));
        }
        colors.entry("bright_cyan".to_string()).or_insert_with(|| hex_to_rgb("#94e2d5"));

        Config {
            width: get_num(&window, "width", 1000.0) as u32,
            height: get_num(&window, "height", 650.0) as u32,
            opacity: get_num(&window, "opacity", 1.0) as f32,
            padding: get_num(&window, "padding", 6.0) as u32,
            font_path: get_str(&font, "path", "/usr/share/fonts/TTF/DejaVuSansMono.ttf"),
            font_size: get_num(&font, "size", 14.0) as f32,
            shell: get_str(&shell, "program", &std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string())),
            colors,
        }
    }
}

impl Default for Config {
    fn default() -> Config {
        Config::load(Path::new("/nonexistent"))
    }
}
