# Distribution

Prebuilt packages are produced from tags. All platforms use the Bun/npm
launcher:

```sh
bun install -g arsenal-tui
# or
npm install -g arsenal-tui
```

| Platform | Asset |
| --- | --- |
| macOS arm64/x64 | release binary + npm package |
| Linux arm64/x64 (glibc) | release binary + npm package |
| Windows x64 | release binary + npm package |

`release-bun.yml` builds and attaches these assets:

```text
arsenal-aarch64-apple-darwin
arsenal-x86_64-apple-darwin
arsenal-aarch64-unknown-linux-gnu
arsenal-x86_64-unknown-linux-gnu
arsenal-x86_64-pc-windows-msvc.exe
<each>.sha256
```

The Windows executable uses the static MSVC runtime, so it does not require the
Visual C++ Redistributable (`VCRUNTIME140.dll`).

## Release

```sh
git tag v0.6.5
git push origin v0.6.5
```

`.github/workflows/release-bun.yml` validates the tag against `Cargo.toml` and
`packaging/npm/package.json`, runs format/clippy/tests, builds all targets,
writes checksums, publishes the GitHub release, and publishes the Bun/npm
package.

## npm / Bun

The package is `packaging/npm` (`arsenal-tui`). It has no install scripts: the
first run downloads the matching release binary, verifies its SHA-256, caches
it, and executes it.

Automatic publishing is controlled by the repository variable
`PUBLISH_BUN=true` and the `NPM_TOKEN` secret:

```sh
gh variable set PUBLISH_BUN --body true
gh secret set NPM_TOKEN            # granular token with publish + Bypass 2FA
```

The Bun job maps `NPM_TOKEN` to `NPM_CONFIG_TOKEN`. The token must be allowed to
publish `arsenal-tui` and must have **Bypass 2FA** enabled for CI. Bun has no
`bun login`.

Manual publishing:

```sh
cd packaging/npm
export NPM_CONFIG_TOKEN=npm_xxx
bun pm pack --dry-run     # inspect files without auth
bun publish               # unscoped package is public by default
```

`bun publish --tag next` handles prereleases. Do not pass `--provenance`: Bun
1.3.x rejects it.

Set `repository.url` in `packaging/npm/package.json` to the real repository;
the launcher derives `owner/repo` from it. Runtime overrides:

- `ARSENAL_REPO=owner/repo`
- `ARSENAL_RELEASE_BASE_URL=...`
- `ARSENAL_CACHE_DIR=...`
- `ARSENAL_BINARY=/path/to/arsenal`

Bun also installs directly from a checkout:

```sh
bun install -g /absolute/path/to/arsenal/packaging/npm
```
