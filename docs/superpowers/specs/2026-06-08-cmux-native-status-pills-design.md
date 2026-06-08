# 설계 문서 — lterm `DelegatedSurface(Cmux)` 실렌더: cmux 네이티브 status pills

> 작성일: 2026-06-08 · 제품: understatus(디렉터리명 `statusticon`은 레거시) + lterm(`light_terminal`)
> 상태: **pending approval** — ralplan 합의 1라운드 반영(Critic ITERATE → pill 구성 재결정·색 매핑·panic 보장·R9·인용 교정). 사용자 spec 리뷰 대기.
> 범위: lterm status 라우팅의 `DelegatedSurface(Cmux)` 백엔드 **실렌더 배선**. 다른 미구현 백엔드(NativeChrome/TitleCueDelegation)는 비범위.
> 교차 레포: lterm = `/Users/jinhongan/Desktop/light_terminal`, understatus = 본 레포.
> 선행: 라우팅 인프라(`select_status_backend`)는 lterm PR #121(PoC)+#122(배선)로 머지됨(behavior-preserving). 본 문서는 그 위에 실렌더를 얹는다.

## 0. 요약 (TL;DR)

lterm-in-cmux에서 도는 에이전트(codex 등)의 status를, **인그리드 DECSTBM 행 대신 cmux 네이티브 사이드바 pill**로 렌더한다. 기존 status-command 콘텐츠 파이프라인(`run_status_command`)을 그대로 재사용하되, 출력을 `build_draw_body`(DECSTBM) 대신 신규 **`CmuxStatusSink`**로 라우팅한다. understatus는 SGR 한 줄 대신 **pill JSON 배열**을 내는 두 번째 출력 모드(`--surface-format cmux-status`)를 얻는다. lterm은 그 배열을 직전 상태와 diff해 `cmux set-status/clear-status/set-progress`를 구동한다.

**본 트랙이 푸는 본질 문제**: DECSTBM 인그리드 행은 codex TUI의 scroll-region과 단일 전역 자원을 두고 충돌해 **입력칸 손상**을 일으킨다(반복된 페인 포인트). cmux pill은 셀 그리드 밖(cmux 자체 chrome)에 렌더되어 **scroll-region 상호작용이 구조적으로 0**이다.

## 0.1 사전 조사로 확정된 사실 (라이브 검증, cmux 0.64.14)

- cmux 네이티브 status API 존재·동작 확인:
  - `cmux set-status <key> <value> [--icon <name>] [--color <#hex>] [--priority <n>] [--workspace <ref>]` → 좌측 사이드바 워크스페이스 탭 행에 **pill** 렌더.
  - `cmux clear-status <key>`, `cmux list-status`(출력 `key=value icon=<name> color=<#hex> priority=<n>`), `cmux set-progress <0.0-1.0> [--label]`, `cmux clear-progress` 동작 확인.
  - 키별로 도구가 자기 항목 관리(cmux 공식 예시 키: `claude_code`, `build`). → **pane 스코프 키 네임스페이스로 충돌 차단 가능**.
- **워크스페이스 식별 함정(라이브 확인)**:
  - 환경변수 `$CMUX_WORKSPACE_ID`는 lterm 세션에서 **stale**(부모 셸 통합 env보다 오래 삶) → `set-status`는 `OK`를 반환하지만 **죽은 워크스페이스에 써서 안 보임**, `list-status`는 "Tab not found".
  - `cmux identify --json`의 `focused.workspace_ref`(예 `workspace:7`)는 **포커스 따라 드리프트** → 테스트 시 엉뚱한 워크스페이스("cleaner")에 pill이 붙음.
  - lterm은 이미 `open_cmux_split`(`tmux_compat.rs:1538`)에서 split 생성 시점에 `cmux_identify_surface()`로 포커스 결정적 컨텍스트를 캡처·저장(`CmuxSurfaceContext`)하는 동형 문제를 해결함. → **attach 시점 1회 캡처** 패턴 재사용.
- `cmux new-split`에 **크기 플래그 없음** → split-렌더러 방식은 화면 ~50% 점유. native pill이 우월.

## 0.2 사용자 확정 결정 (브레인스토밍)

