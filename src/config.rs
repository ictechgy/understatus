//! TOML 설정 (`~/.config/understatus/config.toml`).
//!
//! 계획서 §H-8 스키마를 그대로 반영한다. 파일이 없거나 TOML이 깨졌으면
//! 전 항목 기본값으로 안전 저하하며(깨진 TOML은 stderr 경고), 절대 패닉하지 않는다.

use serde::Deserialize;

/// understatus 전체 설정. 각 섹션은 §H-8 TOML의 테이블에 1:1 대응한다.
///
/// `#[serde(default)]`로 부분 설정/누락 섹션을 안전하게 기본값으로 채운다.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Config {
    /// `[cpu]`: 이모지 임계값, 더블샘플 윈도, 정밀 모드.
    pub cpu: CpuConfig,
    /// `[pulse]`: 펄스 히스테리시스 임계값, 주기, 스타일.
    pub pulse: PulseConfig,
    /// `[chain]`: 기존 statusLine 체이닝 명령, 순서, 캐시 TTL, 타임아웃.
    pub chain: ChainConfig,
    /// `[display]`: 최대 폭 + 세그먼트 표시 토글.
    pub display: DisplayConfig,
    /// `[color]`: 색상 모드 + 펄스 팔레트.
    pub color: ColorConfig,
    /// `[refresh]`: settings.json refreshInterval 주입값.
    pub refresh: RefreshConfig,
}

/// `[cpu]` 섹션. 임계값은 진짜 순간 CPU%(0–100) 기준.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct CpuConfig {
    /// 밴드 전환 임계값 [25, 50, 75, 90] (오름차순 4개).
    pub emoji_thresholds: [f64; 4],
    /// 밴드 글리프(load glyph) 5개: idle/low/mid/high/crit.
    ///
    /// 기본값은 단일 셀 폭(single-cell)의 차분한 글리프 ["○","▁","▄","▆","◆"]
    /// (U+25CB, U+2581, U+2584, U+2586, U+25C6). 밴드별 글리프는 **안정적**이라
    /// 펄스 중에도 두 글리프 사이를 깜빡이지 않는다.
    ///
    /// 귀여운 이모지 얼굴을 복원하려면 config.toml에 다음을 지정하면 된다:
    /// `load_glyphs = ["😌", "🙂", "😅", "🥵", "🔥"]`.
    pub load_glyphs: Vec<String>,
    /// 더블샘플 간격(ms). 지연 예산에 직접 영향(기본 25, 25→50 시 노이즈↓·지연↑).
    pub sample_window_ms: u64,
    /// true면 P3 데몬 사용(옵트인). 기본 false = 더블샘플.
    pub precision_mode: bool,
}

/// `[pulse]` 섹션. 히스테리시스(MAJOR-1): ON ≥ on_threshold, OFF < off_threshold.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct PulseConfig {
    /// 이 값 이상에서 펄스 ON으로 전환(기본 90).
    pub pulse_on_threshold: f64,
    /// 이 값 미만으로 떨어져야 펄스 OFF로 전환(기본 80).
    pub pulse_off_threshold: f64,
    /// 색 출렁임 한 주기 길이(초). 불변식: pulse_period / refreshInterval ≥ 6.
    pub pulse_period_seconds: u64,
    /// "calm"(기본) | "bold"(레거시).
    ///
    /// - "calm": 글리프 모양은 **고정**, 글리프 틴트만 테라코타 high↔low 사이를
    ///   부드럽게 숨쉬듯 보간한다(hue shift 없음).
    /// - "bold": 레거시 옵션. 빨강↔주황 과감 스윙 + 글리프 깜빡임(기본값 아님).
    pub pulse_style: String,
}

/// `[chain]` 섹션. 기존 statusLine을 자식 프로세스로 보존(체이닝).
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ChainConfig {
    /// 설치가 보존한 원본 statusLine 명령. `None`/빈 값이면 체이닝 없음.
    pub chain_command: Option<String>,
    /// "self_first" | "chain_first" (기본 self_first).
    pub order: String,
    /// 체인 자식 stdout 캐시 TTL(초). 무거운 자식 디커플(CRITICAL-1).
    pub chain_cache_ttl_seconds: u64,
    /// 체인 자식 스폰 타임아웃(ms). 초과 시 캐시/빈 문자열로 저하(CRITICAL-1).
    pub chain_timeout_ms: u64,
}

