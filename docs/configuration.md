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

## Extended function-key shortcuts

Bare F13–F24 shortcuts are supported on macOS and Windows. Ordinary keys and
F1–F12 still require a modifier. Linux keeps accepting previously valid
modified F13–F24 configurations, but bare extended function keys remain
unsupported by the current backend.

On macOS, bare F13–F24 use an AppKit event monitor and therefore require the
installed Sagascript app to have Accessibility permission. This is an explicit
product/privacy tradeoff: macOS delivers every system KeyDown/KeyUp event to
the monitor callback, but Sagascript immediately discards any event that is not
one unmodified F13–F24 scalar. Non-matching events are not logged, stored, or
sent anywhere. The monitor exists only for the lifetime of the app process.
Users who do not want this access can continue using a modified shortcut.

The native path covers the whole range rather than only F21–F24 because a
runtime probe of the locked `global-hotkey` 0.7.0 Carbon path on macOS 26.6.2
(build 25G83) found that bare F13–F20 all reached `RegisterEventHotKey` but were
rejected there; F21–F24 failed earlier as unknown scancodes. The source mapping
for F13–F20 therefore did not provide a working permission-free registration
path. This probe was run on 2026-09-04 before adopting the AppKit monitor.

## The ISO section key (Command+§)

Apple ISO keyboards (Swedish, Norwegian, Danish, Finnish, UK and others) have an
extra key left of `1` that is labelled `§` on most of them.
It is a physical key of its own (macOS virtual key `0x0A`, DOM code `IntlBackslash`), separate from the `` ` `` key.
Sagascript names it `IntlBackslash`, so `Command+§` is written `Command+IntlBackslash`:

```bash
sagascript config set hotkey 'Command+IntlBackslash'
```

On macOS and Windows, pressing the key while recording a shortcut in Settings captures it automatically, and macOS shows it as `§`.
`IntlBackslash` is not supported on Linux: the current backend can register a different physical key on some layouts. Choose another shortcut on Linux.
Like every other modified shortcut it is registered through the platform hotkey API and needs no Accessibility permission.
The crates.io release of `global-hotkey` cannot register this key yet, so `src-tauri/Cargo.toml` pins the upstream commit that added it (tauri-apps/global-hotkey#216).

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
