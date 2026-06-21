#!/usr/bin/env node
'use strict';

process.env.UNDERSTATUS_INSTALL_TEST = '1';

const assert = require('assert');
const fs = require('fs');
const os = require('os');
const path = require('path');
const { spawnSync } = require('child_process');
const install = require('./install.js');

const TAR = '/usr/bin/tar';

function runTar(args) {
  const result = spawnSync(TAR, args, { encoding: 'utf8' });
  assert.strictEqual(result.status, 0, result.stderr || result.error && result.error.message);
}

function makeTarball(build) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'understatus-install-fixture-'));
  const tarball = path.join(root, 'fixture.tar.gz');
  const files = path.join(root, 'files');
  fs.mkdirSync(files);
  build(files);
  const entries = fs.readdirSync(files);
  runTar(['-czf', tarball, '-C', files, ...entries]);
  return { root, tarball };
}

function assertThrowsMessage(fn, pattern) {
  let thrown = null;
  try {
    fn();
  } catch (err) {
    thrown = err;
  }
  assert(thrown, 'expected function to throw');
  assert.match(thrown.message, pattern);
}

const fixtures = [];
try {
  let fixture = makeTarball((files) => {
    fs.writeFileSync(path.join(files, 'understatus'), '#!/bin/sh\necho ok\n');
  });
  fixtures.push(fixture.root);
  const extracted = install.extractValidatedBinary(fixture.tarball);
  assert.strictEqual(fs.readFileSync(extracted, 'utf8'), '#!/bin/sh\necho ok\n');
  assert(path.basename(path.dirname(extracted)).startsWith('.understatus-extract-'));
  install.removeDirQuietly(path.dirname(extracted));

  fixture = makeTarball((files) => {
    fs.writeFileSync(path.join(files, 'understatus'), 'ok\n');
    fs.writeFileSync(path.join(files, 'extra'), 'extra\n');
  });
  fixtures.push(fixture.root);
  assertThrowsMessage(
    () => install.extractValidatedBinary(fixture.tarball),
    /예상하지 않은 tarball 항목/
  );

  fixture = makeTarball((files) => {
    fs.symlinkSync('missing-target', path.join(files, 'understatus'));
  });
  fixtures.push(fixture.root);
  assertThrowsMessage(
    () => install.extractValidatedBinary(fixture.tarball),
    /regular file이 아닙니다/
  );

  assertThrowsMessage(
    () => install.expectedChecksum('0.7.0', 'unknown-target'),
    /체크섬이 없습니다/
  );

  console.log('install tests passed');
} finally {
  for (const fixtureRoot of fixtures) {
    fs.rmSync(fixtureRoot, { recursive: true, force: true });
  }
}