/// `[display]` 섹션. 한 줄 폭 제한 + 세그먼트 표시 토글.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct DisplayConfig {
    /// 한 줄 최대 폭(와이드 이모지 2칸 계산). 초과 시 저우선 세그먼트 생략/축약.
    pub max_width: usize,
    /// 모델 표시명 노출.
    pub show_model: bool,
    /// 누적 비용 노출.
    pub show_cost: bool,
    /// 컨텍스트 사용률 노출.
    pub show_context: bool,
    /// git 브랜치(workspace.git_worktree/repo 파생) 노출.
    pub show_git: bool,
    /// 배터리(P2, IOKit + TTL 캐시) 노출.
    pub show_battery: bool,
    /// 디스크 사용률(P2, statfs("/")) 노출.
    pub show_disk: bool,
    /// 네트워크 throughput(P2, getifaddrs 델타) 노출.
    pub show_network: bool,
}

/// `[color]` 섹션. NO_COLOR 환경변수가 있으면 아래와 무관하게 색상 비활성.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ColorConfig {
    /// "auto" | "truecolor" | "256" | "none".
    pub mode: String,
    /// 펄스 틴트 숨쉬기 팔레트 [high, low]. 기본 테라코타 ["#b87848", "#7a5030"]
    /// (high 테라코타, low 약 58% 휘도로 dim). hue shift 없이 두 끝점 사이만 보간.
    pub pulse_palette: Vec<String>,
    /// 밴드별 글리프 틴트 5개: idle/low/mid/high/crit.
    ///
    /// 기본값 ["#5a6878","#6d8296","#86a0b4","#9fbfce","#b87848"]. 밴드 0–3은
    /// 차가운 blue-gray 휘도 사다리, 밴드 4만 따뜻한 예외(muted terracotta).
    /// 글리프 문자에만 적용한다(COLOR-ONCE: CPU% 숫자엔 색을 입히지 않음).
    pub band_tints: Vec<String>,
    /// 라벨/단위(mem, disk, ctx, ↓ ↑, $, git 마커 ⎇)에 쓰는 dim 색.
    /// 기본 "#6b7280"(≈ rgba(255,255,255,0.44) on dark).
    pub label_color: String,
    /// 세그먼트 구분자(가운뎃점). 기본 " · ".
    pub separator: String,
    /// 구분자/HUD seam에 쓰는 더 어두운 dim 색. 기본 "#3b4048"(≈ rgba 0.22).
    pub separator_color: String,
    /// self + chain 출력 사이에 끼우는 HUD 경계 글리프. 기본 "│"(separator_color로 렌더).
    pub hud_seam: String,
}

/// `[refresh]` 섹션. settings.json refreshInterval 주입값(전역 부작용 주의).
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct RefreshConfig {
    /// 주입할 refreshInterval 초. 기본 1(부드러운 ~6–8초 펄스 출렁임).
    pub interval_seconds: u64,
}

impl Default for CpuConfig {
    fn default() -> Self {
        Self {
            emoji_thresholds: [25.0, 50.0, 75.0, 90.0],
            // CALM 기본 글리프(단일 셀 폭): ○ ▁ ▄ ▆ ◆.
            load_glyphs: vec![
                "○".to_string(),
                "▁".to_string(),
                "▄".to_string(),
                "▆".to_string(),
                "◆".to_string(),
            ],
            sample_window_ms: 25,
            precision_mode: false,
        }
    }
}

impl Default for PulseConfig {
    fn default() -> Self {
        Self {
            pulse_on_threshold: 90.0,
            pulse_off_threshold: 80.0,
            pulse_period_seconds: 30,
            pulse_style: "calm".to_string(),
        }
    }
}

impl Default for ChainConfig {
    fn default() -> Self {
        Self {
            chain_command: None,
            order: "self_first".to_string(),
            chain_cache_ttl_seconds: 10,
            chain_timeout_ms: 500,
        }
    }
}

impl Default for DisplayConfig {
    fn default() -> Self {
        Self {
            max_width: 80,
            show_model: true,
            show_cost: true,
            show_context: true,
            show_git: true,
            show_battery: true,
            show_disk: true,
            show_network: true,
        }
    }
}

impl Default for ColorConfig {
    fn default() -> Self {
        Self {
            mode: "auto".to_string(),
            // CALM 펄스 숨쉬기: high 테라코타 ↔ low dim 테라코타(hue shift 없음).
            pulse_palette: vec!["#b87848".to_string(), "#7a5030".to_string()],
            // 밴드 0–3 cool blue-gray 사다리 + 밴드 4 warm 테라코타.
            band_tints: vec![
                "#5a6878".to_string(),
                "#6d8296".to_string(),
                "#86a0b4".to_string(),
                "#9fbfce".to_string(),
                "#b87848".to_string(),
            ],
            label_color: "#6b7280".to_string(),
            separator: " · ".to_string(),
            separator_color: "#3b4048".to_string(),
            hud_seam: "│".to_string(),
        }
    }
}

