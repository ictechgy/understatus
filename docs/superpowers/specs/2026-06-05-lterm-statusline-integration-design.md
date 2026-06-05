# 설계 문서 — lterm command-backed status × understatus `render --source lterm`

> 작성일: 2026-06-05 · 제품: understatus(디렉터리명 `statusticon`은 레거시) + lterm(`light_terminal`)
> 상태: **ralplan 합의 재검토(deliberate) 반영 — `pending approval`** (사용자 spec 리뷰 + ANSI 기본값 1건 확인 대기)
> 범위: **Phase 1만** 본 문서로 확정. Phase 2는 §15 미리보기(별도 spec).
> 교차 레포: lterm = `/Users/jinhongan/Desktop/light_terminal`, understatus = 본 레포.

## 0. ralplan 재검토 요약 (이번 개정의 근거)

Planner→Architect→Critic 합의(deliberate, 보안 신뢰경계 신설 + 렌더 회귀 표면). **Verdict: ITERATE** —
설계 방향(아래 §4 "제3안")은 건전하여 재설계 불필요하나, 초안이 **기존 함수 계약 4개를 오인**해 그대로는
동작하지 않았다. 본 개정이 그 결함과 누락을 닫는다.

**초안의 치명 결함(코드로 확인, 본 개정에서 정정):**
- **A1**: understatus는 `cwd`로 git branch를 도출하지 **않는다**. `derive_git_branch`는 `workspace.git_worktree|repo`
  (워크트리 루트)를 받아 `.git/HEAD`를 읽는다(`claude.rs:94-160`). `$PWD` 폴백은 코드에 **없다**(허위 서술).
  lterm의 `SessionInfo.cwd`(`protocol.rs:72`)는 세션 시작 디렉터리라 워크트리 루트 보장도 없다.
  → **Phase 1에서 git branch 비활성**(§2/§4.1/§15).
- **A2/C3**: `understatus --source …`는 현재 디스패치에서 "알 수 없는 서브커맨드 → FAILURE"(`main.rs:30-64`).
  `run_render_pipeline()`은 무인자(`main.rs:391`), `has_extra_args`가 잉여 인자 거부(`main.rs:68-70`).
  → 인자 파서·시그니처·라우팅 **신설**(§6.1).
- **A4/C2**: 초안이 복제하라던 `spawn_status_metadata_thread`는 subprocess가 아니라 **데몬 RPC**
  (`client.rs:3409-3436`). 진짜 레퍼런스는 understatus `chain.rs::spawn_with_timeout`(`chain.rs:394-435`)인데,
  그건 `sh -c` **셸 실행**이라 §7의 argv-spawn 원칙과 **정반대**. → "타임아웃/reap 패턴만 차용, spawn은 argv"로
  분리 명시(§5.3).
- **A3/M1**: 신뢰경계 기준 함수는 `terminal_text`(C0/C1 char 필터, SGR 본문 잔존)도 `terminal_capture`(상태기계지만
  **SGR도 strip**)도 아니다. **SGR만 선택 허용하는 신규 상태기계**가 필요(§7).
- **M2**: AC가 "잔상 없음" 같은 시각 주장이라 테스트 불가 → **측정 가능 AC**로 교체(§10).
- **신규**: `agent` 도출 함수 `known_agent_name_from_command`는 **bool 반환**(이름 못 줌, `client.rs:1639`)
  → 이름-반환 헬퍼 신설(§6.4).

---

## 1. 배경 / 동기 (Context)

Codex CLI는 Claude Code식 **command-backed statusline**(스크립트가 stdin JSON을 받고 포맷 텍스트를 반환 →
하단 렌더)을 아직 지원하지 않는다(공식 FR `openai/codex#17827` 미구현). 따라서 understatus를 Codex 자체
하단 줄에 직접 주입할 길이 없다.

