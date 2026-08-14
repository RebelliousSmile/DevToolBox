"""XDG Base Directory resolution — shared Linux helper.

Implements the subset of the XDG Base Directory Specification this project
needs: ``XDG_CONFIG_HOME``, ``XDG_DATA_HOME``, ``XDG_CACHE_HOME``, and
``XDG_STATE_HOME``, each resolved from its environment variable override,
falling back to the spec's default (relative to ``$HOME``) when unset or
blank — exactly the override-with-default behavior the spec mandates
(https://specifications.freedesktop.org/basedir-spec/basedir-spec-latest.html).

Used by ``appdata.py`` to give the AppData/LocalAppData first-level
scanners a real Linux equivalent instead of silently returning nothing (no
``LOCALAPPDATA``/``APPDATA`` environment variable exists on Linux):
``XDG_CACHE_HOME`` stands in for ``LOCALAPPDATA`` (both hold machine-local,
non-roamed, often-large caches), ``XDG_DATA_HOME`` stands in for
``APPDATA`` (both hold per-app data meant to persist/roam with the user).

Stdlib only. Never writes to disk.
"""

from __future__ import annotations

import os
from pathlib import Path

# (env var name -> default path relative to $HOME) pairs, per the XDG spec.
_XDG_DEFAULTS: dict[str, str] = {
    "XDG_CONFIG_HOME": ".config",
    "XDG_DATA_HOME": ".local/share",
    "XDG_CACHE_HOME": ".cache",
    "XDG_STATE_HOME": ".local/state",
}


def _resolve(env_var: str, env: dict[str, str] | None) -> Path | None:
    """Resolve one XDG directory: override, then ``$HOME``-relative default.

    ``env`` given (tests): a plain ``dict`` such as
    ``{"XDG_CACHE_HOME": "/tmp/x", "HOME": "/tmp/home"}`` used instead of
    ``os.environ`` — a key absent from ``env`` is treated exactly like an
    unset environment variable. ``env is None`` (production): reads
    straight from ``os.environ``.

    Returns ``None`` only when both the override and ``HOME`` are
    unset/blank (nothing to build a default path from) — this is how
    "resolution entirely fails" degrades without raising, mirrored by every
    caller in ``appdata.py``.
    """
    source = env if env is not None else os.environ
    override = source.get(env_var)
    if override:
        return Path(override)
    home = source.get("HOME")
    if not home:
        return None
    return Path(home) / _XDG_DEFAULTS[env_var]


def config_home(env: dict[str, str] | None = None) -> Path | None:
    """Resolve ``$XDG_CONFIG_HOME``, defaulting to ``$HOME/.config``."""
    return _resolve("XDG_CONFIG_HOME", env)


def data_home(env: dict[str, str] | None = None) -> Path | None:
    """Resolve ``$XDG_DATA_HOME``, defaulting to ``$HOME/.local/share``."""
    return _resolve("XDG_DATA_HOME", env)


def cache_home(env: dict[str, str] | None = None) -> Path | None:
    """Resolve ``$XDG_CACHE_HOME``, defaulting to ``$HOME/.cache``."""
    return _resolve("XDG_CACHE_HOME", env)


def state_home(env: dict[str, str] | None = None) -> Path | None:
    """Resolve ``$XDG_STATE_HOME``, defaulting to ``$HOME/.local/state``."""
    return _resolve("XDG_STATE_HOME", env)
