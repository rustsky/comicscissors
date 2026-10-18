"""Find and run the comic-magic binary. Nothing here touches the GUI, so jobs can call it."""

import os
import shutil
import subprocess
import threading
import time
import zipfile

from calibre.constants import ismacos, iswindows
from calibre.utils.config import JSONConfig, config_dir

EXE = 'comic-magic.exe' if iswindows else 'comic-magic'

prefs = JSONConfig('plugins/comic_magic')
prefs.defaults['device'] = 'paperwhite-5'
prefs.defaults['format'] = 'pdf'
prefs.defaults['lang'] = 'en'
prefs.defaults['ocr'] = True
prefs.defaults['rtl'] = False
prefs.defaults['rect_crop'] = False
prefs.defaults['extra_args'] = ''
prefs.defaults['binary'] = ''  # empty: the binary bundled with the plugin, then PATH

_bundled = None


def bundled_binary():
    """Unpack bin/comic-magic from the plugin zip into calibre's config dir once per version."""
    global _bundled
    if _bundled is None:
        from calibre_plugins.comic_magic import ComicMagicPlugin
        data = get_resources('bin/' + EXE)  # noqa: F821 (injected by calibre's plugin loader)
        if not data:
            _bundled = ''
            return None
        version = '.'.join(map(str, ComicMagicPlugin.version))
        path = os.path.join(config_dir, 'plugins', 'comic_magic', version, EXE)
        if not os.path.exists(path) or os.path.getsize(path) != len(data):
            os.makedirs(os.path.dirname(path), exist_ok=True)
            tmp = path + '.part'
            with open(tmp, 'wb') as f:
                f.write(data)
            os.chmod(tmp, 0o755)
            os.replace(tmp, path)
            if ismacos:
                subprocess.run(['xattr', '-d', 'com.apple.quarantine', path], capture_output=True)
        _bundled = path
    return _bundled or None


def made_by_us(path, fmt):
    """True for a PDF or CBZ written by comic-magic (an earlier repack), False for an original."""
    try:
        if fmt == 'PDF':
            with open(path, 'rb') as f:
                return b'/Producer (comic-magic)' in f.read(1 << 16)
        if fmt == 'CBZ':
            with zipfile.ZipFile(path) as z:
                return z.comment == b'comic-magic'
    except (OSError, TypeError, zipfile.BadZipFile):
        pass
    return False


def find_binary():
    custom = prefs['binary'].strip()
    if custom:
        if not os.path.isfile(custom):
            raise FileNotFoundError(f'comic-magic binary not found: {custom}')
        return custom
    path = bundled_binary() or shutil.which(EXE, path=tool_env()['PATH'])
    if not path:
        raise FileNotFoundError('comic-magic binary not found: set its path in the plugin settings')
    return path


def tool_env():
    """PATH with calibre's bundled Poppler (pdftoppm, pdfinfo) and the usual Tesseract locations."""
    env = dict(os.environ)
    dirs = []
    try:
        from calibre.ebooks.metadata.pdf import get_tools
        dirs.append(os.path.dirname(get_tools()[1]))
    except Exception:
        pass
    if ismacos:
        # GUI apps on macOS don't inherit the shell PATH, so Homebrew/MacPorts are missing.
        dirs += ['/opt/homebrew/bin', '/usr/local/bin', '/opt/local/bin']
    elif iswindows:
        dirs += [os.path.join(os.environ.get(v, ''), 'Tesseract-OCR') for v in ('ProgramFiles', 'ProgramFiles(x86)')]
    else:
        dirs += ['/usr/local/bin', '/usr/bin']
    dirs = [d for d in dirs if d and os.path.isdir(d)]
    env['PATH'] = os.pathsep.join(dirs + [env.get('PATH', '')])
    return env


def has_tesseract(env):
    return shutil.which('tesseract', path=env['PATH']) is not None


def list_devices():
    """[(id, description)] from `comic-magic --list-devices`."""
    out = subprocess.run([find_binary(), '--list-devices'], capture_output=True, text=True, **_no_window())
    devices = []
    for line in out.stdout.splitlines():
        parts = line.split(None, 1)
        if len(parts) == 2:
            devices.append((parts[0], parts[1].strip()))
    return devices


