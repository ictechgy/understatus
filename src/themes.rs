//! 테마 프리셋 카탈로그(단일 소스). 새 테마 추가 = 이 파일에 항목 1개 추가.
//!
//! 모든 테마는 기존 `Config` 필드만으로 표현된다(스키마 변경 없음). 각 테마는
//! 8개 시각 필드의 묶음이며, `calm` 프리셋은 `Config::default()`의 테마 필드와
//! **정확히 동일**해야 한다(회귀 방지: 동등성 테스트로 강제).

/// 테마가 소유하는 시각 필드 묶음(`Config` 테마 키의 부분집합).
///
/// 모든 프리셋은 8개 키를 전부 구체값으로 정의한다(부분 프리셋 금지).
///
/// # 주의
/// `pulse_style`은 기존 테마("calm"·"mono"·"vivid"·"ember"·"emoji")에서는 "calm"이고,
/// 신규 화려한 테마(neon·spectrum="hue", aurora·sunset="flash")에서는 bold 값을 예약한다.
/// 현재 render/theme 어디서도 이 값이 분기에 쓰이지 않는 "데드 데이터"이며, 향후 펄스
/// 애니메이션 구현 시 실제 시각 채널로 승격된다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThemePreset {
    /// `cpu.load_glyphs`(5개): idle/low/mid/high/crit 밴드 글리프.
    pub load_glyphs: Vec<String>,
    /// `pulse.pulse_style`("calm" — 데드 데이터).
    pub pulse_style: String,
    /// `color.band_tints`(5개 hex): 밴드별 글리프 틴트.
    pub band_tints: Vec<String>,
    /// `color.pulse_palette`(2개 hex): 펄스 숨쉬기 [high, low].
    pub pulse_palette: Vec<String>,
    /// `color.label_color`: 라벨/단위 dim 색.
    pub label_color: String,
    /// `color.separator`: 세그먼트 구분자.
    pub separator: String,
    /// `color.separator_color`: 구분자/HUD seam dim 색.
    pub separator_color: String,
    /// `color.hud_seam`: self + chain 경계 글리프.
    pub hud_seam: String,
}

/// 테마가 소유하는 (섹션, 키) 경로 목록 — config 해석 has_key 검사 SSOT.
///
/// [`ThemePreset`] 필드 8개와 1:1 대응한다. 길이/대응은
/// `theme_keys_match_preset_fields` 테스트로 강제한다(동기화 누락 가드).
pub const THEME_KEYS: &[(&str, &str)] = &[
    ("cpu", "load_glyphs"),
    ("pulse", "pulse_style"),
    ("color", "band_tints"),
    ("color", "pulse_palette"),
    ("color", "label_color"),
    ("color", "separator"),
    ("color", "separator_color"),
    ("color", "hud_seam"),
];

/// 출시 테마 (이름, 한 줄 설명) 목록. 출시 순서대로(calm 기본 → mono → vivid → ember → emoji → neon → aurora → sunset → spectrum).
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

