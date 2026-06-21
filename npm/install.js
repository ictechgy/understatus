/**
 * install.js — postinstall 스크립트
 *
 * npm install 후 자동으로 실행됩니다.
 * GitHub Releases에서 현재 플랫폼에 맞는 네이티브 바이너리를 다운로드하고,
 * SHA-256 체크섬을 검증한 뒤, npm/bin/ 디렉터리에 압축을 해제합니다.
 *
 * 왜 외부 의존성을 사용하지 않는가:
 * postinstall 시점에는 node_modules가 아직 완전히 준비되지 않을 수 있으므로
 * Node.js 내장 모듈만 사용합니다.
 */

'use strict';

const https = require('https');
const fs = require('fs');
const crypto = require('crypto');
const { spawnSync } = require('child_process');
const path = require('path');
const os = require('os');

// 네이티브 바이너리를 받아오는 GitHub 릴리스 태그.
// release/publish guard가 Cargo.toml, npm/package.json, 이 값, git tag를 lockstep으로
// 검증합니다. 새 네이티브 릴리스를 낼 때 네 버전을 함께 올리세요.
const VERSION = '0.7.0';
const INSTALL_TEST = process.env.UNDERSTATUS_INSTALL_TEST === '1';

// 플랫폼 확인: macOS 전용 패키지
if (!INSTALL_TEST && process.platform !== 'darwin') {
  process.stdout.write(
    '[understatus] macOS 전용 패키지입니다. 현재 플랫폼(' +
      process.platform +
      ')에서는 설치를 건너뜁니다.\n'
  );
  process.exit(0);
}

// CPU 아키텍처를 Rust 타겟 트리플로 매핑
const archMap = {
  arm64: 'aarch64-apple-darwin',
  x64: 'x86_64-apple-darwin',
};

const target = archMap[process.arch];
if (!INSTALL_TEST && !target) {
  process.stderr.write(
    '[understatus] 지원하지 않는 아키텍처입니다: ' + process.arch + '\n' +
    '[understatus] 지원 아키텍처: arm64 (Apple Silicon), x64 (Intel)\n'
  );
  process.exit(1);
}

// 다운로드 URL 구성
const RELEASE_BASE =
  'https://github.com/ictechgy/understatus/releases/download/v' + VERSION + '/';
const TARBALL_NAME = 'understatus-' + VERSION + '-' + target + '.tar.gz';
const TARBALL_URL = RELEASE_BASE + TARBALL_NAME;

// bin 디렉터리 경로
const BIN_DIR = path.join(__dirname, 'bin');
const BINARY_PATH = path.join(BIN_DIR, 'understatus');
const TAR = '/usr/bin/tar';

/**
 * HTTP(S) GET 요청으로 URL에서 데이터를 다운로드합니다.
 * 리디렉션(302, 301, 307, 308)을 최대 maxRedirects 회 따라갑니다.
 *
 * @param {string} url - 다운로드할 URL
 * @param {string|null} destPath - null이면 Buffer를 반환, 아니면 파일로 저장
 * @param {number} maxRedirects - 최대 리디렉션 횟수
 * @returns {Promise<Buffer|void>}
 */
function download(url, destPath, maxRedirects = 10) {
  return new Promise((resolve, reject) => {
    if (maxRedirects < 0) {
      return reject(new Error('너무 많은 리디렉션이 발생했습니다: ' + url));
    }

    https
      .get(url, (response) => {
        // 리디렉션 처리
        if (
          [301, 302, 307, 308].includes(response.statusCode) &&
          response.headers.location
        ) {
          response.resume(); // 현재 응답 본문 소비
          let nextUrl;
          try {
            nextUrl = new URL(response.headers.location, url);
          } catch (err) {
            return reject(new Error('잘못된 리디렉션 URL: ' + response.headers.location));
          }
          if (nextUrl.protocol !== 'https:') {
            return reject(new Error('HTTPS가 아닌 리디렉션 거부: ' + nextUrl.toString()));
          }
          return resolve(download(nextUrl.toString(), destPath, maxRedirects - 1));
        }

        if (response.statusCode !== 200) {
          response.resume();
          return reject(
            new Error(
              'HTTP ' + response.statusCode + ' 오류: ' + url + '\n' +
              '릴리즈가 존재하는지 확인하세요: ' +
              'https://github.com/ictechgy/understatus/releases'
            )
          );
        }

        if (destPath) {
          // 파일로 저장
          const fileStream = fs.createWriteStream(destPath);
          response.pipe(fileStream);
          fileStream.on('finish', () => fileStream.close(resolve));
          fileStream.on('error', (err) => {
            fs.unlink(destPath, () => {}); // 실패 시 임시 파일 삭제
            reject(err);
          });
          response.on('error', reject);
        } else {
          // Buffer로 수집
          const chunks = [];
          response.on('data', (chunk) => chunks.push(chunk));
          response.on('end', () => resolve(Buffer.concat(chunks)));
          response.on('error', reject);
        }
      })
      .on('error', reject);
  });
}

