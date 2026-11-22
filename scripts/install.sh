#!/usr/bin/env bash
set -euo pipefail
project_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$project_dir"
cargo build --release --locked -j 2
app_bin_dir="$HOME/.local/bin"
app_data_dir="${XDG_DATA_HOME:-$HOME/.local/share}"
install -Dm755 target/release/bili-music "$app_bin_dir/bili-music"
install -Dm644 assets/io.github.bilimusic.Linux.desktop "$app_data_dir/applications/io.github.bilimusic.Linux.desktop"
icon_source="assets/icons/hicolor/512x512/apps/io.github.bilimusic.Linux.png"
for size in 16 22 24 32 48 64 128 256 512; do
    install -Dm644 "$icon_source" \
        "$app_data_dir/icons/hicolor/${size}x${size}/apps/io.github.bilimusic.Linux.png"
done
# Desktop launchers do not always include ~/.local/bin in PATH.
python3 - "$app_data_dir/applications/io.github.bilimusic.Linux.desktop" "$app_bin_dir/bili-music" <<'PY'
import pathlib, sys
path = pathlib.Path(sys.argv[1])
quoted = sys.argv[2].replace('\\', '\\\\').replace('"', '\\"').replace('`', '\\`').replace('$', '\\$').replace('%','%%')
path.write_text(path.read_text().replace('Exec=bili-music', f'Exec="{quoted}"'))
PY
if command -v update-desktop-database >/dev/null; then update-desktop-database "$app_data_dir/applications"; fi
if command -v gtk-update-icon-cache >/dev/null; then
    gtk-update-icon-cache -f -t "$app_data_dir/icons/hicolor" >/dev/null 2>&1 || true
fi
printf '已安装：%s\n可从应用菜单打开「Buisc」。\n' "$app_bin_dir/bili-music"
