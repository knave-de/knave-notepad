#!/usr/bin/env bash
set -euo pipefail

usage() {
    cat <<'USAGE'
Usage: scripts/install.sh [--user | --system]

Builds and installs knave-notepad, its desktop launcher, and application icon.
Defaults to --system. System installs request sudo for privileged file writes.
USAGE
}

mode=system
case "${1-}" in
    "") ;;
    --user) mode=user ;;
    --system) mode=system ;;
    -h|--help) usage; exit 0 ;;
    *) usage >&2; exit 2 ;;
esac
if (($# > 1)); then usage >&2; exit 2; fi

script_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
project_dir=$(cd -- "$script_dir/.." && pwd)

if [[ $mode == user ]]; then
    data_home=${XDG_DATA_HOME:-"$HOME/.local/share"}
    bin_dir=${XDG_BIN_HOME:-"$HOME/.local/bin"}
    app_dir="$data_home/applications"
    icon_dir="$data_home/icons/hicolor/scalable/apps"
else
    prefix=${PREFIX:-/usr/local}
    bin_dir="$prefix/bin"
    app_dir="$prefix/share/applications"
    icon_dir="$prefix/share/icons/hicolor/scalable/apps"
fi

cargo build --release --locked --manifest-path "$project_dir/Cargo.toml"

privilege=()
if [[ $mode == system && $(id -u) -ne 0 ]]; then
    if ! command -v sudo >/dev/null 2>&1; then
        printf 'System installation needs root privileges, but sudo was not found. Run with sudo or use --user.\n' >&2
        exit 1
    fi
    sudo -v
    privilege=(sudo)
fi

"${privilege[@]}" install -Dm755 "$project_dir/target/release/knave-notepad" "$bin_dir/knave-notepad"
"${privilege[@]}" install -Dm644 "$project_dir/data/knave-notepad.desktop" "$app_dir/knave-notepad.desktop"
"${privilege[@]}" install -Dm644 "$project_dir/data/icons/knave-notepad.svg" "$icon_dir/knave-notepad.svg"

if command -v update-desktop-database >/dev/null 2>&1; then
    "${privilege[@]}" update-desktop-database "$app_dir" || true
fi
if command -v gtk-update-icon-cache >/dev/null 2>&1 && [[ $mode == system ]]; then
    "${privilege[@]}" gtk-update-icon-cache -q -t "$prefix/share/icons/hicolor" 2>/dev/null || true
fi
printf 'Installed knave-notepad (%s)\n  binary: %s\n  launcher: %s\n  icon: %s\n' "$mode" "$bin_dir/knave-notepad" "$app_dir/knave-notepad.desktop" "$icon_dir/knave-notepad.svg"
