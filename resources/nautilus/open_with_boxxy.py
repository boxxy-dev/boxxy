# Adapted from Ghostty / WezTerm Nautilus extension for Boxxy Terminal (Nautilus 50+ / 4.1)

import os
import shutil
from pathlib import Path
import gi

gi.require_version("Nautilus", "4.1")
from gi.repository import Gio, GObject, Nautilus

FLATPAK_APP_ID = "dev.boxxy.BoxxyTerminal"


def resolve_boxxy_command() -> list[str]:
    which_bin = shutil.which("boxxy-terminal")
    if which_bin:
        return [which_bin]

    home = Path.home()
    for candidate in (
        home / ".local" / "bin" / "boxxy-terminal",
        home / ".local" / "boxxy-terminal" / "bin" / "boxxy-terminal",
    ):
        if candidate.is_file() and os.access(candidate, os.X_OK):
            return [str(candidate)]

    if shutil.which("flatpak"):
        return ["flatpak", "run", FLATPAK_APP_ID]

    return ["boxxy-terminal"]


def open_in_boxxy_activated(_menu, paths):
    base_cmd = resolve_boxxy_command()
    for path in paths:
        # NOTE: Change "--new-window" to "--tab" if you prefer opening folders
        # as tabs inside the existing Boxxy Terminal window.
        cmd = [*base_cmd, "--new-window", path]
        Gio.Subprocess.new(cmd, Gio.SubprocessFlags.NONE)


# Optional handler if you uncomment the "Open in New Boxxy Tab" menu item in `get_items_for_files`:
# def open_in_boxxy_tab_activated(_menu, paths):
#     base_cmd = resolve_boxxy_command()
#     for path in paths:
#         cmd = [*base_cmd, "--tab", path]
#         Gio.Subprocess.new(cmd, Gio.SubprocessFlags.NONE)


def get_paths_to_open(files):
    paths = []
    for file in files:
        if file is None:
            continue
        location = (
            file.get_location() if file.is_directory() else file.get_parent_location()
        )
        if location is None:
            continue
        path = location.get_path()
        if path and path not in paths:
            paths.append(path)
    if 10 < len(paths):
        # Let's not open anything if the user selected a lot of directories,
        # to avoid accidentally spamming their desktop with dozens of
        # new windows or tabs. Ten is a totally arbitrary limit :)
        return []
    else:
        return paths


def get_items_for_files(name, files):
    paths = get_paths_to_open(files)
    if paths:
        item = Nautilus.MenuItem(
            name=name,
            label="Open With Boxxy",
        )
        item.connect("activate", open_in_boxxy_activated, paths)

        # Uncomment below to also add an "Open in New Boxxy Tab" menu item:
        # tab_item = Nautilus.MenuItem(
        #     name=f"{name}_tab",
        #     label="Open in New Boxxy Tab",
        # )
        # tab_item.connect("activate", open_in_boxxy_tab_activated, paths)
        # return [item, tab_item]

        return [item]
    else:
        return []


class BoxxyMenuProvider(GObject.GObject, Nautilus.MenuProvider):
    def get_file_items(self, files):
        return get_items_for_files("BoxxyNautilus::open_in_boxxy", files)

    def get_background_items(self, file):
        return get_items_for_files("BoxxyNautilus::open_folder_in_boxxy", [file])
