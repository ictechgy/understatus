//! TOML 설정 (`~/.config/understatus/config.toml`).
//!
//! 계획서 §H-8 스키마를 그대로 반영한다. 파일이 없거나 TOML이 깨졌으면
//! 전 항목 기본값으로 안전 저하하며(깨진 TOML은 stderr 경고), 절대 패닉하지 않는다.

use serde::Deserialize;
use std::io::Read;

use crate::themes;

/// 렌더 핫패스에서 허용하는 config.toml 최대 크기.
///
/// 실제 설정은 수 KiB 수준이다. 비정상적으로 큰 파일이 매 statusline 렌더마다 무제한 read/parse 비용을
/// 유발하지 않도록 256KiB에서 기본값으로 안전 저하한다.
const MAX_CONFIG_BYTES: usize = 256 * 1024;

/// understatus 전체 설정. 각 섹션은 §H-8 TOML의 테이블에 1:1 대응한다.
///
/// `#[serde(default)]`로 부분 설정/누락 섹션을 안전하게 기본값으로 채운다.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Config {
    /// 적용 테마 이름. config.toml 최상위 `theme = "vivid"`.
    ///
    /// 미설정/미지 테마면 calm으로 안전 저하한다. `String::default()`는 `""`이므로
    /// `#[serde(default = "default_theme")]`로 명시적으로 "calm" 기본값을 보장한다.
    #[serde(default = "default_theme")]
    pub theme: String,
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
    /// `[codex]`: Codex 세션 심층판독(opt-out, freshness, 스캔 일수).
    pub codex: CodexConfig,
    /// `[context]`: ctx(컨텍스트 사용률%) hold 튜닝(직전 native 유지 시간, hold 해제 임계치).
    pub context: ContextConfig,
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
    /// 펄스 스타일: "calm"(기본) | "flash" | "hue" | "swap".
    ///
    /// - "calm": 글리프 모양 **고정**, 틴트만 테라코타 high↔low를 숨쉬듯 보간(hue 불변).
    /// - "flash": 같은 두 끝점, 더 가파른 곡선(감마 2.2)으로 대비를 키운 호흡.
    /// - "hue": 글리프 고정, 틴트 hue를 한 주기 동안 360° 회전(무지개 시머).
    /// - "swap": hue 순환 + 위상 후반 글리프 모양 교대(◆↔◇ 등).
    ///
    /// `theme::pulse_color`(틴트)와 `theme::pick_emoji`(swap 글리프)가 소비한다.
    /// 미지 값은 런타임에서 calm으로 안전 저하(쓰기 경로는 하드 검증).
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
    /// 체인 자식(예: omc HUD) 출력에서 ctx 표시(인라인 `ctx:NN%`/`CRITICAL`/`COMPRESS?`
    /// 토큰과 `[!] ctx … run /compact` 경고줄)를 제거한다. 기본 true.
    ///
    /// 동기: understatus는 Claude payload의 `used_percentage`를 그대로 표시(권위)하지만,
    /// 체인 HUD는 그 값이 누락된 프레임에서 토큰비율로 ctx를 **자체 계산**해 같은 상태에서도
    /// 값이 튀고(예 86↔98) self ctx와 중복된다. self ctx가 꺼져 있어도 무조건 제거한다
    /// — 표시하지 않는 편이 발명된 값보다 정직하기 때문이다.
    pub strip_chain_ctx: bool,
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
    /// lterm 세션/페인 라벨(예 "codex/%3") 노출. lterm 소스 전용(Claude 경로는 미표시).
    pub show_session: bool,
    /// 배터리(P2, IOKit + TTL 캐시) 노출.
    pub show_battery: bool,
    /// 디스크 사용률(P2, statfs("/")) 노출.
    pub show_disk: bool,
    /// 네트워크 throughput(P2, getifaddrs 델타) 노출.
    pub show_network: bool,
    /// Claude rate-limit(5h/7d 쿼터 % + 리셋 카운트다운) 세그먼트 노출. Claude 소스 전용.
    pub show_rate_limit: bool,
    /// rate-limit 경고 임계치(%). `Some(t)`이고 사용률 ≥ t이면 값에 경고색을 입힌다(opt-in).
    /// 기본 `None`(무색, calm) — 임계 인지가 필요한 사용자만 활성화한다(M2 결정).
    pub rate_limit_warn_threshold: Option<u32>,
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