사용자는 자신이 만든 세션 데몬 **lterm**(tmux 유사, `lterm codex` 등 20+ 에이전트 래핑 내장 `main.rs:437-568`)
안에서 에이전트를 돌린다. lterm은 attach 시 **자체 1행 status bar를 직접 그린다**(화면 소유). "Codex가 안
만들어준 custom-statusline"을 **lterm 레이어에 범용으로** 구현하면, lterm 안에서 도는 모든 에이전트가
에이전트 자체 지원과 무관하게 understatus 줄을 얻는다.

### 현재 사실 (코드 근거)

- lterm status row 내용은 하드코딩: `src/client.rs:format_status_line(session_name, pane_id, width)`
  → `" lterm  {세션}  {페인} "`만 생성. status row에 외부 명령 출력을 넣는 메커니즘 없음.
- lterm은 자기 화면에 보이는 split을 직접 안 그림(cmux 위임). standalone attach = 단일 페인 + 1행 status →
  네이티브 표면은 그 status row가 유일 → **row 모드** 채택.
- 신뢰경계: lterm은 자기 status row에 그릴 바이트 안전성을 **그리는 쪽(lterm)이 직접 책임**진다(예:
  `main.rs:1043-1092`의 출력 직접 살균, `format_status_line`의 grapheme truncation). 이 철학을 본 설계도 따른다.
- understatus 코어 `render()`는 **정확히 1행**(`render.rs:47`). 멀티라인은 chain(OMC node HUD)에서만 옴.

## 2. 목표 (Goals · Phase 1)

1. **lterm**: 하단 status row를 **command-backed**로 확장 — 설정된 명령을 interval로 실행해 그 출력 1행을 렌더.
   명령 미설정 시 **기존 동작(세션+페인) 바이트 단위 동일**.
2. **understatus**: lterm 합성 JSON을 받는 **`render --source lterm`** 어댑터 + status row(1행)용 **`--oneline`**.
3. **검증**: `LTERM_STATUS_COMMAND="understatus render --source lterm --oneline" lterm codex` → 하단 1행에
   understatus 출력(시스템 정보 + 색), 리사이즈/복귀 시 본문 잔상 0(§10 AC로 측정).

> **목표에서 제외(초안 대비 정정):** git branch 표시 — Phase 1 계약(§4.1)이 branch 산출 입력을 보내지
> 않으며 understatus는 cwd로 branch를 못 만든다(A1). Phase 1은 **시스템 정보(cpu/mem/disk/net) + 세션/페인/
> 에이전트** 중심. git branch는 Phase 2(§15).

## 3. 비목표 (Non-Goals · Phase 1)

- **git branch 세그먼트** — A1으로 Phase 1 불가. `--source lterm`은 git 세그먼트 비활성.
- **pane(split) 모드** — Phase 2(cmux 의존 / OMX HUD-watch 플러밍).
- **TOML `[status]` config 파일** — lterm에 config 로더·`toml` 의존 없음 → Phase 1은 `LTERM_STATUS_*` env/flag.
- **Codex 세션 심층 판독**(`~/.codex/sessions`로 model/ctx/tokens) — Phase 2.
- **런타임 명령 변경** — env/flag는 프로세스 시작 시 고정. attach 중 변경 불가(Phase 2 TOML/IPC에서).
- **다중 에이전트 전용 세그먼트** — Phase 1은 에이전트 불문 공통 필드만.
- understatus 멀티라인을 row에 — row는 1행 고정.
- lterm 데몬 측 변경 — status는 attach **클라이언트**가 그림.

---

## 4. 아키텍처 — 채택안: "제3안(Synthesis)" + 원칙/드라이버

### 4.0 Principles (합의)
1. **신뢰경계는 lterm이 단독 통제(위임 불가)**: 자기 화면(reserved row)으로 나가는 바이트는 lterm의 안전
   게이트를 반드시 통과. 임의 `LTERM_STATUS_COMMAND`도 안전 보장을 받는다(범용 host 목표).
2. **기존 렌더 불변식 보존 우선**: scroll-region reserve / SGR-stack push·pop / `\x1b[0m` 경계 / 화면밖 row
   잔상 클리어(`client.rs` draw 경로)는 어떤 경로에서도 안 깨진다.
