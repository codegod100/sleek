"""Backport pathlib's Python 3.12 walk_up argument for Buck's C shim."""

import os
import pathlib


try:
    pathlib.PurePath(".").relative_to(".", walk_up=True)
except TypeError:
    _relative_to = pathlib.PurePath.relative_to

    def relative_to(self, *other, walk_up=False):
        if not walk_up:
            return _relative_to(self, *other)
        target = self.__class__(*other)
        return self.__class__(os.path.relpath(str(self), str(target)))

    pathlib.PurePath.relative_to = relative_to
