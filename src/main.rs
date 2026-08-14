mod config;
mod pty;
mod render;
mod term;

use config::Config;
use render::Renderer;
use std::num::NonZeroU32;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::mpsc::channel;
use term::Term;
use vte::Parser as VteParser;
use winit::event::{ElementState, Event, KeyEvent, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ControlFlow, EventLoop};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::window::WindowBuilder;

/// Converts a pixel position (relative to the window) into a (col, row)
/// grid cell, clamped to the grid's current bounds.
fn pixel_to_cell(x: f64, y: f64, padding: usize, cell_w: usize, cell_h: usize, cols: usize, rows: usize) -> (usize, usize) {
    let col = (x as usize).saturating_sub(padding) / cell_w.max(1);
    let row = (y as usize).saturating_sub(padding) / cell_h.max(1);
    (col.min(cols.saturating_sub(1)), row.min(rows.saturating_sub(1)))
}

/// Reads clipboard text via arboard, falling back to shelling out to
/// `wl-paste` (from wl-clipboard) if that fails. This matters in practice:
/// arboard sometimes can't negotiate a MIME type that Firefox (and some
/// other Wayland apps) offer, even though the content is genuinely on the
/// clipboard — `wl-paste` handles that negotiation more robustly. The
/// fallback is a no-op if wl-clipboard isn't installed.
fn clipboard_get_text(clipboard: &mut Option<arboard::Clipboard>) -> Option<String> {
    if let Some(cb) = clipboard.as_mut() {
        if let Ok(t) = cb.get_text() {
            return Some(t);
        }
    }
    std::process::Command::new("wl-paste")
        .arg("--no-newline")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .filter(|s| !s.is_empty())
}

/// Writes clipboard text via arboard, falling back to `wl-copy` on failure,
/// for the same reason as clipboard_get_text above.
fn clipboard_set_text(clipboard: &mut Option<arboard::Clipboard>, text: String) {
    if let Some(cb) = clipboard.as_mut() {
        if cb.set_text(text.clone()).is_ok() {
            return;
        }
    }
    if let Ok(mut child) = std::process::Command::new("wl-copy").stdin(std::process::Stdio::piped()).spawn() {
        if let Some(stdin) = child.stdin.as_mut() {
            use std::io::Write;
            let _ = stdin.write_all(text.as_bytes());
        }
        let _ = child.wait();
    }
}

fn config_path() -> PathBuf {
    if let Some(home) = dirs_home() {
        let p = home.join(".config/lumiterm/lumiterm.conf");
        if p.exists() {
            return p;
        }
    }
    PathBuf::from("lumiterm.conf")
}

fn dirs_home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