/// 문자열 슬라이스 배열을 `Vec<String>`으로 변환하는 내부 헬퍼.
///
/// 프리셋 정의의 보일러플레이트를 줄인다(각 프리셋이 `.to_string()`을 반복하지 않도록).
fn to_owned(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

/// calm 프리셋(기본). `Config::default()`의 테마 필드와 정확히 동일해야 한다.
fn calm_preset() -> ThemePreset {
    ThemePreset {
        load_glyphs: to_owned(&["○", "▁", "▄", "▆", "◆"]),
        pulse_style: "calm".to_string(),
        band_tints: to_owned(&["#5a6878", "#6d8296", "#86a0b4", "#9fbfce", "#b87848"]),
        pulse_palette: to_owned(&["#b87848", "#7a5030"]),
        label_color: "#6b7280".to_string(),
        separator: " · ".to_string(),
        separator_color: "#3b4048".to_string(),
        hud_seam: "│".to_string(),
    }
}

/// mono 프리셋. 무채색 그레이 사다리, 글리프는 calm과 동일.
fn mono_preset() -> ThemePreset {
    ThemePreset {
        load_glyphs: to_owned(&["○", "▁", "▄", "▆", "◆"]),
        pulse_style: "calm".to_string(),
        band_tints: to_owned(&["#636363", "#7e7e7e", "#9c9c9c", "#bdbdbd", "#e8e8e8"]),
        pulse_palette: to_owned(&["#e8e8e8", "#9c9c9c"]),
        label_color: "#6b7280".to_string(),
        separator: " · ".to_string(),
        separator_color: "#3b4048".to_string(),
        hud_seam: "│".to_string(),
    }
}

/// vivid 프리셋. 신호등 색 사다리 + 블록 글리프.
///
/// # 주의
/// `load_glyphs`의 4·5번째 글리프 중복(`█ █`)은 스펙 §3 표 그대로의 **의도된** 값이다.
fn vivid_preset() -> ThemePreset {
    ThemePreset {
        load_glyphs: to_owned(&["░", "▒", "▓", "█", "█"]),
        pulse_style: "calm".to_string(),
        band_tints: to_owned(&["#2f9150", "#3fb083", "#cda23e", "#f0a24e", "#e34a3a"]),
        pulse_palette: to_owned(&["#e34a3a", "#bf4135"]),
        label_color: "#6b7280".to_string(),
        separator: " · ".to_string(),
        separator_color: "#3b4048".to_string(),
        hud_seam: "│".to_string(),
    }
}

/// ember 프리셋. 따뜻한 앰버/테라코타 단색 사다리 + 도트 글리프.
fn ember_preset() -> ThemePreset {
    ThemePreset {
        load_glyphs: to_owned(&["·", "∙", "•", "●", "◉"]),
        pulse_style: "calm".to_string(),
        band_tints: to_owned(&["#7a6450", "#96714f", "#b08355", "#c79a63", "#cf5a48"]),
        pulse_palette: to_owned(&["#cf5a48", "#a8483a"]),
        label_color: "#7a6f63".to_string(),
        separator: " · ".to_string(),
        separator_color: "#4a4239".to_string(),
        hud_seam: "│".to_string(),
    }
}

/// emoji 프리셋. 이모지 표정 램프(각 글리프 2칸 폭).
fn emoji_preset() -> ThemePreset {
    ThemePreset {
        load_glyphs: to_owned(&["😌", "🙂", "😅", "🥵", "🔥"]),
        pulse_style: "calm".to_string(),
        band_tints: to_owned(&["#6e7d92", "#86978f", "#a39a78", "#c6a35c", "#e0683c"]),
        pulse_palette: to_owned(&["#e0683c", "#a04528"]),
        label_color: "#6b7280".to_string(),
        separator: " · ".to_string(),
        separator_color: "#383d45".to_string(),
        hud_seam: "│".to_string(),
    }
}

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

/// 알려진 테마 이름을 프리셋으로 조회한다.
///
/// # 인자
/// - `name`: 테마 이름(예 "vivid"). 대소문자 구분.
///
/// # 반환
/// 알려진 테마면 [`ThemePreset`], 미지의 이름이면 `None`(호출부가 calm 폴백/에러 결정).
pub fn preset(name: &str) -> Option<ThemePreset> {
    match name {
        "calm" => Some(calm_preset()),
        "mono" => Some(mono_preset()),
        "vivid" => Some(vivid_preset()),
        "ember" => Some(ember_preset()),
        "emoji" => Some(emoji_preset()),
        "neon" => Some(neon_preset()),
        "aurora" => Some(aurora_preset()),
        "sunset" => Some(sunset_preset()),
        "spectrum" => Some(spectrum_preset()),
        _ => None,
    }
}

/// 표시/검증용 (이름, 한 줄 설명) 목록을 출시 순서대로 돌려준다.
pub fn catalog() -> &'static [(&'static str, &'static str)] {
    CATALOG
}