3. **책임 단일화**: 폭(width) 최종 권위는 **lterm 한 곳**. understatus 폭은 best-effort 힌트로 강등(이중 절단 모순 제거).
4. **명령 미설정 시 무변경**: `LTERM_STATUS_COMMAND` 없으면 기존 동작 비트 동일.
5. **실패 안전(fail-safe)**: spawn 실패/타임아웃/빈출력/non-UTF8/긴출력 모두 정의된 폴백으로 수렴 —
   패닉·좀비·블로킹 없음.

### 4.0.1 Decision Drivers (top 3)
- **D1**: status row 보안 회귀(ANSI 통과 = 신뢰경계 신설) — 가장 비싼 실패.
- **D2**: lterm 변경 난이도(타임아웃 subprocess 패턴이 lterm에 부재) — 적을수록 좋다.
- **D3**: 계약 정확도(초안이 함수 계약 4개 오인) — 코드 사실에 정합해야 동작.

### 4.0.2 채택 근거 (제3안 = "lterm 변경은 얇게, 단 신뢰경계 게이트만 두껍게")
- **lterm이 위임 불가로 직접 수행(안전 = 아키텍처 책임)**: ①첫 `\n`/`\r` 절단=1행 강제, ②SGR-only 화이트리스트
  (그 외 ESC/CSI/OSC/DCS/단독ESC/C1 차단), ③끝 `\x1b[0m` 강제 + 기존 보호장치 유지.
- **폭은 understatus 1차 힌트 + lterm 최종 권위(기능 = 위임 가능)**: 이중 폭 모델(understatus `is_wide` 휴리스틱
  `render.rs:341` vs lterm `unicode-width` `Cargo.toml`) 불일치를 lterm 단일 권위로 해소.
- 기각: **Option 1**(lterm full passthrough + 폭절단) — 보안코드 최대 + A4; **Option 2**(strip 기본·understatus
  폭책임) — §1 범용성/색 상실. (상세 비교 §13 ADR.)

### 4.1 아키텍처 다이어그램 & JSON 계약

```
┌─ lterm attach client (client.rs, status bar 소유) ─────────────────┐
│  spawn_status_command_thread(cmd_argv, interval)                   │
│     loop, 매 interval:                                              │
│        payload = build_status_payload(session, pane, agent, cwd, …) │
│        out = spawn_argv_with_timeout(cmd_argv, stdin=payload)       │  ← timeout/reap 패턴은
│        line = sanitize_status_command_line(out)  // 안전 게이트(§7)   │     chain.rs 차용, spawn은 argv
│        tx.send(line)                                                │
│  draw_at_size(): command_line: Option<String>                      │
│        Some(l) → l (게이트 통과본) / None → format_status_line(…)    │  ← 단일 draw 분기
└────────────────────────────────────────────────────────────────────┘
        │ stdin JSON                                ▲ stdout(ANSI 포함 가능)
        ▼                                           │
┌─ understatus render --source lterm --oneline ────────────────────────┐
│  parse_lterm_input(json) → 내부 모델  (parse_claude_input 대칭, lenient) │
│  sample_system() → cpu/mem/disk/net                                   │
│  render() → 코어 1행 (chain 미수행, git 세그먼트 비활성)                 │
└──────────────────────────────────────────────────────────────────────┘
```

**stdin JSON 계약 (lterm → understatus), version 협상 포함:**

```json
{
  "source": "lterm",
  "version": 1,
  "session": "codex",
  "pane": "%3",
  "session_key": "codex/%3",
  "agent": "codex",
  "cwd": "/Users/me/dev/app",
  "cols": 120,
  "rows": 40
}
```

- `version`: understatus는 `#[serde(default)] version: Option<u32>`로 **읽되 Phase 1은 분기 없이 무시**(forward-compat).
- `session_key`: understatus 펄스 히스테리시스·캐시 격리용 안정 키(예 `"<session>/<pane>"`). 없으면 understatus가
  `session/pane` 조합으로 합성. **다중 attach 시 캐시 경합 방지**(Planner B5).
