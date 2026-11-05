#!/usr/bin/env python3
"""Profile the real release binary, including MPRIS controls, in a temporary library."""
import json
import os
import pathlib
import subprocess
import tempfile
import time

ROOT = pathlib.Path(__file__).resolve().parent.parent
DEST = 'org.mpris.MediaPlayer2.bilimusic'
PATH = '/org/mpris/MediaPlayer2'

def dbus(method, *args):
    result = subprocess.run(['gdbus', 'call', '--session', '--dest', DEST,
        '--object-path', PATH, '--method', method, *args], capture_output=True, text=True, timeout=10)
    if result.returncode:
        raise RuntimeError(result.stderr.strip())
    return result.stdout.strip()

def prop(name):
    return dbus('org.freedesktop.DBus.Properties.Get', 'org.mpris.MediaPlayer2.Player', name)

def cpu_ticks(pid):
    fields = pathlib.Path(f'/proc/{pid}/stat').read_text().rsplit(')', 1)[1].split()
    return int(fields[11]) + int(fields[12])

def sample(pid, stage, duration=10):
    start = time.monotonic()
    ticks = cpu_ticks(pid)
    time.sleep(duration)
    elapsed = time.monotonic() - start
    percent = (cpu_ticks(pid) - ticks) / os.sysconf('SC_CLK_TCK') / elapsed * 100
    status = pathlib.Path(f'/proc/{pid}/status').read_text()
    mem = pathlib.Path(f'/proc/{pid}/smaps_rollup').read_text()
    rss = int(next(line.split()[1] for line in status.splitlines() if line.startswith('VmRSS:')))
    pss = int(next(line.split()[1] for line in mem.splitlines() if line.startswith('Pss:')))
    result = dict(stage=stage, rss_mib=round(rss/1024,1), pss_mib=round(pss/1024,1), cpu_percent_one_core=round(percent,2), sample_seconds=round(elapsed,1))
    print(json.dumps(result, ensure_ascii=False), flush=True)
    return result

(ROOT / 'docs').mkdir(exist_ok=True)
with tempfile.TemporaryDirectory(prefix='bili-test-profile-') as directory:
    env = dict(os.environ, XDG_DATA_HOME=directory)
    library_path=pathlib.Path(directory, 'bili-music/library.json')
    library_path.parent.mkdir()
    library_path.write_text(json.dumps({'queue':[{'id':'BV1BDk2YCEHF','bvid':'BV1BDk2YCEHF',
        'title':'网络音频验证样本','artist':'测试','cover':'','duration':212,'play_count':0}]}))
    with (ROOT / 'docs/desktop-smoke.log').open('w') as log:
        process = subprocess.Popen([str(ROOT/'target/release/bili-music')], env=env, stdout=log, stderr=log)
        try:
            time.sleep(8)
            assert process.poll() is None, 'application exited early'
            assert 'Stopped' in prop('PlaybackStatus'), prop('PlaybackStatus')
            measurements = [sample(process.pid, 'idle')]
            dbus('org.freedesktop.DBus.Properties.Set', 'org.mpris.MediaPlayer2.Player', 'Volume', '<0.0>')
            dbus('org.mpris.MediaPlayer2.Player.Play')
            deadline = time.monotonic() + 30
            while 'Playing' not in prop('PlaybackStatus'):
                assert time.monotonic() < deadline, 'MPRIS Play never reached Playing'
                time.sleep(.5)
            time.sleep(2)
            assert 'xesam:title' in prop('Metadata'), 'missing metadata'
            measurements.append(sample(process.pid, 'playing', 5))
            dbus('org.mpris.MediaPlayer2.Player.Pause')
            time.sleep(1)
            assert 'Paused' in prop('PlaybackStatus'), prop('PlaybackStatus')
            measurements.append(sample(process.pid, 'paused'))
            dbus('org.mpris.MediaPlayer2.Player.Stop')
            time.sleep(.5)
            assert 'Stopped' in prop('PlaybackStatus'), prop('PlaybackStatus')
            dbus('org.mpris.MediaPlayer2.Quit')
            process.wait(timeout=10)
            (ROOT/'docs/performance.json').write_text(json.dumps(measurements, ensure_ascii=False, indent=2)+'\n')
            assert library_path.is_file()
            print('MPRIS volume/play/pause/stop/quit and persisted library: OK', flush=True)
        finally:
            if process.poll() is None:
                process.terminate()
                process.wait(timeout=10)

# Local diagnostics are ignored by Git; redact the home directory as well.
log_path = ROOT / 'docs/desktop-smoke.log'
if log_path.exists():
    log_path.write_text(log_path.read_text().replace(str(pathlib.Path.home()), '$HOME'))
