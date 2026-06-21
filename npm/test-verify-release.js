#!/usr/bin/env node
'use strict';

const assert = require('assert');
const crypto = require('crypto');
const fs = require('fs');
const os = require('os');
const path = require('path');
const { spawnSync } = require('child_process');

const TAR = '/usr/bin/tar';

const repoRoot = path.resolve(__dirname, '..');
const version = JSON.parse(fs.readFileSync(path.join(__dirname, 'package.json'), 'utf8')).version;
const targets = ['aarch64-apple-darwin', 'x86_64-apple-darwin'];
const expectedCommit = '0123456789abcdef0123456789abcdef01234567';

function copyFile(src, dest) {
  fs.mkdirSync(path.dirname(dest), { recursive: true });
  fs.copyFileSync(src, dest);
}

function sha256(filePath) {
  const hash = crypto.createHash('sha256');
  hash.update(fs.readFileSync(filePath));
  return hash.digest('hex');
}

function runTar(args) {
  const result = spawnSync(TAR, args, { encoding: 'utf8' });
  assert.strictEqual(result.status, 0, result.stderr || result.error && result.error.message);
}

function writeTarball(filePath, entries) {
  const buildDir = fs.mkdtempSync(path.join(os.tmpdir(), 'understatus-asset-build-'));
  try {
    for (const [name, content] of entries) {
      fs.writeFileSync(path.join(buildDir, name), content);
    }
    runTar(['-czf', filePath, '-C', buildDir, ...entries.map(([name]) => name)]);
  } finally {
    fs.rmSync(buildDir, { recursive: true, force: true });
  }
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
    writeTarball(filePath, [['understatus', `artifact:${target}\n`]]);
    const checksum = sha256(filePath);
    fs.writeFileSync(path.join(assetDir, `${name}.sha256`), `${checksum}  ${name}\n`);
    fs.writeFileSync(
      path.join(assetDir, `${name}.provenance.json`),
      JSON.stringify({
        tag: `v${version}`,
        commit: expectedCommit,
        target,
        asset: name,
        sha256: checksum,
      }, null, 2) + '\n'
    );
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
    '--verify-provenance',
    '--expected-commit',
    expectedCommit,
    '--write-checksums',
    '--verify-packlist',
  ]);
  assert.strictEqual(result.status, 0, result.stderr || result.stdout);

  let result2 = runVerify(fixtureRoot, [
    '--tarball-dir',
    assetDir,
    '--verify-provenance',
    '--expected-commit',
    expectedCommit,
    '--require-checksums',
    '--verify-packlist'
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

  makeAssets(fixtureRoot);
  const badProvenance = path.join(
    assetDir,
    `understatus-${version}-aarch64-apple-darwin.tar.gz.provenance.json`
  );
  const badProvenancePayload = JSON.parse(fs.readFileSync(badProvenance, 'utf8'));
  badProvenancePayload.commit = 'f'.repeat(40);
  fs.writeFileSync(badProvenance, JSON.stringify(badProvenancePayload, null, 2) + '\n');
  result = runVerify(fixtureRoot, [
    '--tarball-dir',
    assetDir,
    '--verify-provenance',
    '--expected-commit',
    expectedCommit,
  ]);
  assert.notStrictEqual(result.status, 0, 'provenance commit mismatch should fail');
  assert.match(result.stderr, /provenance commit mismatch/);

  const corruptedSidecar = path.join(
    assetDir,
    `understatus-${version}-aarch64-apple-darwin.tar.gz.sha256`
  );
  fs.writeFileSync(corruptedSidecar, `${'0'.repeat(64)}  understatus-${version}-aarch64-apple-darwin.tar.gz\n`);
  result = runVerify(fixtureRoot, ['--tarball-dir', assetDir, '--verify-sidecars']);
  assert.notStrictEqual(result.status, 0, 'corrupted sidecar should fail');
  assert.match(result.stderr, /sidecar mismatch/);

  makeAssets(fixtureRoot);
  const changedTarball = path.join(assetDir, `understatus-${version}-x86_64-apple-darwin.tar.gz`);
  writeTarball(changedTarball, [['understatus', 'tampered but installable\n']]);
  fs.writeFileSync(
    `${changedTarball}.sha256`,
    `${sha256(changedTarball)}  understatus-${version}-x86_64-apple-darwin.tar.gz\n`
  );
  result = runVerify(fixtureRoot, ['--tarball-dir', assetDir, '--require-checksums']);
  assert.notStrictEqual(result.status, 0, 'tarball/checksum mismatch should fail');
  assert.match(result.stderr, /checksum mismatch/);

  makeAssets(fixtureRoot);
  const malformedTarball = path.join(assetDir, `understatus-${version}-aarch64-apple-darwin.tar.gz`);
  writeTarball(malformedTarball, [
    ['understatus', 'ok\n'],
    ['extra', 'not allowed\n'],
  ]);
  fs.writeFileSync(
    `${malformedTarball}.sha256`,
    `${sha256(malformedTarball)}  understatus-${version}-aarch64-apple-darwin.tar.gz\n`
  );
  result = runVerify(fixtureRoot, ['--tarball-dir', assetDir, '--verify-sidecars', '--write-checksums']);
  assert.notStrictEqual(result.status, 0, 'malformed tarball layout should fail');
  assert.match(result.stderr, /tarball layout must contain exactly understatus/);

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
