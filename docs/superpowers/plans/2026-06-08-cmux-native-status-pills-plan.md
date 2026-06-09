# 실행 계획 — lterm cmux 네이티브 status pills (DelegatedSurface(Cmux) 실렌더)

> 상태: **pending approval** (ralplan 합의 수렴 — 실행 승인 대기. 본 문서는 계획 산출물이며 코드 수정·커밋 없음.)
> 설계 진실원천: `docs/superpowers/specs/2026-06-08-cmux-native-status-pills-design.md`
> 교차 레포: lterm `/Users/jinhongan/Desktop/light_terminal`, understatus `/Users/jinhongan/Desktop/status_ticon/statusticon`(본 레포)
> 합의: Planner×2 → Architect×3 → Critic×2. 최종 Critic 클로즈(M1) 반영 완료. Architect R3 = SOUND-WITH-CHANGES(BLOCKER 교정 검증), Critic R2 = "M1 한 줄 수정 → APPROVE"(반영됨).
>
> **⚠️ 구현 후 개정(quad-review Codex):** ① ctx 진행바(`set-progress`) 전면 제거 → ctx는 pill만(cmux set-progress 워크스페이스 전역 누수). 아래 C1/C5/C6의 progress·set-progress 기술은 **폐기(superseded)**. ② cmux 호출 타임아웃(3s)·pill key 검증·value/color/icon cap 추가(Codex HIGH 3건). quad-review-loop round 2: lterm APPROVE, understatus 코드 클린.

---

## 0. 합의 요약 (무엇이 검증됐나)

- **본질 결함(라우팅) 교정 검증**: 현재 `lterm`에서 codex(agent=RowOff)는 cmux에서 **status를 아예 못 받는다**(`requests_row()==false`). `sink_enabled`를 `requests_row()`에 묶으면 1차 표적 codex에서 pill 영구 OFF(Architect R3 BLOCKER) → `sink_enabled = matches!(backend,Cmux) && status_command_config.is_some() && !status_explicitly_disabled`(requests_row 비종속)로 교정. `in_grid`와 `sink_enabled`는 backend variant상 **상호배타**라 DECSTBM+pill 이중렌더는 구조적으로 불가(Critic 검증).
- **pill = model·ctx·cpu·mem**(사용자 확정). codex 경로 가용: cpu·mem 무조건, ctx는 codex enrich 성공 시, model은 enrich 성공 시 풍부한 이름·실패 시 bare agent 토큰("codex") 폴백. → enrich-성공 4 pill / enrich-실패 3 pill(ctx만 None).
- **cost·git 제외**: lterm 파서가 영구 None(`claude.rs:288-289`). 향후 payload 확장 트랙.
- **누수=2중**(Drop + attach 고아 재조정), panic 레지스트리는 옵션. **색=신규 매핑**(cpu만 band_tint 재사용). **set-status 전용 신규 인자 빌더**(--workspace만). **ctx 정수% 양자화 diff**. **폴백=blackout+경고**.

---

## 1. 실행 순서 (의존 DAG)

```
understatus:  C1(PillMeta+색+직렬화) → C2(--surface-format 플래그)
lterm:        C3(payload) ─┐  C4(rows R8게이트 + 워크스페이스 식별) ─┐
                           └────────────────────────────────────────┴→ C5(sink diff/apply)
                                                                         → C6(소비점 배선+고아재조정, 라이브)
                                                                         → C7(하드닝, 라이브)
```
- 권장: understatus(C1→C2) 먼저 → lterm(C3·C4 병렬 → C5 → C6 → C7).
- C1·C2·C3·C4는 순수/단위 검증(라이브 불요). **C6·C7만 라이브 cmux 필수.**

---

## 2. 청크별 실행 (목표 / 변경 / AC / 검증)

> 빌드: understatus·lterm 모두 `export PATH="$HOME/.cargo/bin:$PATH"`(Homebrew rustc 깨짐). lterm clippy 게이트 = stable 1.96.

