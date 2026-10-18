"""Plugin settings: e-reader, output format, language and extra CLI flags."""

from qt.core import QCheckBox, QComboBox, QFormLayout, QHBoxLayout, QLabel, QLineEdit, QSpinBox, QWidget

from calibre_plugins.comic_scissors import runner
from calibre_plugins.comic_scissors.runner import prefs


class ConfigWidget(QWidget):

    def __init__(self):
        super().__init__()
        form = QFormLayout(self)

        self.device = QComboBox(self)
        try:
            brand = None
            for dev_brand, dev_id, name in runner.list_devices():
                if dev_brand != brand:
                    # Brand headings: shown in the list but not selectable.
                    brand = dev_brand
                    self.device.addItem(f'\u2014 {brand} \u2014')
                    self.device.model().item(self.device.count() - 1).setEnabled(False)
                self.device.addItem(f'    {name}', dev_id)
        except Exception as e:
            # Still usable without the device list: type the id (see comic-scissors --list-devices).
            self.device.setEditable(True)
            form.addRow(QLabel(f'Could not run comic-scissors: {e}', self))
        self.device.addItem('Custom screen size', 'custom')
        i = self.device.findData(prefs['device'])
        if i >= 0:
            self.device.setCurrentIndex(i)
        elif self.device.isEditable():
            self.device.setEditText(prefs['device'])
        form.addRow('E-reader:', self.device)

        # Width x height @ ppi, for e-readers that aren't in the list.
        self.custom = QWidget(self)
        row = QHBoxLayout(self.custom)
        row.setContentsMargins(0, 0, 0, 0)
        self.screen_w, self.screen_h, self.screen_ppi = (QSpinBox(self.custom) for _ in range(3))
        for box, lo, hi, key, suffix in (
            (self.screen_w, 300, 4000, 'custom_width', ' px'),
            (self.screen_h, 300, 4000, 'custom_height', ' px'),
            (self.screen_ppi, 100, 600, 'custom_ppi', ' ppi'),
        ):
            box.setRange(lo, hi)
            box.setValue(prefs[key])
            box.setSuffix(suffix)
        self.screen_color = QCheckBox('color', self.custom)
        self.screen_color.setChecked(prefs['custom_color'])
        row.addWidget(self.screen_w)
        row.addWidget(QLabel('\u00d7', self.custom))
        row.addWidget(self.screen_h)
        row.addWidget(QLabel('at', self.custom))
        row.addWidget(self.screen_ppi)
        row.addWidget(self.screen_color)
        row.addStretch()
        form.addRow('Screen:', self.custom)
        self.device.currentIndexChanged.connect(self.device_changed)
        self.device_changed()

        self.format = QComboBox(self)
        self.format.addItem('PDF (works on every e-reader)', 'pdf')
        self.format.addItem('CBZ', 'cbz')
        self.format.setCurrentIndex(max(0, self.format.findData(prefs['format'])))
        form.addRow('Output format:', self.format)

        self.quality = QSpinBox(self)
        self.quality.setRange(30, 100)
        self.quality.setValue(prefs['quality'])
        self.quality.setToolTip('JPEG quality of the pages. Lower makes much smaller files;\n'
                                'e-ink shows little difference down to about 70.')
        form.addRow('Image quality:', self.quality)

        self.ocr = QCheckBox('Add text layer for dictionary lookup (PDF only, needs Tesseract)', self)
        self.ocr.setChecked(prefs['ocr'])
        form.addRow(self.ocr)

        self.lang = QLineEdit(prefs['lang'], self)
        self.lang.setPlaceholderText('en, fr, de, es, ja, fa or a Tesseract code')
        form.addRow('Comic language:', self.lang)

        self.rtl = QCheckBox('Manga: read panels right to left', self)
        self.rtl.setChecked(prefs['rtl'])
        form.addRow(self.rtl)

        self.overview = QCheckBox('Show each original page whole before its panels', self)
        self.overview.setChecked(prefs['overview'])
        form.addRow(self.overview)

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
        self.binary.setPlaceholderText('empty: use the bundled comic-scissors')
        form.addRow('comic-scissors binary:', self.binary)

    def device_changed(self):
        self.custom.setEnabled(self.device.currentData() == 'custom')

    def save_settings(self):
        i = self.device.currentIndex()
        if self.device.isEditable() and self.device.itemText(i) != self.device.currentText():
            prefs['device'] = (self.device.currentText().split() or [prefs['device']])[0]
        elif self.device.currentData():
            prefs['device'] = self.device.currentData()
        prefs['custom_width'] = self.screen_w.value()
        prefs['custom_height'] = self.screen_h.value()
        prefs['custom_ppi'] = self.screen_ppi.value()
        prefs['custom_color'] = self.screen_color.isChecked()
        prefs['format'] = self.format.currentData()
        prefs['quality'] = self.quality.value()
        prefs['ocr'] = self.ocr.isChecked()
        prefs['lang'] = self.lang.text().strip() or 'en'
        prefs['rtl'] = self.rtl.isChecked()
        prefs['overview'] = self.overview.isChecked()
        prefs['rect_crop'] = self.rect_crop.isChecked()
        prefs['extra_args'] = self.extra_args.text().strip()
        prefs['binary'] = self.binary.text().strip()