- `cwd`: Phase 1에서는 **표시용으로만** 사용(git 도출 안 함). `$PWD` 폴백 없음(허위 문구 삭제).
- `agent`: best-effort, 미상이면 생략/`null`(§6.4 신규 헬퍼로 추출).
- `cols`/`rows`: understatus 폭 맞춤 **힌트**. 최종 폭 권위는 lterm(§7).
- understatus는 **미상 필드 무시 + 빈 `{}` 무패닉**(기존 `parse_claude_input` 철학).

---

## 5. lterm 측 변경 (Phase 1)

`src/client.rs` 중심 + 신규 유틸은 `src/sanitize.rs`.

### 5.1 신규 env/flag (기존 `LTERM_*` 관례)

| 키 | 기본 | 의미 |
|---|---|---|
| `LTERM_STATUS_COMMAND` | (없음) | 설정 시 status 내용을 이 명령 출력으로 대체. 미설정 시 기존 동작. |
| `LTERM_STATUS_INTERVAL` | `2` | 재실행 주기(초). **하한 1초** 클램프, 상한 `3600` 클램프(폭주 방지). |
| `LTERM_STATUS_ANSI` | `1`(확정 대기 §14) | `1`=SGR-only 통과, `0`=전량 strip(기존 plain). |
| `LTERM_STATUS_DEBUG` | `0` | `1`이면 명령 실패/타임아웃을 stderr 1줄 로그(silent failure 방지, observability). |

- (선택) `lterm attach --status-command "..."` 플래그 별칭. 우선순위: 플래그 > env.
- 명령 파싱: **`shlex::split`**(lterm Cargo.toml에 이미 존재)으로 argv 분해 → `Command::new(argv[0]).args(argv[1..])`
  **셸 미경유**(인젝션 회피). `shlex::split` 실패(따옴표 미닫힘 등) → fallback(세션+페인) + DEBUG 로그.

### 5.2 `StatusBar` 변경 (단일 draw 분기)

- 필드 추가: `command_line: Option<String>` — 게이트(§7)를 이미 통과한 1행. **`None`=명령 미설정/첫 성공 전/
  실패+직전성공없음 → `format_status_line` fallback. `Some`=그 값.**
- `draw_at_size`: `safe_width` 산출 후 한 곳에서 분기:
  - `Some(line)` → 콘텐츠 = `line`. **ANSI 모드(`=1`)에서는 테마 배경 SGR(`{sgr}`)을 적용하지 않고** `\x1b[0m`로
    시작(이전 rendition 차단), `\x1b[0m\x1b[K`로 끝(색 누수 차단). `ANSI=0` 또는 fallback이면 기존 테마 bg 유지.
  - `None` → 기존 `format_status_line(session, pane, safe_width)` 경로 그대로.
  - 두 경우 모두 기존 보호장치 유지: scroll-region reserve, `\x1b[2K` 선클리어, cursor save/restore(`\x1b7…\x1b8`),
    SGR-stack push/pop(기존 `preserve_sgr_stack` 게이트 그대로 — dumb 터미널은 skip), 화면밖 row 잔상 클리어.

### 5.3 명령 실행 스레드 (레퍼런스 분리 명시)

- `spawn_status_command_thread(...)` — **채널/적용 패턴은** `spawn_status_metadata_thread`+
  `apply_pending_status_metadata`(mpsc `try_recv` 최신만)의 동형. **단 RPC가 아니라 subprocess**다.
- **subprocess 실행 = understatus `chain.rs::spawn_with_timeout`에서 다음만 차용**:
  try_wait 폴링 타임아웃 + `kill`/`wait` 좀비 회수. **spawn 방식은 차용하지 않음**(chain은 `sh -c`, 여기선 argv).
- 실행 순서(데드락 방지): stdin에 payload write → **stdin drop으로 EOF** → try_wait 폴링(타임아웃
  = `min(interval, 1500ms)`) → 종료 후 stdout 수집. 자식이 stdin 미소비해도 종료 후 수집이라 PIPE 데드락 회피.
