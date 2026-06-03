# 설계 문서 — 테마 선택 + 설치 시 갱신 주기 입력

> 작성일: 2026-06-03 · 제품: understatus (디렉터리명 `statusticon`은 레거시)
> 상태: 브레인스토밍 합의 완료, 구현 계획 대기

## 1. 목표 (Goals)

설치하는 사용자가:

1. **설치 시 statusline 갱신 주기(`refreshInterval`)를 입력**할 수 있다. 기본값 `5`.
2. **여러 테마 중 하나를 골라** 쓸 수 있다. 기본 테마 `calm`.
3. 설치 후에도 **테마를 쉽게 갈아끼울** 수 있다 (`understatus theme <name>`).

부가 목표:

- **테마 추가가 쉬운 구조** — 향후 더 재미있고 화려한 테마를 프리셋 1개 추가만으로 늘릴 수 있어야 한다.
- 기존 calm 디자인 정체성(**COLOR-ONCE**, 모양=1차 채널, hue 불변 호흡)을 모든 테마가 준수.
- 기존 동작/설정/설치 라운드트립을 깨지 않는다(하위 호환).

## 2. 비목표 (Non-Goals)

- 실제 "bold/화려한 펄스"(hue 스윙·글리프 깜빡임) 구현 — 현재 `pulse_style="bold"`는 no-op이며, 이번 범위에서 구현하지 않는다. **향후 확장**으로 남긴다.
- 라이트/다크 터미널 자동 적응 — 테마는 dark 터미널 기준. (light 틴트 값은 디자인 자료에만 보관.)
- 런타임 테마 핫스왑 외의 GUI/TUI 설정 화면.
- Gemini/Codex 어댑터 (별도 차단 이슈).

## 3. 출시 테마 (5종)

모든 테마는 기존 config 필드만으로 표현된다(스키마 변경 없음). 각 테마 = 아래 8개 필드의 묶음.

| 필드 | calm (기본) | mono | vivid | ember | emoji |
|---|---|---|---|---|---|
| `load_glyphs` | `○ ▁ ▄ ▆ ◆` | `○ ▁ ▄ ▆ ◆` | `░ ▒ ▓ █ █` | `· ∙ • ● ◉` | `😌 🙂 😅 🥵 🔥` |
| `pulse_style` | `calm` | `calm` | `calm` | `calm` | `calm` |
| `band_tints` | `#5a6878 #6d8296 #86a0b4 #9fbfce #b87848` | `#636363 #7e7e7e #9c9c9c #bdbdbd #e8e8e8` | `#2f9150 #3fb083 #cda23e #f0a24e #e34a3a` | `#7a6450 #96714f #b08355 #c79a63 #cf5a48` | `#6e7d92 #86978f #a39a78 #c6a35c #e0683c` |
| `pulse_palette` | `#b87848 #7a5030` | `#e8e8e8 #9c9c9c` | `#e34a3a #bf4135` | `#cf5a48 #a8483a` | `#e0683c #a04528` |
| `label_color` | `#6b7280` | `#6b7280` | `#6b7280` | `#7a6f63` | `#6b7280` |
| `separator` | `" · "` | `" · "` | `" · "` | `" · "` | `" · "` |
| `separator_color` | `#3b4048` | `#3b4048` | `#3b4048` | `#4a4239` | `#383d45` |
| `hud_seam` | `│` | `│` | `│` | `│` | `│` |

**검증 완료**: 모든 `band_tints`(및 `pulse_palette` low 끝점)가 bar 배경 `#0e1017` 대비 ≥ 3:1 (WCAG 상대휘도 실측). 밴드 0–3 명도 단조 증가. calm 호흡은 hue 불변(명도만 보간). 시각 자료: Open Design 프로젝트 `status_ticon` → `understatus-themes.html`.

## 4. 아키텍처

### 4.1 신규 모듈 `src/themes.rs`

테마 프리셋의 단일 소스. **새 테마 추가 = 이 파일에 항목 1개 추가**.

```rust
/// 테마가 소유하는 시각 필드 묶음(Config의 부분집합).
pub struct ThemePreset {
    pub load_glyphs: Vec<String>,
    pub pulse_style: String,
    pub band_tints: Vec<String>,
    pub pulse_palette: Vec<String>,
    pub label_color: String,
    pub separator: String,
    pub separator_color: String,
    pub hud_seam: String,
}

/// 알려진 테마 이름 → 프리셋. 미지의 이름은 None.
pub fn preset(name: &str) -> Option<ThemePreset>;

/// 표시/검증용 (이름, 한 줄 설명) 목록. 출시 순서대로.
pub fn catalog() -> &'static [(&'static str, &'static str)];

/// 유효 테마 이름인지.
pub fn is_known(name: &str) -> bool;
```

