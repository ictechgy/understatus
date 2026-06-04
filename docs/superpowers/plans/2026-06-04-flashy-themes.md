# 화려한 테마 (색 프리셋 4종 + bold 펄스 채널) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** understatus에 화려한 색 테마 4종(neon/aurora/sunset/spectrum)과 옵트인 bold 펄스 채널(`pulse_style`: calm/flash/hue/swap)을 추가한다.

**Architecture:** Phase 1은 `themes.rs`에 프리셋만 추가하는 순수 데이터 변경(render 무변경). Phase 2는 데드 데이터였던 `pulse_style`을 `theme.rs`의 `pulse_color`/`pick_emoji`에서 실제 시각 채널로 분기시키고, `understatus pulse <style>` 명령(install.rs/main.rs)으로 설정을 노출한다. calm 경로는 일절 바뀌지 않아 기존 161 테스트가 회귀 게이트로 남는다.

**Tech Stack:** Rust 2021, macOS, `toml` 0.8, `serde_json`, `anyhow`. 빌드/테스트는 **rustup 툴체인**으로만(`export PATH="$HOME/.cargo/bin:$PATH"` — homebrew cargo는 깨짐).

**선행 spec:** `docs/superpowers/specs/2026-06-04-flashy-themes-design.md`

**구현 순서 메모:** 출시는 Phase 1+2 묶음이므로, 화려한 4종 프리셋은 **처음부터 bold `pulse_style` 값**(neon·spectrum=`hue`, aurora·sunset=`flash`)을 갖는다. Phase 1 단계(Task 1~3)에서는 render가 아직 `pulse_style`을 분기하지 않으므로 이 값들은 무해하게 무시되고(calm 동작), Phase 2(Task 4~)에서 같은 릴리스 안에 점등된다. 별도 "전환" 태스크가 필요 없다.

**모든 빌드/테스트 명령 앞에 반드시:**
```bash
export PATH="$HOME/.cargo/bin:$PATH"
```

---

## File Structure

| 파일 | 책임 | Phase |
|---|---|---|
| `src/themes.rs` | 테마 프리셋 카탈로그(SSOT). neon/aurora/sunset/spectrum 4개 프리셋 + catalog 항목 추가. | 1 |
| `src/theme.rs` | 펄스/밴드 순수 함수. `pulse_color`에 `pulse_style` 분기(flash/hue) + RGB↔HSV 헬퍼; `pick_emoji`에 swap 글리프 교대 + alt 맵; `PULSE_STYLES`/`is_known_pulse_style`. | 2 |
| `src/install.rs` | config.toml 기록. `validate_pulse_style` + `set_pulse_style` + `set_pulse_style_key`(비-table 섹션 안전). | 2 |
| `src/main.rs` | 서브커맨드 디스패치. `pulse <style>`/`pulse` 추가 + help 갱신. | 2 |
| `src/config.rs` | **무변경.** 테마 해석은 catalog/preset 기반으로 이미 일반화됨; 미지 `pulse_style`은 render `_ => calm`으로 안전 저하(쓰기 경로 하드 검증이 1차 가드). | — |
| `README.md` | 테마 갤러리 4종 + 펄스 스타일/`pulse` 명령 문서. | 1·2 |

설계 단위: 프리셋(데이터)·펄스 알고리즘(순수 함수)·설정 기록(I/O)·CLI 배선을 각각 다른 파일에 격리한다. 각 파일은 한 책임만 진다.

---

## Phase 1 — 화려한 색 프리셋 4종

### Task 1: themes.rs — 프리셋 4종 + catalog

**Files:**
- Modify: `src/themes.rs` (CATALOG 상수, preset 함수들, preset() match, 테스트)

- [ ] **Step 1: 새 프리셋의 값 단언 테스트를 먼저 작성(실패)**

`src/themes.rs`의 `#[cfg(test)] mod tests` 안에 추가:

```rust
    /// 신규 화려한 4종의 핵심 값(글리프·밴드 틴트·펄스 팔레트·기본 pulse_style)을 고정한다.
    #[test]
    fn flashy_presets_core_values() {
        let neon = preset("neon").expect("neon 존재");
        assert_eq!(neon.load_glyphs, vec!["░", "▒", "▓", "█", "█"]);
        assert_eq!(
            neon.band_tints,
            vec!["#2bd6ff", "#1ea0ff", "#7c5cff", "#c33cff", "#ff2bd0"]
        );
        assert_eq!(neon.pulse_palette, vec!["#ff2bd0", "#7a1f8a"]);
        assert_eq!(neon.pulse_style, "hue");

        let aurora = preset("aurora").expect("aurora 존재");
        assert_eq!(aurora.load_glyphs, vec!["▁", "▃", "▅", "▆", "█"]);
        assert_eq!(
            aurora.band_tints,
            vec!["#2ad6a0", "#1fb6b0", "#2f9fe0", "#6c7cf0", "#b46cf0"]
        );
        assert_eq!(aurora.pulse_palette, vec!["#b46cf0", "#5a3a8a"]);
        assert_eq!(aurora.pulse_style, "flash");

        let sunset = preset("sunset").expect("sunset 존재");
        assert_eq!(sunset.load_glyphs, vec!["·", "∙", "•", "●", "◉"]);
        assert_eq!(
            sunset.band_tints,
            vec!["#ffd166", "#ff9e4f", "#ff6b6b", "#ef476f", "#c44ad0"]
        );
        assert_eq!(sunset.pulse_palette, vec!["#ef476f", "#8a2a48"]);
        assert_eq!(sunset.pulse_style, "flash");

        let spectrum = preset("spectrum").expect("spectrum 존재");
        assert_eq!(spectrum.load_glyphs, vec!["▁", "▂", "▄", "▆", "█"]);
        assert_eq!(
            spectrum.band_tints,
            vec!["#2fd36b", "#d4d13e", "#f0922e", "#e8443a", "#d23ad0"]
        );
        assert_eq!(spectrum.pulse_palette, vec!["#d23ad0", "#7a1f78"]);
        assert_eq!(spectrum.pulse_style, "hue");
    }
```

또한 기존 `catalog_order_is_release_order` 테스트를 9종으로 교체:

```rust
    /// catalog 순서가 출시 순서(calm..emoji 다음 neon..spectrum)와 일치해야 한다.
    #[test]
    fn catalog_order_is_release_order() {
        let names: Vec<&str> = catalog().iter().map(|(name, _)| *name).collect();
        assert_eq!(
            names,
            vec![
                "calm", "mono", "vivid", "ember", "emoji", "neon", "aurora", "sunset", "spectrum"
            ]
        );
    }
```

- [ ] **Step 2: 테스트 실패 확인**

