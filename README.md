# rnd — Rust Notification Daemon

A GTK4-based Wayland notification daemon implementing the
[FDO Desktop Notifications spec](https://specifications.freedesktop.org/notification-spec/latest/).
Built as a drop-in replacement for Dunst, styled with CSS, and extensible via a TOML config file.

## Features

- **FDO-compliant** D-Bus interface (`org.freedesktop.Notifications`)
- **GTK4 + layer-shell** popups anchored to any corner
- **CSS theming** — override any rule in `~/.config/rnd/style.css`
- **Urgency levels** — low / normal / critical with distinct styling
- **Action buttons** — forwarded back to the sending application
- **Notification history** — in-memory ring buffer, optionally persisted to
  `~/.local/share/rnd/history.json` (for your launcher to consume)
- **Replaces-ID** support — updates existing cards in place

## Dependencies

### System libraries

| Distro | Package names |
|---|---|
| Arch | `gtk4` `gtk4-layer-shell` |
| Ubuntu / Debian | `libgtk-4-dev` `libgtk4-layer-shell-dev` |
| Fedora | `gtk4-devel` `gtk4-layer-shell-devel` |

### Rust toolchain

```
rustup update stable
```

## Build & run

```bash
cargo build --release
./target/release/rnd
```

Or install to `~/.cargo/bin`:

```bash
cargo install --path .
```

## Configuration

Copy the example config to your XDG config dir:

```bash
mkdir -p ~/.config/rnd
cp config.toml ~/.config/rnd/config.toml
```

Edit to taste. All keys are optional — missing keys fall back to built-in defaults.

## Theming

Copy or write `~/.config/rnd/style.css`. It is loaded at a higher priority than
the built-in `style.css`, so you only need to override the rules you care about.

```css
/* Example: rounder cards with a blue accent */
.notification {
  border-radius: 12px;
  border-left: 4px solid #7aa2f7;
  background-color: #1a1b26;
}

.notification-summary {
  color: #c0caf5;
}
```

GTK4 CSS reference: <https://docs.gtk.org/gtk4/css-properties.html>

### CSS classes reference

| Class | Applied to |
|---|---|
| `.notification-list` | The outer VBox container |
| `.notification` | Each card |
| `.urgency-low` `.urgency-normal` `.urgency-critical` | Urgency modifier on the card |
| `.notification-icon` | The app icon image |
| `.notification-text` | The text column inside the card |
| `.notification-app-name` | Small app-name label |
| `.notification-summary` | Bold summary line |
| `.notification-body` | Body text |
| `.notification-actions` | Action button row |
| `.notification-action` | Individual action button |
| `.notification-close` | The × dismiss button |

## Notification history

History is written to `~/.local/share/rnd/history.json` as a JSON array sorted
newest-first. Each entry contains: `id`, `app_name`, `app_icon`, `summary`,
`body`, `urgency`, `timestamp`.

Your launcher (rofi, fuzzel, etc.) can read this file directly or you can build

## Command-line control with `rndctl`

A small companion binary is available at `rndctl`, which talks to the daemon via
`org.freedesktop.Notifications` and the persisted history file.

Example usage:

```bash
cargo run -p rndctl -- close 42
cargo run -p rndctl -- close-all
cargo run -p rndctl -- history
cargo run -p rndctl -- history --limit 20
cargo run -p rndctl -- history clear
cargo run -p rndctl -- info
cargo run -p rndctl -- capabilities
```

If you install the workspace, `rndctl` is available alongside `rnd`.

Your launcher can use `rndctl history` or read the history file directly.

Your launcher (rofi, fuzzel, etc.) can read this file directly or you can build
a small wrapper script around it.

## Replacing Dunst

1. Stop / disable Dunst:
   ```bash
   systemctl --user stop dunst
   systemctl --user disable dunst
   ```
2. Add `rnd` to your Wayland compositor autostart.
3. Any app using `libnotify` / `notify-send` will automatically use `rnd`
   because it claims the `org.freedesktop.Notifications` D-Bus name.

## Roadmap ideas

- [ ] Per-app timeout / urgency rules in config
- [ ] `notify-send`-style CLI client to query history or close notifications
- [x] D-Bus activation (socket-activated via systemd user unit)
- [x] Image-data hint support (inline images in notifications)
- [ ] Animation (slide-in / fade-out via GTK4 transitions)
- [ ] Sound support via PipeWire / libcanberra

## License

MIT
