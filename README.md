# Lumiterm

LumiTerm: a custom terminal by LuminousCat (@ShadowBytess) for CachyOS/Arch Linux, written from scratch in Rust. It uses Alacritty only as a reference for what a config/UX should look like. The PTY handling, ANSI parsing wiring, grid model, and renderer here are all custom, and the config file uses its own brace/colon syntax instead of Alacritty's TOML.

This is a fully maintained project until it becomes stable enough to abandon, as well as being a passion project. If you wish to take this for yourself and continue maintaining it, please email me at my GitHub contact email to discuss a plan.

**Status: bare-bones, working, actively used daily.** It runs a real shell, handles cursor movement, colors (16-color, 256-color, and 24-bit SGR), erase sequences, backspace/tab/newline, resizing (with proper scrollback-aware reflow), keyboard input, mouse-select copy/paste, scrollback, word-delete (Ctrl+Backspace), and answers terminal queries (device attributes, cursor position, bracketed paste). It does NOT yet have: unicode wide-glyph handling, ligatures, or tabs/splits. Treat it as a foundation to iterate on.

If you would like to help, please email my GitHub contact email or submit an Issue/Pull Request to ask. Thank you.

## How it works (architecture)

- `src/pty.rs`: spawns your real shell in a pseudo-terminal via `portable-pty` and streams its output to the main thread over a channel.
- `src/term.rs`: the terminal grid (`Term`). Implements `vte::Perform`, the trait the `vte` crate calls into as it parses incoming bytes, so this is where cursor movement, SGR colors, erase codes, and terminal query responses (device attributes, cursor position, bracketed paste) are handled. Also owns the current mouse selection and scrollback buffer.
- `src/render.rs`: rasterizes glyphs with `fontdue`, blits them into a pixel buffer, and draws the selection highlight overlay.
- `src/config.rs`: a hand-written parser for lumiterm's own config syntax (no TOML/serde).
- `src/main.rs`: window + event loop (`winit`), wires keyboard/mouse input to the PTY and clipboard, debounces resize events, and drives redraws.

## 1. Install dependencies on CachyOS

CachyOS is Arch-based, so this uses `pacman`.

```bash
sudo pacman -Syu --needed base-devel rustup pkgconf fontconfig freetype2 \
    libxkbcommon wayland libxcb libx11 libxcursor libxrandr libxi

rustup default stable
```

Notes:
- `libxkbcommon`, `wayland`, and the `libx*` packages cover both Wayland and X11. `winit` will pick whichever session you're running under automatically.
- Recommended: install `wl-clipboard` if you're on Wayland. Lumiterm's clipboard support uses `arboard` first and falls back to shelling out to `wl-copy`/`wl-paste` if that fails. Some apps (Firefox notably) offer clipboard content in a way `arboard` can't always negotiate correctly on Wayland, and `wl-clipboard` handles that more reliably.
  ```bash
  sudo pacman -S wl-clipboard
  ```
- You'll also need at least one monospace font installed. Check what you have:
  ```bash
  fc-list | grep -i mono
  ```
  If you want the one referenced in the example config:
  ```bash
  sudo pacman -S ttf-jetbrains-mono-nerd
  ```
  Or just point `font.path` in `lumiterm.conf` at any `.ttf`/`.otf` you already have.

## 2. Build

```bash
cd lumiterm
cargo build --release
```

This pulls in five crates on first build: `winit`, `softbuffer`, `portable-pty`, `vte`, `fontdue`, `arboard`. All are ordinary, actively-maintained Rust crates. `winit`/`softbuffer` handle the window and pixel buffer, `portable-pty` spawns the shell, `vte` parses ANSI/VT escape sequences (a low-level byte parser, not a terminal engine), `fontdue` rasterizes glyphs from the font file, and `arboard` handles system clipboard access (copy/paste).

## 3. Configure