/**
 * npm 패키지에 고정된 체크섬 manifest에서 기대 해시값을 반환합니다.
 *
 * @param {string} version - 네이티브 바이너리 버전
 * @param {string} releaseTarget - Rust 타겟 트리플
 * @returns {string} 소문자 16진수 SHA-256 해시
 */
function removeDirQuietly(dirPath) {
  if (!dirPath) {
    return;
  }
  try {
    fs.rmSync(dirPath, { recursive: true, force: true });
  } catch (_) {
    // cleanup best effort
  }
}

function expectedChecksum(version, releaseTarget) {
  const checksums = require('./checksums.json');
  const releaseChecksums = checksums[version];
  const checksum = releaseChecksums && releaseChecksums[releaseTarget];
  if (!checksum || !/^[0-9a-f]{64}$/.test(checksum)) {
    throw new Error(
      'npm/checksums.json에 ' + version + '/' + releaseTarget + ' 체크섬이 없습니다.'
    );
  }
  return checksum;
}

/**
 * tarball 항목을 검증하고 temp dir에 안전하게 압축 해제한 뒤 바이너리 경로를 반환합니다.
 *
 * @param {string} tarballPath - 검증할 tarball 경로
 * @returns {string} temp dir 안에 압축 해제된 understatus 바이너리 경로
 */
function extractValidatedBinary(tarballPath) {
  const listResult = spawnSync(TAR, ['-tzf', tarballPath], {
    encoding: 'utf8',
  });
  if (listResult.error) {
    throw new Error('tar 목록 조회 오류: ' + listResult.error.message);
  }
  if (listResult.status !== 0) {
    throw new Error('tar 목록 조회 실패 (종료 코드 ' + listResult.status + '): ' + listResult.stderr);
  }

  const entries = listResult.stdout
    .split(/\r?\n/)
    .map((entry) => entry.trim())
    .filter(Boolean);
  if (entries.length !== 1 || entries[0] !== 'understatus') {
    throw new Error(
      '예상하지 않은 tarball 항목: ' + (entries.length ? entries.join(', ') : '(비어 있음)')
    );
  }

  const extractDir = fs.mkdtempSync(path.join(BIN_DIR, '.understatus-extract-'));
  const extractResult = spawnSync(TAR, ['-xzf', tarballPath, '-C', extractDir, 'understatus'], {
    stdio: 'pipe',
    encoding: 'utf8',
  });
  if (extractResult.error) {
    removeDirQuietly(extractDir);
    throw new Error('tar 압축 해제 오류: ' + extractResult.error.message);
  }
  if (extractResult.status !== 0) {
    removeDirQuietly(extractDir);
    throw new Error(
      'tar 압축 해제 실패 (종료 코드 ' + extractResult.status + '): ' + extractResult.stderr
    );
  }

  const extractedBinary = path.join(extractDir, 'understatus');
  let stat;
  try {
    stat = fs.lstatSync(extractedBinary);
  } catch (err) {
    removeDirQuietly(extractDir);
    throw new Error('압축 해제된 understatus를 찾을 수 없습니다: ' + err.message);
  }
  if (!stat.isFile()) {
    removeDirQuietly(extractDir);
    throw new Error('압축 해제된 understatus가 regular file이 아닙니다.');
  }
  return extractedBinary;
}

/**
 * 파일의 SHA-256 해시를 계산합니다.
 *
 * @param {string} filePath - 해시를 계산할 파일 경로
 * @returns {Promise<string>} 소문자 16진수 SHA-256 해시
 */
