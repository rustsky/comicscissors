"""Asked on every repack: manga or comic, and whether to fix the panels by hand first.

Both depend on the book rather than on the e-reader, so they are asked here instead of in the
settings. The last answers are remembered as the defaults, and books tagged as manga start as manga.
"""

from qt.core import QButtonGroup, QCheckBox, QDialog, QDialogButtonBox, QLabel, QRadioButton, QVBoxLayout

from calibre_plugins.comic_scissors.runner import prefs


class RepackDialog(QDialog):
    def __init__(self, parent, count, manga_tagged, has_fixes):
        super().__init__(parent)
        self.setWindowTitle('Repack for e-reader')
        layout = QVBoxLayout(self)
        layout.addWidget(QLabel(f'Repack {count} comic(s). Read the panels:' if count != 1 else 'Read the panels:', self))

        self.comic = QRadioButton('Left to right (comic)', self)
        self.manga = QRadioButton('Right to left (manga)', self)
        group = QButtonGroup(self)
        for b in (self.comic, self.manga):
            group.addButton(b)
            layout.addWidget(b)
        (self.manga if manga_tagged or prefs['rtl'] else self.comic).setChecked(True)

        self.edit = QCheckBox('Fix the panels by hand before cutting (opens the panel editor)', self)
        self.edit.setToolTip('After detection, every page is shown with its panels: move, resize, draw, delete, merge or split '
                             'them and set the reading order, then click "Cut the book". Your fixes are kept with the book '
                             '(Data files: comicscissors_panels.json), so later conversions, for any device or format, '
                             'cut the same panels.')
        self.edit.setChecked(prefs['edit'])
        layout.addSpacing(8)
        layout.addWidget(self.edit)
        if has_fixes:
            note = QLabel('Panels fixed by hand earlier are used where the book has them.', self)
            note.setEnabled(False)
            layout.addWidget(note)

        buttons = QDialogButtonBox(QDialogButtonBox.StandardButton.Ok | QDialogButtonBox.StandardButton.Cancel, self)
        buttons.button(QDialogButtonBox.StandardButton.Ok).setText('Repack')
        buttons.accepted.connect(self.accept)
        buttons.rejected.connect(self.reject)
        layout.addWidget(buttons)

    def answers(self):
        """{'rtl': bool, 'edit': bool}, also remembered as the next defaults."""
        out = {'rtl': self.manga.isChecked(), 'edit': self.edit.isChecked()}
        prefs['rtl'], prefs['edit'] = out['rtl'], out['edit']
        return out
