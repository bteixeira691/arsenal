# Distribution

Prebuilt packages are produced from tags:

| Platform | Command | Artifact |
| --- | --- | --- |
| macOS arm64/x64, Linux arm64/x64 | `bun install -g arsenal-tui` | npm package + release binary |
| Windows | `winget install --id Bteixeira.Arsenal -e` | portable winget package |
| Windows (before winget acceptance) | `bun install -g arsenal-tui` | npm package + release binary |

The release workflow builds these assets and attaches them to a GitHub release:

```text
arsenal-aarch64-apple-darwin
arsenal-x86_64-apple-darwin
arsenal-aarch64-unknown-linux-gnu
arsenal-x86_64-unknown-linux-gnu
arsenal-x86_64-pc-windows-msvc.exe
<each>.sha256
```

## Release

```sh
git tag v0.5.0
git push origin v0.5.0
```

`.github/workflows/release.yml` builds all targets, writes checksums, and
publishes the GitHub release.

## npm / Bun

The package is `packaging/npm` (name `arsenal-tui`). It has no install scripts:
the first run downloads the matching release binary, verifies its SHA-256,
caches it under the platform cache directory, and executes it.

```sh
cd packaging/npm
npm publish --access public
```

Automatic publishing is opt-in per registry. On a `v*` tag, after the GitHub
release exists:

- `publish-bun` runs when `PUBLISH_BUN=true` and publishes `packaging/npm` with
  Bun to the npm registry, using the `NPM_TOKEN` secret.
- `publish-winget` runs when `PUBLISH_WINGET=true`, renders and validates the
  portable Windows manifest from the released `.exe`, and opens the
  `microsoft/winget-pkgs` PR with the `WINGET_TOKEN` secret.

Enable both once from the repository root:

```sh
gh variable set PUBLISH_BUN --body true
gh variable set PUBLISH_WINGET --body true
gh secret set NPM_TOKEN            # granular npm token with publish rights
gh secret set WINGET_TOKEN         # classic GitHub PAT with repo scope
```

The Bun job uses `NPM_CONFIG_TOKEN`; Bun has no `bun login`. The token must
allow publishing `arsenal-tui` and bypass 2FA for CI. The winget job defaults to
package ID `Bteixeira.Arsenal` and publisher `Bteixeira`; override with the
`WINGET_PACKAGE_ID` and `WINGET_PUBLISHER` repository variables. Both jobs fail
loudly if their switch is on but the corresponding secret is missing. They run
only after the release assets exist, so `bun install -g arsenal-tui` can
download the binary immediately.

To release: bump both `Cargo.toml` and `packaging/npm/package.json`, commit,
then push a `vX.Y.Z` tag. The workflow builds all binaries, publishes the
release, checks the tag against `package.json`, publishes with Bun, and opens
the winget PR.

Manual publishing remains available:

```sh
cd packaging/npm
export NPM_CONFIG_TOKEN=npm_xxx
bun pm pack --dry-run     # inspect files without auth
bun publish               # unscoped package is public by default
```

`bun publish --tag next` handles prereleases. Do not pass `--provenance`: Bun
1.3.x rejects it.

Set `repository.url` in `packaging/npm/package.json` to the real repository
before publishing; the launcher derives `owner/repo` from it. Override at runtime
with `ARSENAL_REPO=owner/repo` if needed. `ARSENAL_BINARY` bypasses downloads for
local testing.

Bun also installs directly from a checkout:

```sh
bun install -g /absolute/path/to/arsenal/packaging/npm
```

## winget

See [winget/README.md](winget/README.md). After a release exists:

```sh
VERSION=0.5.0 REPO=<owner>/arsenal \
  node packaging/winget/render.mjs \
  --sha256-file arsenal-x86_64-pc-windows-msvc.exe.sha256
```

Validate on Windows, then submit the generated folder to
`microsoft/winget-pkgs`. Publisher and package identifier are configurable; keep
them stable across versions.