function computeSha256(filePath) {
  return new Promise((resolve, reject) => {
    const hash = crypto.createHash('sha256');
    const stream = fs.createReadStream(filePath);
    stream.on('data', (chunk) => hash.update(chunk));
    stream.on('end', () => resolve(hash.digest('hex')));
    stream.on('error', reject);
  });
}

/**
 * 메인 설치 함수
 * 순서: 고정 checksum 조회 → tarball 다운로드 → 체크섬 검증 → 안전 압축 해제 → chmod
 */
async function main() {
  // bin 디렉터리가 없으면 생성
  if (!fs.existsSync(BIN_DIR)) {
    fs.mkdirSync(BIN_DIR, { recursive: true });
  }

  process.stdout.write('[understatus] 설치 중... (버전 ' + VERSION + ', 타겟 ' + target + ')\n');

  // 1단계: npm 패키지에 고정된 SHA-256 체크섬 조회
  let expectedHash;
  try {
    expectedHash = expectedChecksum(VERSION, target);
  } catch (err) {
    process.stderr.write('[understatus] 체크섬 manifest 오류: ' + err.message + '\n');
    process.exit(1);
  }

  const downloadDir = fs.mkdtempSync(path.join(os.tmpdir(), 'understatus-download-'));
  const tarballPath = path.join(downloadDir, TARBALL_NAME);

  // 2단계: tarball 다운로드
  process.stdout.write('[understatus] 바이너리 다운로드 중: ' + TARBALL_URL + '\n');
  try {
    await download(TARBALL_URL, tarballPath);
  } catch (err) {
    removeDirQuietly(downloadDir);
    process.stderr.write(
      '[understatus] tarball 다운로드 실패:\n  ' + err.message + '\n'
    );
    process.exit(1);
  }

  // 3단계: SHA-256 체크섬 검증
  process.stdout.write('[understatus] 체크섬 검증 중...\n');
  let actualHash;
  try {
    actualHash = await computeSha256(tarballPath);
  } catch (err) {
    removeDirQuietly(downloadDir);
    process.stderr.write('[understatus] 체크섬 계산 실패: ' + err.message + '\n');
    process.exit(1);
  }

  if (actualHash !== expectedHash) {
    removeDirQuietly(downloadDir);
    process.stderr.write(
      '[understatus] SHA-256 체크섬 불일치! 다운로드가 손상되었을 수 있습니다.\n' +
      '  기대값: ' + expectedHash + '\n' +
      '  실제값: ' + actualHash + '\n' +
      '[understatus] 설치를 중단합니다. 다시 시도하거나 GitHub에 신고해 주세요:\n' +
      '  https://github.com/ictechgy/understatus/issues\n'
    );
    process.exit(1);
  }
  process.stdout.write('[understatus] 체크섬 검증 통과.\n');

  // 4단계: tarball 항목 검증 후 temp dir에 안전 압축 해제
  process.stdout.write('[understatus] 압축 해제 중...\n');
  let extractedBinary;
  try {
    extractedBinary = extractValidatedBinary(tarballPath);
    fs.renameSync(extractedBinary, BINARY_PATH);
    removeDirQuietly(path.dirname(extractedBinary));
  } catch (err) {
    if (extractedBinary) {
      removeDirQuietly(path.dirname(extractedBinary));
    }
    process.stderr.write('[understatus] 압축 해제 실패: ' + err.message + '\n');
    process.exit(1);
  } finally {
    // tarball 임시 파일 삭제
    removeDirQuietly(downloadDir);
  }

  // 5단계: 실행 권한 부여
  try {
    fs.chmodSync(BINARY_PATH, 0o755);
  } catch (err) {
    process.stderr.write('[understatus] chmod 실패: ' + err.message + '\n');
    process.exit(1);
  }

  process.stdout.write(
    '[understatus] 설치 완료! "understatus render" 또는 "understatus install" 명령을 사용해 보세요.\n'
  );
}

if (INSTALL_TEST) {
  module.exports = {
    computeSha256,
    download,
    expectedChecksum,
    extractValidatedBinary,
    removeDirQuietly,
  };
} else {
  main().catch((err) => {
    process.stderr.write('[understatus] 예기치 않은 오류: ' + err.message + '\n');
    process.exit(1);
  });
}