/// `[codex]` 섹션. Codex 세션 심층판독(`render --source lterm` + agent=codex)의 토글/한도.
///
/// `enabled=false`면 `~/.codex`를 일절 읽지 않는다(프라이버시/성능 opt-out, spec §2.5).
/// freshness/scan_days는 후보 스캔의 바운디드 비용 상한이다(spec §5/§8).
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct CodexConfig {
    /// Codex 세션 심층판독 활성 여부. 기본 true(opt-out). false면 `~/.codex` 무접촉.
    pub enabled: bool,
    /// 후보 rollout 파일의 mtime 신선도 상한(분). 기본 240(4시간). 초과 후보는 제외.
    pub freshness_minutes: u64,
    /// 스캔할 최근 일자 디렉터리 수. 기본 3(전체 4400+ 디렉터리 풀스캔 회피, spec §5).
    pub scan_days: usize,
}

/// `[context].hold_ttl_seconds`의 serde/Default 기본값(초).
///
/// native(`used_percentage`) 누락 프레임에서 직전 native를 유지(hold)할 최대 시간.
/// 다른 모듈/테스트가 "프로덕션 기본 TTL"을 재사용할 수 있도록 pub 상수로 노출한다.
pub const DEFAULT_CONTEXT_HOLD_TTL_SECONDS: u64 = 180;

/// `[context].drop_tolerance`의 serde/Default 기본값(%포인트).
///
/// hold를 깨고 토큰 fallback으로 전환하는 하강 임계치. 관측된 native↔토큰 분모 차이(86↔98=12%p)를
/// 흡수하는 경계값이라, 다른 모듈/테스트가 "프로덕션 기본 tolerance"를 재사용할 수 있도록 pub 상수로 노출한다.
pub const DEFAULT_CONTEXT_DROP_TOLERANCE: f64 = 12.0;

/// `[context]` 섹션. ctx(컨텍스트 사용률%) hold 동작의 반응성/안정성 튜닝.
///
/// Claude Code가 `used_percentage`를 간헐적으로 누락할 때 직전 native를 얼마나 오래 유지할지
/// (`hold_ttl_seconds`)와, 토큰 fallback이 얼마나 낮아져야 hold를 깨고 실제 감소로 반영할지
/// (`drop_tolerance`)를 제어한다. 기본값은 반응성을 높이는 방향으로 조정되어 있다.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ContextConfig {
    /// native 누락 시 직전 native를 유지(hold)할 최대 시간(초). 기본 180.
    ///
    /// 낮출수록 실제값 추종이 빨라지지만, native·토큰이 둘 다 0인 구간에선 ctx가 잠깐 빌 수 있다.
    /// `0`이면 hold가 비활성화되어(TTL 0 = 항상 stale) native 누락 즉시 토큰 fallback으로 저하한다.
    pub hold_ttl_seconds: u64,
    /// hold를 깨고 토큰 fallback으로 전환하는 하강 임계치(%포인트). 기본 12.0.
    ///
    /// 토큰 fallback이 직전 native보다 이만큼 이상 낮으면 실제 컨텍스트 감소(예: `/compact`)로 보고
    /// hold를 깬다. ⚠️ 12 미만으로 낮추면 native↔토큰 분모 차이(관측 86↔98=12%p)로 인한 깜빡임이
    /// 재발할 수 있다.
    pub drop_tolerance: f64,
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
            strip_chain_ctx: true,
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
            show_session: true,
            show_battery: true,
            show_disk: true,
            show_network: true,
            show_rate_limit: true,
            // 기본 off: calm·codex 레이아웃 대칭 유지. 임계 인지는 사용자가 opt-in(M2).
            rate_limit_warn_threshold: None,
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