/// 유효한 테마 이름인지 판정한다(설치/`theme` 명령의 하드 검증용).
pub fn is_known(name: &str) -> bool {
    preset(name).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    /// calm 회귀 핵심 게이트: `preset("calm")` 8개 값 전수가 `Config::default()` 대응 8필드와 동일.
    ///
    /// 기존 94 테스트가 theme 필드를 검사하지 않으므로(블로킹 D) 이 테스트가 실제 게이트다.
    #[test]
    fn preset_calm_matches_default_config() {
        let calm = preset("calm").expect("calm은 항상 존재");
        let default = Config::default();
        assert_eq!(calm.load_glyphs, default.cpu.load_glyphs, "load_glyphs");
        assert_eq!(calm.pulse_style, default.pulse.pulse_style, "pulse_style");
        assert_eq!(calm.band_tints, default.color.band_tints, "band_tints");
        assert_eq!(
            calm.pulse_palette, default.color.pulse_palette,
            "pulse_palette"
        );
        assert_eq!(calm.label_color, default.color.label_color, "label_color");
        assert_eq!(calm.separator, default.color.separator, "separator");
        assert_eq!(
            calm.separator_color, default.color.separator_color,
            "separator_color"
        );
        assert_eq!(calm.hud_seam, default.color.hud_seam, "hud_seam");
    }

    /// `THEME_KEYS`가 8개이고 각 경로가 유효 섹션을 가리키는지(동기화 누락 가드).
    #[test]
    fn theme_keys_match_preset_fields() {
        assert_eq!(
            THEME_KEYS.len(),
            8,
            "THEME_KEYS는 ThemePreset 필드와 1:1(8개)"
        );
        let valid_sections = ["cpu", "pulse", "color"];
        for (section, key) in THEME_KEYS {
            assert!(
                valid_sections.contains(section),
                "알 수 없는 섹션: {section}.{key}"
            );
        }
    }

    /// 5종 프리셋의 band_tints/load_glyphs는 길이 5, pulse_palette는 길이 2여야 한다.
    #[test]
    fn all_presets_have_5_band_tints_and_glyphs() {
        for (name, _) in catalog() {
            let p = preset(name).expect("catalog 이름은 항상 프리셋 존재");
            assert_eq!(p.load_glyphs.len(), 5, "{name} load_glyphs 길이");
            assert_eq!(p.band_tints.len(), 5, "{name} band_tints 길이");
            assert_eq!(p.pulse_palette.len(), 2, "{name} pulse_palette 길이");
        }
    }

    /// 모든 프리셋의 모든 hex 값은 `#rrggbb` 형식이어야 한다.
    #[test]
    fn all_preset_hex_are_valid() {
        let is_valid_hex = |s: &str| {
            s.len() == 7 && s.starts_with('#') && s[1..].chars().all(|c| c.is_ascii_hexdigit())
        };
        for (name, _) in catalog() {
            let p = preset(name).expect("catalog 이름은 항상 프리셋 존재");
            for hex in p.band_tints.iter().chain(p.pulse_palette.iter()) {
                assert!(is_valid_hex(hex), "{name} 잘못된 hex: {hex}");
            }
            assert!(is_valid_hex(&p.label_color), "{name} label_color");
            assert!(is_valid_hex(&p.separator_color), "{name} separator_color");
        }
    }

    /// emoji load_glyphs는 각각 단일 코드포인트이며 render의 2칸 처리 범위(0x1F300..=0x1FAFF)에 든다.
    ///
    /// render의 display_width/char_width/is_wide가 모두 private이므로 직접 호출 대신
    /// 각 코드포인트가 2칸 처리 범위(render.rs:356)에 드는지 검증으로 대체한다.
    #[test]
    fn emoji_glyphs_are_single_char_width_two() {
        let emoji = preset("emoji").expect("emoji 프리셋 존재");
        for glyph in &emoji.load_glyphs {
            assert_eq!(
                glyph.chars().count(),
                1,
                "emoji 글리프는 단일 코드포인트: {glyph}"
            );
            let code = glyph.chars().next().expect("비어있지 않음") as u32;
            assert!(
                (0x1F300..=0x1FAFF).contains(&code),
                "emoji 글리프 {glyph}(U+{code:X})는 2칸 처리 범위 밖"
            );
        }
    }

    /// catalog의 모든 이름이 `is_known`을 통과하고, 역으로 catalog 외 이름은 미지여야 한다.
    #[test]
    fn catalog_matches_is_known() {
        for (name, _) in catalog() {
            assert!(is_known(name), "catalog 이름 {name}은 is_known 통과해야");
        }
        assert!(!is_known("nonexistent"), "미지 테마는 is_known false");
        assert!(!is_known(""), "빈 문자열은 미지 테마");
    }

    /// catalog 순서가 출시 순서(calm..emoji 다음 neon..spectrum)와 일치해야 한다.
    #[test]
    fn catalog_order_is_release_order() {
        let names: Vec<&str> = catalog().iter().map(|(name, _)| *name).collect();
        assert_eq!(
            names,
            vec!["calm", "mono", "vivid", "ember", "emoji", "neon", "aurora", "sunset", "spectrum"]
        );
    }

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
}
