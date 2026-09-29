"""Small runtime compatibility helpers for the pinned conversion environment."""

from __future__ import annotations

import sys


def install_lzma_backport() -> None:
    """Make the PyPI lzma backport satisfy Python 3.10's stdlib wrapper.

    The host's pyenv 3.10.13 was built without ``_lzma``.  NeMo imports the
    Hugging Face datasets package, which imports ``lzma`` even when no lzma
    archive is used.  ``backports.lzma`` supplies the same low-level module;
    the stdlib ``lzma.py`` then provides its normal high-level API.
    """

    try:
        import lzma  # noqa: F401
        return
    except ModuleNotFoundError as exc:
        if exc.name != "_lzma":
            raise

    import backports.lzma._lzma as low_level_lzma

    sys.modules["_lzma"] = low_level_lzma


install_lzma_backport()
