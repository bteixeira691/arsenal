# Windows: winget

`winget install --id Bteixeira.Arsenal -e` works after the manifests are merged
into [microsoft/winget-pkgs](https://github.com/microsoft/winget-pkgs). This
directory generates and documents that submission.

## 1. Publish a GitHub release

Push a tag (`v0.5.0`). `.github/workflows/release.yml` builds
`arsenal-x86_64-pc-windows-msvc.exe` and its `.sha256` and attaches both to the
release.

## 2. Generate the manifests

Download the checksum from the release, then render:

```sh
VERSION=0.5.0 \
REPO=<owner>/arsenal \
WINGET_ID=Bteixeira.Arsenal \
PUBLISHER=Bteixeira \
node packaging/winget/render.mjs \
  --sha256-file arsenal-x86_64-pc-windows-msvc.exe.sha256
```

`--sha256 <hash>`, `--repo`, `--id`, `--publisher`, and `--out` are also
available. Manifests are generated with schema 1.10.0, the version used by
current winget-pkgs manifests. Files are written to
`packaging/winget/manifests/<id>/<version>/`.

## 3. Validate on Windows

```powershell
winget validate --manifest packaging\winget\manifests\Bteixeira.Arsenal\0.5.0
winget install --manifest packaging\winget\manifests\Bteixeira.Arsenal\0.5.0
```

Local `--manifest` installs work before submission; `--id` installs require the
package to be accepted into the winget repository.

## 4. Submit to winget-pkgs

1. Fork `microsoft/winget-pkgs`.
2. Copy the version folder to
   `manifests/b/<Publisher>/<Package>/<version>/`
   (for this example: `manifests/b/Bteixeira/Arsenal/0.5.0/`).
3. Open a PR. Once merged:

```powershell
winget install --id Bteixeira.Arsenal -e
```

Publisher and package identifier are yours to choose; keep them consistent in
the render command, the path under `manifests/`, and future releases.

## Manual GitHub Actions flow

The `winget` workflow is manual and never runs on a tag:

1. Open **Actions → winget → Run workflow**.
2. Enter the release version, package ID, and publisher.
3. Run with `submit = false`. It downloads the Windows release asset, computes
   the SHA-256, renders 1.28 manifests, runs `winget validate`, attempts a local
   install/uninstall, and uploads the manifest set as an artifact.
4. Add a `WINGET_TOKEN` repository secret using a classic GitHub PAT with `repo`
   scope.
5. Run again with `submit = true`. It installs `wingetcreate` and opens the
   winget-pkgs PR from the rendered manifests.
6. Complete the Microsoft CLA personally and follow validation labels.

The local install test is intentionally non-blocking: GitHub runners sometimes
block `LocalManifestFiles`; the winget-pkgs validation pipeline performs the
authoritative install test.
