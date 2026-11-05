#!/usr/bin/env bash
set -euo pipefail
project_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$project_dir"
cargo build --release --locked -j 2
app_bin_dir="$HOME/.local/bin"
app_data_dir="${XDG_DATA_HOME:-$HOME/.local/share}"
install -Dm755 target/release/bili-music "$app_bin_dir/bili-music"
install -Dm644 assets/io.github.bilimusic.Linux.desktop "$app_data_dir/applications/io.github.bilimusic.Linux.desktop"
install -Dm644 assets/io.github.bilimusic.Linux.svg "$app_data_dir/icons/hicolor/scalable/apps/io.github.bilimusic.Linux.svg"
# Desktop launchers do not always include ~/.local/bin in PATH.
python3 - "$app_data_dir/applications/io.github.bilimusic.Linux.desktop" "$app_bin_dir/bili-music" <<'PY'
import pathlib, sys
path = pathlib.Path(sys.argv[1])
quoted = sys.argv[2].replace('\\', '\\\\').replace('"', '\\"').replace('`', '\\`').replace('$', '\\$').replace('%','%%')
path.write_text(path.read_text().replace('Exec=bili-music', f'Exec="{quoted}"'))
PY
if command -v update-desktop-database >/dev/null; then update-desktop-database "$app_data_dir/applications"; fi
printf '已安装：%s\n可从应用菜单打开「Buisc」。\n' "$app_bin_dir/bili-music"