| 결정 | 값 |
|---|---|
| 백엔드 트랙 | **Cmux 우선**(DelegatedSurface(Cmux)) |
| pill 구성 | **`model` · `ctx` · `cpu` · `mem`** (codex 경로 실가용. cost·git은 lterm 파서에서 구조적 None `claude.rs:288-289`이라 제외 — 띄우려면 별도 payload 확장 트랙) |
| ctx 표현 | **진행바 + pill 둘 다** |
| cmux 호출 실패 시 폴백 | **비표시(blackout) + stderr 경고 1줄**(DECSTBM 폴백 안 함 → codex 충돌 재발 방지) |
| 키 네임스페이스(기본값) | `lterm.<pane>.<seg>` (pane 스코프, sanitized) |
| 업데이트 간격(기본값) | 기존 `LTERM_STATUS_INTERVAL`(2s) 재사용 + **diff 게이트**(바뀐 pill만 스폰) |
| 아이콘(기본값) | 최소 베스트에포트(`model`만 `sparkles`, cpu/mem은 무아이콘. 미지원 아이콘명은 cmux가 무시) |
| pill 색(기본값) | **pill 전용 색 매핑 신규 정의** — understatus 값 세그먼트엔 색이 없음(`render.rs:267-273`). cpu는 `band_tint` 재사용, 나머지는 신규 |
| 워크스페이스 식별자 | **UUID 우선 캡처**(`--id-format uuids`); positional ref는 재번호화 드리프트 위험 |

---

## 1. 목표 (Goals)

1. lterm-in-cmux에서 status_backend가 `DelegatedSurface(Cmux)`일 때, understatus 콘텐츠를 **cmux 사이드바 pill 4개 + 진행바**로 렌더한다.
2. 그 경로에서 **DECSTBM 인그리드 행을 그리지 않는다**(풀 rows를 PTY에 전달). → codex 입력칸 손상 제거.
3. pill **누수 없음**: 정상 detach/종료/패닉/하드킬(다음 attach 복구) 전 경로에서 자기 pill을 청소한다.
4. **다중 세션 공존**: 한 워크스페이스에 여러 lterm 세션이 있어도 pill 키가 충돌하지 않는다.
5. **스폰 비용 통제**: 유휴 상태에서 cmux 서브프로세스 스폰 ≈ 0(diff 기반).
6. cmux 미설치/비-cmux 환경/호출 실패에서 **안전 저하**(비표시+경고, 충돌 재발 없음).
7. understatus 변경은 **additive-optional**: 구버전 understatus와도 깨지지 않는다.

## 2. 비목표 (Non-Goals)

- NativeChrome(iTerm OSC1337), TitleCueDelegation 실렌더(별도 트랙).
- DelegatedSurface(**Tmux**) — 본 문서는 Cmux만. (tmux 동형은 후속.)
- cmux `rpc` 배치 경로(스폰 최적화) — 본 범위는 CLI 경로. sink의 apply 레이어를 **pluggable**하게 두어 후속 배치 경로가 diff 로직을 안 건드리고 들어오게만 설계(§5.4).
- understatus 풀세트/시스템 pill — 사용자 결정에 따라 핵심 4개만.
- git branch 도출 방식 변경 — 기존 understatus git 도출 계약 그대로 사용(없으면 `git` pill None-skip).

---

## 3. 아키텍처

### 3.1 데이터 흐름

```
[기존 interval 루프 · 재사용]
 spawn_status_command_thread (client.rs:4062)
   └ build_status_payload (client.rs:3871)  ── stdin JSON, 신규 필드 "surface_format":"cmux-status"
   └ run_status_command   (client.rs:3932)  ── argv-spawn(no-shell), reader 스레드+데드라인
        │ stdout = (oneline SGR) | (cmux-status pill JSON)   ← surface_format에 따라
        ▼
 [understatus] run_render_pipeline (main.rs:569)
   └ 기존 수집(parse_lterm_input + codex::enrich + system::sample) 그대로
   └ surface_format==cmux-status → to_cmux_pills() → JSON 1줄 출력
        │ {"schema":"cmux-status","version":1,"pills":[...],"progress":{...}}
        ▼
 [lterm 소비점] client.rs:3421 부근(apply_pending_status_command), backend 분기
   └ DelegatedSurface(Cmux) → CmuxStatusSink.apply(json)   (StatusBar::refresh 미진입)
        └ 파싱 → 직전 applied와 diff → 변경분만:
              cmux set-status lterm.<pane>.<key> <value> --color --icon --priority
              cmux clear-status lterm.<pane>.<key>   (사라진 세그먼트)
              cmux set-progress <v> --label / clear-progress
```

