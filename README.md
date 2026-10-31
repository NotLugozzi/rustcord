# rustcord
[![Build](https://github.com/NotLugozzi/rustcord/actions/workflows/build.yml/badge.svg?branch=main)](https://github.com/NotLugozzi/rustcord/actions/workflows/build.yml)
A native Discord client written in Rust, using Dear ImGui (via `imgui-rs`) for
the UI, targeting Windows, macOS, and Linux.

## Important

This talks to Discord's **undocumented client API and gateway** as a regular
user account (no bot token). Discord's Terms of Service prohibit third-party
clients automating a user account; using this can get your account disabled.
Use at your own risk.

## Status

- [x] Login (email + password + 2FA)
- [x] hCaptcha challenge handling (embedded webview)
- [x] Gateway connection, READY/MESSAGE_CREATE handling
- [x] Server list, channel list (grouped by category), DM list, chat view
- [x] Friends list, DM/friend search
- [x] Sending messages, chronologically ordered with timestamps, scrollback (load older history)
- [x] Image/GIF attachments: receive (static + animated, up to 3 concurrent animations) and send
- [x] DM list sorted by most recent activity
- [x] TOML-based theming (switchable live from Settings)
- [x] Persisted login (OS keychain)
- [x] Markdown: bold, italic, strikethrough, underline, spoilers, blockquotes, headers, inline/block code, links
- [x] Typing indicators (send + receive)
- [x] Real avatars (users) and guild icons
- [x] User bar with settings dialog (account info, logout, theme switcher)
- [x] About panel (version, tag, commit hash)
- [x] Server Calls
- [x] Message editing/deleting
- [x] Custom emote rendering/sending
- [x] Subtext (`-# tiny text`)
- [x] Guild folders
- [x] Embed Rendering
- [x] Voice calls (DMs: ring/answer/decline, call messages, participant panel)
- [x] Non-image attachments/embeds rendering
- [x] Per-user volume/mute in server calls
- [x] Better statistics for voice chat (dropped frames, latency graph, encryption epoch)
- [x] Trackpad scroll acceleration
- [x] Per-channel permission filtering
- [x] UI Gliphs for buttons and states
- [x] App Keybinds
- [x] Server users panel (low priority)
- [x] VC Leave/Join sound
- [x] Inline video attachment playback (optional `video` feature, needs ffmpeg)
- [ ] Reactions
- [ ] Sync guild folders with discord (or find a way to export server ordering and folders)
- [x] Message read status sync
- [ ] performance improvements in voice calls (SSRC decoder leak fixed; re-measure CPU)
- [ ] Video Calls, screen sharing
- [ ] server admin actions (low priority)

## Credits

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
an optional two-color horizontal gradient:

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
