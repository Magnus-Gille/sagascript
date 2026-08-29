# Configuration files

Sagascript keeps its user-managed configuration in the XDG configuration
directory on every Unix desktop, including macOS:

```text
${XDG_CONFIG_HOME:-$HOME/.config}/sagascript/
├── sagascript-settings.json
├── glossary.txt
└── glossaries/
    ├── default.txt
    └── swedish.txt
```

`XDG_CONFIG_HOME` must be an absolute path. An unset, empty, or relative value
falls back to `$HOME/.config`, as required by the XDG Base Directory
Specification. Windows keeps the same file layout below its normal per-user
configuration base when no XDG path is supplied.

On macOS, `XDG_CONFIG_HOME` must be present in the app process environment.
Finder, Dock, Spotlight, and login-item launches do not normally inherit values
set only in shell startup files. Use the standard `$HOME/.config` location, or
configure the variable for GUI processes as well, to keep GUI and CLI paths
identical.

The JSON file contains application settings and dictation profiles. The plain
text glossary files contain one dictionary entry per line. `glossary.txt` is
global; `glossaries/<profile-id>.txt` is combined only with that profile.

Use the CLI to discover the effective paths rather than duplicating the
resolution rules in scripts:

```console
sagascript config path
sagascript glossary path
sagascript glossary path --profile swedish
```

The GUI and CLI watch and update the same files. Atomic writes preserve
user-managed symlinks, so the files can be checked into a dotfiles repository.

## Overrides and migration

`SAGASCRIPT_SETTINGS_PATH=/absolute/path/to/settings.json` selects one exact
settings file for an isolated CLI session or test. A relative value is resolved
from the process working directory and still disables normal-settings migration.
Its sibling `glossary.txt` and `glossaries/` directory are used for that session.
While this override is active, Sagascript does not inspect or migrate normal user
settings.

On the first launch after upgrading, Sagascript copies the existing macOS
settings from `~/Library/Application Support/ai.gille.sagascript/` into the XDG
directory. The old file remains available for rollback. Any embedded global or
profile dictionaries are written to their new text files and removed from the
JSON copy. Existing Accessibility authorization remains valid because the app's
signed bundle identity does not change.
