# 설계 문서 — Codex 세션 심층판독 (understatus, Phase 2-1)

> 작성일: 2026-06-06 · 제품: understatus(디렉터리명 `statusticon`은 레거시)
> 상태: **ralplan 합의 완료 — `pending approval`** (Architect ACCEPT + Critic APPROVE, v2). 실행 진입은 사용자 승인 대기.
> 범위: lterm 통합 spec(`2026-06-05-lterm-statusline-integration-design.md`) §15가 **Phase 2로 분류한 "Codex 세션 심층판독"의 선구현**.
> 교차 레포 변경 없음: lterm(`/Users/jinhongan/Desktop/light_terminal`, `src/client.rs`)은 **건드리지 않는다**(사용자 결정).

---

## 0. Scope & Phase 관계 (Critic P0-3)

- 본 작업은 lterm 통합 spec §15 line 72/364에서 **Phase 2로 명시 분류**된 항목이다. Phase 1 위에 무단 적재가 아니라 **그 Phase 2의 부분 선구현**임을 명시한다.
- **Phase 1 계약 보존**: `understatus render --source lterm`은 git 비활성(`git_branch=None` 유지)·chain off·정확히 1행·lenient 파싱. 본 작업은 그 위에 **세그먼트 enrich만** 얹으며 git/chain 동작을 **바꾸지 않는다**.
- 사용자 사전 결정(AskUserQuestion): (a) 트리거 = `--source lterm` + `agent=="codex"` 자동, **lterm config 무변경**, (b) 표시 = 풀 프로필, (c) 매칭 = cwd+mtime+freshness, **lterm 변경 불필요**. "lterm이 세션경로를 payload에 전달"은 **명시적 기각**.

---

## 1. 목표 (Goals)

`understatus render --source lterm --oneline`이 lterm payload의 `agent=="codex"`를 감지하면, `$CODEX_HOME/sessions/**/rollout-*.jsonl`을 직접 판독해 statusline에 다음을 표시한다:

**model · ctx% · 5h한도% · 주간한도% · plan · effort**

세션을 못 찾거나 모호하면 기존 lterm 동작(model 슬롯에 `"codex"`, ctx/한도 없음)으로 **정직하게 저하**한다.

---

## 2. Principles (합의 기준)

1. **fail-safe + "모호한 성공도 실패로 취급"**: 실패(부재/깨짐/stale)뿐 아니라 **잘못된 세션을 자신 있게 표시하는 fail-wrong을 금지**한다. 모호하면 표시하지 않는다.
2. **기존 렌더 불변식 보존**: `render()` 시그니처(frozen 계약, `render.rs:34`)·1행·COLOR-ONCE·no-bold·폭/우선순위 모델 무손상. Claude 경로는 **바이트 동일**.
3. **모듈경계 단일책임**: Codex 판독(디스크 I/O·스캔·tail)은 **신규 `src/codex.rs`에 격리**. `claude.rs::parse_lterm_input`은 순수 함수(디스크 무접촉) 유지.
4. **바운디드 비용**: status는 interval(기본 2초)마다 단명 프로세스 → 전체 파싱 금지. 스캔·읽기 모두 상한 + **디스크 캐시로 정상상태 stat 1회**.
5. **opt-out**: `[codex] enabled=false`면 `~/.codex` 일절 안 읽음(프라이버시/성능).

## 2.1 Decision Drivers (top 3)
- **D1 (정확/정직)**: "이 페인의 codex"를 틀리게 매칭해 옆 세션 수치를 표시하면 신뢰 직접 훼손 = 가장 비싼 실패.
- **D2 (비용)**: 2MB 파일 × 2초 + 4400 세션 누적 → 무캐시 풀스캔은 비용 폭증(실측 ~113ms/tick).
- **D3 (계약 정합)**: 실제 jsonl 필드 경로·understatus 함수 계약에 정확히 정합(Phase 1 교훈).

---

## 3. Viable Options

