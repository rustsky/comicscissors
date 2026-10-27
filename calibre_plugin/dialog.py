"""The repack pop-up: everything for a repack on one page.

On top what depends on the book: manga or comic, and the final touch in the panel editor. The last
answers are remembered as the defaults, and books tagged as manga start as manga. Below them the
plugin settings (the same form as in Preferences > Plugins), saved when Repack is clicked.
"""

from qt.core import (
    QButtonGroup, QCheckBox, QDialog, QDialogButtonBox, QGroupBox, QLabel, QRadioButton, QScrollArea, QVBoxLayout,
)

from calibre_plugins.comic_scissors.config import ConfigWidget
from calibre_plugins.comic_scissors.runner import prefs


class RepackDialog(QDialog):
    def __init__(self, parent, count, manga_tagged, has_fixes):
        super().__init__(parent)
        self.setWindowTitle(f'Repack {count} comics for e-reader' if count != 1 else 'Repack for e-reader')
        layout = QVBoxLayout(self)

        book = QGroupBox('Read the panels', self)
        box = QVBoxLayout(book)
        self.comic = QRadioButton('Left to right (comic)', book)
        self.manga = QRadioButton('Right to left (manga)', book)
        group = QButtonGroup(self)
        for b in (self.comic, self.manga):
            group.addButton(b)
            box.addWidget(b)
        (self.manga if manga_tagged or prefs['rtl'] else self.comic).setChecked(True)
        self.edit = QCheckBox('Final touch (open panel editor)', book)
        self.edit.setToolTip('After detection, every page is shown with its panels: move, resize, draw, delete, merge or split '
                             'them and set the reading order, then click "Cut the book". Your fixes are kept with the book '
                             '(Data files: comicscissors_panels.json), so later conversions, for any device or format, '
                             'cut the same panels.')
        self.edit.setChecked(prefs['edit'])
        box.addWidget(self.edit)
        if has_fixes:
            note = QLabel('Panels fixed by hand earlier are used where the book has them.', book)
            note.setEnabled(False)
            box.addWidget(note)
        layout.addWidget(book)

        settings = QGroupBox('Settings', self)
        inner = QVBoxLayout(settings)
        inner.setContentsMargins(0, 0, 0, 0)
        self.settings = ConfigWidget()
        scroll = QScrollArea(settings)
        scroll.setWidget(self.settings)
        scroll.setWidgetResizable(True)
        scroll.setFrameShape(QScrollArea.Shape.NoFrame)
        scroll.setMinimumWidth(self.settings.sizeHint().width() + 24)
        scroll.setMinimumHeight(min(self.settings.sizeHint().height() + 8, 520))
        inner.addWidget(scroll)
        layout.addWidget(settings, 1)

        buttons = QDialogButtonBox(QDialogButtonBox.StandardButton.Ok | QDialogButtonBox.StandardButton.Cancel, self)
        buttons.button(QDialogButtonBox.StandardButton.Ok).setText('Repack')
        buttons.accepted.connect(self.accept)
        buttons.rejected.connect(self.reject)
        layout.addWidget(buttons)

    def answers(self):
        """Save the settings and return {'rtl': bool, 'edit': bool}, also remembered as the next defaults."""
        self.settings.save_settings()
        out = {'rtl': self.manga.isChecked(), 'edit': self.edit.isChecked()}
        prefs['rtl'], prefs['edit'] = out['rtl'], out['edit']
        return out
