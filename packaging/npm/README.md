# arsenal-tui

npm/Bun launcher for [Arsenal](https://github.com/bteixeira691/arsenal), a terminal
UI for agent skills, agents, commands, and rules.

```sh
bun install -g arsenal-tui
arsenal
```

The first run downloads the matching release binary from GitHub, verifies its
SHA-256 checksum, caches it, and executes it. Supported targets: macOS
arm64/x64, Linux arm64/x64 (glibc), Windows x64.

## Uninstall

Remove the global launcher:

```sh
bun remove -g arsenal-tui
# or
npm uninstall -g arsenal-tui
```

Remove an asset that Arsenal installed or manages:

```sh
arsenal uninstall              # choose from the installed asset list
arsenal uninstall <name-or-path>
arsenal uninstall <name-or-path> --kind skill
arsenal uninstall <name-or-path> --purge
```

By default the asset is moved to the Arsenal trash directory. `--purge` deletes
it permanently. Skills are removed as whole directories; agents, commands,
rules, and snippets are removed as single files.

Environment overrides:

- `ARSENAL_BINARY` — use a local binary instead of downloading.
- `ARSENAL_REPO` — change the `owner/repo` used for release downloads.
- `ARSENAL_RELEASE_BASE_URL` — mirror or test a release directory.
- `ARSENAL_CACHE_DIR` — override the cache location.

The CLI accepts the same flags as the Rust binary. See the main repository
README for keymaps and configuration.