**O-Match (채택) — Solution A: 모호성 정직 저하.** 동일 cwd·fresh 후보 ≥2면 enrich 생략. lterm 변경 0.
 - (+) 사용자 "lterm 무변경" 100% 존중, fail-wrong→fail-safe. (−) 동시 동일cwd codex 시 풀 프로필 미표시(정직한 트레이드오프).
 - 기각 대안: **payload 앵커 추가**(PID/launch-cwd+timestamp) — 정확하나 lterm `src/client.rs` 수정 필요 = 사용자 기각. **무대책 cwd+mtime** — fail-wrong(실측 동일 cwd 119세션) 기각.

**O-Integrate (채택) — O2 + 네임스페이스 서브구조.** `render()` 시그니처 불변(frozen 보존), `ClaudeInput`에 `codex: Option<CodexExtras>` 1필드.
 - (+) frozen 계약 보존, 호출부(프로덕션 `main.rs:513` 1곳 + 테스트 다수) 무변경, codex 필드 네임스페이스 격리. (−) `ClaudeInput`이 codex 옵션 1개를 보유(경미 — 이미 lterm 소스도 이 타입 공유).
 - 기각: **O1(render에 `CodexSession` 인자 추가)** — `render.rs:34` frozen 계약 위반 + 호출부 회귀, 이득 없음.

**O-Cost (채택) — 디스크 캐시 재사용.** `chain.rs` 캐시 인프라(`:209-380`) 재사용, session_key별.
 - (+) 정상상태 stat 1회로 바운드. (−) 비-atomic write(읽기전용·lossy라 무해), 모호 매칭 캐시 시 TTL 고착 → **모호 해소는 캐시 안 함**으로 차단.

---

## 4. 데이터 필드 경로 (실측 검증 — Critic P0-1 정정)

샘플: `~/.codex/sessions/2026/06/05/rollout-2026-06-05T20-40-45-019e9795-*.jsonl`.

| 표시 | 정확 경로 | 비고 |
|---|---|---|
| model | `turn_context.payload.model` | 예 `"gpt-5.5"`. 세션중 `/model`로 변동 가능 → tail 최신 우선 |
| effort | `turn_context.payload.effort` | 예 `"xhigh"` |
| ctx% | **`payload.info.last_token_usage.total_tokens` / `payload.info.model_context_window` × 100** | `info` 중첩 명시. `window==0` 가드 → None |
| 5h% | `payload.rate_limits` 중 `window_minutes==300`인 객체의 `used_percent` | **primary=5h 단정 금지**, window_minutes로 식별 |
| 주간% | `payload.rate_limits` 중 `window_minutes==10080`인 객체의 `used_percent` | secondary 단정 금지 |
| plan | `payload.rate_limits.plan_type` | 예 `"pro"` |
| (매칭) | `session_meta.payload.cwd`, `session_meta.payload.originator` | 첫 줄. originator로 TUI/exec 구분 |

- **`rate_limits`는 배열이 아니라 named-field 객체**다(실측): `payload.rate_limits.primary`/`payload.rate_limits.secondary` 중첩 객체이고 각각 `window_minutes`(300/10080) 보유. 파서는 두 named 필드를 받아 **`window_minutes`로 5h(300)/주간(10080)을 식별**한다(§7). `primary=5h` 단정은 금지하되 구조는 Vec가 아니다.
- **token_count는 2단계 중첩**: `event_msg`의 `payload.type=="token_count"`로 게이팅한 뒤 그 `payload.info`(`last_token_usage`/`model_context_window`)·`payload.rate_limits`를 읽는다.
- **`total_token_usage` 사용 절대 금지**: 누적값이라 실측 100% 초과(세션별 210%~9921%, 예 25638158/258400). 코드 주석 + AC-X2 회귀 테스트로 박제.
- 모든 필드 lenient(Option): 부재/타입 드리프트/cli_version 변동 시 **무패닉 → 해당 세그먼트 생략**(AC-X7).

---

## 5. 매칭 — Solution A (CRITICAL #1 해소)

