"""Calibre plugin: repack comics for an e-reader with the comic-scissors command line tool."""

from calibre.customize import InterfaceActionBase


class ComicScissorsPlugin(InterfaceActionBase):
    name = 'ComicScissors'
    description = 'Comic & manga panel splitter for e-readers: cuts out panels and repacks them so they are readable without zooming'
    author = 'rustysky'
    # Both lines are rewritten by scripts/package.py for each release build.
    version = (0, 1, 0)
    supported_platforms = ['windows', 'osx', 'linux']
    minimum_calibre_version = (6, 0, 0)

    actual_plugin = 'calibre_plugins.comic_scissors.ui:ComicScissorsAction'

    def is_customizable(self):
        return True

    def config_widget(self):
        from calibre_plugins.comic_scissors.config import ConfigWidget
        return ConfigWidget()

    def save_settings(self, config_widget):
        config_widget.save_settings()