```bash
mkdir -p ~/.config/lumiterm
cp lumiterm.conf ~/.config/lumiterm/lumiterm.conf
```

Edit `~/.config/lumiterm/lumiterm.conf` and set `font.path` to a real font file on your system (see `fc-list` above). If no config is found, it falls back to `/usr/share/fonts/TTF/DejaVuSansMono.ttf` and Catppuccin-ish default colors.

Note on the config syntax: `#` starts a comment only at the beginning of a line/statement. Inside a value it's read as the start of a hex color, so don't put trailing comments after a color line (put comments on their own line instead).

## 4. Run

```bash
./target/release/lumiterm
```

It'll spawn your `$SHELL` (or whatever you set in `shell { program: ... }`) inside the window.

## Copy / paste

- **Copy**: click and drag to select text. It's copied to the system clipboard automatically on mouse release. `Ctrl+Shift+C` also copies the current selection explicitly.
- **Paste**: `Ctrl+Shift+V`. If the running program has requested bracketed-paste mode (many shells and editors do), the pasted text is wrapped in the appropriate markers so it's treated as pasted input rather than typed input.
- This uses the regular system clipboard for both actions. It doesn't distinguish X11's separate PRIMARY selection (middle-click paste) from the CLIPBOARD selection the way some terminals do.
- Clipboard access tries `arboard` first and falls back to shelling out to `wl-copy`/`wl-paste` if that fails (install `wl-clipboard` for this, see install notes above).
- The selection highlight is suppressed while scrolled back into history, or right after a resize, since the stored selection coordinates only mean something against the current live grid. Rather than show a highlight that might point at the wrong text, it's hidden until you make a fresh selection.

## Scrollback

Scroll with the mouse wheel to view up to 5000 lines of history. Typing anything snaps the view back to the bottom, matching typical terminal behavior. `ESC[3J` (what the `clear` command sends alongside `ESC[2J`) properly wipes scrollback too, not just the visible screen.

Resizing is scrollback-aware: shrinking the window archives the rows that no longer fit into scrollback instead of destroying them, and growing back pulls rows back out of scrollback to refill the space, so a shrink-then-grow cycle doesn't lose content. Resize events are also debounced (settle time of 120ms) since window managers fire a continuous stream of resize events during a drag, and applying the grid resize on every single intermediate event caused real content loss from rounding jitter.

Known limitation: mouse-select-to-copy is based on the live grid, not the scrolled-back view, so selecting text while scrolled up may not grab what you'd expect. Scroll back to the bottom first if you want to select and copy something from recent output. Also, scrollback lines keep whatever width they were captured at, so if you've resized to different widths across a session, older history may look jagged rather than properly rewrapped to the current width.

## Word delete

`Ctrl+Backspace` deletes the previous word. This isn't something Lumiterm implements directly, there's no local line-editing here. It sends the `Ctrl+W` byte, which both fish and bash's readline default-bind to backward-word-delete. If you've rebound that key in your shell, this will follow whatever you've rebound it to.

## Known bugs / rough edges

- **`window.opacity` is parsed but not applied**: real transparency needs alpha-aware compositing that isn't consistent across `softbuffer`'s X11/Wayland backends, so it's left unimplemented rather than faked. The window is always fully opaque for now.
- **No wide-character (CJK/emoji) handling**: each cell assumes 1 column.
- **No font fallback**: a character missing from the configured font renders as a blank/hollow box instead of falling back to another font (e.g. Braille patterns may not render depending on the font in use).
- **Selection is line-stream only, not rectangular/block**: dragging across multiple rows selects like a text editor would (full width on middle rows), not a rectangular block.
- **No cursor blinking / shapes**: just a fixed outline block.
- **`Child` in `PtyHandle` is unused after spawn**: worth wiring up an "exit when shell exits" check in the event loop.

I am currently working to fix these. If you would like to help, please email my GitHub contact email or submit an Issue/Pull Request to ask.

Thank you.