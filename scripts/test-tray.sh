#!/usr/bin/env bash
set -euo pipefail
project_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$project_dir"
python3 - <<'PY'
import os, subprocess, tempfile
with tempfile.TemporaryDirectory(prefix='bili-test-tray-') as directory:
    env=dict(os.environ,XDG_DATA_HOME=directory,BILI_MUSIC_TEST='1',GSK_RENDERER='cairo')
    raise SystemExit(subprocess.call(['cargo','test','--release','--locked','-j','2','native_tray_lifecycle','--','--ignored','--test-threads=1','--nocapture'],env=env))
PY