impl Default for CodexConfig {
    /// 사용자 승인 기본값(spec §13): enabled=true(opt-out), freshness=240분, scan_days=3.
    fn default() -> Self {
        Self {
            enabled: true,
            freshness_minutes: 240,
            scan_days: 3,
        }
    }
}

impl Default for ContextConfig {
    /// ctx hold 기본값: TTL=180초(반응성↑), drop_tolerance=12.0%p(분모 노이즈 흡수 경계).
    ///
    /// 기본값은 단일 출처([`DEFAULT_CONTEXT_HOLD_TTL_SECONDS`]/[`DEFAULT_CONTEXT_DROP_TOLERANCE`])를
    /// 참조해 다른 모듈·테스트와 일관성을 유지한다.
    fn default() -> Self {
        Self {
            hold_ttl_seconds: DEFAULT_CONTEXT_HOLD_TTL_SECONDS,
            drop_tolerance: DEFAULT_CONTEXT_DROP_TOLERANCE,
        }
    }
}

/// `theme` 필드의 serde 기본값("calm"). 키 부재 = calm = 현행 동일(하위호환).
fn default_theme() -> String {
    "calm".to_string()
}

impl Default for Config {
    /// 계획서 §H-8 TOML의 전 항목 기본값으로 [`Config`]를 구성한다.
    fn default() -> Self {
        Self {
            // theme 기본값은 calm. calm 프리셋 = 아래 테마 필드 기본값과 정확히 동일
            // (themes::preset_calm_matches_default_config 테스트로 결합).
            theme: default_theme(),
            cpu: CpuConfig::default(),
            pulse: PulseConfig::default(),
            chain: ChainConfig::default(),
            display: DisplayConfig::default(),
            color: ColorConfig::default(),
            refresh: RefreshConfig::default(),
            codex: CodexConfig::default(),
            context: ContextConfig::default(),
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

    load_config_from_path(&path)
}

/// 주어진 경로의 config를 크기 제한 안에서 로드한다.
///
/// 파일 부재/읽기 실패/UTF-8 오류/크기 초과는 모두 [`Config::default`]로 안전 저하한다. TOML 파싱
/// 실패만 기존처럼 [`parse_config_str`]에서 stderr 경고를 출력한다.
fn load_config_from_path(path: &std::path::Path) -> Config {
    // 파일 부재 → 조용히 기본값(경고 없음, AC7).
    let contents = match read_config_file_limited(path, MAX_CONFIG_BYTES) {
        Ok(contents) => contents,
        Err(_) => return Config::default(),
    };

    parse_config_str(&contents)
}

/// `path`를 `max_bytes` 이하로만 읽어 UTF-8 문자열로 반환한다.
///
/// `max_bytes + 1`까지만 읽어서 초과 여부를 확인하므로, 큰 설정 파일도 bounded work로 끝난다.
fn read_config_file_limited(path: &std::path::Path, max_bytes: usize) -> std::io::Result<String> {
    let file = std::fs::File::open(path)?;
    let mut bytes = Vec::new();
    let limit = (max_bytes as u64).saturating_add(1);
    file.take(limit).read_to_end(&mut bytes)?;
    if bytes.len() > max_bytes {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "config.toml exceeds hot-path size limit",
        ));
    }
    String::from_utf8(bytes)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))
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
            // 미설정 테마 키를 프리셋 구체값으로 채운다(우선순위: 사용자키 > 프리셋 > calm).
            apply_theme(&mut config, contents);
            warn_unknown_pulse_style(&config);
            expand_chain_command(&mut config);
            config
        }
        Err(error) => {
            // 타입 불일치(band_tints에 문자열 등)도 이 경로 → 전체 기본값(=calm, theme 무시) 폴백.
            eprintln!(
                "understatus: config.toml 파싱 실패({error}). 기본값으로 진행합니다(theme 설정 포함 전체 기본값 사용)."
            );
            Config::default()
        }
    }
}

