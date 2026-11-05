#!/usr/bin/env bash
set -euo pipefail
project_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$project_dir"
test_data_dir="$(mktemp -d -t bili-test-XXXXXXXX)"
trap 'rm -rf -- "$test_data_dir"' EXIT
export XDG_DATA_HOME="$test_data_dir"
export BILI_MUSIC_TEST=1
export GSK_RENDERER=cairo
mkdir -p docs
export BILI_MUSIC_SCREENSHOT="$project_dir/docs/screenshot.png"
cargo test --release --locked -j 2 live_native_player -- --ignored --test-threads=1 --nocapture
