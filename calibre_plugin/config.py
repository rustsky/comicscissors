"""Plugin settings: default Kindle, output format, language and extra CLI flags."""

from qt.core import QCheckBox, QComboBox, QFormLayout, QLabel, QLineEdit, QWidget

from calibre_plugins.comic_magic import runner
from calibre_plugins.comic_magic.runner import prefs


class ConfigWidget(QWidget):

    def __init__(self):
        super().__init__()
        form = QFormLayout(self)

        self.device = QComboBox(self)
        self.device.setEditable(True)  # still usable when the device list can't be read
        try:
            for dev_id, desc in runner.list_devices():
                self.device.addItem(f'{dev_id}  -  {desc}', dev_id)
        except Exception as e:
            form.addRow(QLabel(f'Could not run comic-magic: {e}', self))
        i = self.device.findData(prefs['device'])
        if i >= 0:
            self.device.setCurrentIndex(i)
        else:
            self.device.setEditText(prefs['device'])
        form.addRow('Kindle model:', self.device)

        self.format = QComboBox(self)
        self.format.addItem('PDF (works on every Kindle)', 'pdf')
        self.format.addItem('CBZ', 'cbz')
        self.format.setCurrentIndex(max(0, self.format.findData(prefs['format'])))
        form.addRow('Output format:', self.format)

        self.ocr = QCheckBox('Add text layer for dictionary lookup (PDF only, needs Tesseract)', self)
        self.ocr.setChecked(prefs['ocr'])
        form.addRow(self.ocr)

        self.lang = QLineEdit(prefs['lang'], self)
        self.lang.setPlaceholderText('en, fr, de, es, ja, fa or a Tesseract code')
        form.addRow('Comic language:', self.lang)

        self.rtl = QCheckBox('Manga: read panels right to left', self)
        self.rtl.setChecked(prefs['rtl'])
        form.addRow(self.rtl)

        self.rect_crop = QCheckBox('Rectangle panels: crop each panel to its bounding box', self)
        self.rect_crop.setToolTip(
            'Fallback for when exact outlines cut panels badly, e.g. a neighbouring panel cut into an L shape.\n'
            'Slanted panels then keep slivers of their neighbours.'
        )
        self.rect_crop.setChecked(prefs['rect_crop'])
        form.addRow(self.rect_crop)

        self.extra_args = QLineEdit(prefs['extra_args'], self)
        self.extra_args.setPlaceholderText('e.g. -s 1.0 --gamma 1.4 --rotate')
        form.addRow('Extra options:', self.extra_args)

        self.binary = QLineEdit(prefs['binary'], self)
        self.binary.setPlaceholderText('empty: use the bundled comic-magic')
        form.addRow('comic-magic binary:', self.binary)

    def save_settings(self):
        i = self.device.currentIndex()
        text = self.device.currentText()
        # An item's text is "id  -  description"; a typed-in value is the id itself.
        prefs['device'] = self.device.itemData(i) if i >= 0 and self.device.itemText(i) == text else (text.split() or [prefs['device']])[0]
        prefs['format'] = self.format.currentData()
        prefs['ocr'] = self.ocr.isChecked()
        prefs['lang'] = self.lang.text().strip() or 'en'
        prefs['rtl'] = self.rtl.isChecked()
        prefs['rect_crop'] = self.rect_crop.isChecked()
        prefs['extra_args'] = self.extra_args.text().strip()
        prefs['binary'] = self.binary.text().strip()