# The job's steps: CLI stage name -> (start, end) share of the whole job, label.
# Detection dominates: it includes OCR, about 1 s per page per core.
STEPS = {
    'Preparing': (0.0, 0.03, 'Preparing file'),
    'Loading': (0.03, 0.18, 'Loading pages'),
    'Detecting': (0.18, 0.75, 'Detecting panels'),
    'Refining': (0.75, 0.85, 'Checking doubtful panels'),
    'Rendering': (0.85, 0.98, 'Rendering Kindle pages'),
    'Writing': (0.98, 1.0, 'Writing file'),
}
ORDER = list(STEPS)


class Progress:
    """Shows the job's steps in calibre's Jobs window: the current step, progress and time left in
    the Status and Progress columns, and every step with its duration in the job details."""

    def __init__(self, notifications, log):
        self.notifications = notifications
        self.log = log
        self.started = time.monotonic()
        self.step = -1
        self.step_started = self.started
        self.durations = {}  # step index -> seconds; missing = skipped

    def update(self, stage, pos=0, total=0):
        if stage not in STEPS:
            return
        i = ORDER.index(stage)
        if i != self.step:
            self._advance(i)
        start, end, label = STEPS[stage]
        fraction = start + (end - start) * min(pos, total) / total if total > 0 else start
        msg = f'Step {i + 1}/{len(ORDER)}: {label}'
        if total > 0:
            msg += f' {pos}/{total}'
        left = self.time_left(fraction)
        if left:
            msg += f', {left}'
        if self.notifications is not None:
            self.notifications.put((fraction, msg))

    def finish(self):
        self._advance(len(ORDER))
        self.log('')
        self.log(f'Done in {_duration(time.monotonic() - self.started)}:')
        for i, stage in enumerate(ORDER):
            label = STEPS[stage][2]
            if i in self.durations:
                self.log(f'  \u2713 {i + 1}/{len(ORDER)} {label:<26} {_duration(self.durations[i])}')
            else:
                self.log(f'  \u2013 {i + 1}/{len(ORDER)} {label:<26} skipped')

    def _advance(self, i):
        now = time.monotonic()
        if self.step >= 0:
            self.durations[self.step] = now - self.step_started
            self.log(f'\u2713 Step {self.step + 1}/{len(ORDER)}: {STEPS[ORDER[self.step]][2]} ({_duration(now - self.step_started)})')
        for skipped in range(self.step + 1, i):
            if skipped < len(ORDER):
                self.log(f'\u2013 Step {skipped + 1}/{len(ORDER)}: {STEPS[ORDER[skipped]][2]} (skipped)')
        if i < len(ORDER):
            self.log(f'\u25b6 Step {i + 1}/{len(ORDER)}: {STEPS[ORDER[i]][2]}')
        self.step = i
        self.step_started = now

    def time_left(self, fraction):
        elapsed = time.monotonic() - self.started
        if fraction < 0.05 or elapsed < 5:
            return ''
        secs = elapsed * (1 - fraction) / fraction
        if secs < 60:
            return f'about {max(5, round(secs / 5) * 5)} s left'
        return f'about {round(secs / 60)} min left'

    def line(self, line):
        """Handle an '@@progress Detecting|3|12' line."""
        try:
            stage, pos, total = line.split(None, 1)[1].split('|')
            self.update(stage, int(pos), int(total))
        except (IndexError, ValueError):
            pass


def _duration(secs):
    secs = round(secs)
    return f'{secs // 60} min {secs % 60} s' if secs >= 60 else f'{secs} s'


def run(cmd, env, log, abort, progress=None):
    """Run cmd, copying its output into the job log and its progress into the job's progress bar.
    Killed when the job is aborted."""
    cmd = cmd + ['--progress-lines']
    log(' '.join(cmd))
    p = subprocess.Popen(
        cmd, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, env=env, **_no_window()
    )

    def watch():
        while p.poll() is None:
            if abort.wait(0.5):
                p.kill()
                return

    threading.Thread(target=watch, daemon=True).start()
    lines = []
    for raw in p.stdout:
        line = raw.decode('utf-8', 'replace').rstrip()
        if line.startswith('@@progress'):
            if progress is not None:
                progress.line(line)
            continue
        lines.append(line)
        log(line)
    p.wait()
    if abort.is_set():
        raise RuntimeError('cancelled')
    if p.returncode != 0:
        raise RuntimeError('comic-magic failed:\n' + '\n'.join(lines[-20:]))


def _no_window():
    return {'creationflags': subprocess.CREATE_NO_WINDOW} if iswindows else {}
