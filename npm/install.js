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

// 네이티브 바이너리를 받아오는 GitHub 릴리스 태그.
// npm 래퍼 패키지 버전(package.json)과는 분리되어 있습니다 —
// 래퍼(이 스크립트/런처)만 패치 게시될 수 있으므로, 바이너리는 항상
// 아래 고정된 릴리스에서 받습니다. 새 바이너리 릴리스를 낼 때 이 값을 올리세요.
const VERSION = '0.2.0';

// 플랫폼 확인: macOS 전용 패키지
if (process.platform !== 'darwin') {
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
if (!target) {
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
const SHA256_NAME = TARBALL_NAME + '.sha256';
const TARBALL_URL = RELEASE_BASE + TARBALL_NAME;
const SHA256_URL = RELEASE_BASE + SHA256_NAME;

// bin 디렉터리 경로
const BIN_DIR = path.join(__dirname, 'bin');
const TARBALL_PATH = path.join(BIN_DIR, TARBALL_NAME);
const BINARY_PATH = path.join(BIN_DIR, 'understatus');

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
          return resolve(
            download(response.headers.location, destPath, maxRedirects - 1)
          );
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
 * SHA-256 체크섬 파일을 파싱하여 기대 해시값을 반환합니다.
 * 형식: "<sha256>  <filename>"
 *
 * @param {string} sha256Content - 체크섬 파일의 텍스트 내용
 * @returns {string} 소문자 16진수 SHA-256 해시
 */
function parseSha256(sha256Content) {
  const line = sha256Content.trim().split('\n')[0];
  const parts = line.split(/\s+/);
  if (!parts[0] || parts[0].length !== 64) {
    throw new Error(
      'SHA-256 파일 형식을 파싱할 수 없습니다. 내용: ' + sha256Content
    );
  }
  return parts[0].toLowerCase();
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
 * 순서: SHA256 다운로드 → tarball 다운로드 → 체크섬 검증 → 압축 해제 → chmod
 */
async function main() {
  // bin 디렉터리가 없으면 생성
  if (!fs.existsSync(BIN_DIR)) {
    fs.mkdirSync(BIN_DIR, { recursive: true });
  }

  process.stdout.write('[understatus] 설치 중... (버전 ' + VERSION + ', 타겟 ' + target + ')\n');

  // 1단계: SHA-256 체크섬 파일 다운로드
  process.stdout.write('[understatus] SHA-256 체크섬 다운로드 중: ' + SHA256_URL + '\n');
  let sha256Buffer;
  try {
    sha256Buffer = await download(SHA256_URL, null);
  } catch (err) {
    process.stderr.write(
      '[understatus] SHA-256 파일 다운로드 실패:\n  ' + err.message + '\n'
    );
    process.exit(1);
  }
  const expectedHash = parseSha256(sha256Buffer.toString('utf8'));

  // 2단계: tarball 다운로드
  process.stdout.write('[understatus] 바이너리 다운로드 중: ' + TARBALL_URL + '\n');
  try {
    await download(TARBALL_URL, TARBALL_PATH);
  } catch (err) {
    process.stderr.write(
      '[understatus] tarball 다운로드 실패:\n  ' + err.message + '\n'
    );
    process.exit(1);
  }

  // 3단계: SHA-256 체크섬 검증
  process.stdout.write('[understatus] 체크섬 검증 중...\n');
  let actualHash;
  try {
    actualHash = await computeSha256(TARBALL_PATH);
  } catch (err) {
    fs.unlink(TARBALL_PATH, () => {});
    process.stderr.write('[understatus] 체크섬 계산 실패: ' + err.message + '\n');
    process.exit(1);
  }

  if (actualHash !== expectedHash) {
    fs.unlink(TARBALL_PATH, () => {});
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

  // 4단계: tarball 압축 해제 (시스템 tar 사용)
  // tarball 루트에 "understatus" 실행 파일 하나만 포함되어 있습니다.
  process.stdout.write('[understatus] 압축 해제 중...\n');
  const tarResult = spawnSync('tar', ['-xzf', TARBALL_PATH, '-C', BIN_DIR], {
    stdio: 'inherit',
  });

  // tarball 임시 파일 삭제
  try {
    fs.unlinkSync(TARBALL_PATH);
  } catch (_) {
    // 삭제 실패는 무시
  }

  if (tarResult.error) {
    process.stderr.write(
      '[understatus] tar 실행 오류: ' + tarResult.error.message + '\n'
    );
    process.exit(1);
  }
  if (tarResult.status !== 0) {
    process.stderr.write(
      '[understatus] tar 압축 해제 실패 (종료 코드 ' + tarResult.status + ').\n'
    );
    process.exit(1);
  }

  // 5단계: 실행 권한 부여
  try {
    fs.chmodSync(BINARY_PATH, 0o755);
  } catch (err) {
    process.stderr.write(
      '[understatus] chmod 실패: ' + err.message + '\n'
    );
    process.exit(1);
  }

  process.stdout.write(
    '[understatus] 설치 완료! "understatus render" 또는 "understatus install" 명령을 사용해 보세요.\n'
  );
}

main().catch((err) => {
  process.stderr.write('[understatus] 예기치 않은 오류: ' + err.message + '\n');
  process.exit(1);
});
