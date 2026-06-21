#!/usr/bin/env node
'use strict';

const fs = require('fs');
const path = require('path');

const repoRoot = path.resolve(__dirname, '..');
const expected = process.argv[2];

function fail(message) {
  console.error('[understatus release verify] ' + message);
  process.exit(1);
}

if (!expected || !/^\d+\.\d+\.\d+([-.+][0-9A-Za-z.-]+)?$/.test(expected)) {
  fail('usage: node npm/verify-release.js <semver-without-v>');
}

const cargoToml = fs.readFileSync(path.join(repoRoot, 'Cargo.toml'), 'utf8');
const cargoMatch = cargoToml.match(/^version\s*=\s*"([^"]+)"/m);
if (!cargoMatch) {
  fail('Cargo.toml version not found');
}
if (cargoMatch[1] !== expected) {
  fail(`Cargo.toml version ${cargoMatch[1]} does not match tag ${expected}`);
}

const packageJson = JSON.parse(fs.readFileSync(path.join(__dirname, 'package.json'), 'utf8'));
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

const checksums = JSON.parse(fs.readFileSync(path.join(__dirname, 'checksums.json'), 'utf8'));
const releaseChecksums = checksums[expected];
if (!releaseChecksums) {
  fail(`checksums.json has no entry for ${expected}`);
}
for (const target of ['aarch64-apple-darwin', 'x86_64-apple-darwin']) {
  const checksum = releaseChecksums[target];
  if (!/^[0-9a-f]{64}$/.test(checksum || '')) {
    fail(`checksums.json missing valid sha256 for ${expected}/${target}`);
  }
}

console.log(`[understatus release verify] ${expected} versions and checksums are consistent`);