- `calm` 프리셋의 값은 현재 `Config::default()`의 테마 필드와 **정확히 동일**해야 한다(회귀 방지: 단위 테스트로 강제).
- `pulse_style`은 전부 `"calm"`.

### 4.2 설정 해석 (`src/config.rs`)

**우선순위: 사용자가 명시한 개별 키 > 테마 프리셋 > calm 폴백.**

변경점:

1. `Config`에 최상위 필드 추가: `pub theme: String` (`#[serde(default = ...)]` → 기본 `"calm"`).
   - config.toml 최상위 키: `theme = "vivid"`.
2. `parse_config_str`에서 해석 단계 추가:
   - (a) 기존처럼 `toml::from_str::<Config>` → calm 기본값으로 채워진 `Config` 확보.
   - (b) 원본을 `toml::Value`로도 파싱해 **테마 소유 키가 실제로 적혀 있는지** 검사.
   - (c) `themes::preset(config.theme)` 조회. `None`(미지 테마)이면 stderr 경고 후 calm 프리셋 사용(패닉 금지).
   - (d) 테마 소유 키 중 **원본에 없던 것만** 프리셋 값으로 덮어쓴다.

**테마 소유 키 목록** (이것만 프리셋이 채움):

| 섹션.키 | Config 경로 |
|---|---|
| `cpu.load_glyphs` | `config.cpu.load_glyphs` |
| `pulse.pulse_style` | `config.pulse.pulse_style` |
| `color.band_tints` | `config.color.band_tints` |
| `color.pulse_palette` | `config.color.pulse_palette` |
| `color.label_color` | `config.color.label_color` |
| `color.separator` | `config.color.separator` |
| `color.separator_color` | `config.color.separator_color` |
| `color.hud_seam` | `config.color.hud_seam` |

> 키 존재 검사는 `toml::Value`에서 `value.get("color").and_then(|t| t.get("band_tints")).is_some()` 형태.

**핵심**: `Config`의 필드 타입은 그대로 구체값(`Vec<String>`, `String`). 따라서 **`render.rs`/`theme.rs` 등 다운스트림은 일절 변경 없음**(검증 완료 — 둘 다 `cfg.color.*`, `cfg.cpu.load_glyphs`를 구체값으로 읽음).

`theme = "calm"`(또는 키 부재)이면 프리셋=calm=기본값이라 동작은 현재와 100% 동일 → **하위 호환**.

### 4.3 CLI (`src/main.rs`) — 인자 파싱 + 대화형 프롬프트

현행 수동 디스패치(clap 미사용)를 유지하되 플래그 파싱을 추가한다.

**서브커맨드:**

```
understatus [render]              stdin JSON → statusline 한 줄 (기본)
understatus install [옵션]        비파괴 설치 (+ 주기/테마 선택)
understatus uninstall             원본 복원
understatus theme <name>          설치 후 테마 교체 (config.toml만 수정)  [신규]
understatus themes                사용 가능한 테마 목록 출력            [신규]
understatus --help | --version
```

**install 옵션:**

```
--interval <N>    refreshInterval 초(정수 ≥ 1). 미지정 시 프롬프트/기본 5.
--theme <name>    테마 이름. 미지정 시 프롬프트/기본 calm.
--yes, -y         프롬프트 생략(TTY여도). 플래그/기본값 사용.
```

**대화형 흐름 (install):**

1. **TTY 판정**: `std::io::stdin().is_terminal()` (std `IsTerminal`, MSRV 1.75 충족).
2. 각 항목별로:
   - 해당 플래그가 주어졌으면 → 그 값 사용(프롬프트 안 함).
   - 아니고 TTY + `--yes` 아님 → 프롬프트.
   - 아니면(비TTY 또는 `--yes`) → 기본값.
3. **interval 프롬프트**: `Refresh interval in seconds [5]: `
   - 빈 입력 = 기본 5. 정수 ≥ 1만 허용, 위반 시 재프롬프트(최대 3회, 이후 기본값 5로 진행).