- **stdout 캡처 상한 64KB**(긴 출력 메모리 폭증 차단). 초과분 절단(어차피 §7에서 첫 1행만 사용).
- 비-UTF8 → `from_utf8_lossy`(`\u{fffd}`).
- 실패/타임아웃/빈·공백-only 출력 → **직전 성공 라인 유지, 없으면 fallback** + DEBUG 로그. understatus 부재
  (spawn 실패) 반복 시 동일(무한 시도하되 매 tick fallback 표시; 백오프는 Phase 2).
- 다중 클라이언트가 같은 세션 attach → 각자 자기 스레드(Phase 1 허용). 캐시 경합은 `session_key`(§4.1)로 격리.

---

## 6. understatus 측 변경 (Phase 1)

### 6.1 인자 모델 / 디스패처 (확정: render 서브커맨드 + 플래그)

- 호출형: `understatus render --source <claude|lterm> --oneline` (기본 `--source claude`). 무인자/`render`도 허용.
- 작업 항목(초안의 "분기 추가"를 정정 — 3개):
  1. **render 경로용 플래그 파서 신설**(clap 미사용, 자체 파싱). `--source`/`--oneline` 순서 무관, 미지값은
     에러(`ExitCode::FAILURE`, 기존 `Some(other)` 관례 `main.rs:61-63`).
  2. **`run_render_pipeline(source: Source, oneline: bool)` 시그니처 확장**(현재 무인자 `main.rs:391`).
  3. **디스패처/`has_extra_args` 갱신**: `render` 뒤 플래그를 잉여로 오판하지 않도록(`main.rs:30-70`).

### 6.2 `--source lterm`

- `src/claude.rs::parse_claude_input` 대칭으로 **`parse_lterm_input(raw)`** 신설(별도 함수, lenient).
- lterm JSON → 내부 모델 매핑: session/pane/session_key/agent/cwd. **git 세그먼트 비활성**(`show_git=false` 강제).
  `$PWD` 폴백 코드 없음 — 추가하지 않음(Phase 1).
- 이후 파이프라인(`sample_system → render`) 공유.

### 6.3 `--oneline`

- chain_command **미수행**, 코어 `render()` 1행만, **후행 개행 없이** 출력. `--source lterm`은 chain 기본 off.
- `cols`/`rows`가 오면 `max_width` **힌트**로만 참고(강제 절단 안 함 — 최종 권위 lterm §7).
- 기존 ANSI 불변식 유지(`render_has_no_bold_escape` 등). 코어는 이미 truecolor·1행·bold 미사용.

### 6.4 `agent` 필드 추출기 (신규)

- `known_agent_name_from_command`(bool, `client.rs:1639`)는 이름을 못 주므로, **이름-반환 헬퍼 신설**
  (`effective_command_executable_index` 등 기존 토큰 로직 재활용). 이건 **lterm 측** 변경(payload 합성용).
  미상이면 `agent` 생략.

---

## 7. 안전성 — `sanitize_status_command_line` (신규 상태기계, lterm)

**기존 함수 재사용 불가**: `terminal_text`/`strip_controls`는 char 필터(SGR 본문 잔존), `terminal_capture`는
상태기계지만 **SGR도 strip**. 따라서 **"SGR만 선택 허용"하는 신규 상태기계**를 작성한다.

요구 동작(전부 AC로 박제):
1. **단일 행 강제**: 첫 `\n`/`\r`에서 절단 → 1행만(다행이 scroll-region을 깨지 못하게).
2. **SGR-only 화이트리스트**: `CSI <params> m`만 통과. 구체:
   - CSI 진입: `\x1b[` **그리고** C1 형태 `\x9b`/`\u{009b}` 양쪽(`terminal_capture`가 처리하는 경로). C1-form
     SGR는 통과 시 **정규 `\x1b[`-form으로 정규화**(레거시 터미널의 raw C1 오해석 방지).
   - 파라미터 바이트 `0x30–0x3f`(숫자/`;`/`:` 등) + intermediate `0x20–0x2f` 누적.
   - **종료 바이트가 `m`이면 통과, 그 외(`H`,`J`,`K`,`A`…)면 시퀀스 전체 폐기.**
   - **미완결 CSI**(`\x1b[31`+EOF)·OSC(`\x1b]`)·DCS·단독 ESC·기타 제어문자 → 폐기.
   - 파라미터 **길이/개수 상한**(예: 64바이트/16개) 초과 시 그 시퀀스 폐기(파서 폭주 방지).
