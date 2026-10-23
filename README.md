# rustcord

A native Discord client written in Rust, using Dear ImGui (via `imgui-rs`) for
the UI, targeting Windows, macOS, and Linux.

## ⚠️ Important

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
- [x] Sending messages
- [x] TOML-based theming
- [x] Persisted login (OS keychain)
- [ ] Per-channel permission filtering (all guild channels show regardless of your actual access)
- [ ] Voice calls (not yet implemented)
- [ ] Message editing/deleting, reactions, typing indicators
- [ ] Attachments/embeds rendering
- [ ] Real avatars/guild icons (currently text initials)

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
palette:

```toml
name = "my-theme"
background    = [0.13, 0.14, 0.16, 1.0]
panel         = [0.17, 0.18, 0.20, 1.0]
text          = [0.90, 0.90, 0.92, 1.0]
text_muted    = [0.60, 0.62, 0.66, 1.0]
accent        = [0.35, 0.40, 0.95, 1.0]
accent_hover  = [0.45, 0.50, 1.0,  1.0]
border        = [0.08, 0.09, 0.10, 1.0]
```

Copy any file from `themes/` into that directory to try it, then rename it
`discord-dark.toml` (the default loaded at startup) or wire up a theme
switcher in the UI.
