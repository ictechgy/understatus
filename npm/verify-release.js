#!/usr/bin/env node
'use strict';

const crypto = require('crypto');
const fs = require('fs');
const path = require('path');
const { spawnSync } = require('child_process');

const repoRoot = path.resolve(__dirname, '..');
const SUPPORTED_TARGETS = ['aarch64-apple-darwin', 'x86_64-apple-darwin'];
const CHECKSUMS_PATH = path.join(__dirname, 'checksums.json');

function fail(message) {
  console.error('[understatus release verify] ' + message);
  process.exit(1);
}

function usage() {
  fail(
    'usage: node npm/verify-release.js <semver-without-v> ' +
      '[--target <rust-target>] [--tarball-dir <directory>] ' +
      '[--require-checksums] [--write-checksums] [--verify-sidecars] [--verify-packlist]'
  );
}

function parseArgs(argv) {
  const expected = argv[0];
  const options = {
    expected,
    target: null,
    tarballDir: null,
    requireChecksums: false,
    writeChecksums: false,
    verifySidecars: false,
    verifyPacklist: false,
  };

  for (let i = 1; i < argv.length; i += 1) {
    const arg = argv[i];
    if (arg === '--target') {
      if (!argv[i + 1] || argv[i + 1].startsWith('--')) {
        usage();
      }
      options.target = argv[i + 1];
      i += 1;
    } else if (arg === '--tarball-dir') {
      if (!argv[i + 1] || argv[i + 1].startsWith('--')) {
        usage();
      }
      options.tarballDir = argv[i + 1];
      i += 1;
    } else if (arg === '--require-checksums') {
      options.requireChecksums = true;
    } else if (arg === '--write-checksums') {
      options.writeChecksums = true;
    } else if (arg === '--verify-sidecars') {
      options.verifySidecars = true;
    } else if (arg === '--verify-packlist') {
      options.verifyPacklist = true;
    } else {
      usage();
    }
  }

  return options;
}

function readJson(filePath) {
  try {
    return JSON.parse(fs.readFileSync(filePath, 'utf8'));
  } catch (err) {
    fail(`${filePath} is not readable JSON: ${err.message}`);
  }
}

function writeJson(filePath, value) {
  fs.writeFileSync(filePath, JSON.stringify(value, null, 2) + '\n');
}

function computeSha256(filePath) {
  const hash = crypto.createHash('sha256');
  hash.update(fs.readFileSync(filePath));
  return hash.digest('hex');
}

function tarballName(version, target) {
  return `understatus-${version}-${target}.tar.gz`;
}

function tarballPath(version, target, tarballDir) {
  return path.join(tarballDir, tarballName(version, target));
}

function sidecarPath(version, target, tarballDir) {
  return path.join(tarballDir, `${tarballName(version, target)}.sha256`);
}

function requireFile(filePath, label) {
  if (!fs.existsSync(filePath) || !fs.statSync(filePath).isFile()) {
    fail(`${label} not found: ${filePath}`);
  }
}

function readSidecarChecksum(filePath, expectedTarballName) {
  requireFile(filePath, 'expected release checksum sidecar');
  const line = fs.readFileSync(filePath, 'utf8').trim();
  const match = line.match(/^([0-9a-f]{64})\s+(.+)$/);
  if (!match) {
    fail(`invalid sha256 sidecar format: ${filePath}`);
  }
  if (path.basename(match[2]) !== expectedTarballName) {
    fail(
      `sha256 sidecar ${filePath} names ${match[2]}, expected ${expectedTarballName}`
    );
  }
  return match[1];
}

function verifyPacklist() {
  const result = spawnSync('npm', ['pack', '--dry-run', '--json'], {
    cwd: __dirname,
    encoding: 'utf8',
  });
  if (result.error) {
    fail(`npm pack --dry-run failed to start: ${result.error.message}`);
  }
  if (result.status !== 0) {
    fail(`npm pack --dry-run failed (${result.status}): ${result.stderr}`);
  }

  let payload;
  try {
    payload = JSON.parse(result.stdout);
  } catch (err) {
    fail(`npm pack --dry-run did not return JSON: ${err.message}`);
  }
  const files = (((payload || [])[0] || {}).files || []).map((entry) => entry.path);
  for (const required of ['checksums.json', 'install.js', 'bin/understatus.js']) {
    if (!files.includes(required)) {
      fail(`npm packlist missing required file: ${required}`);
    }
  }
}

const options = parseArgs(process.argv.slice(2));
const expected = options.expected;