3. **ANSI-aware 폭 절단(최종 권위)**: SGR 바이트는 폭 계산 제외, grapheme/CJK/이모지 폭으로 `safe_width`까지 절단.
   **SGR 시퀀스 중간에서 절대 자르지 않음**(완결 SGR + grapheme 경계에서만). 절단 시 `…` 정책은 기존
   `format_status_line`과 동일.
4. **색 닫힘 보장**: 출력 끝에 반드시 `\x1b[0m`.
5. **non-UTF8**: 입력은 `from_utf8_lossy` 변환, `\u{fffd}`는 width 1.
6. **방어적 신뢰 경계**: understatus 출력도 신뢰하지 않는 입력으로 취급해 위 전부 적용.

`LTERM_STATUS_ANSI=0`이면 2 대신 **전량 strip**(=`terminal_capture` 동등 plain), 1·3·4·5는 유지.

---

## 8. 렌더링 상호작용 / 회귀 주의

- **테마 배경 vs ANSI**: ANSI 모드(`=1`)에선 테마 bg를 끄므로 status row가 understatus 색 위주가 됨(의도, §14 확인 대기).
- **`\x1b[K` 색 번짐**: understatus가 미닫힌 색을 줘도 §7-4의 끝 `\x1b[0m`가 닫고 그 뒤 `\x1b[K`가 본문으로 색을
  안 흘리게 순서 고정(`…{line}\x1b[0m\x1b[K`).
- **scroll-region/self-heal**: `reserve_terminal_area`, 화면밖 row 잔상 클리어(`drawn_status_rows`/
  `visible_previous_status_rows`), cmux/Termius 리사이즈 복귀 경로는 그대로 통과(command_line도 동일 draw 파이프라인).
- **deferred-wrap 회피**: 기존처럼 `cols-1`만 그림. §7-3 절단도 이 `safe_width` 기준.

## 9. 성능 / Observability

- interval-gated(기본 2초) + 클라이언트 측 1스레드. draw는 캐시 라인만 사용 → 리드로우 비용 무변.
- understatus `sample_system`은 매 tick 비용(더블샘플 CPU 등 `system.rs`). **N=1,2,4 클라이언트 × 2초에서 추가
  CPU·tick 시간을 측정해 §9에 수치 기록**(막연한 "무시 가능" 금지 — AC-P1).
- Observability: `LTERM_STATUS_DEBUG=1`이면 명령 실패/타임아웃/shlex 실패를 stderr 1줄로(silent failure 방지).

## 10. 테스트 / 검증 (측정 가능 AC)

**Unit (understatus)**
- `parse_lterm_input`: 정상/빈`{}`/미상필드/누락 → 무패닉·정확 매핑, `session_key` 합성, git 비활성.
- 플래그 파서: `render --source lterm --oneline`(순서 무관) → **ExitCode::SUCCESS**(초안은 FAILURE) · 미지값 에러.
- `--oneline`: 출력 **정확히 1행, 후행 개행 0**, chain 미수행. cols 힌트가 강제 절단 안 함(권위는 lterm).
- 기존 불변식: `render_has_no_bold_escape`, `render_no_color_env_has_no_escape_bytes` 유지.

**Unit (lterm)**
- `sanitize_status_command_line`: SGR통과 / 종료바이트 `m` 아닌 CSI 차단 / intermediate 포함 차단 / **미완결 CSI 폐기**
  / **C1 `\x9b`·`\u{009b}` SGR 통과(정규 `\x1b[`-form으로 정규화)·非SGR 차단** / OSC·DCS·단독ESC 차단 /
  파라미터 길이·개수 상한 / 다행→1행 /
  CJK·이모지 폭절단 / **임의 폭 절단 후 미완 ESC 0(프로퍼티 테스트)** / 끝 `\x1b[0m` 강제 / non-UTF8 `\u{fffd}` width1
  / `ANSI=0` 전량 strip.