`find_codex_candidates(base, cwd, now, freshness, scan_days) -> Vec<PathBuf>`:
1. `base/sessions` → 연도 desc → 월 desc → **최근 `scan_days`(기본 3) 일자 디렉터리만**(전체 4400+ 회피). 폴더는 **시작시각 기준**이므로 scan_days 밖 장기 활성 세션은 미발견(알려진 한계, §10 S1).
2. 그 안 `rollout-*.jsonl` 중 **mtime이 freshness(기본 240분) 이내**만(cheap stat 선필터).
3. 각 후보의 **첫 줄(session_meta)만** 읽어: `payload.cwd == payload_cwd`(정규화: canonicalize 실패 시 trim 문자열 비교) **AND** `payload.originator`가 **대화형 화이트리스트**(prefix `codex-tui`; `codex_exec` 등 비대화형 제외 — exec 세션엔 token_count/turn_context 없음). 화이트리스트 채택: 미래 새 originator는 보수적으로 안전 저하(블랙리스트는 새는 반면).

**모호성 판정**:
- 후보 **정확히 1개** → 풀 enrich.
- 후보 **≥2개** → "이 페인의 codex 식별 불가" → **enrich 전면 생략**(model="codex" 유지). fail-wrong→fail-safe (AC-X1).
- 후보 0개 → 생략.

외부 입력(payload.cwd)은 **비교에만** 사용하고 파일경로 구성에 쓰지 않는다(traversal 무관).

---

## 6. 통합 — O2 + CodexExtras (BLOCKER #4 해소, render frozen 보존)

- `render()` 시그니처 **불변**(`render.rs:34` frozen).
- `src/claude.rs`의 `ClaudeInput`에 필드 1개 추가:
  ```rust
  pub codex: Option<CodexExtras>,   // lterm/codex 소스 전용. Claude 경로는 None(비트 동일 보장).
  ```
  ```rust
  pub struct CodexExtras {
      pub rate_5h_percent: Option<f64>,
      pub rate_weekly_percent: Option<f64>,
      pub plan: Option<String>,
      pub effort: Option<String>,
  }
  ```
- model/ctx는 기존 슬롯 재사용: enrich가 `model_display_name`(실모델), `context_used_percentage`(ctx%) 설정.
- `render.rs::collect_segments`는 `input.codex`를 읽어 신규 세그먼트 추가:

| 세그먼트 | 출처 | priority | 렌더 |
|---|---|---|---|
| model | `model_display_name`(enriched) | 60(기존, `show_model`) | value |
| ctx % | `context_used_percentage`(enriched) | 50(기존, `show_context`) | `ctx 55%` |
| 5h % | `codex.rate_5h_percent` | 48(신규) | `5h 0%` |
| wk % | `codex.rate_weekly_percent` | 46(신규) | `wk 20%` |
| plan | `codex.plan` | 26(신규) | `pro`(bare value) |
| effort | `codex.effort` | 24(신규) | `xhigh`(bare value) |

- 각 신규 세그먼트는 해당 `Option`이 `Some`일 때만(`render.rs:144/153/166`의 `None`→생략 패턴 동형). `codex=None`(Claude/미매칭/모호) → 신규 세그먼트 0 → 기존 출력 보존.
- `ClaudeInput` 구조체-리터럴 생성처 **3곳**에 새 필드 반영: `claude.rs:76`(parse_claude_input)→`codex: None`, `claude.rs:120`(parse_lterm_input)→enrich가 채움(초기 `None`), `render.rs:536`(`sample_input`)→`codex: None`. `ClaudeInput`은 `#[derive(Default)]` 보유라 `..Default::default()` 활용 가능. Claude 경로 `codex: None` → 불변식 테스트(`render_no_color_env_has_no_escape_bytes` 등) **비트 동일** 보존(미반영 시 컴파일 에러로 즉시 노출).

---

## 7. 모듈 — `src/codex.rs` (BLOCKER #3)

