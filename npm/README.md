# understatus

Claude Code용 macOS statusline 애드온.

CPU, 메모리, 세션 정보를 calm glyph 테마(○▁▄▆◆)로 표시합니다.

## 설치

```bash
npm install -g understatus
```

macOS (Apple Silicon 및 Intel) 전용입니다. 설치 시 GitHub Releases에서 네이티브 바이너리를 자동으로 다운로드합니다.

## 사용법

```bash
# 현재 상태를 statusline 형식으로 출력
understatus render

# Claude Code settings.json에 비파괴적으로 설치
understatus install

# Claude Code settings.json에서 제거
understatus uninstall
```

## 요구 사항

- macOS (arm64 / x64)
- Node.js >= 16

## 링크

- [GitHub 저장소](https://github.com/ictechgy/understatus)
- [릴리즈](https://github.com/ictechgy/understatus/releases)
- [이슈 신고](https://github.com/ictechgy/understatus/issues)

## 라이선스

MIT