/// 테마 해석: `config.theme` 프리셋을 조회한 뒤 원본 TOML에 **명시되지 않은** 테마 키만
/// 프리셋 값으로 채운다(우선순위: 사용자키 > 프리셋 > calm). 미지 테마면 경고 후 calm(패닉 금지).
///
/// # 인자
/// - `config`: in-place로 테마 키를 채울 설정(이미 serde로 calm 기본 채워진 상태).
/// - `raw_toml`: 원본 TOML 본문(키 명시 여부 판정용 재파싱 소스).
///
/// # 주의
/// `has_key`는 "키 존재"만 보고 "값 유효성"은 보지 않는다. 사용자가 타입은 맞지만
/// 길이가 부족하거나 hex 형식이 깨진 `band_tints`(예 `["#fff"]`)를 적으면 프리셋이
/// 채우지 않아 그 값이 그대로 다운스트림으로 흐르고, render/theme의 `.get()`/`parse_hex`
/// 폴백 색으로 표시된다(우선순위 규칙의 의도된 귀결, 패닉 없음).
fn apply_theme(config: &mut Config, raw_toml: &str) {
    let preset = match themes::preset(&config.theme) {
        Some(preset) => preset,
        None => {
            eprintln!(
                "understatus: 알 수 없는 테마 '{}'. calm으로 진행합니다.",
                config.theme
            );
            themes::preset("calm").expect("calm은 항상 존재")
        }
    };

    // 원본을 toml::Value로 재파싱해 각 테마 키의 명시 여부를 판정한다.
    // 재파싱 실패(여기 도달 가능성은 낮음 — 이미 Config로 파싱 성공)면 프리셋 미적용.
    let Ok(value) = toml::from_str::<toml::Value>(raw_toml) else {
        return;
    };

    use themes::THEME_KEYS as keys;
    if !has_key(&value, keys[0].0, keys[0].1) {
        config.cpu.load_glyphs = preset.load_glyphs;
    }
    if !has_key(&value, keys[1].0, keys[1].1) {
        config.pulse.pulse_style = preset.pulse_style;
    }
    if !has_key(&value, keys[2].0, keys[2].1) {
        config.color.band_tints = preset.band_tints;
    }
    if !has_key(&value, keys[3].0, keys[3].1) {
        config.color.pulse_palette = preset.pulse_palette;
    }
    if !has_key(&value, keys[4].0, keys[4].1) {
        config.color.label_color = preset.label_color;
    }
    if !has_key(&value, keys[5].0, keys[5].1) {
        config.color.separator = preset.separator;
    }
    if !has_key(&value, keys[6].0, keys[6].1) {
        config.color.separator_color = preset.separator_color;
    }
    if !has_key(&value, keys[7].0, keys[7].1) {
        config.color.hud_seam = preset.hud_seam;
    }
}

/// 해석된 `pulse_style`이 미지 값이면 stderr 경고 1줄(값은 보존, render가 calm으로 안전 저하).
///
/// theme 미지값 경고(`apply_theme`)와 대칭. 레거시 "bold"를 손편집으로 남긴 사용자에게도
/// 안내가 된다. 핫패스이지만 출력은 미지 값일 때만(정상 config는 무출력).
fn warn_unknown_pulse_style(config: &Config) {
    if !crate::theme::is_known_pulse_style(&config.pulse.pulse_style) {
        eprintln!(
            "understatus: 알 수 없는 pulse_style '{}'. calm으로 진행합니다(사용 가능: {}).",
            config.pulse.pulse_style,
            crate::theme::PULSE_STYLES.join(", ")
        );
    }
}

