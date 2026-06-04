# 설계 문서 — 화려한 테마 (색 프리셋 4종 + bold 펄스 채널)

> 작성일: 2026-06-04 · 제품: understatus (디렉터리명 `statusticon`은 레거시)
> 상태: 브레인스토밍 합의 완료, 사용자 spec 리뷰 대기
> 선행 문서: `2026-06-03-themes-and-interval-design.md` (v0.2.0 테마 시스템 토대)

## 1. 목표 (Goals)

understatus 사용자가 **더 화려한 테마**를 쓸 수 있게 한다. 단, understatus의 핵심 정체성("차분하고
눈에 띄지 않는 statusline")을 **옵트인 경계 안에서만** 깬다.

2단계로 나눈다:

- **Phase 1 (먼저 출시 가능)** — 화려한 **색 프리셋 4종**을 추가한다: `neon` / `aurora` / `sunset` /
  `spectrum`. 기존 calm 동작 모델(글리프 고정, ≥90%에서만 호흡, COLOR-ONCE)은 **그대로** 두고 **색만**
  화려하게. `render.rs`/`theme.rs` 무변경, 순수 데이터 추가. (총 테마 5종 → 9종)
- **Phase 2 (별도 단계)** — `pulse_style`을 실제 시각 채널로 승격해 **화려한 펄스 동작**을 구현한다:
  `calm`(기본·현행) / `flash` / `hue` / `swap`. 발동 구간과 효과를 모두 **사용자 설정**으로 고를 수 있다.

## 2. 비목표 (Non-Goals)

- ANSI bold(`\x1b[1m`) 사용 — 여전히 **절대 금지**(`render_has_no_bold_escape` 불변식 유지). "화려함"은
  색/글리프의 드라마로만 표현한다.
- 라이트/다크 터미널 자동 적응 — 모든 테마는 dark 터미널 기준.
- `pulse_style`/테마 외의 GUI/TUI 설정 화면.
- 스키마(설정 필드 타입) 변경 — Phase 1·2 모두 기존 `Config` 필드만 사용(테마=8개 키 묶음, 펄스=문자열 셀렉터).
- Gemini/Codex 어댑터(별도 차단 이슈).

---

## 3. Phase 1 — 화려한 색 프리셋 4종

모든 테마는 기존 config 필드만으로 표현된다(스키마 변경 없음). 각 테마 = `THEME_KEYS` 8개 필드의 묶음.
글리프는 전부 **단일 셀 폭**(이모지 아님). `pulse_style`은 **Phase 1에서 전부 `calm`**(Phase 2에서 화려한
4종을 bold 기본으로 전환 — §5.4).

| 필드 | neon | aurora | sunset | spectrum |
|---|---|---|---|---|
| 콘셉트 | 네온 사이버펑크 | 오로라(청록→보라) | 노을(골드→퍼플) | 밴드별 무지개 |
| `load_glyphs` | `░ ▒ ▓ █ █` | `▁ ▃ ▅ ▆ █` | `· ∙ • ● ◉` | `▁ ▂ ▄ ▆ █` |
| `pulse_style` | `calm`¹ | `calm`¹ | `calm`¹ | `calm`¹ |
| `band_tints` | `#2bd6ff #1ea0ff #7c5cff #c33cff #ff2bd0` | `#2ad6a0 #1fb6b0 #2f9fe0 #6c7cf0 #b46cf0` | `#ffd166 #ff9e4f #ff6b6b #ef476f #c44ad0` | `#2fd36b #d4d13e #f0922e #e8443a #d23ad0` |
| `pulse_palette` | `#ff2bd0 #7a1f8a` | `#b46cf0 #5a3a8a` | `#ef476f #8a2a48` | `#d23ad0 #7a1f78` |
| `label_color` | `#6b7c99` | `#6b7c8a` | `#8a7a6f` | `#6b7280` |
| `separator` | `" · "` | `" · "` | `" · "` | `" · "` |
| `separator_color` | `#2a3550` | `#2a3848` | `#4a3a40` | `#3b4048` |
| `hud_seam` | `│` | `│` | `│` | `│` |

¹ Phase 1에서는 render가 `pulse_style`을 모르므로 calm. Phase 2 출시 시 화려한 4종의 기본을
bold(neon·spectrum→`hue`, aurora·sunset→`flash`)로 전환한다(§4.4). Phase 1 단독 출시 시에도 각 테마는
자기 `pulse_palette`로 ≥90%에서 calm 호흡하므로 그 자체로 완성된 색 테마다.

> **hex는 시안(始案)이다.** 최종값은 §6 절차(Open Design 갤러리 + 실제 바이너리 ANSI 출력)로 튜닝하며,
> 그때 bar 배경 `#0e1017` 대비 ≥ 3:1(밴드 틴트 + 펄스 low 끝점)을 실측 검증한다. 밴드 0–3 명도 단조 증가 유지.

### 3.1 변경 (themes.rs / config.rs / main.rs)

- `src/themes.rs`:
  - `CATALOG`에 4개 항목 추가(출시 순서: calm·mono·vivid·ember·emoji **다음** neon·aurora·sunset·spectrum).
  - `neon_preset()`/`aurora_preset()`/`sunset_preset()`/`spectrum_preset()` 추가 + `preset()` match 갈래 추가.
  - `catalog_order_is_release_order` 테스트를 9종 순서로 갱신.
  - 기존 테스트(`all_presets_have_5_band_tints_and_glyphs`, `all_preset_hex_are_valid`,
    `catalog_matches_is_known`)는 catalog 순회라 **자동 확장** — 새 테마도 같은 불변식으로 검증된다.
- `src/config.rs`: **무변경**(테마 해석은 catalog/preset 기반, 이미 일반화됨).
- `src/main.rs`: **무변경**(`theme`/`themes` 명령은 `themes::catalog()` 기반이라 자동 반영).
- `README.md`: 테마 갤러리 표에 4종 추가.

Phase 1은 이것으로 끝 — render 경로 손대지 않으므로 161개 테스트가 그대로 통과하고, 새 테마는
catalog 불변식 테스트로만 검증된다.

---

## 4. Phase 2 — bold 펄스 채널 (`pulse_style` 승격)

현재 `pulse_style`은 전 테마 `"calm"`이며 `pick_emoji`/`pulse_color` 어디서도 분기에 쓰이지 않는
**데드 데이터**다. Phase 2는 이를 실제 시각 채널로 승격한다.

### 4.1 효과 선택 — `pulse_style` 값 4종

| 값 | 글리프 | 틴트 동작 | 비고 |
|---|---|---|---|
| `calm` (기본) | 고정 | `pulse_palette` 두 끝점 사이 **휘도 호흡**(hue 불변, 현행) | 기존 5종 + 미설정 = 전부 calm. **현행과 100% 동일.** |
| `flash` | 고정 | 같은 두 끝점, **더 가파른 곡선 + 넓은 진폭**(punchy) | aurora·sunset 기본 |
| `hue` | 고정 | `pulse_palette[0]`의 **hue를 주기 동안 360° 회전**(S/L 유지) → 무지개 시머 | neon·spectrum 기본 |
| `swap` | **교대** | hue 순환 + 위상마다 글리프 **모양 교대**(◆↔◇ 등 내장 맵) | "글리프 고정" 불변식을 이 스타일에서만 해제. 기본값 아님(순수 옵트인). |

알고리즘(전부 `now_ms` 위상 기반 **순수 함수** 유지, frame-per-call):

- **flash**: 기존 `wave = (sin(2π·phase)+1)/2`를 `wave' = wave^k`(예 k≈2.2)로 감마 처리해 어두운 구간을
  길게·밝은 스파이크를 짧게. LERP 끝점은 calm과 동일(`pulse_palette[0..1]`). hue 불변.
- **hue**: 기준색 `base = pulse_palette[0]`을 HSV로 변환 → `H' = (H + 360·phase) mod 360`, S·V 유지 →
  RGB. `pulse_palette`/`band_tints`는 그대로 두되 base S/V만 차용. (구현: 소형 RGB↔HSV 헬퍼를 `theme.rs`에 추가.)
- **swap**: `hue`의 틴트 + `pick_emoji`가 `phase < 0.5`면 band 글리프, 아니면 **alt 글리프**를 반환.
  alt는 내장 맵(`◆→◇`, `●→○`, `█→░`, `▆→▂`, `◉→○` …)에서 조회하고 매핑 없으면 원본 유지(no-op).

**불변식 유지**: 어떤 스타일도 ANSI bold를 쓰지 않는다 → `render_has_no_bold_escape` 통과.
`calm` 경로 미변경 → `pulse_color_*`, `pick_emoji_*`, `render_crit_pulse_breathes_terracotta` 등 기존
테스트 전부 통과. 펄스 **OFF**이면(저부하) 모든 스타일이 정적 밴드 틴트로 동일(호흡 자체가 없음).

### 4.2 발동 구간 — 기존 임계값 노출(새 코드 최소)

`pulse_gate`는 이미 `cpu_percent` vs `pulse.pulse_on_threshold`(90)/`pulse_off_threshold`(80)만 본다.
**발동 구간 = 이 두 값**이며 이미 config.toml `[pulse]`로 조정 가능하다. Phase 2는:

- 이 두 키를 **README/help/갤러리에 문서화**(예: `pulse_on_threshold = 75`로 ≥75%부터, `= 0`으로 상시).
- (선택) `install`/`pulse` 명령에서 프리셋 단축 제공: `--pulse-range <crit|high|always>` →
  각각 `(90,80)`/`(75,65)`/`(0,0)`을 기록(편의 래퍼, 내부적으론 기존 두 키만 씀).

### 4.3 새 명령 — `understatus pulse <style>`

`theme` 명령과 대칭. config.toml `[pulse].pulse_style`만 교체(다른 키 보존, install의 병합 헬퍼 재사용).

```
understatus pulse <calm|flash|hue|swap>   펄스 스타일 교체 (config.toml만 수정)   [신규]
understatus pulse                          현재 펄스 스타일 출력                    [신규]
```

- 유효성: 4개 값만 허용. 실패 시 하드 에러 + 목록(테마 명령과 동일 패턴).
- settings.json은 건드리지 않음(렌더 시 config에서 읽힘 → 다음 렌더부터 적용).

### 4.4 화려한 4종의 bold 기본 전환

Phase 2 출시 시 `themes.rs`에서 화려한 4종의 `pulse_style` 기본값을 전환한다(기존 5종은 calm 유지):

| 테마 | Phase 1 | Phase 2 기본 |
|---|---|---|
| neon | calm | `hue` |
| spectrum | calm | `hue` |
| aurora | calm | `flash` |
| sunset | calm | `flash` |

사용자는 `understatus pulse <style>` 또는 config.toml로 언제든 재정의 가능(개별 키 > 프리셋 우선순위 유지).

---

## 5. 데이터 흐름 (Phase 2 렌더)

```
렌더 (매 호출):
  Claude Code → stdin JSON → understatus render
    └→ load_config(): theme 해석 → pulse_style 포함 테마 키 채움(미설정만)
        └→ render.rs::render(cfg, now_ms, pulse_on)
            └→ glyph_tint(cpu%, now_ms, pulse_on, cfg)
                └→ pulse_color(..., cfg):  match cfg.pulse.pulse_style
                     ├ "calm"  → 현행 휘도 LERP
                     ├ "flash" → 감마 처리 휘도 LERP
                     ├ "hue"   → base hue 360° 회전
                     └ "swap"  → hue 회전(+ pick_emoji가 글리프 교대)
            (pulse_on=false면 스타일 무관, 정적 band_tint)
```

`pulse_color`/`pick_emoji`의 `// CONTRACT: signature is frozen` 주석은 시그니처를 **유지**하면 충족된다
(인자에 이미 `now_ms`/`pulse_on`/`cfg` 전부 있음 → 시그니처 변경 불필요, 본문만 확장).

## 6. Open Design 활용 (시각 튜닝)

현재 OD 데몬에 understatus 갤러리가 없으므로(과거 `understatus-themes.html`은 stale) **새로 생성**한다.

- 새 아티팩트 `understatus-themes.html`: **9종 × 5밴드** 정적 미리보기 한 화면 + Phase 2 펄스 스타일
  (calm/flash/hue/swap)을 CSS 애니메이션으로 시연하는 섹션.
- **ground truth는 바이너리 ANSI 출력.** 각 테마/스타일을 실제 빌드로 렌더(`COLORTERM=truecolor
  ./target/release/understatus < fixture | cat -v`)해 OD 목업과 대조하며 hex를 확정한다.
- 갤러리는 미적 합의/문서용이며 코드의 단일 소스는 `themes.rs`다(갤러리 ↔ 프리셋 값 일치를 사람이 확인).

## 7. 에러 처리 / 안전성

- 런타임: 미지 `pulse_style` → calm 폴백(stderr 경고, 패닉 금지) — 미지 theme 폴백과 동일 패턴.
- `pulse` 명령: 잘못된 값 → 하드 에러 + 유효 목록 + 비0 종료코드.
- hue/HSV 변환: 0 division/NaN 방어, 결과 채널 clamp(0–255). 위상 계산은 기존 `pulse_phase`(0 division 방어) 재사용.
- COLOR-ONCE/색상 비활성(`NO_COLOR`/`mode=none`) 경로 불변 — 스타일과 무관하게 ANSI 미출력 유지.

## 8. 테스트 계획

**Phase 1**
- `themes.rs`: neon/aurora/sunset/spectrum 프리셋 값 정확성; catalog 9종 순서; 기존 hex/길이/`is_known`
  불변식이 새 테마에도 적용됨(자동); calm 회귀 테스트 불변.
- `config.rs`: `theme="neon"` → neon 틴트/글리프; 사용자 키 override 우선; 미지 테마 calm 폴백(기존 유지).
- 회귀: 161개 전부 통과(render 경로 미변경).

**Phase 2**
- `theme.rs`:
  - `flash`: calm 대비 더 가파른 곡선(같은 끝점, 중간 위상에서 더 어두움) — 결정적 스냅샷.
  - `hue`: 위상 0/0.25/0.5/0.75에서 hue가 회전(서로 다른 RGB), S/V 근사 보존; phase 0 ≈ base.
  - `swap`: `pick_emoji`가 phase<0.5 band 글리프 / phase≥0.5 alt 글리프; 매핑 없는 글리프는 원본 유지.
  - `calm` 경로 기존 테스트 전부 불변(회귀 게이트).
  - RGB↔HSV 라운드트립 오차 허용범위.
- `config.rs`: `pulse_style="hue"` 파싱; 미지 스타일 → calm 폴백.
- `main.rs`: `pulse <style>` 검증/기록(병합 헬퍼로 다른 키 보존); `pulse`(인자 없음) 현재값 출력.
- `render.rs`: 각 스타일에서 `render_has_no_bold_escape` 유지; 펄스 OFF면 스타일 무관 정적 틴트 동일.
- 화려한 4종 bold 기본 전환 후: neon/spectrum `pulse_style=="hue"`, aurora/sunset `=="flash"` 단위 검증.

## 9. 변경 파일

| 파일 | Phase | 변경 |
|---|---|---|
| `src/themes.rs` | 1 | neon/aurora/sunset/spectrum 프리셋 + catalog; (P2) 화려한 4종 기본 pulse_style 전환. |
| `src/theme.rs` | 2 | `pulse_color`에 `pulse_style` 분기(flash/hue) + RGB↔HSV 헬퍼; `pick_emoji`에 swap 글리프 교대 + alt 맵. |
| `src/config.rs` | 2 | (필요 시) 미지 `pulse_style` 경고 폴백; 그 외 무변경. |
| `src/main.rs` | 2 | `pulse <style>`/`pulse` 서브커맨드, help 갱신; (선택) `--pulse-range` 래퍼. |
| `src/install.rs` | 2 | (선택) `--pulse-range` 기록 시 `[pulse]` 임계값 병합. |
| `README.md` | 1·2 | 테마 갤러리 4종; (P2) pulse 스타일/발동 구간/`pulse` 명령 문서화. |
| Open Design `understatus-themes.html` | 1·2 | 신규 갤러리(9종 × 5밴드 + 펄스 스타일 시연). |

## 10. 합의된 결정 (브레인스토밍)

1. 범위 = **둘 다, 단계적 구현**. 구현은 Phase 1 먼저 완료·검증 후 Phase 2 진행하되, **출시는 두 Phase를
   묶어 단일 릴리스**(예 v0.3.0)로 낸다.
2. 화려한 색 테마 = **neon + aurora + sunset + spectrum** 4종.
3. bold 펄스 **발동 구간** = 설정값(기본 90%, `pulse_on/off_threshold` 노출).
4. bold 펄스 **효과** = 설정값(`pulse_style`: calm/flash/hue/swap).
5. 화려한 4종은 **bold 기본**(neon·spectrum→hue, aurora·sunset→flash); 기존 5종은 calm 유지; 전부 재정의 가능.
6. ANSI bold 금지 불변식 유지 — 화려함은 색/글리프로만.

## 11. 출시 (이번 spec 범위 밖, 참고)

- **출시 단위 = Phase 1 + Phase 2 묶어 단일 릴리스**(예 v0.3.0) + 4채널(crates.io/Homebrew/npm/GitHub).
  구현 순서만 Phase 1 → Phase 2이고, 릴리스는 둘 다 머지된 뒤 한 번에 컷. npm publish/deprecate는
  패스키(EOTP) 필요 → 사용자 수동(HANDOFF 참조).
- 버전 3종(`Cargo.toml`/`npm/package.json`/`npm/install.js`) 동시 범프 규칙 유지.
