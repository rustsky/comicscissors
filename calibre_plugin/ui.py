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
from qt.core import QDialog

from calibre_plugins.comic_scissors import runner
from calibre_plugins.comic_scissors.dialog import RepackDialog
from calibre_plugins.comic_scissors.editor import EditorOpener
from calibre_plugins.comic_scissors.runner import prefs

# Source formats in order of preference.
# Kindle books last: their images are often smaller than the scans in an archive or PDF.
SOURCES = ('CBZ', 'CBR', 'CB7', 'ZIP', 'RAR', '7Z', 'PDF', 'AZW3', 'MOBI', 'AZW')
# Unpacked with calibre's own extractors: the CLI's bsdtar/unar/7z are often missing (always on Windows).
UNPACK = {'CBR', 'CB7', 'RAR', '7Z'}


class ComicScissorsAction(InterfaceAction):
    name = 'ComicScissors'
    action_spec = ('Repack for e-reader', 'convert.png', 'Split comic pages into panels and repack them for your e-reader', None)
    action_type = 'current'

    def genesis(self):
        self.qaction.triggered.connect(self.repack_selected)
        # Shows the panel editor of a job waiting for its final touch.
        self.editor_opener = EditorOpener(self.gui)

    def repack_selected(self):
        ids = self.gui.library_view.get_selected_ids()
        if not ids:
            return error_dialog(self.gui, 'No comics selected', 'Select one or more comics first.', show=True)
        db = self.gui.current_db.new_api
        books, skipped = [], []
        for book_id in ids:
            title = db.field_for('title', book_id)
            fmts = [f for f in db.formats(book_id) if not runner.made_by_us(db.format_abspath(book_id, f), f)]
            fmt = next((f for f in SOURCES if f in fmts), None)
            if fmt is None:
                skipped.append(title)
            else:
                books.append((book_id, title, fmt))
        if books:
            # Manga or comic, the final touch and the settings, on one page (see dialog.py).
            manga = all(any('manga' in t.lower() for t in db.field_for('tags', b)) for b, _, _ in books)
            fixes = any(kept_panels(db, b) for b, _, _ in books)
            d = RepackDialog(self.gui, len(books), manga, fixes)
            if d.exec() != QDialog.DialogCode.Accepted:
                return
            answers = d.answers()
            opts = {k: prefs[k] for k in ('device', 'custom_width', 'custom_height', 'custom_ppi', 'custom_color', 'format', 'lang', 'ocr', 'rect_crop', 'overview', 'webtoon', 'cover', 'auto_levels', 'quality', 'extra_args', 'title_suffix')}
            opts.update(answers)
        for book_id, title, fmt in books:
            # Written into the output's metadata, so the repacked file is titled like the book.
            authors = [a for a in db.field_for('authors', book_id) if a != _('Unknown')]  # noqa: F821
            book_opts = dict(opts, authors=' & '.join(authors), kept_panels=kept_panels(db, book_id))
            job = ThreadedJob(
                'comic_scissors',
                f'Repack for e-reader: {title}',
                convert,
                (db.format_abspath(book_id, fmt), fmt, title, book_opts),
                {'on_edit': lambda url, t=title: self.editor_opener.open.emit(url, t)},
                partial(self.job_done, db, book_id, fmt, book_opts),
                max_concurrent_count=1,  # the CLI already uses every core
            )
            self.gui.job_manager.run_threaded_job(job)
        started = len(books)
        if started:
            self.gui.status_bar.show_message(f'Repacking {started} comic(s)', 3000)
        if skipped:
            info_dialog(
                self.gui,
                'Some books skipped',
                'These books have no CBZ, CBR, CB7, PDF or DRM-free AZW3/MOBI file:\n\n' + '\n'.join(skipped),
                show=True,
            )

    def job_done(self, db, book_id, src_fmt, opts, job):
        if job.failed:
            return self.gui.job_exception(job, dialog_title='ComicScissors failed')
        out = job.result
        if not db.has_id(book_id):
            shutil.rmtree(os.path.dirname(out), ignore_errors=True)
            return error_dialog(self.gui, 'ComicScissors', 'The book was deleted while it was being repacked.', show=True)
        msg, added = add_result(db, book_id, opts['format'].upper(), out, opts['title_suffix'])
        panels = os.path.join(os.path.dirname(out), 'panels.json')
        if opts.get('edit') and os.path.exists(panels):
            db.add_extra_files(book_id, {PANELS: panels}, replace=True)
            msg += '; your panel fixes are kept with the book'
        shutil.rmtree(os.path.dirname(out), ignore_errors=True)
        if self.gui.current_db.new_api is db:
            view = self.gui.library_view
            if added:
                self.gui.iactions['Add Books'].refresh_gui(1)
            view.model().refresh_ids([book_id], current_row=view.currentIndex().row())
        self.gui.status_bar.show_message(msg, 10000)