Run: `export PATH="$HOME/.cargo/bin:$PATH"; cargo test --lib themes:: 2>&1 | tail -20`
Expected: FAIL — `preset("neon")`가 `None`(`expect` 패닉) + `catalog_order` 불일치.

- [ ] **Step 3: 4개 프리셋 함수 + catalog 항목 + match 갈래 추가**

`src/themes.rs`의 `CATALOG` 상수에 4줄 추가(emoji 다음):

```rust
const CATALOG: &[(&str, &str)] = &[
    ("calm", "차가운 blue-grey + 테라코타 호흡 (기본)"),
    ("mono", "무채색, 제로 색상"),
    ("vivid", "신호등 색 + 블록 글리프"),
    ("ember", "따뜻한 앰버/테라코타 단색"),
    ("emoji", "이모지 표정 램프 (2칸 폭)"),
    ("neon", "네온 사이버펑크 (시안→마젠타, hue 순환)"),
    ("aurora", "오로라 청록→보라 그라데이션 (flash)"),
    ("sunset", "노을 골드→퍼플 (flash)"),
    ("spectrum", "밴드별 무지개 (초록→마젠타, hue 순환)"),
];
```

`emoji_preset()` 다음에 4개 함수 추가:

```rust
/// neon 프리셋. 네온 사이버펑크 — 일렉트릭 시안→마젠타 + 셰이드 블록. 기본 펄스 hue.
fn neon_preset() -> ThemePreset {
    ThemePreset {
        load_glyphs: to_owned(&["░", "▒", "▓", "█", "█"]),
        pulse_style: "hue".to_string(),
        band_tints: to_owned(&["#2bd6ff", "#1ea0ff", "#7c5cff", "#c33cff", "#ff2bd0"]),
        pulse_palette: to_owned(&["#ff2bd0", "#7a1f8a"]),
        label_color: "#6b7c99".to_string(),
        separator: " · ".to_string(),
        separator_color: "#2a3550".to_string(),
        hud_seam: "│".to_string(),
    }
}

/// aurora 프리셋. 청록→보라 오로라 그라데이션 + 바 램프. 기본 펄스 flash.
fn aurora_preset() -> ThemePreset {
    ThemePreset {
        load_glyphs: to_owned(&["▁", "▃", "▅", "▆", "█"]),
        pulse_style: "flash".to_string(),
        band_tints: to_owned(&["#2ad6a0", "#1fb6b0", "#2f9fe0", "#6c7cf0", "#b46cf0"]),
        pulse_palette: to_owned(&["#b46cf0", "#5a3a8a"]),
        label_color: "#6b7c8a".to_string(),
        separator: " · ".to_string(),
        separator_color: "#2a3848".to_string(),
        hud_seam: "│".to_string(),
    }
}

/// sunset 프리셋. 골드→코랄→핑크→퍼플 노을 + 따뜻한 도트. 기본 펄스 flash.
fn sunset_preset() -> ThemePreset {
    ThemePreset {
        load_glyphs: to_owned(&["·", "∙", "•", "●", "◉"]),
        pulse_style: "flash".to_string(),
        band_tints: to_owned(&["#ffd166", "#ff9e4f", "#ff6b6b", "#ef476f", "#c44ad0"]),
        pulse_palette: to_owned(&["#ef476f", "#8a2a48"]),
        label_color: "#8a7a6f".to_string(),
        separator: " · ".to_string(),
        separator_color: "#4a3a40".to_string(),
        hud_seam: "│".to_string(),
    }
}

/// spectrum 프리셋. 밴드별 무지개(초록→노랑→주황→빨강→마젠타). 기본 펄스 hue.
fn spectrum_preset() -> ThemePreset {
    ThemePreset {
        load_glyphs: to_owned(&["▁", "▂", "▄", "▆", "█"]),
        pulse_style: "hue".to_string(),
        band_tints: to_owned(&["#2fd36b", "#d4d13e", "#f0922e", "#e8443a", "#d23ad0"]),
        pulse_palette: to_owned(&["#d23ad0", "#7a1f78"]),
        label_color: "#6b7280".to_string(),
        separator: " · ".to_string(),
        separator_color: "#3b4048".to_string(),
        hud_seam: "│".to_string(),
    }
}
```

`preset()` match에 4갈래 추가(`"emoji" => ...` 다음, `_ => None` 앞):

```rust
        "neon" => Some(neon_preset()),
        "aurora" => Some(aurora_preset()),
        "sunset" => Some(sunset_preset()),
        "spectrum" => Some(spectrum_preset()),
```

- [ ] **Step 4: 테스트 통과 확인**

Run: `export PATH="$HOME/.cargo/bin:$PATH"; cargo test --lib themes:: 2>&1 | tail -20`
Expected: PASS — `flashy_presets_core_values`, `catalog_order_is_release_order`, 그리고 기존 catalog 순회 테스트(`all_presets_have_5_band_tints_and_glyphs`, `all_preset_hex_are_valid`, `catalog_matches_is_known`)가 새 4종까지 자동 검증.

- [ ] **Step 5: 커밋**