- `command_line` 상태: None=fallback / 첫 interval 이내 draw = fallback과 **바이트 동일** / `LTERM_STATUS_COMMAND=false`
  (즉시 실패) → fallback 바이트 동일(AC-E3) / 빈·공백 출력 → fallback.
- subprocess 가드: 타임아웃 kill+wait로 **좀비 0** / 64KB 초과 출력 절단 / shlex 실패 fallback.
- 기존 status truncation 회귀군 무회귀.

**Integration**
- `echo '{"source":"lterm","session":"s","pane":"%1"}' | understatus render --source lterm --oneline`
  → exit 0 · 정확히 1행 · 끝 `\x1b[0m` 닫힘 · git 세그먼트 없음.

**E2E**
- **AC-E1**: `LTERM_STATUS_COMMAND="understatus render --source lterm --oneline" lterm codex` 후 PTY 캡처
  (script/expect/vt100 파서) → status row가 정확히 1행, 끝이 `\x1b[0m`로 닫힘.
- **AC-E2**: 리사이즈(80x24→120x40→80x24) 주입 후 본문 영역 rows에 status SGR/콘텐츠 잔재 **0**(lterm 보유
  `vt100` 크레이트로 파싱한 스냅샷 diff).
- **AC-E3**: `LTERM_STATUS_COMMAND=false` → fallback이 `format_status_line` 출력과 바이트 동일.
- **AC-M**: `NO_COLOR` × `LTERM_STATUS_ANSI`(0/1) 2×2 매트릭스 동작 정의.

**Observability/Perf**
- **AC-P1**: N=4 클라이언트에서 understatus 추가 CPU·tick 시간 측정값 기록(임계 수치는 측정 후 확정).

**툴체인**: 두 레포 모두 rustup `~/.cargo/bin/cargo`로 `test`/`clippy -D warnings`/`fmt --check`
(homebrew cargo 깨짐 — `export PATH="$HOME/.cargo/bin:$PATH"`). x86_64 크로스빌드는 CI(macos-14).

## 11. Pre-mortem (deliberate — 실패 시나리오 → 대응 → 복구)

**S1 — "git branch가 항상 빈다"** *(원인: A1, cwd≠워크트리 루트)*
→ 대응: Phase 1에서 git 세그먼트 **의도적 비활성**(§3), 목표에서 제거(§2). → 복구: Phase 2 walk-up 구현.
→ 조기경보: `--source lterm` integration에 git 세그먼트 부재 확인(있으면 회귀).

**S2 — "특정 터미널에서 status가 본문을 먹거나 색이 번진다"** *(원인: 미완 ESC 누출 / `\x1b[K` 색번짐)*
→ 대응: §7-3 "SGR 중간 절단 금지" + §7-4 끝 `\x1b[0m` + §8 순서 고정. → 복구: AC-E2 스냅샷 회귀 + ANSI draw 경로 전용 테스트.
→ 조기경보: 리사이즈 후 본문 잔상/색 누수(AC-E2 실패).

**S3 — "다중 attach에서 펄스 폭주 / CPU 스파이크"** *(원인: 캐시 경합 + N×샘플링)*
→ 대응: `session_key`로 캐시 격리(§4.1) + interval 하한·상한 클램프(§5.1). → 복구: AC-P1 측정·임계.
→ 조기경보: 2+ attach 시 펄스 불안정, understatus CPU 스파이크.