impl Default for RefreshConfig {
    fn default() -> Self {
        Self {
            interval_seconds: 5,
        }
    }
}

impl Default for Config {
    /// 계획서 §H-8 TOML의 전 항목 기본값으로 [`Config`]를 구성한다.
    fn default() -> Self {
        Self {
            cpu: CpuConfig::default(),
            pulse: PulseConfig::default(),
            chain: ChainConfig::default(),
            display: DisplayConfig::default(),
            color: ColorConfig::default(),
            refresh: RefreshConfig::default(),
        }
    }
}

// CONTRACT: signature is frozen — implement body only, do not change this signature
/// 설정 파일을 로드한다(`~/.config/understatus/config.toml`).
///
/// # 반환
/// 파싱된 [`Config`]. 파일이 없으면 [`Config::default`]를 반환하고,
/// TOML이 깨졌으면 stderr에 경고를 출력한 뒤 기본값으로 저하한다(패닉 금지, AC7).
///
/// # 주의
/// 경로는 `UNDERSTATUS_CONFIG` 환경변수로 재정의 가능(테스트/측정용, AC6).
pub fn load_config() -> Config {
    let path = match config_path() {
        Some(path) => path,
        // HOME조차 알 수 없으면 기본값으로 안전 저하한다.
        None => return Config::default(),
    };

    // 파일 부재 → 조용히 기본값(경고 없음, AC7).
    let contents = match std::fs::read_to_string(&path) {
        Ok(contents) => contents,
        Err(_) => return Config::default(),
    };

    parse_config_str(&contents)
}

/// 설정 파일 경로를 결정한다.
///
/// # 반환
/// `UNDERSTATUS_CONFIG` 환경변수가 있으면 그 경로(테스트/측정용, AC6),
/// 없으면 `~/.config/understatus/config.toml`. HOME을 알 수 없으면 `None`.
fn config_path() -> Option<std::path::PathBuf> {
    if let Ok(override_path) = std::env::var("UNDERSTATUS_CONFIG") {
        return Some(std::path::PathBuf::from(override_path));
    }
    let home = home_dir()?;
    Some(home.join(".config").join("understatus").join("config.toml"))
}

/// TOML 문자열을 [`Config`]로 파싱하는 순수 헬퍼(테스트 가능).
///
/// # 인자
/// - `contents`: TOML 본문.
///
/// # 반환
/// 파싱된 [`Config`](부분 설정은 `#[serde(default)]`로 병합). TOML이 깨졌으면
/// stderr에 경고를 출력하고 [`Config::default`]로 저하한다(패닉 금지, AC7).
/// 파싱 성공 시 `chain_command`의 `$HOME`/`~`를 확장한다.
pub fn parse_config_str(contents: &str) -> Config {
    match toml::from_str::<Config>(contents) {
        Ok(mut config) => {
            expand_chain_command(&mut config);
            config
        }
        Err(error) => {
            eprintln!("understatus: config.toml 파싱 실패({error}). 기본값으로 진행합니다.");
            Config::default()
        }
    }
}

/// `chain_command`에 포함된 `$HOME`/`~`를 사용자 홈 경로로 확장한다.
///
/// # 인자
/// - `config`: in-place로 `chain.chain_command`를 수정할 설정.
///
/// 확장 근거: 설치가 보존하는 원본 명령(예 `node $HOME/.claude/hud/...`)은
/// 셸 변수를 포함하므로, sh -c로 실행하기 전 또는 표시 시점에 홈을 치환한다.
fn expand_chain_command(config: &mut Config) {
    let home = match home_dir() {
        Some(home) => home,
        None => return,
    };
    let home_str = home.to_string_lossy();
    if let Some(command) = config.chain.chain_command.as_mut() {
        let expanded = command
            .replace("$HOME", &home_str)
            .replace("${HOME}", &home_str);
        // 선행 `~`(`~/` 또는 단독 `~`)만 홈으로 치환한다(중간의 ~는 보존).
        let expanded = if let Some(rest) = expanded.strip_prefix("~/") {
            format!("{home_str}/{rest}")
        } else if expanded == "~" {
            home_str.to_string()
        } else {
            expanded
        };
        *command = expanded;
    }
}