### 3.2 컴포넌트 (신규/변경)

**lterm**
- `StatusBackend::reserves_in_grid_row(&self) -> bool` (신규 메서드, `client.rs:4475` enum):
  - `DecstbmOverlay` → `true`; `Disabled | NativeChrome | DelegatedSurface(_) | TitleCueDelegation` → `false`.
- 라우팅 정합성 수정(`client.rs:3096`): 인그리드 게이트와 sink 게이트 분리(§4.1).
- `tmux_compat::cmux_status_identity(pane_id) -> Option<CmuxSurfaceContext>` (신규):
  - **stored split-time 컨텍스트 우선**(`stored_cmux_surface_for_pane`, `tmux_compat.rs:2389`) → 없으면 **attach 시점 `cmux identify` 폴백**. **UUID 우선** 캡처.
  - `CmuxSurfaceContext`를 `pub(crate)`로 승격(`tmux_compat.rs:2155`; `inside_cmux`가 받은 동일 처치).
- `CmuxStatusSink` (신규, lterm):
  ```
  struct CmuxStatusSink {
      workspace: CmuxSurfaceContext,          // attach 시점 1회 캡처(UUID 우선)
      key_prefix: String,                     // "lterm.<pane>."  (sanitized)
      applied: BTreeMap<String, PillState>,   // diff용 직전 적용 상태
      progress_applied: Option<ProgressState>,
      cmux_available: bool,                    // 생성 시 command_exists("cmux")
      healthy: bool,                           // 연속 실패 시 false(서킷 브레이커)
  }
  ```
  - `apply(&mut self, json: &str)`: pill JSON 파싱 → diff → cmux 명령. non-JSON(구버전 oneline) 수신 시 **무해 no-op**(version 하드게이트 안 함).
  - `Drop`: 전 키 `clear-status` + `clear-progress`(`ManagedAttachGuard::drop` 모델, `tmux_compat.rs:1744`).

**understatus**
- `--surface-format <oneline|cmux-status>` 플래그(`parse_render_args`, `main.rs:115`), 기본 `oneline`.
- `collect_segments`(`render.rs:75`)의 핵심 세그먼트(model/ctx/cpu/mem) push 지점에 **additive `pill: Option<PillMeta>`** 부착(전면 IR 리팩터 회피 → oneline SGR 바이트 불변). 같은 `collect_segments` 호출을 두 경로가 공유해 세그먼트 가용성 드리프트 방지.
- `to_cmux_pills(...)`: `pill.is_some()` 세그먼트만 → pill JSON. **색은 pill 전용 신규 매핑**(understatus 값엔 색 없음 `render.rs:267-273`. cpu는 `band_tint`/`glyph_tint` `render.rs:244-246` 재사용, 나머지는 신규 `#RRGGBB`).

### 3.3 JSON 계약

**stdin (lterm → understatus)** — 기존 페이로드에 1필드 추가(additive-optional):
```json
{"source":"lterm","version":1,"surface_format":"cmux-status",
 "session":"codex","pane":"%3","session_key":"codex/%3","agent":"codex",
 "cwd":"...","cols":120,"rows":40}
```
- understatus는 미상 필드를 `serde_json::Value`로 관대 처리(타입드리프트 안전, `claude.rs:547-571`). 구버전 understatus는 `surface_format` 무시 → oneline 출력 → lterm sink가 non-JSON 감지해 무해 처리.

**stdout (understatus → lterm)** — pill 배열(1줄):
```json
{"schema":"cmux-status","version":1,
 "pills":[
   {"key":"model","label":null,"value":"gpt-5.5","color":"#7AA2F7","icon":"sparkles","priority":60},
   {"key":"ctx","label":null,"value":"ctx 42%","color":"#34D399","icon":null,"priority":50},
   {"key":"cpu","label":null,"value":"cpu 31%","color":"#9ECE6A","icon":null,"priority":100},
   {"key":"mem","label":null,"value":"mem 48%","color":"#A0A0A0","icon":null,"priority":90}
 ],
 "progress":{"value":0.42,"label":"ctx 42%"}}
```
- `key`는 **prefix 없는** 세그먼트 id. **lterm이 `key_prefix`를 앞에 붙임**(understatus는 pane 모름 → 순수성 유지).
- ctx는 **둘 다**: `progress`(진행바) + `pills[]`에 `{"key":"ctx","value":"ctx 42%",...}` 동시 포함(사용자 결정).
- 소스 없음(`None`) 세그먼트는 pill 미생성 → lterm diff가 `clear-status`.

