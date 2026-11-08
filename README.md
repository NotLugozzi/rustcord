# rustcord
[![Build](https://github.com/NotLugozzi/rustcord/actions/workflows/build.yml/badge.svg?branch=main)](https://github.com/NotLugozzi/rustcord/actions/workflows/build.yml) ![Rust](https://img.shields.io/badge/rust-%23000000.svg?style=for-the-badge&logo=rust&logoColor=white)

A native Discord client written in Rust, using Dear ImGui (via `imgui-rs`) for
the UI, targeting Windows, macOS, and Linux.

## Important

This talks to Discord's **undocumented client API and gateway** as a regular
user account (no bot token). Discord's Terms of Service prohibit third-party
clients automating a user account; using this can get your account disabled.
Use at your own risk.

## Status

rustcord is close to feature parity with the official client for everyday
use. That covers servers, DMs and group DMs, voice calls, screen sharing and
camera video (Linux), polls, voice messages, message forwarding, message markdown and embeds, reactions, GIFs, stickers and
emoji, search, unread and mention tracking, notification and mute
settings, and most server admin actions. Video playback, screen sharing and camera
are behind the optional `video` and `screenshare` cargo features (both
need ffmpeg).

What's still missing:

- Screen sharing and camera on Windows and macOS
- Threads, and forum channels, which are built on threads
- Slash commands and bot interactions (buttons, select menus)
- Push-to-talk and global hotkeys
- Stage channels behave like ordinary voice channels
- Server folders are local only and don't sync with your Discord account

## Credits

Text rendering falls back to [DejaVu Sans](https://dejavu-fonts.github.io/) for glyphs
Inter does not cover (box drawing, dingbats and similar decorative characters common in
Discord channel names). DejaVu Sans is based on Bitstream Vera, © 2003 Bitstream, Inc.;
see `crates/rustcord-ui/assets/fonts/DejaVuSans-LICENSE.txt`.

Emoji artwork is [Twemoji](https://github.com/jdecked/twemoji) © Twitter, Inc. and other contributors, licensed
[CC-BY 4.0](https://creativecommons.org/licenses/by/4.0/). Emoji are fetched on demand and cached locally.

## Build prerequisites (Linux)

Captcha handling uses an embedded WebKitGTK webview (`wry`/`tao`). Install
before building:

```sh
# Debian/Ubuntu
sudo apt install libwebkit2gtk-4.1-dev libjavascriptcoregtk-4.1-dev

# Fedora
sudo dnf install webkit2gtk4.1-devel
```

Windows and macOS need no extra packages (WebView2 ships with Windows 11 and
is a small redistributable on Windows 10; macOS uses the built-in WKWebView).

## Layout

- `crates/rustcord-client` — REST client, Gateway (WebSocket) client, data models. No UI dependencies; usable headless.
- `crates/rustcord-ui` — winit + wgpu + imgui-rs application shell.
- `themes/` — bundled TOML theme files.

## Running

```sh
cargo run -p rustcord-ui
```

You'll be prompted for your Discord email and password (and a 2FA code if
enabled on your account). On success, the session token is saved to your
OS keychain (Secret Service on Linux, Keychain on macOS, Credential Manager
on Windows) so you won't need to log in again on next launch. Click "Out"
in the server rail to log out and clear the saved token.

## Theming

Themes are plain TOML files under `~/.config/rustcord/themes/` (platform
equivalent paths apply on Windows/macOS), each defining a small color
palette. The built-in themes are installed there on first launch, including
in packaged builds. Alongside Discord Dark, Nord, and Solarized Light, the
picker includes Aurora, Sunset, Cosmic and Cyberpunk gradients. Custom themes
may add an optional global gradient. `background` is the main app surface when
no gradient is set; a gradient replaces it across the main panes.
`panel` colors inputs, popups, and raised surfaces. `text` and `text_muted` set
primary and secondary text, `accent` colors actions and selected items,
`accent_hover` colors their hover and pressed states, and `border` colors
separators. The appearance editor lets you adjust the gradient independently
from the solid colors.

The gradient takes `use_global_gradient`, `global_gradient_start` and
`global_gradient_end`, plus two optional keys. `global_gradient_angle` uses
CSS degrees (0 = to top, 90 = to right, the default, 180 = to bottom).
`global_gradient_ratio` either takes two stops, like CSS
`linear-gradient(… 10%, … 65%)`, where the start colour is solid before the
first and the end colour after the second, or a single number for where the
two colours mix 50/50. The older `gradient = [[start], [end]]` key still loads.

```toml
name = "my-theme"
background    = [0.13, 0.14, 0.16, 1.0]
panel         = [0.17, 0.18, 0.20, 1.0]
text          = [0.90, 0.90, 0.92, 1.0]
text_muted    = [0.60, 0.62, 0.66, 1.0]
accent        = [0.35, 0.40, 0.95, 1.0]
accent_hover  = [0.45, 0.50, 1.0,  1.0]
border        = [0.08, 0.09, 0.10, 1.0]
use_global_gradient   = true
global_gradient_start = [0.10, 0.75, 0.67, 1.0]
global_gradient_end   = [0.31, 0.27, 0.78, 1.0]
global_gradient_angle = 135.0
global_gradient_ratio = [0.1, 0.65]
```

A theme can also bring its own fonts with an optional `[fonts]` table. Each
style (`regular`, `bold`, `italic`, `bold_italic`, `mono`) takes one source:
`google` (a Google Fonts family, with an optional `weight` and `italic`),
`url` (a direct link to a `.ttf` or `.otf` file) or `path` (a font file,
relative to the themes directory). `size` sets the base text size in points
(default 16). Downloaded fonts are cached, so a theme works offline after its
first use, and switching themes swaps fonts without a restart. Styles the
theme leaves out keep the built-in Inter; a missing italic falls back to the
theme's regular face. Glyphs the font lacks fall back to Inter and DejaVu
Sans. WOFF/WOFF2 files are not supported.

```toml
[fonts]
size = 17.0
regular = { google = "Rajdhani", weight = 500 }
bold = { google = "Rajdhani", weight = 700 }
mono = { path = "fonts/MyMono.ttf" }
```

The bundled `cyberpunk` theme uses this, with a palette after Patryk
Bielanin's [Cyberpunk 2077 theme](https://github.com/PatrykBielanin/discord-cyberpunk2077-theme)
for BetterDiscord.

Add your own TOML theme to that directory to make it appear in the theme
switcher. The selected theme applies immediately; `discord-dark.toml` is the
default loaded at startup.
