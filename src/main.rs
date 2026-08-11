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
use winit::event::{ElementState, Event, KeyEvent, WindowEvent};
use winit::event_loop::{ControlFlow, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::WindowBuilder;

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
                WindowEvent::KeyboardInput {
                    event: KeyEvent { state: ElementState::Pressed, logical_key, text, .. },
                    ..
                } => {
                    let bytes: Option<Vec<u8>> = match logical_key {
                        Key::Named(NamedKey::Enter) => Some(vec![b'\r']),
                        Key::Named(NamedKey::Backspace) => Some(vec![0x7f]),
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