4. **theme 프롬프트**: 번호 메뉴 + 이름 둘 다 허용.
   ```
   Theme:
     1) calm   (기본) 차가운 blue-grey + 테라코타 호흡
     2) mono   무채색, 제로 색상
     3) vivid  신호등 색 + 블록 글리프
     4) ember  따뜻한 앰버/테라코타 단색
     5) emoji  이모지 표정 램프 (2칸 폭)
   Select [1]: 
   ```
   - 빈 입력 = calm. 번호 또는 이름 허용. 미지값 재프롬프트.

**플래그 우선순위**: 명시 플래그 > 프롬프트 입력 > 기본값.

**검증 (입력 살균):**
- interval: 정수 파싱 + `≥ 1`. 0/음수/비정수 거부.
- theme: `themes::is_known` 통과만. 실패 시 명확한 에러(유효 목록 표시) — **설치 시엔 하드 에러**(런타임 config 로드의 폴백과 구분).

### 4.4 설치가 기록하는 값 (`src/install.rs`)

`install()` 시그니처 변경: `install(interval: u64, theme: &str) -> Result<()>` (main이 플래그/프롬프트로 해석한 값 전달).

1. **settings.json**: 기존 `apply_install`에 `refresh_interval = interval` 전달(이미 파라미터화됨). → `statusLine.refreshInterval = interval`.
2. **config.toml** (기존 `merge_chain_command` 패턴을 일반화한 헬퍼로, 다른 키 보존하며 병합 기록):
   - 최상위 `theme = "<선택>"`
   - `[refresh] interval_seconds = <선택>` (런타임/재설치/일관성용 단일 소스)
   - 기존 `[chain].chain_command` 보존 로직 그대로.
3. **호흡 불변식 경고(자동 보정 안 함)**: `pulse_period_seconds / interval < 6`이면(즉 현재 기본 30 기준 `interval > 5`) stderr로 경고 1줄:
   > `understatus: refreshInterval=10s에서는 테라코타 호흡이 끊길 수 있습니다(권장: config.toml [pulse] pulse_period_seconds ≥ 60).`
   - `pulse_period_seconds`는 건드리지 않는다(사용자 결정).

### 4.5 `understatus theme <name>` / `understatus themes`

- `themes`: `themes::catalog()`를 사람이 읽기 좋게 출력(현재 적용 테마 표시). config 읽기 전용.
- `theme <name>`:
  - `is_known` 검증(실패 시 하드 에러 + 목록).
  - config.toml의 최상위 `theme` 키만 교체(다른 키 보존, install의 병합 헬퍼 재사용).
  - settings.json은 **건드리지 않음**(테마는 렌더 시 config에서 읽힘 → 다음 렌더부터 적용).
  - 성공 메시지: `understatus: theme를 'vivid'로 변경했습니다.`

## 5. 데이터 흐름

```
설치:
  understatus install --theme vivid --interval 5
    └→ main: 플래그 파싱 → (TTY면 미입력 항목 프롬프트) → 검증
        └→ install(5, "vivid")
            ├→ settings.json: statusLine.refreshInterval = 5 (+ command/backup/chain)
            └→ config.toml: theme="vivid", [refresh].interval_seconds=5

렌더 (매 호출):
  Claude Code → stdin JSON → understatus render
    └→ load_config()
        ├→ toml 파싱 (calm 기본)
        ├→ theme="vivid" 해석 → 미설정 테마 키를 vivid 프리셋으로 채움
        └→ Config (구체값) → render.rs (변경 없음) → 한 줄 출력
```

## 6. 에러 처리 / 안전성

- 런타임(`load_config`): 미지 테마 → stderr 경고 + calm 폴백. 깨진 TOML → 기존처럼 기본값 폴백. **패닉 금지**.
- 설치/`theme` 명령: 미지 테마/잘못된 interval → 명확한 에러 메시지 + 비0 종료코드. settings.json 백업/멱등/라운드트립 불변(기존 보장 유지).
- 프롬프트 입력: 잘못된 값 재프롬프트, EOF/읽기 실패 시 기본값으로 안전 저하.

## 7. 테스트 계획

신규/보강:

