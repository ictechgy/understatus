#!/usr/bin/env node
'use strict';

const assert = require('assert');
const crypto = require('crypto');
const fs = require('fs');
const os = require('os');
const path = require('path');
const { spawnSync } = require('child_process');

const repoRoot = path.resolve(__dirname, '..');
const version = JSON.parse(fs.readFileSync(path.join(__dirname, 'package.json'), 'utf8')).version;
const targets = ['aarch64-apple-darwin', 'x86_64-apple-darwin'];

function copyFile(src, dest) {
  fs.mkdirSync(path.dirname(dest), { recursive: true });
  fs.copyFileSync(src, dest);
}

function sha256(filePath) {
  const hash = crypto.createHash('sha256');
  hash.update(fs.readFileSync(filePath));
  return hash.digest('hex');
}

function runVerify(fixtureRoot, args) {
  return spawnSync(process.execPath, [path.join(fixtureRoot, 'npm', 'verify-release.js'), version, ...args], {
    cwd: fixtureRoot,
    encoding: 'utf8',
  });
}

function makeFixture() {
  const fixtureRoot = fs.mkdtempSync(path.join(os.tmpdir(), 'understatus-verify-release-'));
  fs.mkdirSync(path.join(fixtureRoot, 'npm', 'bin'), { recursive: true });
  for (const rel of [
    'npm/verify-release.js',
    'npm/install.js',
    'npm/checksums.json',
    'npm/package.json',
    'npm/bin/understatus.js',
    'npm/README.md',
  ]) {
    copyFile(path.join(repoRoot, rel), path.join(fixtureRoot, rel));
  }
  fs.writeFileSync(path.join(fixtureRoot, 'Cargo.toml'), `[package]\nversion = "${version}"\n`);
  return fixtureRoot;
}

function makeAssets(fixtureRoot) {
  const assetDir = path.join(fixtureRoot, 'assets');
  fs.mkdirSync(assetDir, { recursive: true });
  for (const target of targets) {
    const name = `understatus-${version}-${target}.tar.gz`;
    const filePath = path.join(assetDir, name);
    fs.writeFileSync(filePath, `artifact:${target}\n`);
    fs.writeFileSync(path.join(assetDir, `${name}.sha256`), `${sha256(filePath)}  ${name}\n`);
  }
  return assetDir;
}

const fixtureRoot = makeFixture();
try {
  const assetDir = makeAssets(fixtureRoot);

  let result = runVerify(fixtureRoot, [
    '--tarball-dir',
    assetDir,
    '--verify-sidecars',
    '--write-checksums',
    '--verify-packlist',
  ]);
  assert.strictEqual(result.status, 0, result.stderr || result.stdout);

  let result2 = runVerify(fixtureRoot, [
    '--tarball-dir',
    assetDir,
    '--require-checksums',
    '--verify-packlist',
  ]);
  assert.strictEqual(result2.status, 0, result2.stderr || result2.stdout);

  result2 = runVerify(fixtureRoot, [
    '--tarball-dir',
    assetDir,
    '--write-checksums',
    '--require-checksums',
  ]);
  assert.notStrictEqual(result2.status, 0, 'write+require combination should fail');
  assert.match(result2.stderr, /separate verify step/);

  const generated = JSON.parse(fs.readFileSync(path.join(fixtureRoot, 'npm', 'checksums.json'), 'utf8'));
  for (const target of targets) {
    assert.strictEqual(
      generated[version][target],
      sha256(path.join(assetDir, `understatus-${version}-${target}.tar.gz`))
    );
  }

  const corruptedSidecar = path.join(
    assetDir,
    `understatus-${version}-aarch64-apple-darwin.tar.gz.sha256`
  );
  fs.writeFileSync(corruptedSidecar, `${'0'.repeat(64)}  understatus-${version}-aarch64-apple-darwin.tar.gz\n`);
  result = runVerify(fixtureRoot, ['--tarball-dir', assetDir, '--verify-sidecars']);
  assert.notStrictEqual(result.status, 0, 'corrupted sidecar should fail');
  assert.match(result.stderr, /sidecar mismatch/);

  makeAssets(fixtureRoot);
  fs.writeFileSync(
    path.join(assetDir, `understatus-${version}-x86_64-apple-darwin.tar.gz`),
    'tampered\n'
  );
  result = runVerify(fixtureRoot, ['--tarball-dir', assetDir, '--require-checksums']);
  assert.notStrictEqual(result.status, 0, 'tarball/checksum mismatch should fail');
  assert.match(result.stderr, /checksum mismatch/);

  const pkgPath = path.join(fixtureRoot, 'npm', 'package.json');
  const pkg = JSON.parse(fs.readFileSync(pkgPath, 'utf8'));
  pkg.files = pkg.files.filter((entry) => entry !== 'checksums.json');
  fs.writeFileSync(pkgPath, JSON.stringify(pkg, null, 2) + '\n');
  result = runVerify(fixtureRoot, ['--verify-packlist']);
  assert.notStrictEqual(result.status, 0, 'missing checksums.json in packlist should fail');
  assert.match(result.stderr, /packlist missing required file: checksums\.json/);

  result = spawnSync(process.execPath, [path.join(fixtureRoot, 'npm', 'verify-release.js'), '1.2.3.4'], {
    cwd: fixtureRoot,
    encoding: 'utf8',
  });
  assert.notStrictEqual(result.status, 0, 'invalid semver should fail');

  console.log('verify-release tests passed');
} finally {
  fs.rmSync(fixtureRoot, { recursive: true, force: true });
}
