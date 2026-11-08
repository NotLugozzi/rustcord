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
use. That covers servers, DMs and group DMs, voice calls, screen sharing
(Linux), message markdown and embeds, reactions, GIFs, stickers and
emoji, search, unread and mention tracking, notification and mute
settings, and most server admin actions. Video playback and screen sharing
are behind the optional `video` and `screenshare` cargo features (both
need ffmpeg).

What's still missing:

- Camera video in calls (sending and receiving webcams)
- Screen sharing on Windows and macOS
- Threads, and forum channels, which are built on threads
- Slash commands and bot interactions (buttons, select menus)
- Polls
- Voice message sending and message forwarding
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
picker includes Aurora, Sunset, and Cosmic gradients. Custom themes may add
an optional two-color horizontal gradient. `background` is the main app
surface when no gradient is set; a gradient replaces it across the main panes.
`panel` colors inputs, popups, and raised surfaces. `text` and `text_muted` set
primary and secondary text, `accent` colors actions and selected items,
`accent_hover` colors their hover and pressed states, and `border` colors
separators. The appearance editor lets you adjust the gradient independently
from the solid colors.

```toml
name = "my-theme"
background    = [0.13, 0.14, 0.16, 1.0]
panel         = [0.17, 0.18, 0.20, 1.0]
text          = [0.90, 0.90, 0.92, 1.0]
text_muted    = [0.60, 0.62, 0.66, 1.0]
accent        = [0.35, 0.40, 0.95, 1.0]
accent_hover  = [0.45, 0.50, 1.0,  1.0]
border        = [0.08, 0.09, 0.10, 1.0]
gradient      = [[0.10, 0.75, 0.67, 1.0], [0.31, 0.27, 0.78, 1.0]]
```

Add your own TOML theme to that directory to make it appear in the theme
switcher. The selected theme applies immediately; `discord-dark.toml` is the
default loaded at startup.
