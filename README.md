# Arsenal

A terminal UI for the Markdown assets that power coding agents: **skills**,
**agents**, **commands**, and **rules**. Browse, fuzzy-search, preview, create,
edit, delete — and install a skill straight from a GitHub link.

Arsenal is local-first: everything is plain Markdown on disk. Apart from an
explicit `arsenal install` (git clone), it never talks to a network service,
never rewrites files behind your back, and never takes ownership of your
content.

```text
 ARSENAL · agents  ~/.config/arsenal/config.toml                       2/7 assets
╭ Assets 2 ──────────────────────────────╮╭ Preview · security-auditor ──────────────╮
│ A reviewer  Reviews diffs for …        ││security-auditor                          │
│ A security-auditor  Audits repositor…  ││A agent · ~/.claude/agents                │
│                                        ││~/.claude/agents/security-auditor.md      │
│                                        ││2d ago · 1.1 KB                           │
│                                        ││                                          │
│                                        ││Audit the repository and report …         │
╰────────────────────────────────────────╯╰──────────────────────────────────────────╯
 NORMAL   / search  a new  e edit  d delete  ? help
```

## Install

Requires Rust 1.88+.

```sh
cargo install --path .
arsenal --version
```

For development, `cargo run -- --help`.

## Quick start

```sh
arsenal                                  # use the configured sources
arsenal --root skill=./skills --root agent=./agents
arsenal --query deploy                   # start with a search filter
arsenal list --json | jq                 # scriptable, no TUI
arsenal install https://github.com/anthropics/skills/tree/main/skills/pdf
```

On first launch Arsenal writes a commented config to
`~/.config/arsenal/config.toml` and scans the conventional locations below.

## Asset conventions

| Kind       | Layout                          | Frontmatter                     |
| ---------- | ------------------------------- | ------------------------------- |
| `skill`    | `<root>/<name>/SKILL.md`        | `name`, `description`, `tags`   |
| `agent`    | `<root>/<name>.md`              | `name`, `description`, `tools`  |
| `command`  | `<root>/<name>.md`              | `description`                   |
| `rule`     | `<root>/<name>.md`              | `description`, `globs`          |
| `snippet`  | `<root>/<name>.md`              | `name`, `description`           |

- Skills are directories containing `SKILL.md`; a flat `*.md` directly inside a
  skill root is also accepted.
- Other kinds are `*.md` files scanned recursively up to `behavior.scan_depth`.
- Frontmatter is YAML between `---` lines. If YAML parsing fails, a lenient
  `key: value` parser keeps the file usable. `tags` (or `keywords`) accepts a
  comma list, an inline list, or a block list.
- Displayed name: frontmatter `name`, then the directory name (skills) or file
  stem. Displayed description: frontmatter `description`, then the first prose
  line of the body.

## Default sources

For each of skills, agents, commands, and rules:

```text
~/.claude/<kind>s
~/.omp/agent/<kind>s
~/.agent/<kind>s
~/.agents/<kind>s
./.claude/<kind>s
./.omp/<kind>s
./.agent/<kind>s
./.agents/<kind>s
```

Snippets default to `~/.config/arsenal/snippets` and `./.arsenal/snippets`.
Missing directories are ignored. Duplicate paths are scanned once. `--root`
replaces the configured source list entirely, which is handy for a single
project.

## Keymaps