fn main() {
    let cfg = Config::load(&config_path());

    let font_bytes = std::fs::read(&cfg.font_path).unwrap_or_else(|_| {
        panic!(
            "Could not read font at '{}'. Set font.path in your lumiterm.conf to a valid .ttf/.otf file.",
            cfg.font_path
        )
    });

    let mut renderer = Renderer::new(&font_bytes, cfg.font_size);

    let cols = ((cfg.width - cfg.padding * 2) as usize / renderer.cell_w).max(10);
    let rows = ((cfg.height - cfg.padding * 2) as usize / renderer.cell_h).max(4);

    let default_fg = *cfg.colors.get("foreground").unwrap();
    let default_bg = *cfg.colors.get("background").unwrap();
    let palette_names = [
        "black", "red", "green", "yellow", "blue", "magenta", "cyan", "white", "bright_black", "bright_red",
        "bright_green", "bright_yellow", "bright_blue", "bright_magenta", "bright_cyan", "bright_white",
    ];
    let mut palette = [[0u8; 3]; 16];
    for (i, name) in palette_names.iter().enumerate() {
        palette[i] = *cfg.colors.get(*name).unwrap_or(&default_fg);
    }

    let term = Rc::new(std::cell::RefCell::new(Term::new(cols, rows, default_fg, default_bg, palette)));
    let vte_parser = Rc::new(std::cell::RefCell::new(VteParser::new()));

    let (tx, rx) = channel::<Vec<u8>>();
    let mut pty_handle = pty::PtyHandle::spawn(&cfg.shell, cols as u16, rows as u16, tx);

    let event_loop = EventLoop::new().expect("failed to create event loop");
    event_loop.set_control_flow(ControlFlow::Wait);

    let window = Rc::new(
        WindowBuilder::new()
            .with_title("Lumiterm")
            .with_inner_size(winit::dpi::LogicalSize::new(cfg.width, cfg.height))
            .build(&event_loop)
            .expect("failed to create window"),
    );

    let context = softbuffer::Context::new(window.clone()).expect("failed to create softbuffer context");
    let mut surface = softbuffer::Surface::new(&context, window.clone()).expect("failed to create surface");

    let mut win_width = cfg.width;
    let mut win_height = cfg.height;
    let padding = cfg.padding as usize;

    surface
        .resize(NonZeroU32::new(win_width).unwrap(), NonZeroU32::new(win_height).unwrap())
        .unwrap();

    let term_for_events = term.clone();
    let vte_for_events = vte_parser.clone();

    // Clipboard is optional: if the platform clipboard is unavailable for
    // some reason, copy/paste silently no-op instead of crashing the app.
    let mut clipboard = arboard::Clipboard::new().ok();
    if std::env::var_os("LUMITERM_DEBUG").is_some() {
        eprintln!("[debug] clipboard initialized: {}", clipboard.is_some());
    }
    let mut modifiers = ModifiersState::empty();
    let mut mouse_down = false;
    let mut mouse_pos: (f64, f64) = (0.0, 0.0);

    let _ = event_loop.run(move |event, elwt| {
        // drain any PTY output that arrived
        while let Ok(data) = rx.try_recv() {
            let mut t = term_for_events.borrow_mut();
            let mut p = vte_for_events.borrow_mut();
            for byte in data {
                p.advance(&mut *t, byte);
            }
            if !t.response_queue.is_empty() {
                let response = std::mem::take(&mut t.response_queue);
                drop(t);
                pty_handle.write(&response);
            } else {
                drop(t);
            }
            window.request_redraw();
        }

        match event {
            Event::WindowEvent { event, .. } => match event {
                WindowEvent::CloseRequested => {
                    elwt.exit();
                }
                WindowEvent::Resized(size) => {
                    win_width = size.width.max(1);
                    win_height = size.height.max(1);
                    surface
                        .resize(NonZeroU32::new(win_width).unwrap(), NonZeroU32::new(win_height).unwrap())
                        .unwrap();
                    let new_cols = ((win_width as usize).saturating_sub(padding * 2) / renderer.cell_w).max(10);
                    let new_rows = ((win_height as usize).saturating_sub(padding * 2) / renderer.cell_h).max(4);
                    term.borrow_mut().resize(new_cols, new_rows);
                    pty_handle.resize(new_cols as u16, new_rows as u16);
                    window.request_redraw();
                }
                WindowEvent::ModifiersChanged(mods) => {
                    modifiers = mods.state();
                }
                WindowEvent::CursorMoved { position, .. } => {
                    mouse_pos = (position.x, position.y);
                    if mouse_down {
                        let (grid_cols, grid_rows) = {
                            let t = term.borrow();
                            (t.cols, t.rows)
                        };
                        let end_cell =
                            pixel_to_cell(mouse_pos.0, mouse_pos.1, padding, renderer.cell_w, renderer.cell_h, grid_cols, grid_rows);
                        let mut t = term.borrow_mut();
                        if let Some((start, _)) = t.selection {
                            t.selection = Some((start, end_cell));
                            t.dirty = true;
                        }
                        drop(t);
                        window.request_redraw();
                    }
                }
                WindowEvent::MouseInput { state, button: MouseButton::Left, .. } => {
                    match state {
                        ElementState::Pressed => {
                            let (grid_cols, grid_rows) = {
                                let t = term.borrow();
                                (t.cols, t.rows)
                            };
                            let cell = pixel_to_cell(
                                mouse_pos.0,
                                mouse_pos.1,
                                padding,
                                renderer.cell_w,
                                renderer.cell_h,
                                grid_cols,
                                grid_rows,
                            );
                            let mut t = term.borrow_mut();
                            t.selection = Some((cell, cell));
                            t.dirty = true;
                            drop(t);
                            mouse_down = true;
                            window.request_redraw();
                        }
                        ElementState::Released => {
                            mouse_down = false;
                            // Selecting with the mouse copies to the clipboard
                            // immediately, matching common terminal behavior
                            // (select-to-copy). Note: this uses the regular
                            // system clipboard for both copy and paste — it
                            // doesn't distinguish X11's PRIMARY vs CLIPBOARD
                            // selections the way some terminals do.
                            let selected = term.borrow().selected_text();
                            if std::env::var_os("LUMITERM_DEBUG").is_some() {
                                eprintln!("[debug] selection on release: {:?}", selected);
                            }
                            if let Some(text) = selected {
                                if !text.is_empty() {
                                    clipboard_set_text(&mut clipboard, text);
                                }
                            }
                        }
                    }
                }
                WindowEvent::MouseWheel { delta, .. } => {
                    // Each "notch" of a line-based wheel scrolls a few
                    // lines at a time, matching typical terminal feel.
                    let lines: i32 = match delta {
                        MouseScrollDelta::LineDelta(_, y) => (y * 3.0) as i32,
                        MouseScrollDelta::PixelDelta(pos) => (pos.y / renderer.cell_h.max(1) as f64) as i32,
                    };
                    if lines > 0 {
                        term.borrow_mut().scroll_up(lines as usize);
                    } else if lines < 0 {
                        term.borrow_mut().scroll_down((-lines) as usize);
                    }
                    window.request_redraw();
                }
                WindowEvent::KeyboardInput {
                    event: KeyEvent { state: ElementState::Pressed, logical_key, text, .. },
                    ..
                } => {
                    let ctrl_shift = modifiers.control_key() && modifiers.shift_key();
                    let mut handled = false;

                    if ctrl_shift {
                        if let Key::Character(ref s) = logical_key {
                            if s.eq_ignore_ascii_case("v") {
                                // Paste: wrap in bracketed-paste markers if
                                // the running program asked for that mode.
                                let clip_text = clipboard_get_text(&mut clipboard);
                                if std::env::var_os("LUMITERM_DEBUG").is_some() {
                                    eprintln!("[debug] clipboard get_text result: {:?}", clip_text);
                                }
                                if let Some(clip_text) = clip_text {
                                    let bracketed = term.borrow().bracketed_paste;
                                    let mut data = Vec::new();
                                    if bracketed {
                                        data.extend_from_slice(b"\x1b[200~");
                                    }
                                    data.extend_from_slice(clip_text.as_bytes());
                                    if bracketed {
                                        data.extend_from_slice(b"\x1b[201~");
                                    }
                                    pty_handle.write(&data);
                                }
                                handled = true;
                            } else if s.eq_ignore_ascii_case("c") {
                                // Explicit copy shortcut, in addition to the
                                // automatic copy-on-select behavior above.
                                let selected = term.borrow().selected_text();
                                if let Some(t) = selected {
                                    if !t.is_empty() {
                                        clipboard_set_text(&mut clipboard, t);
                                    }
                                }
                                handled = true;
                            }
                        }
                    }

                    if !handled {
                        // Typing clears any existing selection highlight and
                        // snaps the view back to the bottom if scrolled into
                        // history, matching typical terminal behavior.
                        {
                            let mut t = term.borrow_mut();
                            t.clear_selection();
                            t.scroll_reset();
                        }
                        let bytes: Option<Vec<u8>> = match logical_key {
                            Key::Named(NamedKey::Enter) => Some(vec![b'\r']),
                            Key::Named(NamedKey::Backspace) => {
                                if modifiers.control_key() {
                                    // Ctrl+Backspace: no special terminal
                                    // sequence for this: shells' line
                                    // editors (fish, bash/readline) both
                                    // default-bind Ctrl+W to delete the
                                    // previous word, so send that instead.
                                    Some(vec![0x17])
                                } else {
                                    Some(vec![0x7f])
                                }
                            }
                            Key::Named(NamedKey::Tab) => Some(vec![b'\t']),
                            Key::Named(NamedKey::Escape) => Some(vec![0x1b]),
                            Key::Named(NamedKey::ArrowUp) => Some(b"\x1b[A".to_vec()),
                            Key::Named(NamedKey::ArrowDown) => Some(b"\x1b[B".to_vec()),
                            Key::Named(NamedKey::ArrowRight) => Some(b"\x1b[C".to_vec()),
                            Key::Named(NamedKey::ArrowLeft) => Some(b"\x1b[D".to_vec()),
                            Key::Named(NamedKey::Space) => Some(vec![b' ']),
                            _ => text.as_ref().map(|s| s.as_bytes().to_vec()),
                        };
                        if let Some(b) = bytes {
                            pty_handle.write(&b);
                        }
                    }
                }
                WindowEvent::RedrawRequested => {
                    let mut buffer = surface.buffer_mut().unwrap();
                    let t = term.borrow();
                    renderer.render(&t, &mut buffer, win_width as usize, win_height as usize, padding);
                    drop(t);
                    buffer.present().unwrap();
                }
                _ => {}
            },
            Event::AboutToWait => {
                // small idle wait; PTY reader thread wakes us via channel + redraw request
                std::thread::sleep(std::time::Duration::from_millis(8));
                window.request_redraw();
            }
            _ => {}
        }
    });
}
