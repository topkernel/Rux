"""Repo-root resolver for scripts/test drivers (move-immune).

Every driver used to compute the repo root by counting dirname
levels; each directory reorganization broke the whole set at once
(observed 2026-11-05: test/ -> scripts/test/ broke 34 drivers + 5
shell scripts). This helper walks UP from the script's own location
until it finds the repo marker (Cargo.toml + kernel/), so any future
move of scripts/test leaves every driver working unchanged.
"""


def repo_root(path) -> str:
    import os
    d = os.path.dirname(os.path.abspath(path))
    while d != os.path.dirname(d):  # stop at filesystem root
        if os.path.isfile(os.path.join(d, "Cargo.toml")) and os.path.isdir(
            os.path.join(d, "kernel")
        ):
            return d
        d = os.path.dirname(d)
    raise RuntimeError("repo root not found above %r (no Cargo.toml+kernel)" % path)
