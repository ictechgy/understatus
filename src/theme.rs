//! 밴드 임계값 + CALM 펄스(히스테리시스, 시각→틴트 순수 함수) + 불변식 헬퍼.
//!
//! 계획서 §H-2/§H-3/§H-4/§H-5/AC4를 따른다. 펄스는 understatus이 자체 루프를
//! 도는 게 아니라 `(cpu%, now, prev_on) → 틴트`의 순수 함수로 매 호출마다 한
//! 프레임을 산출한다(frame-per-call). 히스테리시스로 경계 출렁임을 흡수한다.
//!
//! CALM 디자인: 밴드 글리프(load glyph)는 **고정**이며 펄스 중에도 모양이
//! 깜빡이지 않는다. 펄스 ON일 때는 글리프 **틴트만** 테라코타 high↔low 사이를
//! 숨쉬듯 보간한다(hue shift 없음). 색은 글리프 문자에만 입히고 CPU% 숫자엔
//! 입히지 않는다(COLOR-ONCE 규칙은 render 단계에서 강제).

use crate::config::Config;

/// 펄스 색 한 프레임의 RGB 값. ANSI truecolor/256 렌더에 사용.
///
/// 펄스 미발동(정적) 시에도 정적 색을 표현하며, `NO_COLOR`/색상 비활성 시에는
/// render 단계에서 무시된다([`pulse_color`]가 `None`을 반환할 수 있음).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ColorSpec {
    /// 빨강 채널(0–255).
    pub r: u8,
    /// 초록 채널(0–255).
    pub g: u8,
    /// 파랑 채널(0–255).
    pub b: u8,
}

// CONTRACT: signature is frozen — implement body only, do not change this signature
/// 히스테리시스 게이트: 직전 on/off 상태를 고려해 이번 프레임의 펄스 on/off를 결정한다.
///
/// # 인자
/// - `cpu_percent`: 진짜 순간 CPU%(0–100).
/// - `prev_on`: 직전 렌더의 펄스 on/off 상태(단기 TTL 캐시에서 읽음).
/// - `cfg`: `pulse.pulse_on_threshold`(기본 90) / `pulse.pulse_off_threshold`(기본 80).
///
/// # 반환
/// 이번 프레임 펄스 on 여부. OFF→ON은 `cpu_percent ≥ on_threshold`,
/// ON→OFF는 `cpu_percent < off_threshold`에서만 전환한다(경계 출렁임 흡수, AC4).
pub fn pulse_gate(cpu_percent: f64, prev_on: bool, cfg: &Config) -> bool {
    if prev_on {
        // 이미 ON: off_threshold 미만으로 떨어져야 OFF로 전환(그 전까진 ON 유지).
        cpu_percent >= cfg.pulse.pulse_off_threshold
    } else {
        // 현재 OFF: on_threshold 이상이어야 ON으로 전환.
        cpu_percent >= cfg.pulse.pulse_on_threshold
    }
}

/// CPU%를 밴드 인덱스(0..=4)로 환산한다(idle/low/mid/high/crit).
///
/// `emoji_thresholds`[25,50,75,90] 기준: <25→0, [25,50)→1, [50,75)→2,
/// [75,90)→3, ≥90→4. 글리프/틴트 선택의 공통 기준이 된다.
pub fn band_index(cpu_percent: f64, cfg: &Config) -> usize {
    let thresholds = cfg.cpu.emoji_thresholds;
    if cpu_percent < thresholds[0] {
        0
    } else if cpu_percent < thresholds[1] {
        1
    } else if cpu_percent < thresholds[2] {
        2
    } else if cpu_percent < thresholds[3] {
        3
    } else {
        4
    }
}

/// 밴드의 정적 틴트(`color.band_tints[band]`)를 [`ColorSpec`]으로 반환한다.
///
/// 인덱스 범위 밖이거나 헥스 형식이 잘못되면 안전 기본값(중립 blue-gray)으로
/// 저하한다(패닉 금지). 펄스 OFF일 때 글리프 틴트로 쓰인다.
pub fn band_tint(band: usize, cfg: &Config) -> ColorSpec {
    cfg.color
        .band_tints
        .get(band)
        .and_then(|hex| parse_hex(hex))
        .unwrap_or(ColorSpec {
            r: 0x6d,
            g: 0x82,
            b: 0x96,
        })
}

