"""Find and run the comic-scissors binary. Nothing here touches the GUI, so jobs can call it."""

import os
import platform
import shutil
import subprocess
import sys
import threading
import time
import zipfile

from calibre.constants import ismacos, iswindows
from calibre.utils.config import JSONConfig, config_dir

EXE = 'comic-scissors.exe' if iswindows else 'comic-scissors'
# File extension of each output format.
EXTENSIONS = {'pdf': 'pdf', 'cbz': 'cbz', 'epub': 'epub', 'kepub': 'kepub.epub', 'azw3': 'azw3'}

prefs = JSONConfig('plugins/comic_scissors')
prefs.defaults['device'] = 'paperwhite-5'
prefs.defaults['format'] = 'pdf'
prefs.defaults['lang'] = 'en'
prefs.defaults['ocr'] = True
prefs.defaults['rtl'] = False
prefs.defaults['rect_crop'] = False
prefs.defaults['overview'] = False
prefs.defaults['webtoon'] = False
prefs.defaults['edit'] = False
prefs.defaults['cover'] = True
prefs.defaults['auto_levels'] = True
prefs.defaults['quality'] = 88
prefs.defaults['custom_width'] = 1264
prefs.defaults['custom_height'] = 1680
prefs.defaults['custom_ppi'] = 300
prefs.defaults['custom_color'] = False
prefs.defaults['extra_args'] = ''
prefs.defaults['binary'] = ''  # empty: the binary bundled with the plugin, then PATH

_bundled = None


def _platform_dir():
    """Folder of this platform's binary in the universal plugin zip (scripts/package.py --universal)."""
    if iswindows:
        return 'windows-x86_64'
    if ismacos:
        return 'macos-universal'
    return 'linux-' + platform.machine()


def bundled_binary():
    """Unpack the bundled comic-scissors from the plugin zip into calibre's config dir once per version.

    The universal zip has one binary per platform in bin/<platform>/, a per-platform zip has it in bin/.
    """
    global _bundled
    if _bundled is None:
        from calibre_plugins.comic_scissors import ComicScissorsPlugin
        from calibre.customize.ui import find_plugin
        # Look before reading: get_resources prints a traceback for a missing name.
        with zipfile.ZipFile(find_plugin(ComicScissorsPlugin.name).plugin_path) as z:
            names = set(z.namelist())
        name = next((n for n in (f'bin/{_platform_dir()}/{EXE}', 'bin/' + EXE) if n in names), None)
        # get_resources is injected by calibre's plugin loader.
        data = get_resources(name) if name else None  # noqa: F821
        if not data:
            _bundled = ''
            return None
        version = '.'.join(map(str, ComicScissorsPlugin.version))
        path = os.path.join(config_dir, 'plugins', 'comic_scissors', version, EXE)
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
    """True for a file written by comic-scissors (an earlier repack), False for an original."""
    try:
        if fmt == 'PDF':
            with open(path, 'rb') as f:
                return b'/Producer (comic-scissors)' in f.read(1 << 16)
        if fmt == 'AZW3':
            # The book producer in the EXTH header, near the start of the file.
            with open(path, 'rb') as f:
                return b'comic-scissors' in f.read(1 << 16)
        if fmt in ('CBZ', 'EPUB', 'KEPUB'):
            with zipfile.ZipFile(path) as z:
                return z.comment == b'comic-scissors'
    except (OSError, TypeError, zipfile.BadZipFile):
        pass
    return False


def find_binary():
    custom = prefs['binary'].strip()
    if custom:
        if not os.path.isfile(custom):
            raise FileNotFoundError(f'comic-scissors binary not found: {custom}')
        return custom
    path = bundled_binary() or shutil.which(EXE, path=tool_env()['PATH'])
    if not path:
        raise FileNotFoundError('comic-scissors binary not found: set its path in the plugin settings')
    return path


def tool_env():
    """PATH with calibre's own tools (ebook-convert, for AZW3), its bundled Poppler (pdftoppm,
    pdfinfo) and the usual Tesseract locations."""
    env = dict(os.environ)
    dirs = [getattr(sys, 'executables_location', '')]
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
    """[(brand, id, name)] from `comic-scissors --list-devices`, which prints a "Brand:" line
    followed by "  id  WIDTHxHEIGHT PPI ppi  name" lines."""
    out = subprocess.run([find_binary(), '--list-devices'], capture_output=True, text=True, **_no_window())
    devices = []
    brand = ''
    for line in out.stdout.splitlines():
        if line and not line[0].isspace() and line.endswith(':'):
            brand = line[:-1]
            continue
        parts = line.split(None, 4)
        if len(parts) == 5:
            devices.append((brand, parts[0], parts[4]))
    return devices


# The job's steps: CLI stage name -> (start, end) share of the whole job, label.
# Detection dominates: it includes OCR, about 1 s per page per core.
STEPS = {
    'Preparing': (0.0, 0.03, 'Preparing file'),
    'Loading': (0.03, 0.18, 'Loading pages'),
    'Detecting': (0.18, 0.75, 'Detecting panels'),
    'Refining': (0.75, 0.85, 'Checking doubtful panels'),
    # Only with "Fix panels by hand": the job waits until the editor's Cut button is pressed.
    'Editing': (0.85, 0.85, 'Waiting for your panel fixes in the editor window'),
    'Rendering': (0.85, 0.98, 'Rendering pages'),
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
            elif stage != 'Editing':
                self.log(f'  \u2013 {i + 1}/{len(ORDER)} {label:<26} skipped')

    def _advance(self, i):
        now = time.monotonic()
        if self.step >= 0:
            self.durations[self.step] = now - self.step_started
            self.log(f'\u2713 Step {self.step + 1}/{len(ORDER)}: {STEPS[ORDER[self.step]][2]} ({_duration(now - self.step_started)})')
        for skipped in range(self.step + 1, i):
            if skipped < len(ORDER) and ORDER[skipped] != 'Editing':
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


def run(cmd, env, log, abort, progress=None, on_edit=None):
    """Run cmd, copying its output into the job log and its progress into the job's progress bar.
    Killed when the job is aborted. An '@@edit <url>' line (the panel editor waiting) is passed to
    on_edit."""
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
        if line.startswith('@@edit '):
            if on_edit is not None:
                on_edit(line.split(None, 1)[1])
            continue
        lines.append(line)
        log(line)
    p.wait()
    if abort.is_set():
        raise RuntimeError('cancelled')
    if p.returncode != 0:
        raise RuntimeError('comic-scissors failed:\n' + '\n'.join(lines[-20:]))


def _no_window():
    return {'creationflags': subprocess.CREATE_NO_WINDOW} if iswindows else {}