### 3.4 세그먼트 → pill 매핑 (model·ctx·cpu·mem — codex 경로 실가용)

| understatus 세그먼트(render.rs) | cmux 출력 | 색 소스 | 비고 |
|---|---|---|---|
| model (`render.rs:144`, prio 60) | pill `model` (icon `sparkles`) | 신규 accent | codex/claude 세션, bare value |
| ctx% (`render.rs:153`, prio 50) | **progress + pill `ctx`** | 신규 band | 둘 다(사용자 결정). 진행바 값·pill 텍스트 모두 **정수 %로 양자화**(§4.4) |
| cpu (`render.rs:87`, prio 100) | pill `cpu` | **`band_tint` 재사용**(`render.rs:244`) | 시스템 샘플링(소스 무관) |
| mem (`render.rs:101`, prio 90) | pill `mem` | 신규 중립 | 시스템 샘플링(소스 무관) |

**색은 pill 전용 신규 매핑**(model=accent, ctx=band, cpu=`band_tint` 재사용, mem=중립). understatus 값 세그먼트엔 색이 없으므로(`render.rs:267-273` COLOR-ONCE) "기존 색 재사용"은 불가 — cpu만 glyph band_tint를 재사용한다.
**가용성(조건부 명시 — Architect R2 / Critic M1)**: `cpu`·`mem`은 `system::sample`로 **무조건 가용**(소스 무관). `ctx`는 **codex enrich 성공 시만 가용**(`codex.rs:862`; Claude 직접 경로 `claude.rs:282` None). `model`은 enrich 성공 시 풍부한 이름, **실패 시 bare agent 토큰('codex')으로 폴백 표시**(`claude.rs:281` `model_display_name: raw_input.agent`, `config.rs:221` show_model 기본 on — oneline과 동일 일관성). → **enrich-성공 = model·ctx·cpu·mem (4 pill), enrich-실패 = model·cpu·mem (3 pill, ctx만 None-skip)**. AC는 양 상태를 **이 개수**로 단언(2가 아니라 3).
**(범위 밖)** git·cost는 lterm 파서가 영구 None(`claude.rs:288-289`, codex enrich도 미채움 `codex.rs:856-864`)이라 본 트랙에서 제외. 향후 payload 확장 트랙에서 부활 가능.

---

## 4. lterm 측 변경 (상세)

### 4.1 라우팅 정합성 수정 (본 트랙의 본질 — Architect R3 BLOCKER 반영)

현행 `status_enabled = backend != Disabled && requests_row()`(`client.rs:3096-3098`)의 두 현상:
- **(active 세션)** RowAuto(셸)·ForceRow가 cmux에 있으면 `requests_row()==true` → `attach_pty_rows(rows,true)` rows-1 예약 + `StatusBar` DECSTBM → codex류 scroll-region 충돌(없애려는 손상).
- **(agent 세션)** codex 등은 `show_status:false`(`main.rs:2574`)/`likely_agent_session`로 **RowOff** → `requests_row()==false`(프로필 경로 `status_presence` `main.rs:2520`, **attach 경로 `status_presence_for_existing_attach` `main.rs:2258` `likely_agent_session→RowOff`**) → 현재 **codex는 cmux에서 아무 status도 못 받음**.

→ `requests_row()`는 **in-grid 행 의도**다. cmux pill은 off-grid라 in-grid 의도와 무관 — pill을 `requests_row()`에 AND로 묶으면 1차 표적 codex(RowOff)에서 **영구 비활성(BLOCKER)**. 본 트랙의 가치가 정확히 "in-grid가 불가/위험한 agent 세션에 off-grid 안전 pill 제공"이므로 sink는 `requests_row()`에 종속되면 안 된다.