// CONTRACT: signature is frozen — implement body only, do not change this signature
/// 현재 CPU% 밴드에 해당하는 **고정** load glyph를 고른다(CALM 디자인).
///
/// # 인자
/// - `cpu_percent`: 진짜 순간 CPU%(0–100).
/// - `now_ms`: (미사용) 시그니처 고정을 위해 보존. CALM에선 글리프가 깜빡이지 않음.
/// - `pulse_on`: (미사용) 시그니처 고정을 위해 보존. 펄스 중에도 글리프 모양은 고정.
/// - `cfg`: `cpu.emoji_thresholds`, `cpu.load_glyphs`.
///
/// # 반환
/// 밴드에 매핑된 글리프 문자열(`load_glyphs[band]`). 기본 ○ ▁ ▄ ▆ ◆.
/// 펄스가 켜져도 글리프 **모양은 바뀌지 않는다**(틴트만 숨쉰다, §H-4 CALM).
/// `load_glyphs`가 비었거나 짧으면 안전 기본값 글리프로 저하한다(패닉 금지).
pub fn pick_emoji(cpu_percent: f64, now_ms: u128, pulse_on: bool, cfg: &Config) -> String {
    // CALM: 펄스 상태/시각과 무관하게 밴드 글리프는 고정(깜빡임 없음).
    let _ = now_ms;
    let _ = pulse_on;
    let band = band_index(cpu_percent, cfg);
    const FALLBACK: [&str; 5] = ["○", "▁", "▄", "▆", "◆"];
    match cfg.cpu.load_glyphs.get(band) {
        Some(glyph) if !glyph.is_empty() => glyph.clone(),
        _ => FALLBACK[band.min(FALLBACK.len() - 1)].to_string(),
    }
}

// CONTRACT: signature is frozen — implement body only, do not change this signature
/// 현재 CPU%/시각/펄스 상태로 표시할 글리프 틴트를 산출한다(CALM 펄스 숨쉬기).
///
/// # 인자
/// - `cpu_percent`: 진짜 순간 CPU%(0–100).
/// - `now_ms`: 현재 시각(ms). 사인파 위상 계산에 사용.
/// - `pulse_on`: [`pulse_gate`]가 산출한 이번 프레임 펄스 on 여부.
/// - `cfg`: `pulse.pulse_period_seconds`, `color.pulse_palette`.
///
/// # 반환
/// - `Some(ColorSpec)`: 펄스 on이면 `now_ms` 위상으로 테라코타 high↔low를 부드럽게
///   보간한 틴트(hue shift 없음, 같은 톤 안에서 휘도만 숨쉰다).
/// - `None`: 펄스 off. 정적 틴트는 호출자(render)가 밴드 틴트로 결정한다(계약 고정).
///
/// 순수 함수: 동일 `(cpu_percent, now_ms, pulse_on, cfg)` → 동일 출력(테스트 가능, AC4).
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

    // CALM 숨쉬기: 사인파로 phase(0..1)→wave(0..1)를 만들어 팔레트 두 끝점을 LERP.
    let phase = pulse_phase(now_ms, cfg);
    let wave = (f64::sin(2.0 * std::f64::consts::PI * phase) + 1.0) / 2.0;

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

    Some(ColorSpec {
        r: lerp_channel(start.r, end.r, wave),
        g: lerp_channel(start.g, end.g, wave),
        b: lerp_channel(start.b, end.b, wave),
    })
}

// CONTRACT: signature is frozen — implement body only, do not change this signature
/// 펄스 불변식 값 `samples_per_period = pulse_period / refreshInterval`을 계산한다.
///
/// # 인자
/// - `cfg`: `pulse.pulse_period_seconds`.
/// - `refresh_interval_seconds`: settings.json에 주입된 refreshInterval(초).
///
/// # 반환
/// 한 색 주기 안에 그려지는 프레임 수. 불변식상 `≥ 6`이어야 출렁임이 끊기지 않는다
/// (계획서 §H-5, AC4 단위 테스트로 강제). `refresh_interval_seconds=0`이면 0 나눗셈을
/// 방어해 1초로 간주한다.
pub fn samples_per_period(cfg: &Config, refresh_interval_seconds: u64) -> u64 {
    let interval = refresh_interval_seconds.max(1);
    cfg.pulse.pulse_period_seconds / interval
}

/// 현재 시각을 펄스 한 주기 내 위상(0.0..1.0)으로 환산한다.
///
/// `phase = (now_ms mod period_ms) / period_ms`. `pulse_period_seconds=0`이면
/// 0 나눗셈을 방어해 1초로 간주한다(§H-4).
fn pulse_phase(now_ms: u128, cfg: &Config) -> f64 {
    let period_ms = cfg.pulse.pulse_period_seconds.max(1) as u128 * 1000;
    let offset = (now_ms % period_ms) as f64;
    offset / period_ms as f64
}

/// 두 채널값(0–255)을 비율 `t`(0..1)로 선형 보간한다.
fn lerp_channel(start: u8, end: u8, t: f64) -> u8 {
    let value = start as f64 + (end as f64 - start as f64) * t;
    value.round().clamp(0.0, 255.0) as u8
}