- `themes.rs`: `preset` 5종 값 정확성; `calm` 프리셋 == `Config::default()` 테마 필드(회귀); `catalog`/`is_known` 일관성; 모든 `band_tints`/`pulse_palette` 길이·hex 형식.
- `config.rs`:
  - `theme="vivid"` + override 없음 → vivid 틴트/글리프.
  - `theme="vivid"` + 사용자 `band_tints` 명시 → 사용자 값 우선(나머지는 vivid).
  - `theme` 키 부재 → calm(현행과 동일, 기존 테스트 유지).
  - 미지 테마 → calm 폴백(경고).
- `install.rs`: config.toml에 `theme`/`interval_seconds` 기록 + 기존 라운드트립/멱등 불변; interval 경고 트리거 조건.
- `main.rs`(또는 분리한 순수 파서): 플래그 파싱·우선순위; interval/theme 검증 함수; 프롬프트 파싱 함수(reader 주입으로 테스트).
- 회귀: 기존 94개 테스트 그대로 통과(calm 기본 경로 불변).

## 8. 변경 파일

| 파일 | 변경 |
|---|---|
| `src/themes.rs` | **신규** — 프리셋 + catalog. |
| `src/config.rs` | `theme` 필드 추가; `parse_config_str`에 테마 해석 단계; 키 존재 검사 헬퍼. |
| `src/main.rs` | 플래그 파싱, TTY 프롬프트, `theme`/`themes` 서브커맨드, help 갱신. |
| `src/install.rs` | `install(interval, theme)` 시그니처; config.toml `theme`/`interval_seconds` 기록 헬퍼(merge 일반화); 호흡 불변식 경고. |
| `README.md` | 설치 플래그/프롬프트, 테마 갤러리, `theme`/`themes` 명령, config `theme` 키 문서화. |
| `docs/` | (선택) 테마 갤러리 프리뷰 이미지/링크. |

## 9. 향후 확장 (이번 범위 밖)

- **bold/화려한 펄스 스타일 실제 구현** — hue 스윙·글리프 깜빡임 옵션. 사용자가 원하는 "더 눈에 띄는" 테마 계열의 토대.
- 추가 테마(네온/레트로/시즌 등) — `themes.rs`에 프리셋만 추가.
- 라이트 터미널 적응 틴트(테마별 light override 값은 이미 디자인 자료에 보관됨).

## 10. 합의된 결정 (브레인스토밍)

1. 설치 입력 = **대화형 + 플래그 둘 다**.
2. 테마 저장 = **이름 참조** (`theme = "name"`, 런타임 해석, 개별 키 override 가능).
3. 출시 테마 = **calm(기본) + mono + vivid + ember + emoji**.
4. 호흡 불변식 = **경고만**(자동 보정 안 함).
5. 테마 전환 = **`understatus theme <name>` + `understatus themes` 추가**.

---

## 11. 버그 수정 (추가 범위 — 같은 작업에 포함)

### 11.1 증상

1. **ctx 표시가 최신이 아님**: statusline의 컨텍스트 사용률이 실제보다 뒤처져 보인다.
2. **세션 간 값 오염**: 여러 Claude 터미널을 동시에 쓰면, 다른 터미널의 값(특히 ctx, 예: 85% 빨강)이 현재 터미널 statusline의 **체인(OMC HUD) 부분**에 잠깐 나타났다가 잠시 후 원복된다. understatus **자체 값이 아니라 OMC 기본 statusline 쪽**에서만 발생.

### 11.2 근본 원인 (코드 확인 완료)

`src/chain.rs`의 `cache_file(name)`이 `~/Library/Caches/understatus/<고정이름>`을 쓰며 **session_id/터미널 구분이 전혀 없다.** 단기 TTL 캐시가 머신 전역으로 공유된다:

| 캐시 파일 | 용도 | 공유 시 문제 |
|---|---|---|
| `chain_output` | 체인 자식(OMC HUD) stdout (TTL `chain_cache_ttl_seconds`, 기본 10s) | 터미널 A가 자기 stdin으로 만든 OMC 출력을 캐시 → TTL 내 터미널 B가 같은 파일을 읽어 **A의 값**을 표시. TTL 만료 후 B가 재생성 → 원복. **증상 1·2의 직접 원인.** |
| `pulse_state` | 펄스 히스테리시스 on/off | 세션 간 공유 시 글리프 펄스 깜빡임 교란(경미). |
| `net_counters` | 네트워크 throughput 델타용 prev 카운터 | 세션 간 공유 시 델타가 다른 터미널 샘플 시점 기준으로 계산되어 throughput 값이 튄다. |
| `battery` (`BATTERY_CACHE_FILE`) | 배터리(IOKit, 30s TTL) | **머신 전역 값이라 공유가 정상 — 수정하지 않는다.** |