/// 사용자 홈 디렉터리를 반환한다(`HOME` 환경변수).
///
/// # 반환
/// `HOME` 경로. 미설정 시 `None`(호출부에서 기본값/무확장으로 안전 저하).
fn home_dir() -> Option<std::path::PathBuf> {
    std::env::var_os("HOME").map(std::path::PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 빈 TOML은 전 항목 기본값을 반환해야 한다(AC7).
    #[test]
    fn empty_toml_is_default() {
        let config = parse_config_str("");
        assert_eq!(config.cpu.sample_window_ms, 25);
        assert_eq!(config.pulse.pulse_on_threshold, 90.0);
        assert_eq!(config.pulse.pulse_off_threshold, 80.0);
        assert_eq!(config.chain.chain_cache_ttl_seconds, 10);
        assert_eq!(config.chain.chain_timeout_ms, 500);
        assert_eq!(config.display.max_width, 80);
        assert!(config.display.show_disk);
        assert!(config.display.show_network);
        assert!(config.display.show_battery);
        assert_eq!(config.refresh.interval_seconds, 5);
        assert_eq!(config.chain.chain_command, None);
    }

    /// Default impl이 CALM 디자인 기본값과 일치하는지 직접 검증한다.
    #[test]
    fn default_impl_matches_spec() {
        let config = Config::default();
        assert_eq!(config.cpu.emoji_thresholds, [25.0, 50.0, 75.0, 90.0]);
        // CALM: pulse_style 기본은 "calm"(레거시 "bold" 아님).
        assert_eq!(config.pulse.pulse_style, "calm");
        assert_eq!(config.chain.order, "self_first");
        assert_eq!(config.color.mode, "auto");
        // CALM 글리프 사다리(단일 셀 폭): ○ ▁ ▄ ▆ ◆.
        assert_eq!(
            config.cpu.load_glyphs,
            vec!["○", "▁", "▄", "▆", "◆"]
        );
        // 펄스 숨쉬기 끝점: high/low 테라코타(hue shift 없음).
        assert_eq!(
            config.color.pulse_palette,
            vec!["#b87848".to_string(), "#7a5030".to_string()]
        );
        // 밴드 틴트: cool blue-gray 사다리 4 + warm 테라코타 1.
        assert_eq!(
            config.color.band_tints,
            vec!["#5a6878", "#6d8296", "#86a0b4", "#9fbfce", "#b87848"]
        );
        // dim 라벨/구분자/seam 기본값.
        assert_eq!(config.color.label_color, "#6b7280");
        assert_eq!(config.color.separator, " · ");
        assert_eq!(config.color.separator_color, "#3b4048");
        assert_eq!(config.color.hud_seam, "│");
    }

    /// 부분 설정은 해당 키만 덮어쓰고 나머지는 기본값을 유지해야 한다(serde default 병합).
    #[test]
    fn partial_toml_merges_with_defaults() {
        let toml = r#"
            [pulse]
            pulse_on_threshold = 75
            [display]
            show_battery = false
        "#;
        let config = parse_config_str(toml);
        // 명시한 키는 반영.
        assert_eq!(config.pulse.pulse_on_threshold, 75.0);
        assert!(!config.display.show_battery);
        // 미명시 키는 기본값 유지.
        assert_eq!(config.pulse.pulse_off_threshold, 80.0);
        assert_eq!(config.cpu.sample_window_ms, 25);
        assert!(config.display.show_model);
    }

    /// 깨진 TOML은 기본값으로 저하해야 한다(stderr 경고, 패닉 금지, AC7).
    #[test]
    fn broken_toml_falls_back_to_default() {
        let config = parse_config_str("this is = = not valid toml ][");
        // 기본값과 동일해야 한다.
        assert_eq!(config.cpu.sample_window_ms, 25);
        assert_eq!(config.pulse.pulse_on_threshold, 90.0);
    }

    /// chain_command의 `$HOME`이 실제 홈 경로로 확장되어야 한다.
    #[test]
    fn expands_home_var_in_chain_command() {
        // HOME 의존: 테스트 환경에 HOME이 설정되어 있다고 가정.
        let home = std::env::var("HOME").expect("테스트 환경에 HOME 필요");
        let toml = r#"
            [chain]
            chain_command = "node $HOME/.claude/hud/lterm-omc-hud.mjs"
        "#;
        let config = parse_config_str(toml);
        let command = config.chain.chain_command.expect("chain_command 있어야 함");
        assert_eq!(
            command,
            format!("node {home}/.claude/hud/lterm-omc-hud.mjs")
        );
        assert!(!command.contains("$HOME"));
    }

    /// 선행 `~/`도 홈 경로로 확장되어야 한다.
    #[test]
    fn expands_leading_tilde_in_chain_command() {
        let home = std::env::var("HOME").expect("테스트 환경에 HOME 필요");
        let toml = r#"
            [chain]
            chain_command = "~/bin/myhud"
        "#;
        let config = parse_config_str(toml);
        let command = config.chain.chain_command.expect("chain_command 있어야 함");
        assert_eq!(command, format!("{home}/bin/myhud"));
    }
}
