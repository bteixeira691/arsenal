#!/usr/bin/env node
// Render winget manifests for a published GitHub release.
//
//   SHA256=<hash> VERSION=0.5.0 REPO=bteixeira691/arsenal node packaging/winget/render.mjs
//
// Then validate on Windows with:
//   winget validate --manifest packaging/winget/manifests/Bteixeira.Arsenal/0.5.0

import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(here, "..", "..");
const npmPackage = JSON.parse(
  readFileSync(path.join(repoRoot, "packaging", "npm", "package.json"), "utf8"),
);

function arg(name, fallback) {
  const index = process.argv.indexOf(`--${name}`);
  return index > -1 && process.argv[index + 1] ? process.argv[index + 1] : fallback;
}

function slug(input) {
  const match = input.match(/github\.com[/:]([^/]+)\/([^/.]+?)(?:\.git)?$/);
  return match ? `${match[1]}/${match[2]}` : input;
}

const version = arg("version", process.env.VERSION || npmPackage.version);
const repo = slug(arg("repo", process.env.REPO || npmPackage.repository.url));
const id = arg("id", process.env.WINGET_ID || "Bteixeira.Arsenal");
const publisher = arg("publisher", process.env.PUBLISHER || "Bteixeira");
const out = path.resolve(
  arg("out", process.env.OUT || path.join(here, "manifests")),
  id,
  version,
);

let sha256 = arg("sha256", process.env.SHA256 || "");
const shaFile = arg("sha256-file", process.env.SHA256_FILE || "");
if (!sha256 && shaFile) {
  sha256 = readFileSync(shaFile, "utf8").trim().split(/\s+/)[0];
}
sha256 = sha256.toLowerCase();
if (!/^[0-9a-f]{64}$/.test(sha256)) {
  console.error(
    "SHA256 is required (64 hex chars). Pass --sha256 or --sha256-file, " +
      "or set SHA256. Download the release .sha256 asset first.",
  );
  process.exit(1);
}

const tag = `v${version}`;
const asset = "arsenal-x86_64-pc-windows-msvc.exe";
const installerUrl = `https://github.com/${repo}/releases/download/${tag}/${asset}`;
const repoUrl = `https://github.com/${repo}`;

PackageIdentifier: ${id}
PackageVersion: ${version}
DefaultLocale: en-US
ManifestType: version
`;

PackageIdentifier: ${id}
PackageVersion: ${version}
Installers:
  - Architecture: x64
    InstallerType: portable
    InstallerUrl: ${installerUrl}
    InstallerSha256: ${sha256}
    Commands:
      - arsenal
ManifestType: installer
`;

PackageIdentifier: ${id}
PackageVersion: ${version}
PackageLocale: en-US
Publisher: ${publisher}
PublisherUrl: https://github.com/${repo.split("/")[0]}
PackageName: Arsenal
PackageUrl: ${repoUrl}
License: MIT
LicenseUrl: ${repoUrl}/blob/main/LICENSE
ShortDescription: Terminal UI for agent skills, agents, commands, and rules
Description: Browse, search, preview, edit, create, and install agent skills, agents, commands, and rules in the terminal.
Moniker: arsenal
Tags:
  - agent-skills
  - claude-code
  - cli
  - ratatui
  - tui
ManifestType: defaultLocale
`;

mkdirSync(out, { recursive: true });
writeFileSync(path.join(out, `${id}.yaml`), versionManifest);
writeFileSync(path.join(out, `${id}.installer.yaml`), installerManifest);
writeFileSync(path.join(out, `${id}.locale.en-US.yaml`), localeManifest);

console.log(`wrote ${out}`);
console.log(`installer: ${installerUrl}`);
console.log(`sha256:    ${sha256}`);
console.log("");
console.log("Next steps:");
console.log(`  winget validate --manifest ${out}`);
console.log(
  "  copy the folder into microsoft/winget-pkgs under manifests/b/<Publisher>/<Package>/<version>/",
);
console.log(`  after merge: winget install --id ${id} -e`);
