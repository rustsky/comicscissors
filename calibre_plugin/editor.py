"""The panel editor in a calibre window.

The CLI serves the editor on a local address and prints it as an '@@edit <url>' line. The job
thread hands the address over here (on the GUI thread), and a window shows the page in a web view.
The window closes when the editor's "Cut the book" button is pressed; closing it presses that
button, so the book is cut with the panels as they are. Without a web view (some Linux builds of
calibre), the editor opens in the browser instead.
"""

from qt.core import QDialog, QObject, QUrl, QVBoxLayout, pyqtSignal

try:
    from qt.webengine import QWebEngineView
except ImportError:  # calibre built without QtWebEngine
    QWebEngineView = None

# The editor page sets this title once the book is being cut.
DONE_TITLE = 'Panel Editor: done'


class EditorWindow(QDialog):
    def __init__(self, url, title, parent=None):
        super().__init__(parent)
        self.done_ = False
        self.setWindowTitle(f'Fix panels: {title}')
        layout = QVBoxLayout(self)
        layout.setContentsMargins(0, 0, 0, 0)
        self.view = QWebEngineView(self)
        self.view.titleChanged.connect(self.title_changed)
        layout.addWidget(self.view)
        self.view.load(QUrl(url))
        screen = (parent.screen() if parent is not None else None) or self.screen()
        if screen is not None:
            geo = screen.availableGeometry()
            self.resize(int(geo.width() * 0.9), int(geo.height() * 0.9))

    def title_changed(self, title):
        if title == DONE_TITLE:
            self.done_ = True
            self.accept()

    def reject(self):
        # Closing the window (Esc, the close button) cuts the book with the panels as they are,
        # instead of leaving the job waiting.
        if self.done_:
            return super().reject()
        self.view.page().runJavaScript("document.getElementById('cut').click()")

    def closeEvent(self, event):
        if self.done_:
            return super().closeEvent(event)
        event.ignore()
        self.reject()


class EditorOpener(QObject):
    """Opens editor windows on the GUI thread, asked from job threads: emit `open(url, title)`."""

    open = pyqtSignal(str, str)

    def __init__(self, gui):
        super().__init__(gui)
        self.gui = gui
        self.windows = []
        self.open.connect(self.show)

    def show(self, url, title):
        if QWebEngineView is None:
            from calibre.gui2 import open_url
            open_url(QUrl(url))
            return
        w = EditorWindow(url, title, self.gui)
        self.windows.append(w)
        w.finished.connect(lambda _=None, w=w: self.windows.remove(w))
        w.show()
        w.raise_()
        w.activateWindow()
