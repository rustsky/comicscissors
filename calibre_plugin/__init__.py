"""Calibre plugin: repack comics for a Kindle with the comic-magic command line tool."""

from calibre.customize import InterfaceActionBase


class ComicMagicPlugin(InterfaceActionBase):
    name = 'Comic Magic'
    description = 'Split comic pages into panels and repack them so they are readable on a Kindle without zooming'
    author = 'rustysky'
    # Both lines are rewritten by scripts/package.py for each release build.
    version = (0, 1, 0)
    supported_platforms = ['windows', 'osx', 'linux']
    minimum_calibre_version = (6, 0, 0)

    actual_plugin = 'calibre_plugins.comic_magic.ui:ComicMagicAction'

    def is_customizable(self):
        return True

    def config_widget(self):
        from calibre_plugins.comic_magic.config import ConfigWidget
        return ConfigWidget()

    def save_settings(self, config_widget):
        config_widget.save_settings()
