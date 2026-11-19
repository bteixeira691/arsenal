#!/usr/bin/env node
"use strict";

// Installs the matching Arsenal release binary on first use, verifies its
// SHA-256, caches it, and executes it. Works under Node >= 18 and Bun.

const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const crypto = require("node:crypto");
const { spawnSync } = require("node:child_process");

const pkg = require("../package.json");
const VERSION = pkg.version;

function platformTarget() {
  const { platform, arch } = process;
  if (platform === "darwin" && arch === "arm64") return "aarch64-apple-darwin";
  if (platform === "darwin" && arch === "x64") return "x86_64-apple-darwin";
  if (platform === "linux" && arch === "arm64") return "aarch64-unknown-linux-gnu";
  if (platform === "linux" && arch === "x64") return "x86_64-unknown-linux-gnu";
  if (platform === "win32" && arch === "x64") return "x86_64-pc-windows-msvc";
  throw new Error(`unsupported platform ${platform}/${arch}`);
}

function repositorySlug() {
  if (process.env.ARSENAL_REPO) return process.env.ARSENAL_REPO;
  const url = (pkg.repository && pkg.repository.url) || "";
  const match = url.match(/github\.com[/:]([^/]+)\/([^/.]+?)(?:\.git)?$/);
  return match ? `${match[1]}/${match[2]}` : "bteixeira691/arsenal";
}

function cacheRoot() {
  if (process.env.ARSENAL_CACHE_DIR) return process.env.ARSENAL_CACHE_DIR;
  if (process.platform === "win32") {
    return path.join(
      process.env.LOCALAPPDATA || path.join(os.homedir(), "AppData", "Local"),
      "arsenal",
      "bin",
    );
  }
  if (process.platform === "darwin") {
    return path.join(os.homedir(), "Library", "Caches", "arsenal", "bin");
  }
  return path.join(
    process.env.XDG_CACHE_HOME || path.join(os.homedir(), ".cache"),
    "arsenal",
    "bin",
  );
}

const target = platformTarget();
const suffix = process.platform === "win32" ? ".exe" : "";
const asset = `arsenal-${target}${suffix}`;
const binaryPath = path.join(cacheRoot(), `v${VERSION}`, `arsenal${suffix}`);

function describeError(error) {
  const cause = error && error.cause;
  const detail = cause && (cause.code || cause.message);
  return `${error && error.message ? error.message : error}${detail ? ` (${detail})` : ""}`;
}

function shellDownload(url) {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), "arsenal-download-"));
  const file = path.join(directory, "asset");
  const quote = (value) => `'${String(value).replace(/'/g, "''")}'`;
  try {
    let result;
    if (process.platform === "win32") {
      result = spawnSync(
        "powershell.exe",
        [
          "-NoProfile",
          "-NonInteractive",
          "-Command",
          `Invoke-WebRequest -UseBasicParsing -Uri ${quote(url)} -OutFile ${quote(file)}`,
        ],
        { stdio: "ignore" },
      );
      if (!result || result.status !== 0) {
        result = spawnSync(
          "curl.exe",
          ["-fsSL", "--retry", "3", "--connect-timeout", "15", "-o", file, url],
          { stdio: "ignore" },
        );
      }
    } else {
      result = spawnSync(
        "curl",
        ["-fsSL", "--retry", "3", "--connect-timeout", "15", "-o", file, url],
        { stdio: "ignore" },
      );
    }
    if (!result || result.status !== 0) {
      throw new Error("curl/PowerShell fallback failed");
    }
    return fs.readFileSync(file);
  } finally {
    fs.rmSync(directory, { recursive: true, force: true });
  }
}

async function download(url) {
  const failures = [];
  for (let attempt = 1; attempt <= 3; attempt += 1) {
    try {
      const response = await fetch(url, {
        redirect: "follow",
        signal: AbortSignal.timeout(30_000),
      });
      if (!response.ok) {
        throw new Error(`download failed (${response.status}) from ${url}`);
      }
      return Buffer.from(await response.arrayBuffer());
    } catch (error) {
      failures.push(describeError(error));
      if (attempt < 3) {
        await new Promise((resolve) => setTimeout(resolve, attempt * 1_000));
      }
    }
  }
  try {
    return shellDownload(url);
  } catch (error) {
    failures.push(describeError(error));
  }
  throw new Error(`download failed from ${url}: ${failures.join("; ")}`);
}

async function ensureBinary() {
  if (process.env.ARSENAL_BINARY) {
    const override = path.resolve(process.env.ARSENAL_BINARY);
    if (!fs.existsSync(override)) {
      throw new Error(`ARSENAL_BINARY points to a missing file: ${override}`);
    }
    return override;
  }
  if (fs.existsSync(binaryPath)) return binaryPath;

  const base =
    process.env.ARSENAL_RELEASE_BASE_URL ||
    `https://github.com/${repositorySlug()}/releases/download/v${VERSION}`;
  const url = `${base}/${asset}`;
  const checksumUrl = `${url}.sha256`;

  fs.mkdirSync(path.dirname(binaryPath), { recursive: true });
  process.stderr.write(`arsenal: downloading ${asset} (v${VERSION})...\n`);

  const [binary, checksumFile] = await Promise.all([
    download(url),
    download(checksumUrl),
  ]);
  const expected = checksumFile
    .toString("utf8")
    .trim()
    .split(/\s+/)[0]
    .toLowerCase();
  if (!/^[0-9a-f]{64}$/.test(expected)) {
    throw new Error(`invalid checksum file from ${checksumUrl}`);
  }
  const actual = crypto.createHash("sha256").update(binary).digest("hex");
  if (actual !== expected) {
    throw new Error(`checksum mismatch for ${asset}`);
  }

  const temporary = `${binaryPath}.${process.pid}.tmp`;
  fs.writeFileSync(temporary, binary, { mode: 0o755 });
  fs.renameSync(temporary, binaryPath);
  if (process.platform !== "win32") fs.chmodSync(binaryPath, 0o755);
  return binaryPath;
}

(async () => {
  const binary = await ensureBinary();
  const result = spawnSync(binary, process.argv.slice(2), {
    stdio: "inherit",
    env: { ...process.env, ARSENAL_LAUNCHER: "1" },
  });
  if (result.error) throw result.error;
  if (result.signal) {
    process.exit(1);
  }
  process.exit(result.status === null ? 1 : result.status);
})().catch((error) => {
  console.error(`arsenal: ${error.message}`);
  console.error(
    "Check network/proxy access to GitHub, or set ARSENAL_RELEASE_BASE_URL to a mirror.",
  );
  console.error(
    "For an offline install, set ARSENAL_BINARY to a local binary or use: cargo install --path .",
  );
  process.exit(1);
});
