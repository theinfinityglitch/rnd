#!/usr/bin/env bash
# install.sh — build and install rnd as a proper user daemon
#
# Usage:
#   ./install.sh           — build release, install everything, start the service
#   ./install.sh --no-start — install but don't start/enable the service
set -euo pipefail

START=true
for arg in "$@"; do
  [[ "$arg" == "--no-start" ]] && START=false
done

# ── 1. Build ──────────────────────────────────────────────────────────────────
echo ":: Building rnd (release)..."
cargo build --release

BINARY="$(pwd)/target/release/rnd"
INSTALL_BIN="$HOME/.local/bin/rnd"

# ── 2. Install binary ─────────────────────────────────────────────────────────
echo ":: Installing binary → $INSTALL_BIN"
mkdir -p "$HOME/.local/bin"
install -m 755 "$BINARY" "$INSTALL_BIN"

# ── 3. systemd user service ───────────────────────────────────────────────────
SYSTEMD_DIR="$HOME/.config/systemd/user"
echo ":: Installing systemd unit → $SYSTEMD_DIR/rnd.service"
mkdir -p "$SYSTEMD_DIR"
# Rewrite %h in ExecStart with the literal path to be safe across
# systemd versions that don't expand %h in user units.
sed "s|%h/.cargo/bin/rnd|$INSTALL_BIN|g" contrib/rnd.service \
  > "$SYSTEMD_DIR/rnd.service"

systemctl --user daemon-reload

# ── 4. D-Bus session service override ────────────────────────────────────────
DBUS_DIR="$HOME/.local/share/dbus-1/services"
DBUS_FILE="$DBUS_DIR/org.freedesktop.Notifications.service"
echo ":: Installing D-Bus activation override → $DBUS_FILE"
mkdir -p "$DBUS_DIR"
sed "s|@@RND_BIN@@|$INSTALL_BIN|g" contrib/org.freedesktop.Notifications.service \
  > "$DBUS_FILE"

# ── 5. Stop competing daemons ─────────────────────────────────────────────────
for daemon in dunst mako; do
  if systemctl --user is-active --quiet "$daemon" 2>/dev/null; then
    echo ":: Stopping $daemon (will be replaced by rnd)..."
    systemctl --user stop "$daemon" || true
    # Mask it so it doesn't auto-start again via D-Bus activation.
    systemctl --user mask "$daemon" || true
    echo "   Masked $daemon.service — undo with: systemctl --user unmask $daemon"
  fi
done

# ── 6. Enable and start rnd ────────────────────────────────────────────────────
if $START; then
  echo ":: Enabling and starting rnd..."
  systemctl --user enable rnd.service
  systemctl --user restart rnd.service
  sleep 1
  if systemctl --user is-active --quiet rnd; then
    echo "✓ rnd is running."
  else
    echo "✗ rnd failed to start. Check: journalctl --user -u rnd -e"
    exit 1
  fi
else
  echo ":: Skipped start (--no-start). Run manually:"
  echo "   systemctl --user enable --now rnd"
fi

cat <<'DONE'

── Done ────────────────────────────────────────────────────────────────────────

Hyprland autostart (add ONE of these to hyprland.conf, not both):

  # Option A — preferred: let systemd manage the lifecycle
  exec-once = systemctl --user start rnd

  # Option B — direct launch (works without systemd integration)
  exec-once = $HOME/.local/bin/rnd

Make sure your Hyprland config exports the Wayland environment to systemd.
Most setups already have this, but if not, add:

  exec-once = dbus-update-activation-environment --systemd WAYLAND_DISPLAY XDG_CURRENT_DESKTOP DISPLAY

Useful commands:
  journalctl --user -u rnd -f          # live logs
  systemctl --user status rnd          # service status
  systemctl --user restart rnd         # restart after a rebuild
DONE