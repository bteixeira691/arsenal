# arsenal-tui

npm/Bun launcher for [Arsenal](https://github.com/bteixeira/arsenal), a terminal
UI for agent skills, agents, commands, and rules.

```sh
bun install -g arsenal-tui
arsenal
```

The first run downloads the matching release binary from GitHub, verifies its
SHA-256 checksum, caches it, and executes it. Supported targets: macOS
arm64/x64, Linux arm64/x64 (glibc), Windows x64.

Environment overrides:

- `ARSENAL_BINARY` — use a local binary instead of downloading.
- `ARSENAL_REPO` — change the `owner/repo` used for release downloads.
- `ARSENAL_RELEASE_BASE_URL` — mirror or test a release directory.
- `ARSENAL_CACHE_DIR` — override the cache location.

The CLI accepts the same flags as the Rust binary. See the main repository
README for keymaps and configuration.