Vim-style sequences are supported (`g g`, not just single keys). Every default
below is remappable; see [Configuration](#configuration).

| Key         | Action                                    |
| ----------- | ----------------------------------------- |
| `j` / `k`   | Move down / up                            |
| `g g` / `G` | Jump to top / bottom                      |
| `ctrl-d` / `ctrl-u` | Half page down / up                |
| `ctrl-f` / `ctrl-b` | Page down / up                     |
| `J` / `K`   | Scroll preview down / up                  |
| `/`         | Incremental fuzzy search (`enter` keeps, `esc` cancels) |
| `n` / `N`   | Next / previous result (wraps)            |
| `0`–`5`     | All kinds, skills, agents, commands, rules, snippets |
| `s`         | Cycle sort: kind, name, modified, path    |
| `t`         | Toggle the preview pane                   |
| `T`         | Cycle theme: dark, light, mono            |
| `a`         | Create an asset                           |
| `i`         | Install a skill/asset from a GitHub URL   |
| `e`         | Edit in `$EDITOR` (TUI suspends)          |
| `d`         | Delete (confirmation, moves to trash)     |
| `y`         | Copy the path with OSC 52                 |
| `o`         | Reveal in the file manager                |
| `R`         | Rescan sources                            |
| `g c`       | Edit the configuration                    |
| `:`         | Command palette                           |
| `?`         | Keymap and command help                   |
| `q`         | Quit                                      |

`ctrl-c` always quits. `esc` clears the pending key sequence or the active
search. Modals capture keys, so `a`, `d`, and friends are literal text there.

### Search

Terms are ANDed and fuzzy-matched against name, kind, tags, and description.
A term prefixed with `#` filters tags by substring:

```text
/railway
/#ops deploy
```

Results are ranked by match score; with an empty query the list follows the
current sort mode. The header shows `visible/total assets` and the active kind
filter.

## Installing from GitHub

`i` (or `:install <url>`) opens the install form. Paste a GitHub repository or
tree URL, choose the kind and destination source, and press `enter`. The clone
runs in the foreground with git's progress visible; when it finishes the TUI
reloads, selects the new asset, and reports where it landed.

```sh
# a skill inside a monorepo (tree URL)
arsenal install https://github.com/anthropics/skills/tree/main/skills/pdf

# repository root that is itself a skill
arsenal install https://github.com/owner/my-skill

# blob URL for a single SKILL.md
arsenal install https://github.com/owner/repo/blob/main/skills/pdf/SKILL.md

# other kinds, explicit destination, renamed install
arsenal install https://github.com/owner/agents --kind agent --dest ~/.claude/agents
arsenal install owner/repo --subdir skills/deploy --name deploy

# replace an existing install (old copy moves to trash)
arsenal install <url> --force
```

Behavior:

- Shallow `git clone --depth 1` into a temporary directory; only the selected
  asset is copied, and the clone is removed afterwards. `--ref` accepts a
  branch, tag, or commit (falls back to a full clone for commit SHAs).
- GitHub `tree/<ref>/<path>` and `blob/<ref>/<path>` URLs carry their own ref and
  subdirectory. `owner/repo` shorthand and any git/ssh/file URL also work.
- Skills are copied as whole directories, including `scripts/`, `references/`,
  and other bundled files. The install name comes from `--name`, then the
  skill's frontmatter `name`, then the directory or repository name.
- If a repository contains several skills and none was selected, the error lists
  the candidates; rerun with the tree URL or `--subdir`.
- Agents, commands, rules, and snippets install a single `*.md` file. The
  resolver prefers `--name.md`, then `<repo>.md`, then the only non-README
  Markdown file in the selected directory.
- The destination defaults to the first configured source for the kind. With the
  stock configuration that is `~/.claude/skills` for skills and
  `~/.claude/agents` for agents, which Claude Code discovers immediately; omp's
  Claude skill provider also reads `~/.claude/skills`. For omp-native agents,
  commands, rules, or skills, install into `~/.omp/agent/skills`,
  `~/.omp/agent/agents`, `~/.omp/agent/commands`, or `~/.omp/agent/rules` with
  `--dest` (or the TUI source field). Add any other root as a `[[sources]]`
  entry to make it selectable.
- Nothing is executed during install: it is a clone plus a file copy.

## Creating assets

`a` (or `:new agent`) opens a form:

```text
╭ New skill ─────────────────────────────────────────────╮
│ kind        skill                                      │
│ name        incident-runbook                           │
│ description Step-by-step incident response for on-call │
│ source      ~/.claude/skills                           │
│ enter creates the file; tab cycles fields               │
╰────────────────────────────────────────────────────────╯
```

- `tab` / `shift-tab` move between fields; `←`/`→` change kind and source.
- `enter` validates and writes the file, then selects it.
- Names are slugified (`Team Deploy` → `team-deploy`); existing files are never
  overwritten.
- Each kind gets a frontmatter-complete scaffold with a purpose-specific body.

## Command palette

| Command               | Effect                                       |
| --------------------- | -------------------------------------------- |
| `:new <kind>`         | Open the create form (`skill` is the default)|
| `:install <url>`      | Clone a skill/asset from GitHub              |
| `:filter <kind\|all>` | Restrict the list                            |
| `:sort <mode>`        | `kind`, `name`, `modified`, `path`           |
| `:theme <preset>`     | `dark`, `light`, `mono`                      |
| `:edit`, `:delete`    | Apply to the selected asset                  |
| `:yank`, `:reveal`    | Copy path, reveal in file manager            |
| `:rescan`, `:w`       | Rescan all sources                           |
| `:config`             | Edit the config, then reload it              |
| `:warnings`           | Show the first configuration/scan warning    |
| `:help`, `:q`         | Help, quit                                   |

## CLI

```text
arsenal [--config PATH] [--root KIND=PATH]... [--query QUERY]
arsenal list [--json] [--kind KIND] [--sort MODE]
arsenal install <URL|owner/repo> [--kind KIND] [--dest PATH] [--name NAME]
                [--ref REF] [--subdir PATH] [--force]
arsenal config path
arsenal config init [--force]
arsenal config validate
```

`list` prints `kind  name  path` columns; `--json` emits one object per asset
with `kind`, `name`, `description`, `tags`, `path`, `source`, and `modified`
(Unix seconds). Scan warnings go to stderr.

`config validate` parses the file, reports invalid colors/keys/presets, and exits
non-zero only when the file cannot be read or parsed.

## Configuration

Resolution order:

1. `--config PATH`
2. `$ARSENAL_CONFIG`
3. `$XDG_CONFIG_HOME/arsenal/config.toml`
4. `~/.config/arsenal/config.toml`

The file is created on first use. `arsenal config init --force` rewrites it.

```toml
[editor]
# Empty: $VISUAL, then $EDITOR, then vi.
command = ""
# {file} and {line} are substituted; the path is appended if {file} is absent.
args = ["+{line}", "{file}"]

[layout]
list_width = 38      # percent of the main area
show_preview = true

[behavior]
scan_depth = 4       # recursion for non-skill sources (1–12)
confirm_delete = true
trash = true         # move to the trash directory instead of unlinking

[theme]
preset = "dark"      # dark | light | mono
# All optional; names or #rrggbb.
# accent = "cyan"
# border = "darkgray"
# muted  = "gray"
# success = "green"
# warning = "yellow"
# error   = "red"
# skill   = "cyan"
# agent   = "magenta"
# command = "green"
# rule    = "yellow"
# snippet = "blue"

[keys]
# act = "sequence"; space separates chords; "" disables a binding.
quit = "q"
search = "/"
top = "g g"
half_page_down = "ctrl-d"
# ...

[[sources]]
kind = "skill"
path = "~/.claude/skills"
# label = "personal"
# depth = 3

[[sources]]
kind = "agent"
path = "./agents"
```

Paths expand a leading `~` and `$VAR` / `${VAR}`. Unknown variables are left
visible instead of silently collapsing.

### Editor integration

The TUI restores the terminal, runs the editor to completion, then re-initializes
and rescans. Defaults suit vim/neovim (`+{line} {file}`).

```toml
[editor]
command = "code --wait"
args = ["--goto", "{file}:{line}"]

# or a GUI editor that blocks
# command = "zed --wait"
```

Without `[editor].command`, `$VISUAL` / `$EDITOR` are used, splitting on
whitespace. Shell expansions are not evaluated; put fixed arguments in `args`.

### Themes

`dark`, `light`, and `mono` presets. `mono` uses reverse video and no colors, for
minimal terminals. `T` cycles presets for the session; edit `theme.preset` to
make it permanent. All color fields accept terminal names (`cyan`, `lightred`,
`darkgray`, …) or `#rrggbb`.

### Key remapping

Bindings are sequences of chords. Modifiers use `ctrl-`, `alt-`, `shift-`
(vim style) or `+` (`ctrl+shift+k`). Named keys: `enter`, `esc`, `tab`,
`backtab`, `space`, `backspace`, `delete`, `insert`, `home`, `end`, `pageup`,
`pagedown`, `left`, `right`, `up`, `down`, `f1`…`f24`.

Unknown actions, unparsable sequences, and duplicate chords are reported as
warnings on startup (press `?`), and the built-in binding is kept.

## Deletion and the trash

By default `d` asks for confirmation and moves the file to:

```text
$XDG_DATA_HOME/arsenal/trash/<timestamp>/<filename>
# or ~/.local/share/arsenal/trash/...
```

Nothing is removed permanently. Set `behavior.trash = false` to unlink, or
`confirm_delete = false` to skip the dialog. Skills lose only `SKILL.md`; the
containing directory is left in place.

## Clipboard

`y` sends an OSC 52 escape sequence with the asset path. Most modern terminals
support it; tmux requires `set -g set-clipboard on` and a terminal that allows
it. If your terminal does not support OSC 52, nothing is copied and the
statusline still reports that the sequence was sent.

## Design notes

- Markdown and `SKILL.md` layouts are the source of truth; there is no database.
- Installs clone to a temporary directory and copy only the chosen asset; the
  clone is always deleted. No install scripts run.
- No file watcher: `R` rescans explicitly, and every editor round-trip rescans.
- No mouse capture, so the terminal's native selection and copy keep working.
- Previews are capped at 4,000 body lines; very large files stay responsive.
- Scan warnings are bounded and never abort a session.

## Development

```sh
cargo fmt
cargo clippy --all-targets -- -D warnings
cargo test
cargo run -- --config ./tmp/config.toml --root skill=./tmp/skills
```

Layout:

```text
src/main.rs    CLI, terminal lifecycle, external effects, OSC 52
src/app.rs     state machine, modals, commands, selection/search wiring
src/asset.rs   scanning, frontmatter, scaffolding, trash
src/config.rs  config model, path resolution, themes
src/keys.rs    chords, sequences, actions, keymap resolution
src/search.rs  fuzzy scoring and ordering
src/ui.rs      rendering
src/editor.rs  $EDITOR and file-manager integration
src/install.rs git cloning, URL parsing, asset resolution
```

Unit tests cover frontmatter fallbacks, scanning, scaffolding, trash, fuzzy
filtering, sorting, key sequences, UTF-8 text editing, form validation, delete
confirmation, GitHub URL parsing, and install/force/ambiguity cases.

## License

MIT. See [LICENSE](LICENSE).