```
// 순수 파서(fixture 단위테스트):
fn parse_session_meta(first_line)   -> Option<{cwd, originator}>
fn parse_turn_context(line)         -> Option<{model: Option, effort: Option}>
fn parse_token_count(line)          -> Option<TokenSnapshot{ last_total_tokens, context_window,
                                          // rate_limits는 named 객체(배열 아님): primary/secondary 각 {window_minutes, used_percent}.
                                          // window_minutes로 5h(300)/주간(10080) 식별. event_msg payload.type=="token_count" 게이팅 후 payload.info/payload.rate_limits.
                                          rate_limits: { primary: Option<RateWindow>, secondary: Option<RateWindow> }, plan }>
fn compute_context_percentage(total, window) -> Option<f64>   // window==0 가드
// 발견/IO(주입 base/now/freshness/scan_days — 테스트 격리):
fn find_codex_candidates(base, cwd, now, freshness, scan_days) -> Vec<PathBuf>
fn extract_from_file(path)          -> Option<CodexSession>   // head 16KB + tail 256KB
fn read_codex_session(base, cwd, now, freshness, scan_days)   -> Resolution { Single(CodexSession, path) | Ambiguous | None }
// 통합:
pub fn maybe_enrich(input: &mut ClaudeInput, cfg: &Config)    // 아래 게이팅
fn codex_home() -> PathBuf   // env CODEX_HOME or ~/.codex
```