/// `color.pulse_palette[index]`의 "#rrggbb" 헥스를 [`ColorSpec`]으로 파싱한다.
///
/// 인덱스 범위 밖이거나 형식이 잘못되면 `None`을 반환한다(호출자가 기본값으로 저하).
fn palette_color(cfg: &Config, index: usize) -> Option<ColorSpec> {
    let entry = cfg.color.pulse_palette.get(index)?;
    parse_hex(entry)
}

/// "#rrggbb" 헥스 문자열을 [`ColorSpec`]으로 파싱하는 공개 래퍼(render에서 사용).
///
/// 라벨/구분자/HUD seam 색(`label_color`/`separator_color`)을 ANSI로 렌더할 때
/// render 단계가 호출한다. 형식이 잘못되면 `None`(호출자가 기본값으로 저하).
pub fn parse_hex_pub(hex: &str) -> Option<ColorSpec> {
    parse_hex(hex)
}

/// "#rrggbb" 또는 "rrggbb" 형식의 헥스 문자열을 [`ColorSpec`]으로 파싱한다.
///
/// 길이/16진수 형식이 맞지 않으면 `None`을 반환한다(패닉 금지).
fn parse_hex(hex: &str) -> Option<ColorSpec> {
    let trimmed = hex.trim().trim_start_matches('#');
    if trimmed.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&trimmed[0..2], 16).ok()?;
    let g = u8::from_str_radix(&trimmed[2..4], 16).ok()?;
    let b = u8::from_str_radix(&trimmed[4..6], 16).ok()?;
    Some(ColorSpec { r, g, b })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    /// 펄스 위상 영향을 배제한 정적 글리프 검증용 시각(CALM에선 어차피 위상 무관).
    const STATIC_NOW: u128 = 0;

    #[test]
    fn band_index_boundaries() {
        let cfg = Config::default();
        // 경계값: <25→0 / [25,50)→1 / [50,75)→2 / [75,90)→3 / ≥90→4.
        assert_eq!(band_index(24.9, &cfg), 0);
        assert_eq!(band_index(25.0, &cfg), 1);
        assert_eq!(band_index(49.9, &cfg), 1);
        assert_eq!(band_index(50.0, &cfg), 2);
        assert_eq!(band_index(74.9, &cfg), 2);
        assert_eq!(band_index(75.0, &cfg), 3);
        assert_eq!(band_index(89.9, &cfg), 3);
        assert_eq!(band_index(90.0, &cfg), 4);
        assert_eq!(band_index(100.0, &cfg), 4);
    }

    #[test]
    fn pick_emoji_band_glyphs() {
        let cfg = Config::default();
        // CALM 밴드 글리프: <25 ○ / [25,50) ▁ / [50,75) ▄ / [75,90) ▆ / ≥90 ◆.
        assert_eq!(pick_emoji(24.9, STATIC_NOW, false, &cfg), "○");
        assert_eq!(pick_emoji(25.0, STATIC_NOW, false, &cfg), "▁");
        assert_eq!(pick_emoji(60.0, STATIC_NOW, false, &cfg), "▄");
        assert_eq!(pick_emoji(89.9, STATIC_NOW, false, &cfg), "▆");
        assert_eq!(pick_emoji(90.0, STATIC_NOW, false, &cfg), "◆");
        assert_eq!(pick_emoji(100.0, STATIC_NOW, false, &cfg), "◆");
    }

    #[test]
    fn pick_emoji_glyph_is_stable_when_pulsing() {
        // CALM: 펄스가 켜져도, 그리고 시각(위상)이 바뀌어도 글리프 모양은 깜빡이지 않는다.
        let cfg = Config::default(); // pulse_period=30s, style="calm"
        let early = pick_emoji(95.0, 1_000, true, &cfg);
        let mid = pick_emoji(95.0, 15_000, true, &cfg);
        let late = pick_emoji(95.0, 20_000, true, &cfg);
        assert_eq!(early, "◆", "crit 밴드 글리프는 ◆");
        assert_eq!(early, mid, "펄스 위상이 달라도 글리프는 고정(깜빡임 금지)");
        assert_eq!(early, late, "펄스 위상이 달라도 글리프는 고정(깜빡임 금지)");
    }

    #[test]
    fn pick_emoji_respects_custom_emoji_glyphs() {
        // 사용자가 귀여운 얼굴을 복원하도록 load_glyphs를 재정의할 수 있다.
        let mut cfg = Config::default();
        cfg.cpu.load_glyphs = vec![
            "😌".to_string(),
            "🙂".to_string(),
            "😅".to_string(),
            "🥵".to_string(),
            "🔥".to_string(),
        ];
        assert_eq!(pick_emoji(10.0, STATIC_NOW, false, &cfg), "😌");
        assert_eq!(pick_emoji(95.0, STATIC_NOW, true, &cfg), "🔥");
    }

    #[test]
    fn band_tint_maps_band_to_palette() {
        let cfg = Config::default();
        // 밴드 0–3은 cool blue-gray, 밴드 4는 warm 테라코타(#b87848).
        assert_eq!(
            band_tint(0, &cfg),
            ColorSpec {
                r: 0x5a,
                g: 0x68,
                b: 0x78
            }
        );
        assert_eq!(
            band_tint(4, &cfg),
            ColorSpec {
                r: 0xb8,
                g: 0x78,
                b: 0x48
            }
        );
        // 밴드 4(warm)는 밴드 3(cool)과 명확히 다른 톤이어야 한다(유일한 warm 예외).
        assert_ne!(band_tint(3, &cfg), band_tint(4, &cfg));
    }

    #[test]
    fn pulse_gate_hysteresis() {
        let cfg = Config::default(); // on=90, off=80
                                     // prev=false: 88 → OFF 유지(<90), 92 → ON 전환
        assert!(!pulse_gate(88.0, false, &cfg));
        assert!(pulse_gate(92.0, false, &cfg));
        // prev=true: 85 → ON 유지(≥80), 78 → OFF 전환
        assert!(pulse_gate(85.0, true, &cfg));
        assert!(!pulse_gate(78.0, true, &cfg));
    }

    #[test]
    fn pulse_color_none_when_off() {
        let cfg = Config::default();
        assert_eq!(pulse_color(95.0, 1_234, false, &cfg), None);
    }

    #[test]
    fn pulse_color_pure_same_now_same_spec() {
        let cfg = Config::default();
        let a = pulse_color(95.0, 1_500, true, &cfg);
        let b = pulse_color(95.0, 1_500, true, &cfg);
        assert!(a.is_some());
        assert_eq!(a, b, "순수 함수: 같은 now → 같은 ColorSpec");
    }

    #[test]
    fn pulse_color_varies_across_now() {
        let cfg = Config::default();
        // 주기 30000ms 안에서 사인파 wave가 다른 두 시점은 틴트가 달라야 한다(지각성).
        // phase=0.25(7500ms)→wave=1.0(low 끝), phase=0.75(22500ms)→wave=0.0(high 끝).
        let peak = pulse_color(95.0, 7_500, true, &cfg);
        let trough = pulse_color(95.0, 22_500, true, &cfg);
        assert!(peak.is_some() && trough.is_some());
        assert_ne!(peak, trough, "펄스 틴트는 시각(위상)에 따라 변해야 함");
    }

    #[test]
    fn pulse_color_breathes_between_terracotta_endpoints() {
        let cfg = Config::default(); // pulse_palette = ["#b87848", "#7a5030"]
                                     // wave=0(phase=0.75) → start(high 테라코타 #b87848).
        let high = pulse_color(95.0, 22_500, true, &cfg).expect("펄스 ON 틴트");
        assert_eq!(
            high,
            ColorSpec {
                r: 0xb8,
                g: 0x78,
                b: 0x48
            },
            "wave=0 끝점은 high 테라코타여야 함"
        );
        // wave=1(phase=0.25) → end(low dim 테라코타 #7a5030).
        let low = pulse_color(95.0, 7_500, true, &cfg).expect("펄스 ON 틴트");
        assert_eq!(
            low,
            ColorSpec {
                r: 0x7a,
                g: 0x50,
                b: 0x30
            },
            "wave=1 끝점은 low dim 테라코타여야 함"
        );
    }

    #[test]
    fn pulse_color_has_no_hue_shift() {
        // CALM: hue shift 없음 — 두 끝점은 같은 따뜻한 톤(R>G>B) 안에서 휘도만 변한다.
        let cfg = Config::default();
        for &now in &[7_500u128, 15_000, 22_500, 1_000] {
            let tint = pulse_color(95.0, now, true, &cfg).expect("펄스 ON 틴트");
            assert!(
                tint.r > tint.g && tint.g > tint.b,
                "테라코타 톤(R>G>B) 유지: {tint:?}"
            );
        }
    }

    #[test]
    fn samples_per_period_default_is_six() {
        let cfg = Config::default(); // pulse_period_seconds=30, refresh=5
                                     // 불변식: pulse_period(30) / refresh(5) = 6 ≥ 6
        assert_eq!(samples_per_period(&cfg, cfg.refresh.interval_seconds), 6);
        assert!(
            samples_per_period(&cfg, cfg.refresh.interval_seconds) >= 6,
            "불변식: samples_per_period ≥ 6 (지각성)"
        );
    }

    #[test]
    fn samples_per_period_guards_zero_interval() {
        let cfg = Config::default(); // pulse_period_seconds=30
                                     // 0 나눗셈 방어: interval=0이면 1초로 간주 → 30/1=30.
        assert_eq!(samples_per_period(&cfg, 0), 30);
    }
}