/// `[section].key`가 원본 TOML에 실제로 적혀 있는지 판정한다.
///
/// 부분 섹션/부재 섹션도 `None`으로 흡수해 `false`를 반환하므로 프리셋이 채운다.
/// "키 존재"만 판정하며 값의 타입/길이 유효성은 검사하지 않는다(`apply_theme` 주석 참조).
fn has_key(value: &toml::Value, section: &str, key: &str) -> bool {
    value
        .get(section)
        .and_then(|table| table.get(key))
        .is_some()
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
        assert!(config.display.show_session);
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
        // CALM: pulse_style 기본은 "calm"(기본 스타일).
        assert_eq!(config.pulse.pulse_style, "calm");
        assert_eq!(config.chain.order, "self_first");
        assert_eq!(config.color.mode, "auto");
        // CALM 글리프 사다리(단일 셀 폭): ○ ▁ ▄ ▆ ◆.
        assert_eq!(config.cpu.load_glyphs, vec!["○", "▁", "▄", "▆", "◆"]);
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

    /// 테스트별 임시 config 경로를 만든다(프로세스 pid + 라벨로 충돌 회피).
    fn temp_config_path(label: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("understatus-config-{label}-{}", std::process::id()))
    }

    /// config 파일 제한 헬퍼는 한도 이하 UTF-8 설정을 그대로 읽어야 한다.
    #[test]
    fn read_config_file_limited_accepts_within_limit() {
        let path = temp_config_path("within-limit");
        std::fs::write(&path, "[display]\nmax_width = 120\n").expect("임시 config 작성");

        let contents = read_config_file_limited(&path, MAX_CONFIG_BYTES).expect("읽기 성공");
        assert!(contents.contains("max_width = 120"));

        let _ = std::fs::remove_file(path);
    }

    /// 정확히 한도 크기인 config는 허용한다(off-by-one 회귀 방지).
    #[test]
    fn read_config_file_limited_allows_exact_limit() {
        let path = temp_config_path("exact-limit");
        std::fs::write(&path, "abcd").expect("임시 config 작성");

        assert_eq!(read_config_file_limited(&path, 4).unwrap(), "abcd");

        let _ = std::fs::remove_file(path);
    }

    /// 한도를 초과한 config는 파싱하지 않고 기본값으로 안전 저하한다.
    #[test]
    fn oversized_config_falls_back_to_default() {
        let path = temp_config_path("oversize");
        let mut contents = String::from("theme = \"vivid\"\n#");
        contents.push_str(&"x".repeat(MAX_CONFIG_BYTES + 1));
        std::fs::write(&path, contents).expect("임시 config 작성");

        assert!(read_config_file_limited(&path, 8).is_err());
        let config = load_config_from_path(&path);
        assert_eq!(
            config.theme, "calm",
            "MAX_CONFIG_BYTES 초과 파일은 사용자 테마를 파싱하지 않고 기본값으로 저하"
        );

        let _ = std::fs::remove_file(path);
    }

    /// 손상 UTF-8 config도 TOML 파서까지 넘기지 않고 기본값으로 저하한다.
    #[test]
    fn invalid_utf8_config_falls_back_to_default() {
        let path = temp_config_path("invalid-utf8");
        std::fs::write(&path, [0xff, 0xfe, 0xfd]).expect("임시 config 작성");

        assert!(read_config_file_limited(&path, MAX_CONFIG_BYTES).is_err());
        let config = load_config_from_path(&path);
        assert_eq!(config.theme, "calm");

        let _ = std::fs::remove_file(path);
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

    /// 블로킹 D 필수 게이트: `Config::default().theme == "calm"`.
    ///
    /// 기존 default 테스트(default_impl_matches_spec)가 theme 필드를 검사하지 않는
    /// 구멍을 메운다. `Config::default()`에 `theme: "calm"` 누락 시 즉시 실패한다.
    #[test]
    fn theme_default_is_calm_string() {
        assert_eq!(Config::default().theme, "calm");
    }

    /// theme="vivid" + override 없음 → vivid 프리셋의 틴트/글리프로 채워져야 한다.
    #[test]
    fn theme_vivid_fills_unset_keys() {
        let config = parse_config_str(r#"theme = "vivid""#);
        assert_eq!(config.theme, "vivid");
        // vivid 블록 글리프 + 신호등 색.
        assert_eq!(config.cpu.load_glyphs, vec!["░", "▒", "▓", "█", "█"]);
        assert_eq!(
            config.color.band_tints,
            vec!["#2f9150", "#3fb083", "#cda23e", "#f0a24e", "#e34a3a"]
        );
        assert_eq!(config.color.pulse_palette, vec!["#e34a3a", "#bf4135"]);
    }

    /// theme="vivid" + 사용자 band_tints 명시 → 사용자 값 우선, 나머지는 vivid.
    #[test]
    fn user_key_overrides_preset() {
        let toml = r##"
            theme = "vivid"
            [color]
            band_tints = ["#111111", "#222222", "#333333", "#444444", "#555555"]
        "##;
        let config = parse_config_str(toml);
        // band_tints는 사용자 값 우선.
        assert_eq!(
            config.color.band_tints,
            vec!["#111111", "#222222", "#333333", "#444444", "#555555"]
        );
        // pulse_palette는 명시 안 했으므로 vivid 프리셋.
        assert_eq!(config.color.pulse_palette, vec!["#e34a3a", "#bf4135"]);
        // load_glyphs도 명시 안 했으므로 vivid.
        assert_eq!(config.cpu.load_glyphs, vec!["░", "▒", "▓", "█", "█"]);
    }

    /// theme 키 부재 → calm(현행과 동일). 기존 calm 값으로 채워져야 한다.
    #[test]
    fn missing_theme_key_is_calm() {
        let config = parse_config_str("");
        assert_eq!(config.theme, "calm");
        assert_eq!(config.cpu.load_glyphs, vec!["○", "▁", "▄", "▆", "◆"]);
        assert_eq!(
            config.color.band_tints,
            vec!["#5a6878", "#6d8296", "#86a0b4", "#9fbfce", "#b87848"]
        );
    }

    /// 미지 테마 → calm 폴백(경고). calm 값으로 채워져야 한다.
    #[test]
    fn unknown_theme_falls_back_to_calm() {
        let config = parse_config_str(r#"theme = "neon-does-not-exist""#);
        // theme 문자열 자체는 사용자가 적은 값 유지(해석만 calm).
        assert_eq!(config.theme, "neon-does-not-exist");
        assert_eq!(config.cpu.load_glyphs, vec!["○", "▁", "▄", "▆", "◆"]);
        assert_eq!(
            config.color.band_tints,
            vec!["#5a6878", "#6d8296", "#86a0b4", "#9fbfce", "#b87848"]
        );
    }

    /// 미지 테마 + 사용자 band_tints 명시 → 사용자값 보존 + 나머지 calm(Architect 권고 3b).
    #[test]
    fn unknown_theme_preserves_user_keys() {
        let toml = r##"
            theme = "neon-does-not-exist"
            [color]
            band_tints = ["#abcdef", "#abcdef", "#abcdef", "#abcdef", "#abcdef"]
        "##;
        let config = parse_config_str(toml);
        // 사용자 band_tints 보존.
        assert_eq!(
            config.color.band_tints,
            vec!["#abcdef", "#abcdef", "#abcdef", "#abcdef", "#abcdef"]
        );
        // 나머지는 calm 폴백.
        assert_eq!(config.color.pulse_palette, vec!["#b87848", "#7a5030"]);
    }

    /// 미지 pulse_style은 값이 보존되며(경고는 부작용) 파싱은 성공해야 한다.
    #[test]
    fn unknown_pulse_style_is_preserved() {
        let config = parse_config_str("[pulse]\npulse_style = \"bogus\"\n");
        assert_eq!(config.pulse.pulse_style, "bogus");
    }

    /// 알려진 pulse_style은 그대로 유지된다.
    #[test]
    fn known_pulse_style_unchanged() {
        let config = parse_config_str("[pulse]\npulse_style = \"hue\"\n");
        assert_eq!(config.pulse.pulse_style, "hue");
    }

    /// theme="vivid" + band_tints="blue"(타입 불일치) → from_str 실패 → 전체 default(=calm), theme 무시.
    #[test]
    fn type_mismatch_falls_back_to_full_default() {
        let toml = r#"
            theme = "vivid"
            [color]
            band_tints = "blue"
        "#;
        let config = parse_config_str(toml);
        // 파싱 실패 → Config::default() = calm. theme 무시.
        assert_eq!(config.theme, "calm");
        assert_eq!(config.cpu.load_glyphs, vec!["○", "▁", "▄", "▆", "◆"]);
        assert_eq!(
            config.color.band_tints,
            vec!["#5a6878", "#6d8296", "#86a0b4", "#9fbfce", "#b87848"]
        );
    }

    /// theme="vivid" + band_tints=["#fff"](타입 OK, 길이 1) → has_key true → 프리셋 미충전,
    /// 사용자값 보존(Architect 권고 3b). "키 존재 ≠ 값 유효" 한계 고정.
    #[test]
    fn valid_type_but_short_array_preserved() {
        let toml = r##"
            theme = "vivid"
            [color]
            band_tints = ["#fff"]
        "##;
        let config = parse_config_str(toml);
        // 길이 1이어도 has_key true → 프리셋 미충전 → 사용자값 그대로.
        assert_eq!(config.color.band_tints, vec!["#fff"]);
        // 다른 미명시 키는 vivid.
        assert_eq!(config.color.pulse_palette, vec!["#e34a3a", "#bf4135"]);
    }

    /// theme="vivid" + band_tints=["nothex",...](타입 OK, 길이 5, hex 형식 깨짐) → has_key true →
    /// 프리셋 미충전, 사용자값 보존(Architect 권고 3a). "길이 부족"과 별개 폴백 경로.
    #[test]
    fn valid_type_broken_hex_preserved() {
        let toml = r##"
            theme = "vivid"
            [color]
            band_tints = ["nothex", "#a", "#b", "#c", "#d"]
        "##;
        let config = parse_config_str(toml);
        // 형식이 깨져도 has_key true → 프리셋 미충전 → 사용자값 그대로(다운스트림 parse_hex가 폴백색 처리).
        assert_eq!(
            config.color.band_tints,
            vec!["nothex", "#a", "#b", "#c", "#d"]
        );
    }

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
        // 화려한 테마의 기본 pulse_style(hue)이 해석되어 들어온다.
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

    /// `[context]` 기본값: hold_ttl_seconds=180(반응성↑), drop_tolerance=12.0(분모 노이즈 흡수).
    #[test]
    fn context_default_values() {
        let context = ContextConfig::default();
        assert_eq!(context.hold_ttl_seconds, 180);
        assert_eq!(context.drop_tolerance, 12.0);
    }

    /// `[context]` 부분 설정은 명시한 키만 반영한다(serde default 병합).
    #[test]
    fn context_partial_toml_parses() {
        let toml = "[context]\nhold_ttl_seconds = 300\ndrop_tolerance = 8.0\n";
        let config = parse_config_str(toml);
        assert_eq!(config.context.hold_ttl_seconds, 300);
        assert_eq!(config.context.drop_tolerance, 8.0);
    }

    /// `[context]` 섹션 누락 시 기본값으로 채워진다(serde default).
    #[test]
    fn context_missing_section_uses_defaults() {
        let config = parse_config_str("");
        assert_eq!(config.context.hold_ttl_seconds, 180);
        assert_eq!(config.context.drop_tolerance, 12.0);
    }

    /// AC12/AC13: show_rate_limit 기본 true, false 명시 반영.
    #[test]
    fn display_show_rate_limit_toggle() {
        assert!(parse_config_str("").display.show_rate_limit, "기본 true");
        let off = parse_config_str("[display]\nshow_rate_limit = false\n");
        assert!(!off.display.show_rate_limit);
    }

    /// AC23: rate_limit_warn_threshold 기본 None(warn off), 명시 시 Some.
    #[test]
    fn display_rate_limit_warn_threshold() {
        assert_eq!(parse_config_str("").display.rate_limit_warn_threshold, None);
        let on = parse_config_str("[display]\nrate_limit_warn_threshold = 80\n");
        assert_eq!(on.display.rate_limit_warn_threshold, Some(80));
    }
}