**누락 케이스 흡수(C1~C12 → 본문 반영):** 빈출력→fallback(§5.3) · 좀비→kill/wait(§5.3) · 긴출력→64KB(§5.3) ·
non-UTF8→lossy(§7-5) · NO_COLOR×ANSI→AC-M(§10) · 첫프레임→None fallback(§5.2) · 런타임 명령변경→비목표(§3) ·
understatus 부재→fallback+DEBUG(§5.3) · 실행 env/cwd→argv spawn(§5.1) · stdin 블로킹→write+drop 순서(§5.3) ·
shlex 실패→fallback(§5.1) · interval 하한/상한→클램프(§5.1).

## 12. 레포 분할 / 순서

1. **계약 고정**(§4.1 JSON, §5.1 env) — 양쪽 합의 seam.
2. **understatus**: `render --source lterm` + `--oneline`(독립 검증 — `echo … | understatus render --source lterm --oneline`).
3. **lterm**: `sanitize_status_command_line`(§7) → 명령 스레드(§5.3) → `StatusBar.command_line`(§5.2) → draw 통합
   → `agent` 추출 헬퍼(§6.4).
4. **E2E**(§10 AC-E/M/P).

각 레포는 자기 `feature/…` 브랜치, main 직접 커밋 금지, lint/타입/테스트 통과 후 PR.

## 13. ADR

- **Decision**: lterm status row를 command-backed로 확장하되 **"제3안(Synthesis)"** 채택 — lterm은 신뢰경계 안전
  게이트(1행 강제·SGR-only·끝 `0m`)를 **직접 두껍게** 통제하고, 폭은 understatus 힌트 + lterm 최종 권위. 데이터는
  lterm JSON → understatus `render --source lterm`. Phase 1 git branch 비활성.
- **Drivers**: D1(보안 회귀) > D2(lterm 난이도) > D3(계약 정확도).
- **Alternatives considered**: Opt1(lterm full SGR passthrough+폭절단) — 색 최대치나 보안코드 최대+A4;
  Opt2(strip 기본·understatus 폭책임) — 안전/얇음이나 §1 범용성·색 상실; Opt3 원형(SGR통과+폭 분할) — 이중 폭
  모델로 가장 깨지기 쉬움.
- **Why chosen**: 제3안이 §1 범용 host 목표(임의 명령도 게이트로 보호)와 코드베이스 철학(자기 화면은 lterm이
  통제)을 동시에 보존하면서, 폭 단일 권위로 이중 절단 모순을 제거. Opt1보다 가볍고 Opt3보다 안전.
- **Consequences**: (+) lterm 변경 표면 한정, 보안 게이트 일원화, 범용 소비자 지원. (−) lterm에 SGR-선택-허용
  상태기계라는 보안 민감 코드 신설(테스트로 방어). git branch는 Phase 1 미제공(정직한 축소).
- **Follow-ups**: §14 ANSI 기본값 사용자 확인 / Phase 2 git walk-up·pane·TOML·Codex 세션판독 / interval 상한
  정책 / PTY 포그라운드 cwd 취득 가능성 조사.

## 14. 열린 결정 (사용자 입력 필요)

1. **`LTERM_STATUS_ANSI` 기본값** — `1`(understatus 색 위주, §1 범용성·색 가치) **권장** vs `0`(기존 "차분한
   파란 띠" 유지). **순수 미적 취향이라 사용자 확정 필요.** (게이트는 두 경우 모두 항상 on이라 안전성은 동일.)

(비차단·디폴트 확정: interval 상한 3600s, 캡처 64KB, 타임아웃 `min(interval,1500ms)`. 필요 시 조정.)

## 15. Phase 2 미리보기 (이번 범위 아님)

- git branch: `--source lterm`에서 cwd→`.git` walk-up(워크트리 루트 도출) 또는 payload에 `git_worktree` 추가.
- `mode=pane`: cmux split / OMX HUD-watch 플러밍 재사용 → full truecolor 다행.
- TOML `[status]` config 파일(lterm config 로더 신설 시) + 런타임 변경.
- `~/.codex/sessions/*.jsonl` 심층 판독 → model/ctx/tokens/rate-limit.
- Gemini/OpenCode 등 다중 에이전트 세그먼트.
- spawn 실패 백오프, status row 테마 bg 옵션화(ANSI 모드 bg 유지 선택).