**수정**:
```
// in-grid DECSTBM: 해당 backend + 정책이 행을 원할 때만(기존 의미 보존)
let in_grid = status_backend.reserves_in_grid_row() && presence_policy.requests_row();
// off-grid sink: backend==Cmux + 콘텐츠 명령 구성 + 명시적 비활성 아님. requests_row() 비종속.
let sink_enabled = matches!(status_backend, DelegatedSurface(SurfaceKind::Cmux))
    && status_command_config.is_some()   // LTERM_STATUS_COMMAND 설정(understatus 콘텐츠 소스)
    && !status_explicitly_disabled;      // --no-status면 끔
attach_pty_rows(rows, in_grid)           // Cmux → 풀 rows(rows-1 아님)
// 콘텐츠 게이트(in_grid || sink_enabled)로 확장 — sink도 콘텐츠/메타/poll 필요:
//   명령 스레드 스폰(client.rs:3297), status_info=info(target)(client.rs:3104), idle_wakeup_enabled(client.rs:3099)
// StatusBar::enter/refresh는 in_grid 경로에서만
```
- **agent-RowOff vs --no-status 구분(구현 요구)**: 정책 `RowOff`는 "agent 기본(→pill OK)"과 "--no-status 명시 비활성(→pill 끔)"을 구분 못 함. 명시적 비활성 신호(`no_status`)를 attach 진입부에서 sink 게이트로 plumb(또는 env 강제-off를 `select_status_backend`가 `Disabled`로 반환).
- **회귀 게이트(3종, 전부 차단성, §7)**:
  ① `in_grid`가 Cmux에서 false(DECSTBM+pill 이중렌더 방지).
  ② **codex agent 세션(RowOff)+cmux+LTERM_STATUS_COMMAND → `sink_enabled==true`**(트랙이 1차 표적에서 켜짐 — R3 BLOCKER 회귀 봉인).
  ③ codex+cmux+**`--no-status` → `sink_enabled==false`**(명시 비활성 존중).
- **config 호이스트(Critic m1)**: `StatusCommandConfig::from_env()`는 현재 `status_enabled` 게이트 안(`client.rs:3298`)에서 생성됨 → `sink_enabled`가 `status_command_config.is_some()`를 읽으려면 그 생성을 게이트 밖으로 호이스트.
- **셸도 포함(Critic m3)**: RowAuto 셸이 cmux에 있으면 `inside_cmux`가 `RowOff` 체크보다 먼저라(`select_status_backend` `client.rs:4543`) backend==Cmux → `in_grid`는 false(Cmux는 `reserves_in_grid_row()`=false)지만 `sink_enabled`는 true → **셸도 pill 받음**(off-grid 안전, 의도된 동작). `in_grid`와 `sink_enabled`는 backend variant상 **상호배타**(동시 true 불가 — DECSTBM+pill 이중렌더 구조적 봉인).

### 4.2 워크스페이스 식별 (attach 시점 1회)

- 위치: `attach_with_presence_and_cue`(`client.rs:3081`), `status_backend` 계산 직후.
- `cmux_status_identity(pane_id)`:
  1. stored split-time 컨텍스트(`stored_cmux_surface_for_pane`) — split 자손이면 포커스 결정적 시점 캡처본. 최우선.
  2. 없으면 `cmux identify --json --id-format uuids`의 `focused`(attach 순간엔 대상 서피스가 포커스이므로 정당). `find_cmux_surface_context`(`tmux_compat.rs:2292`)의 stale-`caller` 거부 로직 재사용.
- **env(`$CMUX_WORKSPACE_ID`)는 타깃에 절대 미사용**(검출용으로만 `inside_cmux`에 쓰일 수 있음). `cmux_session_env`(`tmux_compat.rs:1722`)가 자식 env에 그 값을 쓰므로 중첩 lterm이 부모값을 읽는 vector 차단.

### 4.3 누수 방지 (2중 + 옵션 3중)

> **정직화(Critic C3)**: panic hook(`client.rs:5882`)은 단일 클로저로 sink의 `applied` 키에 접근 불가 → "panic hook이 pill을 청소"는 전역 레지스트리 없이는 불가. 따라서 기본 보장은 **2중**(Drop + 다음 attach 재조정), panic 레지스트리는 옵션 하드닝.

