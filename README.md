# Notes

**knave-notepad** is a plain-text document editor built with
[Knave UI](https://github.com/knave-de/knave-ui). Its package and binary are
`knave-notepad`; its window and desktop launcher are named **Notes**.

Notes combines an autosaving library with a tabbed filesystem editor. The
permanent **Library** tab contains managed notes and files you add explicitly.
Each editor window keeps its own tabs and caret, selection, and scroll positions.
Windows displaying the same document share its text and edit history.

## Run and install

Requires Rust edition 2024, Linux with Wayland or X11, a working wgpu adapter,
and the companion `knave-ui` checkout at `../knave-ui`. File pickers use the
desktop's XDG portal.

```sh
cargo run --locked
cargo run --locked -- ./README.md /absolute/path/config.toml
scripts/install.sh --user
```

Opening the launcher shows the Library. File arguments open directly in an
editor. When Notes is running, a CLI or file-manager invocation sends its paths
to the primary process and opens a new window. Relative paths resolve in the
invoking shell. A new path is created on its first successful Save.

Use `scripts/install.sh --system` to state the system-wide choice explicitly.
System installation needs write access to `/usr/local`; set `PREFIX` to choose
another prefix. The installer builds the release binary and installs the desktop
entry plus the scalable Notes icon. User installation uses `XDG_DATA_HOME` and
`XDG_BIN_HOME` when set, otherwise `~/.local/share` and `~/.local/bin`.

## Editing

- **New note** opens an empty managed draft. The first nonempty line becomes its
  title unless you rename it. Notes autosave to the Library.
- **Open File** opens a filesystem document. Ctrl+S saves its edits. The bookmark
  adds or removes the file's path in the Library. Close ends the current tab's
  view. Drag tabs to reorder them, or scroll the strip when tabs overflow.
- **Import as Note** creates an independent managed copy. **Export as File**
  writes an independent copy of a managed note.
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
- The status bar reports save state, encoding, line endings, indentation,
  position, selection length, editing mode, and the current external file path.
- Find and Replace operate across the full document, including text outside the
  viewport. Searches are literal and case sensitive, and wrap at the document end.
- Direct editing preserves UTF-8 BOM, uniform LF/CRLF/CR line endings, and the
  presence of a trailing newline. Import normalizes line endings to LF.
- Save checks current bytes and file identity before replacing the target.
  Conflicts offer Reload from Disk, Overwrite, Save As, and Cancel. Save follows
  symlinks to their original target. Hard links and attributes the writer cannot
  preserve require saving to a new path.

**Settings** contains font size, Vim motions, absolute/relative line numbers,
spaces versus tabs, tab width (1–16), auto-indent, current-line highlighting, and
visible whitespace. Settings persist across restarts. Relative numbers show the
absolute line number on the cursor's line and distances on other lines.

| Shortcut | Action |
| --- | --- |
| Ctrl+T / Ctrl+N | New managed note |
| Ctrl+O | Open a filesystem file |
| Ctrl+W | Close the current tab |
| Ctrl+Tab / Ctrl+Shift+Tab | Next / previous document |
| Ctrl+Page Down / Ctrl+Page Up | Next / previous document |
| Ctrl+S | Save / retry a failed autosave |
| Ctrl+Shift+S | Export a managed note or Save As to a new path |
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
then `n`/`N` repeat. The command bar supports `:w`, `:q`, `:wq`, `:x`, `:e <path>`,
`:123` to jump to a line, and these `:set` options:

- `nu`/`nonu`, `rnu`/`nornu`
- `et`/`noet` (spaces/tabs)
- `ai`/`noai` (auto-indent)
- `ts=N` or `tabstop=N`

This is Vim-style editing, not a complete Vim implementation. Macro execution,
block-selection painting, regex search, and arbitrary Ex commands are not
implemented. Unsupported modalkit actions report a status message.

## Storage

The default locations follow the XDG base directories.

| Location | Contents |
| --- | --- |
| `~/.local/share/knave-notepad/notes/<uuid>.json` | One authoritative record per managed note |
| `~/.local/share/knave-notepad/library.json` | Note previews and metadata, external file references, and managed tab order |
| `~/.config/knave-notepad/settings.json` | Editor preferences |
| `~/.local/state/knave-notepad/recovery/<uuid>.json` | Checkpointed unsaved external edits |
| `~/.local/state/knave-notepad/session.json` | Clean shutdown marker |

Managed records can rebuild the note index. The first launch with a legacy
notebook migrates its notes and retains the original as
`notebook.json.migrated`. Back up both the `notes` directory and `library.json`
to retain the Library. Back up referenced files at their original paths.

External edits are checkpointed separately from the original file. After a
crash, Notes offers the available checkpoints for inspection. Saving recovered
content uses the same conflict checks as any other filesystem save.

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
commands, per-note storage, shared buffers, safe writes, conflict handling, and
crash recovery.

The app-local host is adapted from knave-app to expose native input and custom
paint hooks; see [source notes](src/host/README.md). Widgets, display commands,
font measurement, GPU rendering, clipboard, and IME hosting use Knave UI's stack.
