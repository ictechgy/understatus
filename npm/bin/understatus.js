#!/usr/bin/env node

/**
 * understatus launcher
 *
 * 이 파일은 npm 패키지의 CLI 진입점입니다.
 * postinstall 단계에서 다운로드된 네이티브 바이너리(../bin/understatus)를
 * 실행하는 얇은 래퍼입니다.
 *
 * 왜 이렇게 하는가: npm은 JS 파일만 bin으로 등록할 수 있으므로,
 * 실제 네이티브 바이너리를 직접 bin에 넣을 수 없습니다.
 * 대신 이 JS 파일이 네이티브 바이너리를 찾아 실행합니다.
 */

'use strict';

const { spawnSync } = require('child_process');
const path = require('path');
const fs = require('fs');

// 네이티브 바이너리 위치: 이 파일(bin/understatus.js)과 같은 디렉터리의 understatus
const binaryPath = path.join(__dirname, 'understatus');

// 바이너리가 존재하는지 확인
if (!fs.existsSync(binaryPath)) {
  process.stderr.write(
    '[understatus] 네이티브 바이너리를 찾을 수 없습니다: ' + binaryPath + '\n' +
    '[understatus] 패키지를 다시 설치해 보세요: npm install -g understatus\n' +
    '[understatus] 문제가 지속되면 https://github.com/ictechgy/understatus/issues 에 신고해 주세요.\n'
  );
  process.exit(1);
}

// 네이티브 바이너리를 사용자 인수와 함께 실행
const result = spawnSync(binaryPath, process.argv.slice(2), {
  stdio: 'inherit',
  // 환경 변수를 그대로 전달
  env: process.env,
});

// spawnSync 자체 오류 처리 (실행 실패 등)
if (result.error) {
  process.stderr.write(
    '[understatus] 바이너리 실행 중 오류가 발생했습니다: ' + result.error.message + '\n'
  );
  process.exit(1);
}

// 네이티브 바이너리의 종료 코드를 그대로 전달
process.exit(result.status !== null ? result.status : 1);