`maybe_enrich` 게이팅(이중 + observability):
- **`Source::Lterm`** && `cfg.codex.enabled` && `input.model_display_name`이 **codex 계열**(정규화/prefix 매칭, 정확 동등 아님 — A-2) && `input.cwd=Some` && `codex_home()` 존재일 때만 발동. **Source 게이팅 필수**: Claude 경로에서 모델 별칭이 우연히 "codex" 계열이어도 enrich가 오발동해 `~/.codex`를 읽지 않도록 호출부(`main.rs`)에서 `Source::Lterm`으로 한정한다(Architect 재검토 #1).
- 캐시 조회(§8) → 단일 해소면 `input.model_display_name=model`·`input.context_used_percentage=ctx%`·`input.codex=Some(CodexExtras)`. 모호/None이면 무변경.
- 실패/모호 시 `LTERM_STATUS_DEBUG`/stderr 1줄(silent off 방지).
- `claude.rs::parse_lterm_input`은 **수정하지 않는다**(순수 유지).

---

## 8. 비용 — 디스크 캐시 (BLOCKER #2, `chain.rs` 재사용)

- `chain.rs` 캐시 인프라(`session_cache_file`/`read_cache_entry`/`write_cache_entry`/`is_cache_fresh`/TTL, `:209-380`) 재사용.
- 키 = session_key(페인별 안정, lterm payload 유래). 저장: 해소된 rollout 경로 + 그 파일 mtime + 파싱 결과(model/ctx/CodexExtras).
- **캐시 payload 직렬화**: `chain.rs::write_cache_entry(path, ts, payload: &str)`가 단일 문자열만 받으므로, 캐시 본문은 **`serde_json`으로 1라인 직렬화**(rollout 경로·mtime·model·ctx·CodexExtras 묶음). 역직렬화 실패(스키마 드리프트)는 §9 lenient로 무패닉 → 풀 재해소(캐시 버저닝 불필요).
- 매 틱:
  1. 캐시 히트 & 경로 mtime 불변 & freshness 이내 → **재사용**(스캔 0, **stat 1회**).
  2. 캐시 히트 & mtime 변동 & freshness 이내 → **그 파일만 tail 재독** → 캐시 갱신.
  3. 미스/경로 stale/없음 → **풀 후보스캔 재해소**(§5 모호성 재판정). **모호(≥2)였던 결과는 캐시하지 않는다**(fail-wrong TTL 고착 차단, Critic P1-5).
- 비-atomic write race는 읽기전용 + `from_utf8_lossy`로 무해.
- AC-X6: 정상상태 stat 1회 비용 측정 기록.

## 8.1 tail/head 경계읽기
- **tail 256KB**(실측: 마지막 token_count gap max 14KB, 단일 라인 max 132KB → 안전마진 충분). EOF 역방향 청크, **첫 부분 라인 폐기**(개행 경계 정렬), `from_utf8_lossy`. 역방향이라 처음 만난 token_count=최신.
- **head 16KB**: 첫 줄 session_meta + 첫 turn_context(baseline model/effort). tail에 더 최신 turn_context 있으면 그것 우선.
- token_count 전무(신생/exec) → ctx/rate `None`(부분/생략, AC-X5).

## 8.2 config (`[codex]` 신설 — 현재 부재 확인)
`CodexConfig { enabled: bool, freshness_minutes: u64, scan_days: usize }`, `#[serde(default)]` + `impl Default`(`Config`에 `pub codex` 추가 + `Config::default` 갱신, 기존 `config.rs:96-202` 패턴).
- 기본값(확정): `enabled=true`, `freshness_minutes=240`, `scan_days=3`.

---

## 9. 실패 안전

CODEX_HOME 부재 · sessions 없음 · cwd 불일치 · freshness 초과 · **모호(≥2)** · originator=exec · JSON 깨짐 · token_count/turn_context 누락 · cli_version 드리프트 → 전부 **무패닉·무블록 → enrich 생략 → 기존 lterm 출력**(model="codex").

---

## 10. Pre-mortem (deliberate — Critic REJECT 사유 해소)

- **S1 잘못된 세션 매칭**(동시 동일 cwd / scan_days 밖 활성): → §5 모호성 생략 + originator TUI 필터. 복구: payload 앵커(사용자 재결정 시). 조기경보: 동일 cwd 2+ fresh 후보 빈발, ctx%가 실제와 괴리.
- **S2 codex_exec 혼입**(token_count/turn_context 없음): → §5 originator 대화형 필터 + 누락데이터 생략. 조기경보: 후보가 exec 세션.
- **S3 cli_version 필드 드리프트**(비공개 내부 포맷, 예 `0.137.0`): → lenient serde Option, 무패닉 정직 생략(AC-X7). 복구: 버전별 어댑터(향후). 조기경보: 필드 경로 부재로 항상 생략.
- **S4 비용/IO 회귀**(매 tick 디스크): → §8 디스크 캐시(정상상태 stat 1) + 바운디드 스캔 + `enabled=false` 단락. 조기경보: N≥2 attach tick 지연(AC-X6).
- **S5 compaction ctx% 급락**(실측 91%→10%, auto-compact): → Phase 2-1은 마지막 token_count 그대로 표시(정직). 향후 `~` 근사 마커(§13 follow-up 명문화). 조기경보: ctx 급락 직후 Codex TUI와 괴리.

---

## 11. 테스트 (unit/integration/e2e/observability — 측정 가능 AC)

**Unit(순수, `codex.rs`)**
- `parse_token_count`: **info 중첩 정확**(AC-X3, 27.5% fixture) / `window_minutes` 식별(300→5h, 10080→주간, AC-X4) / **`total_token_usage` 미사용**(210% fixture 금지, AC-X2) / rate_limits 부재→빈 / plan 추출.
- `compute_context_percentage`: window==0 → None.
- `parse_session_meta`: cwd/originator 추출, exec 식별.
- `parse_turn_context`: model/effort 부분·누락 안전.
- 깨진/미상 cli_version 변형 → None, **무패닉**(AC-X7).

**Unit(IO, temp fixture)**
- `find_codex_candidates`: 단일 정상 / **동일 cwd 2개→모호**(AC-X1) / stale 제외 / scan_days 밖 미발견 / **exec 제외**(AC-X5) / cwd 정규화(trailing slash).
- `extract_from_file`: head+tail 결합 / 거대 레코드(132KB) 상한 안전 / 비-UTF8 lossy / token_count 없는 신생→부분.

**Integration**
- `maybe_enrich`: agent≠codex→무변경 / `enabled=false`→무변경+IO 0 / **단일 후보→model·ctx·codex 설정** / **모호→무변경**.
- 디스크 캐시: 2회차 호출 정상상태 **stat 1회**(AC-X6).

**E2E**
- temp `CODEX_HOME`에 합성 세션 + `echo '{"source":"lterm","agent":"codex","cwd":...}' | understatus render --source lterm --oneline` → 1행에 풀 프로필.
- 미매칭/모호/disabled → `--source lterm` 기존 출력과 **바이트 동일**(AC2 회귀).

**불변식**: `render_has_no_bold_escape`, `render_no_color_env_has_no_escape_bytes` 유지. Claude 경로 비트 동일.

**툴체인**: rustup `export PATH="$HOME/.cargo/bin:$PATH"`로 `test`/`clippy -D warnings`/`fmt --check`.

---

## 12. Acceptance Criteria

- **AC1**: 단일 후보 세션 → model=실모델·ctx%·5h·wk·plan·effort 표시.
- **AC2**: 미매칭·disabled·**모호**·깨짐 → `--source lterm` 기존 출력과 **바이트 동일**.
- **AC3**: 전체 파싱 0회(읽기 ≤ head16KB + tail256KB + 후보 첫줄들), 무패닉.
- **AC4**: `cargo test`/`clippy -D warnings`/`fmt --check` 클린(rustup).
- **AC-X1**: 동일 cwd 2+ fresh 후보 → ctx/rate 생략(fail-wrong 차단).
- **AC-X2**: `total_token_usage` 기반 계산 경로 부재(210% fixture 회귀).
- **AC-X3**: `payload.info.last_token_usage.total_tokens` 중첩 정확 파싱.
- **AC-X4**: rate_limits `window_minutes`로 5h/주간 식별.
- **AC-X5**: codex_exec(token_count 부재) → 무패닉 생략.
- **AC-X6**: 캐시 정상상태 stat 1회(비용 바운드 측정).
- **AC-X7**: cli_version 드리프트/깨진 포맷 → 무패닉 생략.

---

## 13. 순서 / ADR / 열린 결정

**순서**: (1) `codex.rs` 순수 파서+단위테스트 → (2) `find/extract` IO+fixture → (3) 디스크 캐시(chain 재사용) → (4) `[codex]` config → (5) `ClaudeInput.codex`+`collect_segments` 세그먼트 → (6) `main.rs` 배선(enrich를 `load_config` 뒤, **`Source::Lterm` 한정** 호출) → (7) E2E. feature 브랜치, main 직접 커밋 금지, lint/test 통과 후 PR.

**ADR**
- **Decision**: O-Integrate=O2(`ClaudeInput.codex: Option<CodexExtras>`, render frozen 보존) + O-Match=Solution A(모호→생략) + O-Cost=디스크 캐시(chain 재사용, 모호 비캐시).
- **Drivers**: D1(정확/정직) > D2(비용) > D3(계약 정합).
- **Alternatives considered**: O1(render 시그니처 변경 — frozen 위반 기각) / payload 앵커(정확하나 lterm 변경 = 사용자 기각) / 무캐시 풀스캔(틱당 113ms 기각) / 무대책 cwd+mtime(fail-wrong 기각).
- **Why chosen**: 사용자 "lterm 무변경" 결정을 존중하면서 fail-wrong을 정직한 저하로 닫고, frozen 계약·바운디드 비용을 동시 보존.
- **Consequences**: (+) fail-safe·바운디드·모듈 격리·확장(향후 gemini). (−) 동시 동일 cwd codex 시 풀 프로필 미표시(정직 트레이드오프) · scan_days 밖 장기 세션 미발견 · ClaudeInput에 codex 옵션 1필드.
- **Follow-ups**: payload 앵커(사용자 재결정 시 정확 매칭) · compaction `~` 근사 마커 · Gemini/기타 에이전트 세그먼트 · window_minutes별 동적 라벨 · cli_version 어댑터.

**확정된 결정(사용자 승인 2026-06-06)**: (a) `[codex] enabled` 기본 **on**. (b) freshness **240분**. (c) plan/effort **bare value**(`pro`/`xhigh`, 라벨 없음). (d) scan_days **3**. 실행 경로: **team**.
