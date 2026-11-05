#!/usr/bin/env bash
set -euo pipefail
app_data_dir="${XDG_DATA_HOME:-$HOME/.local/share}"
rm -f "$HOME/.local/bin/bili-music" "$app_data_dir/applications/io.github.bilimusic.Linux.desktop" "$app_data_dir/icons/hicolor/scalable/apps/io.github.bilimusic.Linux.svg"
printf '已卸载程序，个人资料库保留在 %s/bili-music/。\n' "$app_data_dir"
