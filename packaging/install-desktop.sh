#!/usr/bin/env bash
# Adds MiniToo Studio to the KDE application menu (user-level, no root):
#   ~/.local/share/applications/minitoo-studio.desktop
#   ~/.local/share/icons/hicolor/scalable/apps/minitoo-studio.svg
# Also lets the xdg-desktop-portal recognise the app id (screen capture dialog).
# Pass --autostart to start it hidden in the tray on login. Undo with --uninstall.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
apps="$HOME/.local/share/applications"
icons="$HOME/.local/share/icons/hicolor/scalable/apps"
autostart="$HOME/.config/autostart"

if [ "${1:-}" = "--uninstall" ]; then
    rm -f "$apps/minitoo-studio.desktop" "$icons/minitoo-studio.svg" "$autostart/minitoo-studio.desktop"
    echo "removed"
    exit 0
fi

mkdir -p "$apps" "$icons"
install -m644 "$root/packaging/minitoo-studio.svg" "$icons/minitoo-studio.svg"
cat > "$apps/minitoo-studio.desktop" <<DESKTOP
[Desktop Entry]
Type=Application
Name=MiniToo Studio
GenericName=Divoom MiniToo
Comment=Pictures, screen mirroring and Claude Code status on a Divoom MiniToo
Comment[ru]=Изображения, трансляция экрана и статус Claude Code на Divoom MiniToo
Exec=$root/build/minitoo-studio
Icon=minitoo-studio
Categories=Utility;
StartupWMClass=minitoo-studio
DESKTOP
if [ "${1:-}" = "--autostart" ]; then
    mkdir -p "$autostart"
    sed "s|^Exec=.*|Exec=$root/build/minitoo-studio --hidden|" "$apps/minitoo-studio.desktop" > "$autostart/minitoo-studio.desktop"
fi
command -v update-desktop-database >/dev/null && update-desktop-database "$apps" 2>/dev/null || true
echo "installed: $apps/minitoo-studio.desktop"