if (!expected || !/^\d+\.\d+\.\d+(-[0-9A-Za-z.-]+)?(\+[0-9A-Za-z.-]+)?$/.test(expected)) {
  usage();
}

if (options.target && !SUPPORTED_TARGETS.includes(options.target)) {
  fail(`unsupported target ${options.target}; expected one of ${SUPPORTED_TARGETS.join(', ')}`);
}

if ((options.writeChecksums || options.verifySidecars) && !options.tarballDir) {
  fail('--write-checksums and --verify-sidecars require --tarball-dir');
}

if (options.tarballDir) {
  options.tarballDir = path.resolve(options.tarballDir);
  if (!fs.existsSync(options.tarballDir) || !fs.statSync(options.tarballDir).isDirectory()) {
    fail(`--tarball-dir is not a directory: ${options.tarballDir}`);
  }
}

const cargoToml = fs.readFileSync(path.join(repoRoot, 'Cargo.toml'), 'utf8');
const cargoMatch = cargoToml.match(/^version\s*=\s*"([^"]+)"/m);
if (!cargoMatch) {
  fail('Cargo.toml version not found');
}
if (cargoMatch[1] !== expected) {
  fail(`Cargo.toml version ${cargoMatch[1]} does not match tag ${expected}`);
}

const packageJson = readJson(path.join(__dirname, 'package.json'));
if (packageJson.version !== expected) {
  fail(`npm/package.json version ${packageJson.version} does not match tag ${expected}`);
}

const installJs = fs.readFileSync(path.join(__dirname, 'install.js'), 'utf8');
const installMatch = installJs.match(/const VERSION = '([^']+)'/);
if (!installMatch) {
  fail('install.js VERSION not found');
}
if (installMatch[1] !== expected) {
  fail(`install.js VERSION ${installMatch[1]} does not match tag ${expected}`);
}

const targetsToCheck = options.target ? [options.target] : SUPPORTED_TARGETS;
let checksums = readJson(CHECKSUMS_PATH);

if (options.verifySidecars) {
  for (const target of targetsToCheck) {
    const artifactName = tarballName(expected, target);
    const artifactPath = tarballPath(expected, target, options.tarballDir);
    requireFile(artifactPath, 'expected release artifact');
    const actualChecksum = computeSha256(artifactPath);
    const sidecarChecksum = readSidecarChecksum(
      sidecarPath(expected, target, options.tarballDir),
      artifactName
    );
    if (sidecarChecksum !== actualChecksum) {
      fail(
        `sha256 sidecar mismatch for ${artifactName}: ` +
          `sidecar=${sidecarChecksum} actual=${actualChecksum}`
      );
    }
  }
}

if (options.writeChecksums) {
  const releaseChecksums = Object.assign({}, checksums[expected] || {});
  for (const target of targetsToCheck) {
    const artifactPath = tarballPath(expected, target, options.tarballDir);
    requireFile(artifactPath, 'expected release artifact');
    releaseChecksums[target] = computeSha256(artifactPath);
  }
  checksums = Object.assign({}, checksums, { [expected]: releaseChecksums });
  writeJson(CHECKSUMS_PATH, checksums);
}

const releaseChecksums = checksums[expected];
if (!releaseChecksums) {
  if (options.requireChecksums || options.tarballDir) {
    fail(`checksums.json has no entry for ${expected}`);
  }
} else {
  for (const target of targetsToCheck) {
    const checksum = releaseChecksums[target];
    if (!/^[0-9a-f]{64}$/.test(checksum || '')) {
      fail(`checksums.json missing valid sha256 for ${expected}/${target}`);
    }

    if (options.tarballDir) {
      const artifactPath = tarballPath(expected, target, options.tarballDir);
      requireFile(artifactPath, 'expected release artifact');
      const actualChecksum = computeSha256(artifactPath);
      if (actualChecksum !== checksum) {
        fail(
          `checksum mismatch for ${tarballName(expected, target)}: ` +
            `checksums.json=${checksum} actual=${actualChecksum}`
        );
      }
    }
  }
}

if (options.verifyPacklist) {
  verifyPacklist();
}

const checked = ['versions'];
if (releaseChecksums || options.requireChecksums || options.writeChecksums) {
  checked.push('manifest');
}
if (options.tarballDir) {
  checked.push('release artifacts');
}
if (options.verifySidecars) {
  checked.push('sha256 sidecars');
}
if (options.verifyPacklist) {
  checked.push('npm packlist');
}
console.log(`[understatus release verify] ${expected} ${checked.join(', ')} are consistent`);