1. **RAII Drop (1차)**: `CmuxStatusSink::drop` → 전 applied 키 `clear-status` + `clear-progress`(`ManagedAttachGuard::drop` 모델, `tmux_compat.rs:1744`). 정상 종료·`?` 전파·**언와인드 패닉**(기본 빌드) 커버.
2. **고아 pill 재조정 (2차, attach 시 — 필수)**: sink `new` 직후 `cmux list-status`(워크스페이스 한정) → 자기 `key_prefix`인데 이번 세션 desired에 없는 키 = 직전 하드킬/abort 잔재 → 시작 전 `clear-status`. **SIGKILL·`panic=abort` 복구의 유일 경로**. **단 `list-status` 자체가 실패하면(예 stale ref "Tab not found") best-effort로 재조정 생략하고 sink 생성은 계속**(복구 경로 실패가 sink를 막지 않음, Critic gap).
3. **(옵션) panic 레지스트리 (3차)**: 전역 `Mutex<BTreeSet<key>>`에 set/clear를 미러 → panic hook이 best-effort drain. `panic=abort`에서도 즉시 청소. 미구현 시 2가 복구하므로 비차단(청크7 하드닝).

### 4.4 diff & apply, 서킷 브레이커

- `apply`: 새 pill 집합 vs `applied`를 diff → `(set/clear/set-progress/clear-progress)` 명령 리스트(순수). 변경 없으면 빈 리스트 → 유휴 스폰 0.
- **ctx 양자화(Critic Ambiguity)**: ctx pill 텍스트와 progress 값을 **정수 %로 양자화**해 diff. 그래야 ctx가 매 poll 미세 변동해도 같은 정수면 스폰 0(원칙 "유휴 스폰≈0"과 "ctx 둘 다" 양립). 정수 % 바뀔 때만 set-status+set-progress(2 스폰).
- 각 cmux 호출은 `run_cmux_command`(`tmux_compat.rs:2166`, 출력 상한+좀비 회수) 재사용. **단 set-status 인자 빌더는 신규**(`add_cmux_status_target_args` = `--workspace`만) — 기존 `add_cmux_surface_context_args`(`tmux_compat.rs:2098`)는 `--surface`/`--window`까지 방출해 set-status가 거부할 수 있어 재사용 금지(Architect E8/M1).
- 연속 실패 **3회**(기본 `N=3`) → `healthy=false`: 스폰 중단, `eprintln!` 경고 1회(`tmux_compat.rs:2108` 패턴), **비표시 유지**(폴백=blackout, 사용자 결정). cmux 복구 감지 시(다음 성공) 재개.
- apply 레이어는 trait/enum으로 분리(`apply_via_cli` / 후속 `apply_via_rpc`).

---

## 5. understatus 측 변경 (상세)

### 5.1 인자 / 라우팅
- `RenderArgs`(`main.rs:97`)에 `surface_format: SurfaceFormat` 추가; `parse_render_args`(**`main.rs:115`**)에 `--surface-format <oneline|cmux-status>` 매치 암 추가. 기본 `oneline`(behavior-preserving). 기존 `--oneline`은 `run_render_pipeline(source, oneline: bool)`(`main.rs:569`) 시그니처를 `SurfaceFormat`로 확장하며 `Oneline`에 흡수 — `--surface-format`이 우선, 미지정 시 `--oneline`→`Oneline`.

### 5.2 직렬화
- `run_render_pipeline`(`main.rs:569`)의 oneline 분기점(`main.rs:623`)과 동위치에서 분기:
  ```
  if surface_format == CmuxStatus { print!("{}", serde_json::to_string(&to_cmux_pills(...))?); return; }
  ```
- 수집부는 전부 재사용(이미 oneline 전에 실행). **전면 IR 리팩터 회피**(현 `Segment.colored`가 SGR-완성이라 재생성 시 oneline 바이트 드리프트 위험) → 핵심 세그먼트에 **additive `pill: Option<PillMeta{id,label,value,color,priority,icon}>`** 부착(`render.rs:25` `Segment`에 필드 추가). 두 경로가 같은 `collect_segments` 호출 공유.
- **3중 소스 드리프트 봉인(Architect R2-C)**: `Segment`는 이미 `plain`(폭계산)/`colored`(표시) 이중 소스 → `pill`이 3번째가 됨. `PillMeta.value`는 세그먼트의 **원시 값(plain의 소스)에서 단일 파생**(별도 손계산 금지). 차단성 단위테스트: 핵심 세그먼트에서 `pill.value`가 `plain`의 값 부분과 일치함을 단언(세 표현 동기화 고정).