```bash
export PATH="$HOME/.cargo/bin:$PATH"
git add src/themes.rs
git commit -m "feat(themes): 화려한 색 프리셋 4종(neon/aurora/sunset/spectrum) 추가

각 테마는 THEME_KEYS 8개 전부 구체값. 화려한 4종은 처음부터 bold
pulse_style(neon·spectrum=hue, aurora·sunset=flash)을 갖되 Phase 2
render 분기 전까지는 무시되어 calm 동작.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: config.rs — 신규 테마 해석 회귀 테스트 (코드 무변경)

**Files:**
- Modify: `src/config.rs` (테스트만 추가)

- [ ] **Step 1: theme="neon" 해석 테스트 작성(통과 예상 — 해석 경로 일반성 회귀 가드)**

`src/config.rs`의 `mod tests`에 추가:

```rust
    /// theme="neon" + override 없음 → neon 프리셋 틴트/글리프/pulse_style로 채워져야 한다.
    #[test]
    fn theme_neon_fills_unset_keys() {
        let config = parse_config_str(r#"theme = "neon""#);
        assert_eq!(config.theme, "neon");
        assert_eq!(config.cpu.load_glyphs, vec!["░", "▒", "▓", "█", "█"]);
        assert_eq!(
            config.color.band_tints,
            vec!["#2bd6ff", "#1ea0ff", "#7c5cff", "#c33cff", "#ff2bd0"]
        );
        assert_eq!(config.color.pulse_palette, vec!["#ff2bd0", "#7a1f8a"]);
        // 화려한 테마의 bold 기본 pulse_style이 해석되어 들어온다.
        assert_eq!(config.pulse.pulse_style, "hue");
    }

    /// theme="spectrum" + 사용자 pulse_style 명시 → 사용자 값 우선(나머지는 spectrum).
    #[test]
    fn user_pulse_style_overrides_flashy_preset() {
        let toml = r#"
            theme = "spectrum"
            [pulse]
            pulse_style = "calm"
        "#;
        let config = parse_config_str(toml);
        // 사용자가 calm으로 끈 경우 우선(개별 키 > 프리셋).
        assert_eq!(config.pulse.pulse_style, "calm");
        // band_tints는 명시 안 했으므로 spectrum.
        assert_eq!(
            config.color.band_tints,
            vec!["#2fd36b", "#d4d13e", "#f0922e", "#e8443a", "#d23ad0"]
        );
    }
```

- [ ] **Step 2: 테스트 실행(통과 확인)**

Run: `export PATH="$HOME/.cargo/bin:$PATH"; cargo test --lib config:: 2>&1 | tail -20`
Expected: PASS (config.rs 코드는 안 바꿈 — `apply_theme`의 `pulse.pulse_style` 채움과 사용자 override 경로가 새 테마에도 동일하게 동작함을 고정).

- [ ] **Step 3: 커밋**

```bash
export PATH="$HOME/.cargo/bin:$PATH"
git add src/config.rs
git commit -m "test(config): 신규 화려한 테마 해석/override 회귀 테스트

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 3: README 갤러리 + Phase 1 전체 검증

**Files:**
- Modify: `README.md` (테마 갤러리 표)

- [ ] **Step 1: README의 테마 표에 4종 추가**

`README.md`에서 기존 테마 목록(calm/mono/vivid/ember/emoji)을 찾아 같은 형식으로 4행 추가. 예(기존 표 형식에 맞춰):

```markdown
| `neon`     | 네온 사이버펑크 — 일렉트릭 시안→마젠타, hue 순환 펄스 |
| `aurora`   | 오로라 청록→보라 그라데이션, flash 펄스 |
| `sunset`   | 노을 골드→코랄→퍼플, flash 펄스 |
| `spectrum` | 밴드별 무지개(초록→마젠타), hue 순환 펄스 |
```

> 기존 README의 정확한 표 헤더/구분선 형식을 먼저 확인하고 그대로 따른다(`grep -n "calm" README.md`).

- [ ] **Step 2: Phase 1 전체 검증(테스트·clippy·빌드)**

Run:
```bash
export PATH="$HOME/.cargo/bin:$PATH"
cargo test 2>&1 | tail -15
cargo clippy --all-targets -- -D warnings 2>&1 | tail -15
cargo build --release 2>&1 | tail -5
```
Expected: 전부 통과(테스트 갯수가 161 + 신규만큼 증가, clippy 0 경고, 릴리스 빌드 green).

- [ ] **Step 3: 새 테마 실제 ANSI 렌더 육안 확인(ground truth)**

Run(각 테마):
```bash
export PATH="$HOME/.cargo/bin:$PATH"
for t in neon aurora sunset spectrum; do
  echo "=== $t ==="
  printf 'theme = "%s"\n' "$t" > /tmp/us-$t.toml
  for cpu in 10 95; do
    UNDERSTATUS_CONFIG=/tmp/us-$t.toml COLORTERM=truecolor \
      ./target/release/understatus < tests/fixtures/claude_full.json | cat -v
  done
done
```
Expected: 각 테마가 자기 밴드 틴트로 렌더됨. (Phase 1에선 pulse_style이 무시되어 calm 동작 — 정상.) 색이 의도와 어긋나면 `themes.rs` hex를 조정하고 Task 1 테스트 값을 함께 갱신 후 재커밋.

> `tests/fixtures/claude_full.json`이 없으면 `ls tests/fixtures/`로 실제 픽스처명을 확인해 대체.

- [ ] **Step 4: 커밋**

```bash
export PATH="$HOME/.cargo/bin:$PATH"
git add README.md
git commit -m "docs(readme): 화려한 테마 4종 갤러리 추가

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Phase 2 — bold 펄스 채널

### Task 4: theme.rs — RGB↔HSV 순수 헬퍼

**Files:**
- Modify: `src/theme.rs` (private 헬퍼 + 테스트)

- [ ] **Step 1: 라운드트립 테스트 작성(실패)**

`src/theme.rs`의 `mod tests`에 추가:

```rust
    #[test]
    fn rgb_hsv_roundtrip_within_tolerance() {
        // 대표 색들이 RGB→HSV→RGB 라운드트립 시 채널 오차 ≤ 2여야 한다.
        for c in [
            ColorSpec { r: 0xff, g: 0x2b, b: 0xd0 },
            ColorSpec { r: 0x2f, g: 0xd3, b: 0x6b },
            ColorSpec { r: 0xb8, g: 0x78, b: 0x48 },
            ColorSpec { r: 0x00, g: 0x00, b: 0x00 },
            ColorSpec { r: 0xff, g: 0xff, b: 0xff },
        ] {
            let (h, s, v) = rgb_to_hsv(c);
            let back = hsv_to_rgb(h, s, v);
            assert!(
                (back.r as i16 - c.r as i16).abs() <= 2
                    && (back.g as i16 - c.g as i16).abs() <= 2
                    && (back.b as i16 - c.b as i16).abs() <= 2,
                "라운드트립 오차 초과: {c:?} -> {back:?}"
            );
        }
    }

    #[test]
    fn hsv_to_rgb_rotates_hue() {
        // 같은 S/V에서 hue만 120°씩 돌리면 빨강→초록→파랑 계열로 바뀐다.
        let red = hsv_to_rgb(0.0, 1.0, 1.0);
        let green = hsv_to_rgb(120.0, 1.0, 1.0);
        let blue = hsv_to_rgb(240.0, 1.0, 1.0);
        assert_eq!(red, ColorSpec { r: 255, g: 0, b: 0 });
        assert_eq!(green, ColorSpec { r: 0, g: 255, b: 0 });
        assert_eq!(blue, ColorSpec { r: 0, g: 0, b: 255 });
    }
```

- [ ] **Step 2: 테스트 실패 확인**

Run: `export PATH="$HOME/.cargo/bin:$PATH"; cargo test --lib theme::tests::rgb 2>&1 | tail -15`
Expected: FAIL — `rgb_to_hsv`/`hsv_to_rgb` 미정의(컴파일 에러).

- [ ] **Step 3: 헬퍼 구현**

`src/theme.rs`의 `parse_hex` 함수 근처(private 헬퍼 구역)에 추가:

```rust
/// [`ColorSpec`](RGB)을 HSV로 변환한다. 반환 `(h: 0–360, s: 0–1, v: 0–1)`.
///
/// hue 순환 펄스(`pulse_style="hue"`)에서 기준색의 색상환 위치를 얻는 데 쓴다.
/// 무채색(델타 0)이면 h=0으로 안전 처리한다(패닉 없음).
fn rgb_to_hsv(c: ColorSpec) -> (f64, f64, f64) {
    let r = c.r as f64 / 255.0;
    let g = c.g as f64 / 255.0;
    let b = c.b as f64 / 255.0;
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let delta = max - min;
    let h = if delta == 0.0 {
        0.0
    } else if max == r {
        60.0 * ((g - b) / delta).rem_euclid(6.0)
    } else if max == g {
        60.0 * (((b - r) / delta) + 2.0)
    } else {
        60.0 * (((r - g) / delta) + 4.0)
    };
    let s = if max == 0.0 { 0.0 } else { delta / max };
    (h.rem_euclid(360.0), s, max)
}

/// HSV(`h: 0–360`, `s/v: 0–1`)를 [`ColorSpec`](RGB)으로 변환한다. 채널은 clamp(0–255).
fn hsv_to_rgb(h: f64, s: f64, v: f64) -> ColorSpec {
    let h = h.rem_euclid(360.0);
    let c = v * s;
    let x = c * (1.0 - ((h / 60.0).rem_euclid(2.0) - 1.0).abs());
    let m = v - c;
    let (r1, g1, b1) = match (h / 60.0) as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let to_u8 = |f: f64| ((f + m) * 255.0).round().clamp(0.0, 255.0) as u8;
    ColorSpec {
        r: to_u8(r1),
        g: to_u8(g1),
        b: to_u8(b1),
    }
}
```

- [ ] **Step 4: 테스트 통과 확인**

Run: `export PATH="$HOME/.cargo/bin:$PATH"; cargo test --lib theme::tests::rgb theme::tests::hsv 2>&1 | tail -15`
Expected: PASS.

- [ ] **Step 5: 커밋**

```bash
export PATH="$HOME/.cargo/bin:$PATH"
git add src/theme.rs
git commit -m "feat(theme): RGB<->HSV 순수 변환 헬퍼 추가 (hue 펄스 토대)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 5: theme.rs — pulse_color에 flash/hue 스타일 분기

**Files:**
- Modify: `src/theme.rs` (`pulse_color` 본문 + private wave/breath/hue 헬퍼 + 테스트)

- [ ] **Step 1: flash/hue 동작 테스트 작성(실패)**

`src/theme.rs`의 `mod tests`에 추가:

```rust
    /// flash: calm과 같은 끝점(phase 0.25/0.75)이지만 중간 위상(0.5)에서 틴트가 다르다.
    #[test]
    fn pulse_color_flash_sharpens_midtone() {
        let mut cfg = Config::default();
        cfg.pulse.pulse_style = "flash".to_string();
        let mut calm = Config::default(); // pulse_style="calm"
        calm.pulse.pulse_style = "calm".to_string();
        // 끝점은 동일(wave 0/1 지점).
        assert_eq!(
            pulse_color(95.0, 22_500, true, &cfg),
            pulse_color(95.0, 22_500, true, &calm),
            "flash와 calm은 high 끝점(phase=0.75)에서 동일"
        );
        assert_eq!(
            pulse_color(95.0, 7_500, true, &cfg),
            pulse_color(95.0, 7_500, true, &calm),
            "flash와 calm은 low 끝점(phase=0.25)에서 동일"
        );
        // 중간(phase=0.5, 15000ms)은 곡선이 달라 틴트가 다르다.
        assert_ne!(
            pulse_color(95.0, 15_000, true, &cfg),
            pulse_color(95.0, 15_000, true, &calm),
            "flash는 중간 위상에서 calm과 다른(더 가파른) 틴트"
        );
    }

    /// hue: 위상에 따라 기준색의 hue가 회전해 서로 다른 색이 나온다(phase 0 ≈ 기준색).
    #[test]
    fn pulse_color_hue_rotates() {
        let mut cfg = Config::default();
        cfg.pulse.pulse_style = "hue".to_string();
        // phase=0(now=0) → 기준색(pulse_palette[0] = #b87848)에 근사.
        let base = pulse_color(95.0, 0, true, &cfg).expect("hue 틴트");
        assert!(
            (base.r as i16 - 0xb8).abs() <= 2
                && (base.g as i16 - 0x78).abs() <= 2
                && (base.b as i16 - 0x48).abs() <= 2,
            "phase 0은 기준색에 근사: {base:?}"
        );
        // 서로 다른 위상은 서로 다른 색.
        let q = pulse_color(95.0, 7_500, true, &cfg).expect("hue 틴트");
        let h = pulse_color(95.0, 15_000, true, &cfg).expect("hue 틴트");
        assert_ne!(base, q);
        assert_ne!(q, h);
    }

    /// 펄스 OFF이면 스타일과 무관하게 None(정적 틴트는 render가 밴드 틴트로 결정).
    #[test]
    fn pulse_color_off_is_none_regardless_of_style() {
        for style in ["calm", "flash", "hue", "swap"] {
            let mut cfg = Config::default();
            cfg.pulse.pulse_style = style.to_string();
            assert_eq!(pulse_color(95.0, 1_234, false, &cfg), None, "{style} OFF");
        }
    }
```

- [ ] **Step 2: 테스트 실패 확인**

Run: `export PATH="$HOME/.cargo/bin:$PATH"; cargo test --lib theme::tests::pulse_color_flash theme::tests::pulse_color_hue 2>&1 | tail -15`
Expected: FAIL — 현재 `pulse_color`는 스타일을 무시(flash/calm 동일, hue 미회전)하므로 `assert_ne`가 실패.

- [ ] **Step 3: pulse_color 리팩터 + 헬퍼 추가**

`src/theme.rs`의 기존 `pulse_color` 본문을 아래로 교체(시그니처 고정 — 본문만):

```rust
pub fn pulse_color(
    cpu_percent: f64,
    now_ms: u128,
    pulse_on: bool,
    cfg: &Config,
) -> Option<ColorSpec> {
    // 펄스 off면 틴트 미적용(render가 정적 밴드 틴트를 선택). cpu_percent는 계약상 보존.
    let _ = cpu_percent;
    if !pulse_on {
        return None;
    }

    let phase = pulse_phase(now_ms, cfg);
    // 팔레트 부재/짧을 때 안전 기본값(테라코타 high↔low).
    let start = palette_color(cfg, 0).unwrap_or(ColorSpec {
        r: 0xb8,
        g: 0x78,
        b: 0x48,
    });
    let end = palette_color(cfg, 1).unwrap_or(ColorSpec {
        r: 0x7a,
        g: 0x50,
        b: 0x30,
    });

    let tint = match cfg.pulse.pulse_style.as_str() {
        // hue/swap: 기준색(start)의 hue를 한 주기 동안 360° 회전(S/V 유지) → 무지개 시머.
        // swap의 글리프 교대는 pick_emoji가 담당하고, 틴트는 hue와 동일하게 순환한다.
        "hue" | "swap" => hue_rotate(start, phase),
        // flash: calm과 같은 두 끝점, 더 가파른 곡선(어두운 구간 길고 밝은 스파이크 짧음).
        "flash" => luminance_breath(start, end, flash_wave(phase)),
        // calm(기본) + 미지 스타일: 현행 휘도 호흡(hue 불변).
        _ => luminance_breath(start, end, calm_wave(phase)),
    };
    Some(tint)
}

/// calm 휘도 호흡의 사인파 wave(0..1). `wave=0`→start, `wave=1`→end.
fn calm_wave(phase: f64) -> f64 {
    (f64::sin(2.0 * std::f64::consts::PI * phase) + 1.0) / 2.0
}

/// flash 호흡 wave: calm wave에 감마(2.2)를 적용해 곡선을 가파르게(중간톤 대비↑).
/// 끝점(wave 0/1)은 calm과 동일하게 보존되어 high/low 색은 변하지 않는다.
fn flash_wave(phase: f64) -> f64 {
    calm_wave(phase).powf(2.2)
}

/// 두 끝점을 wave로 LERP한 틴트를 만든다(휘도 호흡 공통).
fn luminance_breath(start: ColorSpec, end: ColorSpec, wave: f64) -> ColorSpec {
    ColorSpec {
        r: lerp_channel(start.r, end.r, wave),
        g: lerp_channel(start.g, end.g, wave),
        b: lerp_channel(start.b, end.b, wave),
    }
}

/// 기준색의 hue를 위상만큼(0..1 → 0..360°) 회전한 틴트(S/V 유지).
fn hue_rotate(base: ColorSpec, phase: f64) -> ColorSpec {
    let (h, s, v) = rgb_to_hsv(base);
    hsv_to_rgb(h + 360.0 * phase, s, v)
}
```

> 주의: `calm` 갈래는 기존 `wave = (sin(2π·phase)+1)/2` 계산과 **수학적으로 동일**하므로 기존 calm 스냅샷 테스트(`pulse_color_breathes_between_terracotta_endpoints` 등)가 그대로 통과한다.

- [ ] **Step 4: 신규 + 기존(calm 회귀) 테스트 통과 확인**

Run: `export PATH="$HOME/.cargo/bin:$PATH"; cargo test --lib theme:: 2>&1 | tail -25`
Expected: PASS — 신규 flash/hue/off 테스트 + 기존 calm 테스트(`pulse_color_breathes_between_terracotta_endpoints`, `pulse_color_has_no_hue_shift`, `pulse_color_pure_same_now_same_spec`, `pulse_color_varies_across_now`) 전부 통과.

- [ ] **Step 5: 커밋**

```bash
export PATH="$HOME/.cargo/bin:$PATH"
git add src/theme.rs
git commit -m "feat(theme): pulse_color에 flash/hue 펄스 스타일 분기

calm 경로는 수학적 동치로 보존(기존 스냅샷 테스트 그대로 통과).

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 6: theme.rs — pick_emoji swap 글리프 교대 + 스타일 레지스트리

**Files:**
- Modify: `src/theme.rs` (`pick_emoji` 본문, `alt_glyph`, `PULSE_STYLES`/`is_known_pulse_style` + 테스트)

- [ ] **Step 1: swap + 레지스트리 테스트 작성(실패)**

`src/theme.rs`의 `mod tests`에 추가:

```rust
    /// swap: 펄스 ON이면 위상 전반(phase<0.5)은 band 글리프, 후반(phase>=0.5)은 alt 글리프.
    #[test]
    fn pick_emoji_swap_alternates_glyph() {
        let mut cfg = Config::default(); // crit 밴드 글리프 ◆
        cfg.pulse.pulse_style = "swap".to_string();
        // pulse_period=30s → now=0 phase 0(전반) → ◆; now=20000 phase 0.666(후반) → 교대 ◇.
        assert_eq!(pick_emoji(95.0, 0, true, &cfg), "◆");
        assert_eq!(pick_emoji(95.0, 20_000, true, &cfg), "◇");
    }

    /// swap이라도 펄스 OFF면 글리프 고정(저부하에서 깜빡이지 않음).
    #[test]
    fn pick_emoji_swap_stable_when_off() {
        let mut cfg = Config::default();
        cfg.pulse.pulse_style = "swap".to_string();
        assert_eq!(pick_emoji(95.0, 0, false, &cfg), "◆");
        assert_eq!(pick_emoji(95.0, 20_000, false, &cfg), "◆");
    }

    /// calm/flash/hue는 글리프를 교대하지 않는다(펄스 ON이어도 고정).
    #[test]
    fn pick_emoji_non_swap_styles_stable() {
        for style in ["calm", "flash", "hue"] {
            let mut cfg = Config::default();
            cfg.pulse.pulse_style = style.to_string();
            assert_eq!(pick_emoji(95.0, 0, true, &cfg), "◆", "{style} 전반");
            assert_eq!(pick_emoji(95.0, 20_000, true, &cfg), "◆", "{style} 후반");
        }
    }

    #[test]
    fn pulse_styles_registry() {
        assert!(is_known_pulse_style("calm"));
        assert!(is_known_pulse_style("flash"));
        assert!(is_known_pulse_style("hue"));
        assert!(is_known_pulse_style("swap"));
        assert!(!is_known_pulse_style("bogus"));
        assert_eq!(PULSE_STYLES, &["calm", "flash", "hue", "swap"]);
    }
```

- [ ] **Step 2: 테스트 실패 확인**

Run: `export PATH="$HOME/.cargo/bin:$PATH"; cargo test --lib theme::tests::pick_emoji_swap theme::tests::pulse_styles 2>&1 | tail -15`
Expected: FAIL — swap 미구현(글리프 교대 안 함), `is_known_pulse_style`/`PULSE_STYLES` 미정의.

- [ ] **Step 3: pick_emoji 본문 교체 + alt_glyph + 레지스트리 추가**

`src/theme.rs`의 `pick_emoji` 본문을 교체(시그니처 고정):

```rust
pub fn pick_emoji(cpu_percent: f64, now_ms: u128, pulse_on: bool, cfg: &Config) -> String {
    let band = band_index(cpu_percent, cfg);
    const FALLBACK: [&str; 5] = ["○", "▁", "▄", "▆", "◆"];
    let base = match cfg.cpu.load_glyphs.get(band) {
        Some(glyph) if !glyph.is_empty() => glyph.clone(),
        _ => FALLBACK[band.min(FALLBACK.len() - 1)].to_string(),
    };
    // swap 스타일 + 펄스 ON일 때만 위상 후반부에서 글리프를 교대한다(그 외엔 고정 — CALM).
    if pulse_on && cfg.pulse.pulse_style == "swap" && pulse_phase(now_ms, cfg) >= 0.5 {
        return alt_glyph(&base);
    }
    base
}

/// swap 스타일에서 글리프의 "교대형"(filled↔hollow 등)을 돌려준다. 매핑 없으면 원본 유지(no-op).
fn alt_glyph(glyph: &str) -> String {
    let alt = match glyph {
        "◆" => "◇",
        "◇" => "◆",
        "●" => "○",
        "○" => "●",
        "◉" => "◎",
        "◎" => "◉",
        "█" => "░",
        "░" => "█",
        "▓" => "▒",
        "▒" => "▓",
        "▆" => "▂",
        "▂" => "▆",
        "▄" => "▁",
        "▁" => "▄",
        other => other,
    };
    alt.to_string()
}

/// 알려진 펄스 스타일 목록(설치/`pulse` 명령의 하드 검증 + render 분기 SSOT).
pub const PULSE_STYLES: &[&str] = &["calm", "flash", "hue", "swap"];

/// 유효 펄스 스타일 이름인지 판정한다(쓰기 경로 하드 검증용).
pub fn is_known_pulse_style(name: &str) -> bool {
    PULSE_STYLES.contains(&name)
}
```

> `pick_emoji`의 기존 `let _ = now_ms; let _ = pulse_on;`(미사용 표시)는 swap에서 두 인자를 실제로 쓰므로 제거된다.

- [ ] **Step 4: 신규 + 기존 회귀 테스트 통과 확인**

Run: `export PATH="$HOME/.cargo/bin:$PATH"; cargo test --lib theme:: 2>&1 | tail -25`
Expected: PASS — swap/레지스트리 신규 테스트 + 기존 `pick_emoji_glyph_is_stable_when_pulsing`(default=calm → 교대 안 함), `pick_emoji_band_glyphs`, `pick_emoji_respects_custom_emoji_glyphs` 전부 통과.

- [ ] **Step 5: 커밋**

```bash
export PATH="$HOME/.cargo/bin:$PATH"
git add src/theme.rs
git commit -m "feat(theme): swap 펄스 글리프 교대 + 펄스 스타일 레지스트리

swap만 글리프 고정 불변식을 해제(펄스 ON 위상 후반 교대). calm/flash/hue는
글리프 고정 유지.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 7: install.rs — set_pulse_style + 검증 + 섹션 헬퍼

**Files:**
- Modify: `src/install.rs` (`validate_pulse_style`, `set_pulse_style`, `set_pulse_style_key` + 테스트)

- [ ] **Step 1: 변환/검증 테스트 작성(실패)**

`src/install.rs`의 `mod tests`에 추가:

```rust
    /// set_pulse_style 변환은 [pulse].pulse_style만 교체하고 다른 키를 보존한다(인메모리).
    #[test]
    fn set_pulse_style_replaces_only_pulse_style_key() {
        let existing =
            "theme = \"neon\"\n[pulse]\npulse_period_seconds = 30\n[refresh]\ninterval_seconds = 5\n";
        let serialized = edit_config_doc_str(Some(existing), |table| {
            set_pulse_style_key(table, "hue");
            Ok(())
        })
        .expect("변환 성공");
        let parsed: toml::Value = toml::from_str(&serialized).expect("재파싱 성공");
        assert_eq!(
            parsed
                .get("pulse")
                .and_then(|t| t.get("pulse_style"))
                .and_then(toml::Value::as_str),
            Some("hue")
        );
        // 기존 [pulse] 다른 키 + theme + refresh 보존.
        assert_eq!(
            parsed
                .get("pulse")
                .and_then(|t| t.get("pulse_period_seconds"))
                .and_then(toml::Value::as_integer),
            Some(30)
        );
        assert_eq!(
            parsed.get("theme").and_then(toml::Value::as_str),
            Some("neon")
        );
        assert_eq!(
            parsed
                .get("refresh")
                .and_then(|t| t.get("interval_seconds"))
                .and_then(toml::Value::as_integer),
            Some(5)
        );
    }

    /// 손상된(비-table) [pulse] 섹션이어도 pulse_style을 확실히 기록한다(부분 기록 방지).
    #[test]
    fn set_pulse_style_key_overwrites_non_table_section() {
        let existing = "pulse = \"corrupted\"\n";
        let serialized = edit_config_doc_str(Some(existing), |table| {
            set_pulse_style_key(table, "flash");
            Ok(())
        })
        .expect("변환 성공");
        let parsed: toml::Value = toml::from_str(&serialized).expect("재파싱 성공");
        assert_eq!(
            parsed
                .get("pulse")
                .and_then(|t| t.get("pulse_style"))
                .and_then(toml::Value::as_str),
            Some("flash")
        );
    }

    /// set_pulse_style은 미지 스타일을 거부하고 에러에 사용 가능 목록을 포함한다.
    #[test]
    fn set_pulse_style_rejects_unknown() {
        let error = set_pulse_style("bogus").expect_err("미지 스타일은 Err");
        let message = format!("{error}");
        assert!(message.contains("bogus"), "에러에 입력 스타일 포함");
        assert!(message.contains("calm"), "에러에 사용 가능 목록 포함");
    }

    /// validate_pulse_style은 4개 출시 스타일을 통과시킨다.
    #[test]
    fn validate_pulse_style_accepts_known() {
        for style in ["calm", "flash", "hue", "swap"] {
            assert!(validate_pulse_style(style).is_ok(), "{style} 통과");
        }
    }
```

- [ ] **Step 2: 테스트 실패 확인**

Run: `export PATH="$HOME/.cargo/bin:$PATH"; cargo test --lib install::tests::set_pulse_style install::tests::validate_pulse 2>&1 | tail -15`
Expected: FAIL — `set_pulse_style_key`/`set_pulse_style`/`validate_pulse_style` 미정의(컴파일 에러).

- [ ] **Step 3: 함수 3개 구현**

`src/install.rs`의 `set_theme` 함수 바로 다음에 추가:

```rust
/// 설치 후 펄스 스타일만 교체한다(config.toml `[pulse].pulse_style`만, settings.json 무접근).
///
/// # 인자
/// - `style`: 적용할 펄스 스타일. [`validate_pulse_style`]를 통과해야 한다(미지면 하드 에러 + 목록).
///
/// # 반환
/// 성공 시 `Ok(())`. 미지 스타일/I-O 실패는 [`anyhow::Error`]로 전파한다.
pub fn set_pulse_style(style: &str) -> Result<()> {
    validate_pulse_style(style)?;
    edit_config_doc(|table| {
        set_pulse_style_key(table, style);
        Ok(())
    })
}

/// 펄스 스타일 이름이 출시 스타일인지 하드 검증한다(순수 함수, I/O 없음).
///
/// `set_pulse_style`(및 향후 모든 쓰기 경로)이 공유하는 단일 검증점이다. 미지 스타일이면
/// 사용 가능 목록을 담은 에러를 돌려 디스크 쓰기 이전에 중단하게 한다(render calm 폴백 의존 방지).
fn validate_pulse_style(style: &str) -> Result<()> {
    if theme::is_known_pulse_style(style) {
        return Ok(());
    }
    Err(anyhow!(
        "알 수 없는 펄스 스타일 '{style}'. 사용 가능: {}",
        theme::PULSE_STYLES.join(", ")
    ))
}

/// 최상위 테이블에 `[pulse].pulse_style`을 설정한다(다른 키 보존). `set_chain_command`와 동형.
///
/// `[pulse]`가 존재하나 테이블이 아니면(손상된 config) 새 테이블로 교체해 키 기록을 보장한다
/// (조용한 no-op로 인한 부분 기록 방지).
fn set_pulse_style_key(table: &mut toml::value::Table, style: &str) {
    let pulse = table
        .entry("pulse".to_string())
        .or_insert_with(|| toml::Value::Table(toml::map::Map::new()));
    if !pulse.is_table() {
        *pulse = toml::Value::Table(toml::map::Map::new());
    }
    let pulse_table = pulse
        .as_table_mut()
        .expect("set_pulse_style_key: 위에서 테이블로 보장했으므로 항상 Some");
    pulse_table.insert(
        "pulse_style".to_string(),
        toml::Value::String(style.to_string()),
    );
}
```

> `theme` 모듈은 install.rs 상단에서 이미 `use crate::theme;`로 임포트되어 있다(기존 `warn_if_pulse_period_too_short`가 `theme::samples_per_period` 사용). 추가 임포트 불필요.

- [ ] **Step 4: 테스트 통과 확인**

Run: `export PATH="$HOME/.cargo/bin:$PATH"; cargo test --lib install:: 2>&1 | tail -20`
Expected: PASS — 신규 4개 테스트 + 기존 install 테스트(라운드트립/멱등/병합) 전부 통과.

- [ ] **Step 5: 커밋**

```bash
export PATH="$HOME/.cargo/bin:$PATH"
git add src/install.rs
git commit -m "feat(install): set_pulse_style + 검증 + 비-table 섹션 안전 기록

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 8: main.rs — `pulse` 서브커맨드 + help

**Files:**
- Modify: `src/main.rs` (디스패치 갈래, `run_pulse`, `print_help`)

- [ ] **Step 1: 디스패치 갈래 추가**

`src/main.rs`의 `match subcommand` 안, `Some("themes") => {...}` 다음에 추가:

```rust
        Some("pulse") => run_pulse(&args),
```

- [ ] **Step 2: run_pulse 구현**

`run_theme` 함수 다음에 추가:

```rust
/// pulse 서브커맨드를 실행한다(`pulse <style>` → 교체, 스타일 누락 → 현재 스타일 + 사용법).
fn run_pulse(args: &[String]) -> ExitCode {
    match args.get(1) {
        Some(style) => match install::set_pulse_style(style) {
            Ok(()) => {
                eprintln!("understatus: 펄스 스타일을 '{style}'로 변경했습니다.");
                ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("understatus: 펄스 스타일 변경 실패: {error:#}");
                ExitCode::FAILURE
            }
        },
        None => {
            let current = config::load_config().pulse.pulse_style;
            println!("understatus: 현재 펄스 스타일은 '{current}'입니다.");
            println!("사용법: understatus pulse <calm|flash|hue|swap>");
            ExitCode::SUCCESS
        }
    }
}
```

- [ ] **Step 3: help에 pulse 줄 추가**

`print_help`의 `understatus themes` 줄 다음에 한 줄 추가(기존 `\x20 understatus themes ...` 다음):

```rust
         \x20 understatus pulse <style>  펄스 스타일 교체(calm|flash|hue|swap, config.toml만 수정)\n\
```

- [ ] **Step 4: 빌드 + 수동 동작 확인**

Run:
```bash
export PATH="$HOME/.cargo/bin:$PATH"
cargo build --release 2>&1 | tail -5
# 임시 config로 pulse 명령 검증(실제 ~/.config 미오염: UNDERSTATUS_CONFIG 사용)
export UNDERSTATUS_CONFIG=/tmp/us-pulse-test.toml
rm -f "$UNDERSTATUS_CONFIG"
./target/release/understatus pulse hue;  echo "exit=$?"
./target/release/understatus pulse;       # 현재 스타일 출력 → 'hue'
./target/release/understatus pulse bogus; echo "exit=$?"   # 실패(비0) + 목록
cat "$UNDERSTATUS_CONFIG"                  # [pulse] pulse_style = "hue"
unset UNDERSTATUS_CONFIG
```
Expected: `pulse hue` 성공(exit 0) + config에 `pulse_style = "hue"` 기록; `pulse` 인자 없음 → 현재 'hue' 출력; `pulse bogus` → 비0 종료 + 사용 가능 목록.

- [ ] **Step 5: 커밋**

```bash
export PATH="$HOME/.cargo/bin:$PATH"
git add src/main.rs
git commit -m "feat(cli): understatus pulse <style> 명령 + help 갱신

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 9: README Phase 2 문서 + 전체 검증

**Files:**
- Modify: `README.md` (펄스 스타일/발동 구간/`pulse` 명령)

- [ ] **Step 1: README에 펄스 스타일 섹션 추가**

`README.md`에 펄스/테마 관련 섹션을 찾아(없으면 테마 갤러리 다음에) 아래 내용을 기존 문체에 맞춰 추가:

```markdown
### 펄스 스타일 (`pulse_style`)

높은 CPU 부하에서 글리프가 "숨쉬는" 방식을 고른다. config.toml `[pulse].pulse_style` 또는 `understatus pulse <style>`로 설정한다.

| 스타일 | 동작 |
|---|---|
| `calm` (기본) | 테라코타 휘도 호흡(hue 불변). 가장 차분함. |
| `flash` | 같은 색의 더 가파른 호흡(대비↑). |
| `hue` | 색이 색상환을 따라 순환(무지개 시머). 글리프는 고정. |
| `swap` | hue 순환 + 글리프 모양 교대(◆↔◇ 등). 가장 화려(글리프 깜빡임). |

화려한 테마는 어울리는 bold 기본을 갖는다: neon·spectrum=`hue`, aurora·sunset=`flash`. 기존 테마는 `calm`.

**발동 구간**은 `[pulse].pulse_on_threshold`(기본 90)/`pulse_off_threshold`(기본 80)로 조절한다. 낮추면 더 낮은 부하에서도 펄스가 켜진다(예: `pulse_on_threshold = 75`). 펄스가 꺼진 구간에서는 스타일과 무관하게 정적 밴드 틴트로 표시된다.

```bash
understatus pulse hue       # 펄스 스타일 변경
understatus pulse           # 현재 스타일 출력
```
```

- [ ] **Step 2: 전체 검증(테스트·clippy·fmt·릴리스 빌드)**

Run:
```bash
export PATH="$HOME/.cargo/bin:$PATH"
cargo test 2>&1 | tail -15
cargo clippy --all-targets -- -D warnings 2>&1 | tail -15
cargo fmt --check 2>&1 | tail -5
cargo build --release 2>&1 | tail -5
```
Expected: 테스트 전부 통과(161 + Phase 1·2 신규), clippy 0 경고, `fmt --check` 차이 없음, 릴리스 빌드 green.
> `fmt --check`가 차이를 보고하면 `cargo fmt` 실행 후 그 변경을 별도 `style: cargo fmt` 커밋으로 분리(CI fmt 게이트 — HANDOFF 참조).

- [ ] **Step 3: bold 펄스 실제 ANSI 시계열 육안 확인(ground truth)**

Run(neon=hue, sunset=flash, swap 직접 지정 — 같은 now에 결정적):
```bash
export PATH="$HOME/.cargo/bin:$PATH"
printf 'theme = "neon"\n' > /tmp/us-neon.toml          # pulse_style=hue (프리셋 기본)
printf 'theme = "calm"\n[pulse]\npulse_style="swap"\n' > /tmp/us-swap.toml
# CPU 95%(crit) 픽스처가 필요 — 없으면 [cpu] precision/임계값으로 crit을 유도하거나
# pulse_on_threshold를 낮춰 일반 픽스처에서 펄스를 켠다:
printf 'theme = "neon"\n[pulse]\npulse_on_threshold=0\npulse_off_threshold=0\n' > /tmp/us-neon-on.toml
UNDERSTATUS_CONFIG=/tmp/us-neon-on.toml COLORTERM=truecolor \
  ./target/release/understatus < tests/fixtures/claude_full.json | cat -v
```
Expected: hue/swap에서 글리프 틴트가 위상에 따라 색이 도는 ANSI(`\x1b[38;2;...m`)로 렌더됨. 색감이 과하거나 약하면 themes.rs hex/`flash_wave` 감마를 조정하고 관련 테스트 값 갱신 후 재커밋.

- [ ] **Step 4: 커밋**

```bash
export PATH="$HOME/.cargo/bin:$PATH"
git add README.md
git commit -m "docs(readme): 펄스 스타일/발동 구간/pulse 명령 문서화

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Open Design 갤러리 (시각 검토 — 선택, 코드 SSOT 아님)

OD MCP 데몬이 이 세션에서 끊겨 있다(`/mcp` 재연결 ENOENT). 갤러리는 **독립 HTML 파일**로 만들어 브라우저/OD로 연다:
- `docs/understatus-themes.html`(또는 OD 재연결 시 `create_artifact`로 아티팩트): 9종 × 5밴드 정적 스와치 + 펄스 스타일(calm/flash/hue/swap) CSS 애니메이션 시연.
- **코드의 단일 소스는 `themes.rs`다.** 갤러리는 미적 검토/문서용이며, hex 확정의 ground truth는 위 Task 3/9의 **실제 바이너리 ANSI 출력**이다. 갤러리 스와치 값은 `themes.rs`와 사람이 대조한다.

이 작업은 출시 게이트가 아니다(코드/테스트와 독립). 별도 태스크로 다루거나, OD 재연결 후 진행한다.

---

## Self-Review (작성자 체크)

**Spec 커버리지:**
- §3 색 프리셋 4종 → Task 1·2·3. ✅
- §4.1 pulse_style 4종(calm/flash/hue/swap) → Task 5(flash/hue)·Task 6(swap)·calm 보존. ✅
- §4.2 발동 구간 = 기존 임계값 노출 → Task 9 README 문서화(새 코드 없음, 의도대로). ✅
- §4.3 `understatus pulse <style>` → Task 7(install)·Task 8(main). ✅
- §4.4 화려한 4종 bold 기본 → Task 1에서 프리셋이 직접 bold 값 보유(출시 묶음이라 전환 태스크 불필요). ✅
- §6 OD 갤러리 → 별도 섹션(독립, OD 끊김 대응 HTML 폴백). ✅
- §7 에러: 미지 스타일 → render `_ => calm` + 쓰기 경로 하드 검증(Task 6·7). hue HSV clamp(Task 4). ✅
- §8 테스트 계획 → 각 Task의 TDD 스텝이 대응. ✅

**Placeholder 스캔:** 모든 코드 스텝에 완전한 코드 포함. "적절히 처리" 류 없음. ✅

**타입/시그니처 일관성:** `pulse_color`/`pick_emoji` 시그니처 불변(본문만); `set_pulse_style`/`validate_pulse_style`/`set_pulse_style_key`/`is_known_pulse_style`/`PULSE_STYLES`/`alt_glyph`/`hue_rotate`/`luminance_breath`/`calm_wave`/`flash_wave`/`rgb_to_hsv`/`hsv_to_rgb` 이름이 정의/사용처에서 일치. ✅

> **§7 미세 차이(의도):** spec은 미지 `pulse_style` 런타임 시 stderr 경고를 언급하나, 핫패스(pulse_color) eprintln을 피하려고 **런타임은 조용한 calm 폴백 + 쓰기 경로 하드 검증**으로 구현한다(CLI는 미지 스타일을 결코 기록하지 않음; 손으로 편집한 미지 값만 calm으로 저하). 설계 의도(미지 값이 깨지지 않음)는 충족.

---

## Release (이 계획 범위 밖 — 참고)

Phase 1+2 머지 후 단일 릴리스(예 v0.3.0): `Cargo.toml`/`npm/package.json`/`npm/install.js` VERSION 3종 동시 범프 → tag push(release.yml) → `cargo publish` → Homebrew formula sha256 갱신 → `npm publish`(패스키 EOTP, **사용자 수동**). 상세는 HANDOFF.md "Future release process".
