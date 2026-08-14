## LumiTerm: A custom terminal by LuminousCat (@ShadowBytess) for CachyOS/Arch Linux.

This is a fully maintained environment until it becomes stable enough to abandon, as well as it is a passion project.

If you wish to take this for yourself and continue maintaining it, please email me at my GitHub contact email to discuss a plan.

## Known bugs

- No wide-character support (CJK, emoji) — each cell assumes one column, so these will render misaligned.
- `window.opacity` in the config is parsed but not yet applied — the window is always fully opaque for now.
- Cursor is a fixed outline block — no blinking, no shape options (beam/underline), no visibility toggling (some programs that hide the cursor via `ESC[?25l` won't have it hidden here).

**Fixed:**

- typed text not appearing in Fish (and any shell using similar 256-color theming) — was a bug in the config file parser confusing hex colors with comments. If you still see missing/wrong-colored text with a different shell or prompt theme, that's a new bug — please report it with the shell/prompt you're using.
- Copy-paste works now.
- Backwards scrolling and previous command viewing works.

I am currently working to fix these.

If you would like to help, please email my GitHub contact email or submit an Issue/Pull Request to ask.

Thank you.