### 5.3 색 (신규 매핑, 재사용 아님)
- **Critic C2 정정**: understatus 값 세그먼트엔 구조화된 색이 없다(`render.rs:267-273` value_segment "색 없음"; 전 렌더러에서 밴드 색은 cpu glyph만 `render.rs:88,244-246`). 따라서 pill `color`는 **신규 매핑**: model=테마 accent, ctx=ctx 밴드 색, **cpu=`band_tint` 재사용**, mem=중립. `ColorSpec→#RRGGBB` 변환 함수만 `ansi_fg`(`render.rs:462`)의 truecolor 산식과 공유.

---

## 6. 안전성 / 신뢰경계

- **키 sanitization**: `key_prefix`는 `SessionInfo.pane_id` 기반, cmux 키 안전문자(`is_valid_cmux_ref_segment`, `tmux_compat.rs:1673`)로 정규화. 값/색/아이콘도 인자 주입 방지(argv-spawn, 셸 미경유 — 기존 `run_cmux_command` 계약).
- **값 길이 cap**: pill value/label 길이 상한(기존 payload 필드 cap 관례 준수).
- **understatus 출력 신뢰**: lterm은 understatus stdout(JSON)을 파싱하되, 파싱 실패/스키마 불일치 시 **무해 no-op**(blackout). 패닉 금지.
- **non-cmux 라우팅 오인**: `cmux_available==false`거나 식별 실패 시 sink 미생성 → blackout + 경고. DECSTBM로 silent fallback 안 함(사용자 결정).

---

## 7. 테스트 / 검증

### 7.1 자동(순수, GUI 불요)
- understatus `to_cmux_pills`: ClaudeInput+snapshot → pill JSON. None-skip(소스 없음→pill 없음), ctx→progress+pill 동시, 색→#hex.
- pill **diff**: `(applied, desired) → Vec<CmuxCommand>`. 케이스: 무변경→빈, 값변경→set, 제거→clear, 추가→set, progress add/change/clear.
- 워크스페이스 식별: JSON 픽스처 → `CmuxSurfaceContext`, UUID 우선, stale-`caller` 거부(`find_cmux_surface_context` 픽스처 재사용).
- 페이로드: `surface_format` 필드 존재·기본값.
- **rows 라우팅(회귀 게이트)**: `DelegatedSurface(Cmux)` → `reserves_in_grid_row()==false` → `attach_pty_rows` 풀 rows + `StatusBar` draw 미진입. **DECSTBM+pill 이중렌더 방지의 핵심 테스트.**
- 키 네임스페이스/정규화: pane id → 안전 키.

### 7.2 라이브 cmux 육안(자동 불가 — 필수)
- pill이 **올바른** 워크스페이스 탭에 렌더(포커스 다른 워크스페이스로 이동 후 복귀 포함).
- poll마다 갱신, **detach/종료 시 청소**(누수 0). `kill -9` 후 재attach → 고아 pill 재조정 확인.
- 한 워크스페이스에 codex pane 2개 → 키 충돌 없음, 둘 다 표시.
- cmux 세션 중간 kill → 서킷 브레이커 작동, 스폰 폭주 없음, blackout+경고.
- **DECSTBM 행 미출현 + codex 입력칸 무손상**(본 트랙의 수용 기준).

---

## 8. 리스크 & 대응 (Pre-mortem)

| ID | 리스크 | 심각도 | 대응 |
|---|---|---|---|
| R1 | positional ref 재번호화 드리프트 | HIGH | UUID 캡처(`--id-format uuids`), stored 컨텍스트 우선 |
| R2 | `$CMUX_WORKSPACE_ID` stale(=죽은 ws) | 확정 | sink 타깃에 env 미사용 |
| R3 | 서브프로세스 스폰 폭주(4 pill×2s) | MEDIUM | diff 후 변경분만; 유휴 ≈0; 후속 rpc 배치 |
| R4 | 다중 세션 키 충돌 | HIGH | pane 스코프 prefix |
| R5 | cmux CLI 중간 실패 | MEDIUM | 서킷 브레이커 + blackout + 경고 1회 |
| R6 | cmux 라우팅됐으나 식별 실패 | MED | blackout(silent 0 아님) |
| R7 | "Tab not found"류(죽은 ref) | 확정 | UUID+attach-시점 identify; 첫 set 후 1회 list 검증(옵션) |
| R8 | rows 수정 미흡 → DECSTBM+pill 이중 | HIGH | §4.1 테스트 게이트 |
| R9 | 한 pane 다중 attach → sink 경합 | MEDIUM | dup-attach 차단은 `MANAGED_ATTACH_ENV` 설정 시만(`tmux_compat.rs:1759`); env-off면 2-attach 가능하나 **동일 prefix라 blast radius=자기 키 churn**(교차 손상 아님). churn 관측 시 per-pane sink 리스 추가(비차단) |
| R10 | 약속 pill 일부가 소스 부재로 영구 미표시→사용자 혼란 | HIGH→**해소** | **가용 데이터만 선택**(cost·git 범위 밖). 단언: enrich-성공 4 pill / enrich-실패 3 pill(model bare 토큰+cpu+mem, ctx만 None). §3.4 |
| R11 | pill 무색(SGR 라인 대비 빈약) | MED | pill 전용 색 매핑 **신규 정의**(cpu는 `band_tint` 재사용); §5.3 |

