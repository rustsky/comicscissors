"""Toolbar action: repack the selected comics and add each result to its book, never replacing an original."""

import os
import re
import shlex
import shutil
from functools import partial

from calibre.constants import iswindows
from calibre.gui2 import error_dialog, info_dialog
from calibre.gui2.actions import InterfaceAction
from calibre.gui2.threaded_jobs import ThreadedJob
from calibre.ptempfile import PersistentTemporaryDirectory
from qt.core import QMenu, QToolButton

from calibre_plugins.comic_scissors import runner
from calibre_plugins.comic_scissors.runner import prefs

# Source formats in order of preference.
SOURCES = ('CBZ', 'CBR', 'CB7', 'ZIP', 'RAR', '7Z', 'PDF')
# Unpacked with calibre's own extractors: the CLI's bsdtar/unar/7z are often missing (always on Windows).
UNPACK = {'CBR', 'CB7', 'RAR', '7Z'}


class ComicScissorsAction(InterfaceAction):
    name = 'ComicScissors'
    action_spec = ('Repack for e-reader', 'convert.png', 'Split comic pages into panels and repack them for your e-reader', None)
    popup_type = QToolButton.ToolButtonPopupMode.MenuButtonPopup
    action_type = 'current'

    def genesis(self):
        self.menu = QMenu(self.gui)
        self.menu.addAction('Repack selected comics', self.repack_selected)
        self.menu.addAction('Settings...', self.show_settings)
        self.qaction.setMenu(self.menu)
        self.qaction.triggered.connect(self.repack_selected)

    def show_settings(self):
        self.interface_action_base_plugin.do_user_config(self.gui)

    def repack_selected(self):
        ids = self.gui.library_view.get_selected_ids()
        if not ids:
            return error_dialog(self.gui, 'No comics selected', 'Select one or more comics first.', show=True)
        db = self.gui.current_db.new_api
        opts = {k: prefs[k] for k in ('device', 'custom_width', 'custom_height', 'custom_ppi', 'custom_color', 'format', 'lang', 'ocr', 'rtl', 'rect_crop', 'overview', 'quality', 'extra_args')}
        skipped = []
        for book_id in ids:
            title = db.field_for('title', book_id)
            fmts = [f for f in db.formats(book_id) if not runner.made_by_us(db.format_abspath(book_id, f), f)]
            fmt = next((f for f in SOURCES if f in fmts), None)
            if fmt is None:
                skipped.append(title)
                continue
            job = ThreadedJob(
                'comic_scissors',
                f'Repack for e-reader: {title}',
                convert,
                (db.format_abspath(book_id, fmt), fmt, title, opts),
                {},
                partial(self.job_done, db, book_id, fmt, opts),
                max_concurrent_count=1,  # the CLI already uses every core
            )
            self.gui.job_manager.run_threaded_job(job)
        started = len(ids) - len(skipped)
        if started:
            self.gui.status_bar.show_message(f'Repacking {started} comic(s)', 3000)
        if skipped:
            info_dialog(
                self.gui,
                'Some books skipped',
                'These books have no CBZ, CBR, CB7 or PDF file:\n\n' + '\n'.join(skipped),
                show=True,
            )

    def job_done(self, db, book_id, src_fmt, opts, job):
        if job.failed:
            return self.gui.job_exception(job, dialog_title='ComicScissors failed')
        out = job.result
        if not db.has_id(book_id):
            shutil.rmtree(os.path.dirname(out), ignore_errors=True)
            return error_dialog(self.gui, 'ComicScissors', 'The book was deleted while it was being repacked.', show=True)
        fmt = opts['format'].upper()
        existing = db.format_abspath(book_id, fmt)
        if existing is None or runner.made_by_us(existing, fmt):
            # A new format, or replacing our own earlier output: listed under Formats, sendable to the e-reader.
            db.add_format(book_id, fmt, out, run_hooks=False)
            msg = f'Added {fmt} to {db.field_for("title", book_id)}'
        else:
            # The book's original file has this format: keep it, and save ours as a data file instead,
            # "data/Title - Author_repanel.pdf" (shown under Data files in Book details).
            stem = os.path.splitext(os.path.basename(existing))[0]
            name = f'{stem}_repanel.{opts["format"]}'
            db.add_extra_files(book_id, {f'data/{name}': out})
            msg = f'The original {fmt} was kept; saved {name} to the book\'s data files'
        shutil.rmtree(os.path.dirname(out), ignore_errors=True)
        if self.gui.current_db.new_api is db:
            view = self.gui.library_view
            view.model().refresh_ids([book_id], current_row=view.currentIndex().row())
        self.gui.status_bar.show_message(msg, 10000)


def convert(src, fmt, title, opts, notifications=None, abort=None, log=None):
    """Runs in a job thread. Returns the path of the repacked file."""
    progress = runner.Progress(notifications, log)
    progress.update('Preparing')
    work = PersistentTemporaryDirectory('_comic_scissors')
    # The CLI uses the input file name as the PDF title.
    name = re.sub(r'[\\/:*?"<>|\x00-\x1f]', '_', title).strip(' .')[:80] or 'comic'
    if fmt in UNPACK:
        from calibre import extract
        inp = os.path.join(work, name)
        os.mkdir(inp)
        log(f'Unpacking {fmt}')
        extract(src, inp)
    else:
        inp = os.path.join(work, f'{name}.{fmt.lower()}')
        shutil.copyfile(src, inp)
    out = os.path.join(work, f'{name}_repanel.{opts["format"]}')

    env = runner.tool_env()
    cmd = [runner.find_binary(), inp, '-o', out, '-f', opts['format']]
    if opts['device'] == 'custom':
        cmd += ['--width', str(opts['custom_width']), '--height', str(opts['custom_height']), '--ppi', str(opts['custom_ppi'])]
        if opts['custom_color']:
            cmd.append('--color')
    else:
        cmd += ['-d', opts['device']]
    if opts['rtl']:
        cmd.append('--rtl')
    cmd += ['--quality', str(opts['quality'])]
    if opts['overview']:
        cmd.append('--overview')
    if opts['rect_crop']:
        cmd.append('--rect-crop')
    if opts['format'] == 'pdf' and opts['ocr'] and runner.has_tesseract(env):
        cmd += ['--lang', opts['lang']]
    else:
        if opts['format'] == 'pdf' and opts['ocr']:
            log('Tesseract not found: converting without the dictionary text layer')
        cmd.append('--no-ocr')
    cmd += shlex.split(opts['extra_args'], posix=not iswindows)
    runner.run(cmd, env, log, abort, progress)
    progress.finish()
    return out