### C1 — understatus: PillMeta + 신규 색맵 + 직렬화 (레포: understatus)
- **변경**: `Segment`(`render.rs:25`)에 additive `pill: Option<PillMeta{id,label,value,color,priority,icon}>`. `collect_segments`(`render.rs:75`)의 model/ctx/cpu/mem push 지점만 `pill: Some(..)`. 신규 색맵(model=accent·ctx=band·**cpu=`band_tint` 재사용** `render.rs:244-246`·mem=중립), `color_to_hex`(`ansi_fg` `render.rs:462` 산식 공유). `to_cmux_pills() -> {schema,version,pills,progress}`.
- **AC**: enrich-성공 → pill 집합 `{model,ctx,cpu,mem}`(4); enrich-실패 → `{model,cpu,mem}`(3, ctx만 부재). 색 `#RRGGBB`/null. **oneline 출력 바이트 불변**(골든). `pill.value == plain 값`(3중-소스 동기화). ctx 정수% 양자화.
- **검증**: `cargo test to_cmux_pills`; `echo '<lterm-codex-json>' | cargo run -- render --source lterm --surface-format cmux-status | jq`.

### C2 — understatus: `--surface-format` 플래그 (레포: understatus)
- **변경**: `SurfaceFormat{Oneline,CmuxStatus}`, `parse_render_args`(`main.rs:115`) 매치 암, `run_render_pipeline(source, oneline:bool)`(`main.rs:569`)을 `SurfaceFormat`로 확장(oneline 흡수, `--surface-format` 우선).
- **AC**: 미지정→oneline 바이트 불변; `cmux-status`→JSON; `bogus`→에러 exit.
- **검증**: `cargo test parse_render_args`; CLI 양방향.

### C3 — lterm: 페이로드 필드 (레포: lterm)
- **변경**: `build_status_payload`(`client.rs:3871`)에 `surface_format`(backend==Cmux→"cmux-status", else 생략/oneline).
- **AC**: backend별 필드 존재/부재 단위테스트. 기존 페이로드 테스트 green.
- **검증**: `cargo test build_status_payload`.

### C4 — lterm: rows R8 게이트 + 워크스페이스 식별 (레포: lterm)
- **변경**: `StatusBackend::reserves_in_grid_row()`(`client.rs:4475`). `client.rs:3096` 라우팅을 `in_grid`/`sink_enabled` 분리(§4.1 스펙). `StatusCommandConfig::from_env()`를 게이트 밖 호이스트. 콘텐츠 게이트(`client.rs:3099`/`3104`/`3297`) `in_grid||sink_enabled`로 확장. `cmux_status_identity(pane_id)`(stored 우선→`cmux identify --json --id-format uuids`), `CmuxSurfaceContext` `pub(crate)`.
- **AC (차단성)**: ① Cmux→`reserves_in_grid_row()==false`→풀 rows·StatusBar 미진입. ② **codex(RowOff)+cmux+LTERM_STATUS_COMMAND→`sink_enabled==true`**. ③ `--no-status`→`sink_enabled==false`. 식별 UUID 우선·stored 우선·stale-caller 거부.
- **검증**: `cargo test status_backend in_grid sink_enabled cmux_status_identity`. 라이브 불요.

### C5 — lterm: `CmuxStatusSink` diff+apply (레포: lterm)
- **변경**: sink 구조체(§3.2). **신규 `add_cmux_status_target_args`(--workspace만)**(재사용 `add_cmux_surface_context_args` `tmux_compat.rs:2098` 금지). `apply`: 비-JSON no-op → 정수% 양자화 → diff → `Vec<CmuxCommand>` → `run_cmux_command`(`tmux_compat.rs:2166`). 서킷브레이커 N=3. `Drop`→전 키 clear. apply 레이어 trait 분리(후속 rpc).
- **AC**: diff(무변경 빈/set/clear/progress); ctx 양자화 빈-vec; set-status argv에 `--workspace` 有·`--surface`/`--window` 無; Drop clear-all; 서킷 3회→스폰0; 키 정규화(`is_valid_cmux_ref_segment` `:1673`).
- **검증**: `cargo test cmux_status_sink plan_commands`. 라이브 불요(runner mock).