---

## 9. 빌드 순서 (작은 청크 — 긴 단일 태스크는 API 소켓 에러 이력)

1. **understatus: pill 스키마+직렬화(순수, cmux 불요)** — `SurfaceFormat` enum + 플래그, `to_cmux_pills`, `collect_segments` 공유 중간표현 리팩터. 검증: 단위테스트 + `echo '<json>' | understatus render --source lterm --surface-format cmux-status`.
2. **lterm: 페이로드 필드** — `StatusCommandPayload`/`build_status_payload`에 `surface_format`. 검증: 페이로드 단위테스트(기본값 oneline).
3. **lterm: rows 수정** — `reserves_in_grid_row()`, `in_grid`/`sink_enabled` 분리, 풀 rows. 검증: 라우팅 테스트 확장(**R8 게이트**). 라이브 불요.
4. **lterm: 워크스페이스 식별** — `cmux_status_identity`(stored-first, identify-fallback, UUID), `CmuxSurfaceContext` `pub(crate)`. 검증: JSON 선택 단위테스트.
5. **lterm: `CmuxStatusSink` diff+apply(CLI)** — 파싱·diff·set/clear/progress·Drop·서킷브레이커. 검증: diff 순수 단위테스트.
6. **lterm: sink 소비점 배선 + 고아 재조정** — `client.rs:3421`(`apply_pending_status_command`) Cmux 분기. 검증: **라이브 필수**(올바른 ws·갱신·detach 청소·`list-status`로 누수 0 확인 필수).
7. **하드닝** — 폴백·(옵션)1회 list 검증·후속 rpc 배치 자리. 검증: 라이브(유발 실패: cmux kill·포커스 이동·동일 ws 2 pane).

**라이브 검증 필수: 6, 7.**

---

## 10. ADR (주요 결정 근거)

- **ADR-1 native pill > split-렌더러**: `new-split` 크기 플래그 부재(화면 ~50% 점유), 생명주기 복잡. native pill은 셀 무점유·테마 렌더·검증됨.
- **ADR-2 콘텐츠 파이프라인 재사용**: 기존 `spawn_status_command_thread`/`run_status_command`의 blocking-safety(reader 스레드·데드라인·버퍼 cap)를 공짜로 승계. 소비점만 분기.
- **ADR-3 prefix는 lterm이 부여**: pane 정체는 lterm에만 존재 → understatus 순수성 유지. understatus에 pane 배선 안 함.
- **ADR-4 additive-optional 계약(version 하드게이트 금지)**: 구버전 understatus와도 무해 저하. sink가 non-JSON을 견디는 게 전제.
- **ADR-5 폴백=blackout**: DECSTBM 폴백은 트랙이 없애려는 충돌을 재도입 → 금지(사용자 결정).
- **ADR-6 UUID 캡처**: positional ref 드리프트(R1)는 포커스 드리프트(이미 적발)와 동급 치명 → 선택 아닌 필수.

## 11. 열린 결정 (구현 중 확인)
- cmux 아이콘명 어휘 안정성(버전 간) — 미지원명은 무시되므로 베스트에포트, 실패해도 pill 텍스트는 남음.
- `cmux rpc` 배치 set 가능 여부(스폰 최적화) — 후속, apply 레이어 분리로 무비용 합류.
- 다중 attach 리스 의미(R9) — `ManagedAttachGuard` 코드 확인 후 planner에 위임.