understatus 자체 값(CPU/mem/ctx 등)은 매 렌더 stdin에서 새로 계산되므로 오염되지 않는다 → "understatus 값이 아니라 OMC 쪽에 나타난다"는 관찰과 정확히 일치. **버그 1·2는 같은 뿌리(공유된 `chain_output` 캐시)다.**

### 11.3 수정 설계

**세션별 캐시 격리.** `chain_output`/`pulse_state`/`net_counters`를 `session_id`로 키잉:

```
~/Library/Caches/understatus/sessions/<sanitized_session_id>/<name>
```

- `session_id`는 이미 `ClaudeInput.session_id`로 파싱됨. `main.rs`가 렌더 파이프라인에서 각 캐시 함수로 전달.
- `battery`는 전역 경로 유지(머신 단위 값, 공유가 오히려 IOKit 호출 절감).
- **보안(입력 살균)**: `session_id`를 파일명/경로에 쓰기 전 `[A-Za-z0-9_-]`만 남기고 길이 제한(예: 64자). 경로 traversal(`../`) 방지. session_id는 stdin(외부)에서 오므로 신뢰하지 않는 입력으로 취급.
- **폴백**: `session_id`가 `None`/빈 값이면 `sessions/default/` 사용(단일 세션 환경에선 현행과 동일하게 무해).

**시그니처 변경** (기존 `// CONTRACT: signature is frozen`은 초기 병렬 빌드용 계약이며, 근거 있는 버그 수정이므로 해제):

| 함수 | 변경 |
|---|---|
| `chain::run_chain(cmd, raw_stdin, cfg)` | → `run_chain(cmd, raw_stdin, cfg, session_key: &str)` |
| `chain::read_prev_pulse_state()` | → `read_prev_pulse_state(session_key: &str)` |
| `chain::write_pulse_state(on)` | → `write_pulse_state(on, session_key: &str)` |
| `system::sample_system(cfg)` | → `sample_system(cfg, session_key: &str)` (net_counters 키잉용) |
| (신규 헬퍼) | `chain::session_cache_file(session_key, name)` + `sanitize_session_key(raw) -> String` |

`read_named_cache`/`write_named_cache`는 호출부(system.rs)에서 세션 키를 포함한 경로를 쓰도록 조정하거나, 세션 키 인자를 받는 변형을 추가.

**캐시 정리(누적 방지)**: `uninstall`은 캐시 디렉터리 전체 삭제(현행 유지). 런타임 best-effort로 `sessions/` 하위에서 mtime 24h 초과 디렉터리를 정리(경량, 실패 무시).

### 11.4 잔여(설계상, 버그 아님)

- 단일 세션 내 체인 출력 staleness ≤ `chain_cache_ttl_seconds`(기본 10s)는 무거운 OMC 자식을 디커플하기 위한 **의도된** 캐시. 너무 길게 느끼면 config에서 낮추거나, 이번에 추가되는 `--interval`로 갱신 주기 조정.
- statusLine 이벤트/타이머 모델상 최대 `refreshInterval`초 지연은 Claude Code 구조상 불가피.

### 11.5 테스트

- 서로 다른 `session_key` → 서로 다른 캐시 경로(격리 단위 테스트).
- `sanitize_session_key`: `../`, 슬래시, 공백, 과도 길이 → 안전 문자열.
- 같은 `session_key` 라운드트립 유지(기존 캐시 라운드트립 테스트를 세션 경로로 갱신).
- net 델타가 세션별 독립.
- 회귀: battery 캐시는 전역 유지, 기존 동작 불변.

### 11.6 변경 파일 (8절에 추가)

| 파일 | 변경 |
|---|---|
| `src/chain.rs` | 세션 키 경로 헬퍼 + 살균; `run_chain`/`read_prev_pulse_state`/`write_pulse_state` 시그니처에 `session_key`; stale 세션 정리. |
| `src/system.rs` | `sample_system`에 `session_key`; net_counters 세션 키잉(battery는 전역 유지). |
| `src/main.rs` | 렌더 파이프라인에서 `claude_input.session_id`를 살균해 각 캐시 함수로 전달. |
| `src/install.rs` | (영향 시) uninstall의 캐시 디렉터리 삭제 경로 확인 — 현행대로 전체 삭제면 무변경. |
