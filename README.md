# Notes

**knave-notepad** is a plain-text document editor built with
[Knave UI](https://github.com/knave-de/knave-ui). Its package and binary are
`knave-notepad`; its window and desktop launcher are named **Notes**.

The editor follows gedit and Windows Notepad: documents in tabs, a full-window
editing surface, a line-number gutter, and a compact status bar. The library
follows Evince/Foliate: a grid of miniature document pages with titles underneath.
Notes manages storage automatically, so writing does not require choosing a file.

## Run and install

Requires Rust edition 2024, Linux with Wayland or X11, system fonts, and a working
wgpu adapter. Dependencies pin Knave UI and its renderer to the same Git revision;
no sibling checkout is required. Open/Export use the desktop's XDG file chooser
portal through rfd.

```sh
cargo run --locked
make install PREFIX="$HOME/.local"
```

For packaging, `make install DESTDIR=... PREFIX=/usr` installs the binary and
launcher. The launcher uses the system theme's standard `accessories-text-editor`
icon. There is no custom Notes icon yet.

## Editing

- New documents start as **Untitled Document N**. After writing, the first
  nonempty line becomes the title. Imported documents retain their filename.
- Tabs preserve each open document's cursor, selection, undo, and scroll position.
  Click × to close a tab; the document remains in the library. Drag tabs to
  reorder them, or scroll the strip when tabs overflow the window.
- The editor renders visible lines directly through Knave UI display commands.
  It does not use a form text-input widget and has no 16 KiB document restriction.
- Text navigation includes grapheme-aware left/right/delete, word movement,
  Home/End, Ctrl+Home/End, Page Up/Down, and selection with Shift.
- Click/drag selects text; double-click selects a word; triple-click or the gutter
  selects a line. Shift-click extends selection. Both scrollbars can be dragged;
  Shift+wheel scrolls horizontally. Long lines scroll rather than wrap.
- Return splits lines and carries leading indentation when enabled. Tab inserts
  a literal tab or enough spaces to reach the next tab stop. With a selection,
  Tab indents complete lines; Shift+Tab removes one indentation level.
- The status bar reports save state, UTF-8/LF, indentation style/width, line,
  visual column, selection length, and Insert/Overwrite or Vim mode.
- Find and Replace operate across the full document, including text outside the
  viewport. Searches are literal and case sensitive, and wrap at the document end.
- Open imports a UTF-8 text document into the managed library. Export writes a
  copy to a chosen path. CRLF and CR input are normalized to LF.

**Settings** contains font size, Vim motions, absolute/relative line numbers,
spaces versus tabs, tab width (1–16), auto-indent, current-line highlighting, and
visible whitespace. Settings persist across restarts. Relative numbers show the
absolute line number on the cursor's line and distances on other lines.

| Shortcut | Action |
| --- | --- |
| Ctrl+N | New document |
| Ctrl+O | Import a text document |
| Ctrl+W | Close the current tab |
| Ctrl+Tab / Ctrl+Shift+Tab | Next / previous document |
| Ctrl+Page Down / Ctrl+Page Up | Next / previous document |
| Ctrl+S | Save / retry a failed autosave |
| Ctrl+Shift+S | Export a text copy |
| Ctrl+F / Ctrl+H | Find / replace |
| Ctrl+A | Select all in plain editing |
| Ctrl+C / Ctrl+X / Ctrl+V | Plain editor clipboard |
| Ctrl+Z / Ctrl+Shift+Z or Ctrl+Y | Plain editor undo / redo |
| Ctrl++ / Ctrl+− / Ctrl+0 | Increase / decrease / reset font size |
| Insert | Toggle overwrite in plain editing |

## Vim motions

Modalkit owns modal state, motions, operators, registers, repeat, and undo.
Supported core editing includes Normal/Insert/Visual modes, counts, `hjkl`, word
and line motions, `gg`/`G`, operators such as `d`/`c`/`y`, `p`/`P`, `.`, and
`u`/Ctrl+R. Registers are shared across open tabs. App indentation and newline
policies participate in the same modal undo history.

Use `i` to insert and Escape to return to Normal mode. Ctrl+Shift+C/X/V access
the system clipboard. `/` and `?` open the app's literal search; Enter searches,
then `n`/`N` repeat. The command bar supports `:w`, `:q`, `:wq`, `:x`, `:e`,
`:123` to jump to a line, and these `:set` options:

- `nu`/`nonu`, `rnu`/`nornu`
- `et`/`noet` (spaces/tabs)
- `ai`/`noai` (auto-indent)
- `ts=N` or `tabstop=N`

This is Vim-style editing, not a complete Vim implementation. Macro execution,
block-selection painting, regex search, and arbitrary Ex commands are not
implemented. Unsupported modalkit actions report a status message.

## Storage

Respects the XDG directories, with these defaults:

- `~/.local/share/knave-notepad/notebook.json`: text, document IDs/titles,
  edit times, and open tab order.
- `~/.config/knave-notepad/settings.json`: editor preferences.

Edits are saved automatically. Writes use a temporary file, fsync, and atomic
rename; an exclusive lock prevents concurrent instances from overwriting the
same notebook. Save errors leave text in memory, show a retry action, and block
normal window closing until saving succeeds. Invalid existing JSON is reported
at startup and left untouched. Back up `notebook.json` to back up the library.
Startup opens the library with previous tabs available when a document is opened.

## Development and verification

```sh
cargo fmt --all -- --check
cargo test --locked
cargo clippy --all-targets --locked -- -D warnings
cargo build --locked
```

Tests cover document navigation, Unicode graphemes, newlines/indentation,
selection and clipboard hooks, undo/redo, large documents, viewport rendering,
relative numbers, scrolling, tab order/lifecycle, modal history and registers,
commands, persistence, settings, and save-failure recovery.

The app-local host is adapted from knave-app to expose native input and custom
paint hooks; see [source notes](src/host/README.md). Widgets, display commands,
font measurement, GPU rendering, clipboard, and IME hosting use Knave UI's stack.