### C6 — lterm: 소비점 배선 + 고아 재조정 (레포: lterm, **라이브 필수**)
- **변경**: 소비점 `client.rs:3421`(`apply_pending_status_command`) Cmux 분기→`sink.apply`(StatusBar 미진입). 고아 재조정(sink new 직후 `list-status`→자기 prefix 잔재 clear; **list-status 실패 시 best-effort 생략·sink 생성 계속**).
- **AC (라이브)**: 올바른 ws 렌더; 포커스 이동 후 복귀 유지; **detach/종료 시 `list-status`에 자기 prefix 0건(누수0 필수)**; `kill -9` 후 재attach 고아 0; **DECSTBM 미출현+codex 입력칸 무손상**.
- **검증**: 라이브 cmux 0.64.14 + codex-in-lterm.

### C7 — 하드닝 (레포: lterm, **라이브 필수**)
- **변경**: panic 레지스트리(옵션 3중), `LTERM_CMUX_VERIFY`(첫 set 후 list 검증, 기본 off), `apply_via_rpc` stub.
- **AC (라이브 유발실패)**: cmux 중간 kill→서킷브레이커·스폰 무폭주·blackout; 2-pane 충돌0(prefix); 유휴 스폰≈0(정수% 무변동 60s 스폰0 모니터); `LTERM_CMUX_VERIFY`로 죽은 ref 감지.

---

## 3. 테스트 게이트 요약
- **자동(머지 게이트)**: 가용 pill 집합(4/3) 단언 · oneline 바이트 불변 · **R8 rows 게이트** · **sink_enabled 3종 회귀** · diff/양자화 · 식별 · 키정규화 · 비-JSON no-op.
- **라이브 필수(육안)**: 올바른 ws · 누수0(list-status) · 고아복구 · DECSTBM 미출현+무손상 · 서킷브레이커 · 2-pane.

---

## 4. ADR (합의 결정)

- **Decision**: lterm-in-cmux의 status를 DECSTBM 인그리드 행이 아니라 **cmux 네이티브 사이드바 pill(`set-status`)+진행바**로 렌더. 콘텐츠는 기존 `LTERM_STATUS_COMMAND`(understatus) 파이프라인 재사용, understatus가 `--surface-format cmux-status`로 pill JSON 출력, lterm `CmuxStatusSink`가 diff 후 `cmux set-status` 구동.
- **Drivers**: ① codex 입력칸 손상 제거(off-grid라 scroll-region 충돌 0) ② 누수 0 ③ 유휴 스폰 ≈ 0.
- **Alternatives considered**:
  - split-렌더러(서피스 통째 소유) — `new-split` 크기 플래그 부재로 화면 ~50% 점유 → 기각.
  - iTerm OSC1337(NativeChrome) — 데일리 환경(cmux) 무관·plain text only → 별도 트랙.
  - `requests_row()` 게이트 유지 — codex(RowOff)에서 pill 영구 OFF → 기각(BLOCKER).
  - 전면 IR 리팩터 — oneline 바이트 드리프트 위험 → additive PillMeta로 대체.
  - 폴백 DECSTBM — 없애려는 충돌 재도입 → blackout 채택.
- **Why chosen**: cmux `set-status`는 이 용도로 설계된 네이티브 API(라이브 검증). 셀 무점유·테마 렌더·키별 다중도구 공존. 기존 콘텐츠 파이프라인 재사용으로 blocking-safety 승계.
- **Consequences**: agent 세션(codex)이 cmux에서 처음으로 안전한 status를 얻음(셸도 포함). cost·git은 미표시(향후 트랙). lterm↔cmux 계약은 cmux 버전에 결합(서킷브레이커로 완화). 워크스페이스 식별은 UUID 캡처 의존.
- **Follow-ups**: (1) `cmux rpc` 배치 set으로 스폰 최적화. (2) cost·git용 payload 확장 트랙. (3) Tmux 동형(DelegatedSurface(Tmux)). (4) lterm 릴리스(실렌더 완료 시).

---

## 5. 다음 단계 (실행 승인 필요)
본 계획은 `pending approval`이다. 승인 시:
- 사용자 전역 규칙(main 직접 커밋 금지)에 따라 **feature 브랜치** 생성 후 진행.
- 청크 단위 실행(executor) → 청크별 AC 검증 → C6·C7은 라이브 cmux 육안 검증.
- understatus·lterm 각 PR, quad-review-loop 권장(HANDOFF 워크플로).