# On a repacked copy (a book of its own): the id of the book it was made from.
COPY_ID = 'comicscissors'


def add_result(db, book_id, fmt, out, suffix):
    """Store the repacked file `out` and say where: (message, whether a new book was added).

    It becomes a format of the book, unless the book's original file has that format (a CBZ
    repacked to CBZ): a book holds one file per format, so the result then goes to a copy of the
    book titled with `suffix`, made once and updated by later repacks.
    """
    existing = db.format_abspath(book_id, fmt)
    if existing is None or runner.made_by_us(existing, fmt):
        # A new format, or replacing our own earlier output: listed under Formats, sendable to the e-reader.
        db.add_format(book_id, fmt, out, run_hooks=False)
        return f'Added {fmt} to {db.field_for("title", book_id)}', False
    ids = db.all_field_for('identifiers', db.all_book_ids())
    copy = next((b for b, i in ids.items() if i.get(COPY_ID) == str(book_id)), None)
    added = copy is None
    if added:
        from calibre.ebooks.metadata import title_sort
        mi = db.get_metadata(book_id, get_cover=True, cover_as_data=True)
        mi.title = (mi.title or '') + suffix
        mi.title_sort = title_sort(mi.title)
        mi.set_identifiers({COPY_ID: str(book_id)})
        cover = mi.cover_data[1] if mi.cover_data and mi.cover_data[1] else None
        copy = db.create_book_entry(mi, cover=cover, apply_import_tags=False)
    db.add_format(copy, fmt, out, run_hooks=False)
    return f'The original {fmt} was kept; added the repacked one as "{db.field_for("title", copy)}"', added


# Panels fixed by hand in the panel editor, kept with the book (under Data files in Book details), so
# a later conversion for another device or format cuts them the same way.
PANELS = 'data/comicscissors_panels.json'


def kept_panels(db, book_id):
    return next((f.file_path for f in db.list_extra_files(book_id) if f.relpath == PANELS), None)


def convert(src, fmt, title, opts, notifications=None, abort=None, log=None, on_edit=None):
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
    out = os.path.join(work, f'{name}_repanel.{runner.EXTENSIONS[opts["format"]]}')

    env = runner.tool_env()
    cmd = [runner.find_binary(), inp, '-o', out, '-f', opts['format'], '--title', title]
    if opts.get('authors'):
        cmd += ['--author', opts['authors']]
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
    if opts['webtoon']:
        cmd.append('--webtoon')
    if opts.get('edit'):
        # The editor is shown in a calibre window (see editor.py), not the browser.
        cmd += ['--edit', '--edit-url-only']
    # Earlier panel fixes are used when they fit these pages; after editing, the CLI writes the
    # panels here and job_done keeps them with the book.
    panels = os.path.join(work, 'panels.json')
    if opts.get('kept_panels') and os.path.exists(opts['kept_panels']):
        shutil.copyfile(opts['kept_panels'], panels)
    cmd += ['--keep-panels', panels]
    if not opts['cover']:
        cmd.append('--no-cover')
    if not opts['auto_levels']:
        cmd.append('--no-auto-levels')
    if opts['rect_crop']:
        cmd.append('--rect-crop')
    # The language also goes into EPUB/AZW3 metadata; every format but CBZ gets the OCR text layer.
    cmd += ['--lang', opts['lang']]
    if opts['format'] != 'cbz' and opts['ocr'] and not runner.has_tesseract(env):
        log('Tesseract not found: converting without the dictionary text layer')
        cmd.append('--no-ocr')
    elif opts['format'] != 'cbz' and not opts['ocr']:
        cmd.append('--no-ocr')
    cmd += shlex.split(opts['extra_args'], posix=not iswindows)
    runner.run(cmd, env, log, abort, progress, on_edit=on_edit)
    progress.finish()
    return out
