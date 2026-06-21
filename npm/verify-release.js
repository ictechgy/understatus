#!/usr/bin/env node
'use strict';

const crypto = require('crypto');
const fs = require('fs');
const path = require('path');

const repoRoot = path.resolve(__dirname, '..');
const SUPPORTED_TARGETS = ['aarch64-apple-darwin', 'x86_64-apple-darwin'];

function fail(message) {
  console.error('[understatus release verify] ' + message);
  process.exit(1);
}

function usage() {
  fail(
    'usage: node npm/verify-release.js <semver-without-v> ' +
      '[--target <rust-target>] [--tarball-dir <directory>]'
  );
}

function parseArgs(argv) {
  const expected = argv[0];
  const options = {
    expected,
    target: null,
    tarballDir: null,
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

function computeSha256(filePath) {
  const hash = crypto.createHash('sha256');
  hash.update(fs.readFileSync(filePath));
  return hash.digest('hex');
}

const options = parseArgs(process.argv.slice(2));
const expected = options.expected;

if (!expected || !/^\d+\.\d+\.\d+([-.+][0-9A-Za-z.-]+)?$/.test(expected)) {
  usage();
}

if (options.target && !SUPPORTED_TARGETS.includes(options.target)) {
  fail(`unsupported target ${options.target}; expected one of ${SUPPORTED_TARGETS.join(', ')}`);
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

const checksums = readJson(path.join(__dirname, 'checksums.json'));
const releaseChecksums = checksums[expected];
if (!releaseChecksums) {
  fail(`checksums.json has no entry for ${expected}`);
}

const targetsToCheck = options.target ? [options.target] : SUPPORTED_TARGETS;
for (const target of targetsToCheck) {
  const checksum = releaseChecksums[target];
  if (!/^[0-9a-f]{64}$/.test(checksum || '')) {
    fail(`checksums.json missing valid sha256 for ${expected}/${target}`);
  }

  if (options.tarballDir) {
    const tarballName = `understatus-${expected}-${target}.tar.gz`;
    const tarballPath = path.join(options.tarballDir, tarballName);
    if (!fs.existsSync(tarballPath) || !fs.statSync(tarballPath).isFile()) {
      fail(`expected release artifact not found: ${tarballPath}`);
    }
    const actualChecksum = computeSha256(tarballPath);
    if (actualChecksum !== checksum) {
      fail(
        `checksum mismatch for ${tarballName}: ` +
          `checksums.json=${checksum} actual=${actualChecksum}`
      );
    }
  }
}

const artifactSuffix = options.tarballDir ? ' and release artifacts' : '';
console.log(
  `[understatus release verify] ${expected} versions, manifest${artifactSuffix} are consistent`
);
