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
git tag v0.4.1
git push origin v0.4.1
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

Automatic publishing is opt-in: set the repository variable `PUBLISH_NPM` to
`true` and the repository secret `NPM_TOKEN` to an npm token with publish
rights. The release workflow's `publish-npm` job then publishes when the tag
matches `package.json`. Without both, publish manually:

```sh
cd packaging/npm
npm publish --access public
```

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
VERSION=0.4.1 REPO=<owner>/arsenal \
  node packaging/winget/render.mjs \
  --sha256-file arsenal-x86_64-pc-windows-msvc.exe.sha256
```

Validate on Windows, then submit the generated folder to
`microsoft/winget-pkgs`. Publisher and package identifier are configurable; keep
them stable across versions.
