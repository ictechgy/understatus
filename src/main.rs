//! understatus 진입점.
//!
//! 서브커맨드를 `std::env::args`로 디스패치한다(clap 미사용, 계획서 §E).
//! 무인자/`render`는 렌더 파이프라인을, `install`/`uninstall`은 설치/제거를,
//! `--help`/`--version`은 사용법/버전을 출력한다.
//!
//! 렌더 파이프라인(계획서 §D-1):
//!   stdin 읽기 → parse_claude_input → load_config → sample_system
//!   → read_prev_pulse_state → pulse_gate → write_pulse_state
//!   → render(self 세그먼트) → run_chain(chain_command) → compose(order) → 한 줄 출력

mod chain;
mod claude;
mod config;
mod install;
mod render;
mod system;
mod theme;
mod themes;

use std::io::{BufRead, IsTerminal, Read, Write};
use std::process::ExitCode;

/// 진입점: 서브커맨드를 디스패치한다.
///
/// # 반환
/// 정상 종료 시 `ExitCode::SUCCESS`. 설치/제거 실패 시 stderr에 에러를 출력하고
/// `ExitCode::FAILURE`를 반환한다. 렌더 경로는 어떤 입력에도 패닉 없이 한 줄을 출력한다(AC1).
fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let subcommand = args.first().map(String::as_str);

    match subcommand {
        None | Some("render") => {
            run_render_pipeline();
            ExitCode::SUCCESS
        }
        Some("install") => run_install(&args),
        Some("uninstall") => match install::uninstall() {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("understatus: 제거 실패: {error:#}");
                ExitCode::FAILURE
            }
        },
        Some("theme") => run_theme(&args),
        Some("themes") => {
            print_themes();
            ExitCode::SUCCESS
        }
        Some("--help") | Some("-h") => {
            print_help();
            ExitCode::SUCCESS
        }
        Some("--version") | Some("-V") => {
            print_version();
            ExitCode::SUCCESS
        }
        Some(other) => {
            eprintln!("understatus: 알 수 없는 서브커맨드 '{other}'. --help 참조.");
            ExitCode::FAILURE
        }
    }
}

/// 설치 가능한 테마 기본값(미지정 + 비TTY/`--yes` 폴백).
const DEFAULT_THEME: &str = "calm";
/// 설치 가능한 갱신 주기 기본값(초).
const DEFAULT_INTERVAL: u64 = 5;
/// TTY 프롬프트 재시도 최대 횟수(이후 폴백값으로 진행).
const MAX_PROMPT_RETRIES: u32 = 3;

/// install 서브커맨드의 파싱된 옵션.
struct InstallArgs {
    /// `--interval N`(정수 ≥ 1). 미지정 시 `None`(프롬프트/승계/기본).
    interval: Option<u64>,
    /// `--theme NAME`. 미지정 시 `None`(프롬프트/calm).
    theme: Option<String>,
    /// `--yes`/`-y`. true면 프롬프트 생략(TTY여도 플래그/기본/승계값 사용).
    assume_yes: bool,
}

/// install 서브커맨드를 실행한다(인자 파싱 → 승계 → 프롬프트/플래그 해석 → 설치).
///
/// 디스크 read(기존 config.toml)는 install 모듈에 격리하고, 프롬프트는 stdin/stderr를
/// 주입한 순수 [`resolve_install_params`]가 담당한다. main은 얇은 배선만 수행한다.
fn run_install(args: &[String]) -> ExitCode {
    let install_args = match parse_install_args(args) {
        Ok(parsed) => parsed,
        Err(message) => {
            eprintln!("understatus: {message}");
            return ExitCode::FAILURE;
        }
    };

    // 미지정 시 기존 config.toml의 interval을 승계(플래그 > 기존값 > 5).
    let existing = install::existing_interval(install::read_existing_config_str().as_deref());

    let is_tty = std::io::stdin().is_terminal();
    let stdin = std::io::stdin();
    let mut reader = stdin.lock();
    let stderr = std::io::stderr();
    let mut writer = stderr.lock();
    let (interval, theme) =
        resolve_install_params(&install_args, &mut reader, &mut writer, is_tty, existing);

    match install::install(interval, &theme) {
        Ok(()) => {
            eprintln!("understatus: 설치 완료(theme='{theme}', refreshInterval={interval}s).");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("understatus: 설치 실패: {error:#}");
            ExitCode::FAILURE
        }
    }
}

/// theme 서브커맨드를 실행한다(`theme <name>` → 교체, 이름 누락 → 현재 테마 + 사용법).
fn run_theme(args: &[String]) -> ExitCode {
    match args.get(1) {
        Some(name) => match install::set_theme(name) {
            Ok(()) => {
                eprintln!("understatus: theme를 '{name}'로 변경했습니다.");
                ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("understatus: 테마 변경 실패: {error:#}");
                ExitCode::FAILURE
            }
        },
        None => {
            // 이름 누락은 에러가 아니다(미해결 질문 1 결정): 현재 테마 + 사용법 안내.
            let current = config::load_config().theme;
            println!("understatus: 현재 테마는 '{current}'입니다.");
            println!("사용법: understatus theme <name>   (목록: understatus themes)");
            ExitCode::SUCCESS
        }
    }
}

/// install 인자를 파싱한다(순수 함수). `--interval N`/`--theme NAME`/`--yes`/`-y`.
///
/// # 반환
/// 파싱된 [`InstallArgs`]. 알 수 없는 플래그/값 누락/잘못된 interval은 `Err(메시지)`.
/// `args[0]`은 서브커맨드("install")이므로 건너뛴다.
fn parse_install_args(args: &[String]) -> Result<InstallArgs, String> {
    let mut interval: Option<u64> = None;
    let mut theme: Option<String> = None;
    let mut assume_yes = false;

    let mut index = 1; // args[0] == "install"
    while index < args.len() {
        match args[index].as_str() {
            "--interval" => {
                let raw = args
                    .get(index + 1)
                    .ok_or_else(|| "--interval 뒤에 값이 필요합니다(정수 ≥ 1).".to_string())?;
                interval = Some(parse_interval(raw)?);
                index += 2;
            }
            "--theme" => {
                let value = args
                    .get(index + 1)
                    .ok_or_else(|| "--theme 뒤에 테마 이름이 필요합니다.".to_string())?;
                theme = Some(value.clone());
                index += 2;
            }
            "--yes" | "-y" => {
                assume_yes = true;
                index += 1;
            }
            other => {
                return Err(format!("알 수 없는 install 옵션 '{other}'. --help 참조."));
            }
        }
    }

    Ok(InstallArgs {
        interval,
        theme,
        assume_yes,
    })
}

/// 정수 interval 문자열을 검증한다(순수 함수). 정수 ≥ 1만 허용.
fn parse_interval(raw: &str) -> Result<u64, String> {
    let value: u64 = raw
        .trim()
        .parse()
        .map_err(|_| format!("interval은 정수여야 합니다(받은 값: '{raw}')."))?;
    if value < 1 {
        return Err("interval은 1 이상이어야 합니다.".to_string());
    }
    Ok(value)
}

/// interval 프롬프트 한 줄을 해석한다(순수 함수).
///
/// # 반환
/// - 빈 입력 → `Ok(None)`(호출부가 승계/기본값 적용).
/// - 정수 ≥ 1 → `Ok(Some(값))`.
/// - 그 외 → `Err(메시지)`(재프롬프트).
fn parse_interval_prompt(line: &str) -> Result<Option<u64>, String> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    parse_interval(trimmed).map(Some)
}

/// theme 프롬프트 한 줄을 해석한다(순수 함수). 번호(1~N) 또는 이름 허용.
///
/// # 반환
/// - 빈 입력 → `Ok("calm")`.
/// - 유효 번호/이름 → `Ok(이름)`.
/// - 그 외 → `Err(메시지)`(재프롬프트).
fn parse_theme_prompt(line: &str) -> Result<String, String> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return Ok(DEFAULT_THEME.to_string());
    }
    let catalog = themes::catalog();
    // 번호 입력(1-based)이면 해당 테마 이름으로 변환.
    if let Ok(number) = trimmed.parse::<usize>() {
        if number >= 1 && number <= catalog.len() {
            return Ok(catalog[number - 1].0.to_string());
        }
        return Err(format!("번호는 1~{} 범위여야 합니다.", catalog.len()));
    }
    // 이름 입력이면 is_known 검증.
    if themes::is_known(trimmed) {
        return Ok(trimmed.to_string());
    }
    Err(format!(
        "알 수 없는 테마 '{trimmed}'. 'themes' 명령으로 목록을 확인하세요."
    ))
}

/// 대화형 흐름으로 (interval, theme)을 해석한다. reader/writer/is_tty/승계값 주입(테스트 가능).
///
/// # interval 폴백 체인(항목별 독립, BLOCKING-1)
/// 1. `args.interval`(플래그) 있으면 그 값.
/// 2. 없고 is_tty + `--yes` 아님 → 프롬프트(빈 입력은 `existing_interval`, 그것도 None이면 5).
/// 3. 없고 비TTY/`--yes` → `existing_interval`, 그것도 None이면 5.
///
/// # theme 폴백 체인(항목별 독립)
/// 플래그 > (TTY 프롬프트) > calm. theme은 승계하지 않는다(테마 명령으로 별도 관리).
///
/// # 종료 조건
/// 비TTY/`--yes`는 즉시 폴백. TTY는 항목별 최대 [`MAX_PROMPT_RETRIES`]회. EOF/읽기 실패는
/// 즉시 폴백(무한 루프 방지).
fn resolve_install_params<R: BufRead, W: Write>(
    args: &InstallArgs,
    reader: &mut R,
    writer: &mut W,
    is_tty: bool,
    existing_interval: Option<u64>,
) -> (u64, String) {
    let interval = resolve_interval(args, reader, writer, is_tty, existing_interval);
    let theme = resolve_theme(args, reader, writer, is_tty);
    (interval, theme)
}

/// interval 항목을 해석한다(플래그 > 프롬프트 > 승계 > 기본). [`resolve_install_params`] 참조.
fn resolve_interval<R: BufRead, W: Write>(
    args: &InstallArgs,
    reader: &mut R,
    writer: &mut W,
    is_tty: bool,
    existing_interval: Option<u64>,
) -> u64 {
    if let Some(value) = args.interval {
        return value;
    }
    let fallback = existing_interval.unwrap_or(DEFAULT_INTERVAL);
    if !is_tty || args.assume_yes {
        return fallback;
    }
    for _ in 0..MAX_PROMPT_RETRIES {
        let _ = write!(writer, "Refresh interval in seconds [{fallback}]: ");
        let _ = writer.flush();
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) | Err(_) => return fallback, // EOF/읽기 실패 → 폴백(무한 루프 방지).
            Ok(_) => match parse_interval_prompt(&line) {
                // 빈 입력은 승계/기본값 적용.
                Ok(None) => return fallback,
                Ok(Some(value)) => return value,
                Err(message) => {
                    let _ = writeln!(writer, "  {message}");
                }
            },
        }
    }
    fallback
}

/// theme 항목을 해석한다(플래그 > 프롬프트 > calm). [`resolve_install_params`] 참조.
fn resolve_theme<R: BufRead, W: Write>(
    args: &InstallArgs,
    reader: &mut R,
    writer: &mut W,
    is_tty: bool,
) -> String {
    if let Some(value) = &args.theme {
        return value.clone();
    }
    if !is_tty || args.assume_yes {
        return DEFAULT_THEME.to_string();
    }
    for _ in 0..MAX_PROMPT_RETRIES {
        write_theme_menu(writer);
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) | Err(_) => return DEFAULT_THEME.to_string(), // EOF/읽기 실패 → calm.
            Ok(_) => match parse_theme_prompt(&line) {
                Ok(name) => return name,
                Err(message) => {
                    let _ = writeln!(writer, "  {message}");
                }
            },
        }
    }
    DEFAULT_THEME.to_string()
}

/// theme 선택 메뉴를 writer에 출력한다(번호 + 이름 + 설명).
fn write_theme_menu<W: Write>(writer: &mut W) {
    let _ = writeln!(writer, "Theme:");
    for (index, (name, tagline)) in themes::catalog().iter().enumerate() {
        let _ = writeln!(writer, "  {}) {:<6} {}", index + 1, name, tagline);
    }
    let _ = write!(writer, "Select [1]: ");
    let _ = writer.flush();
}

/// 사용 가능한 테마 목록을 stdout에 출력한다(현재 적용 테마 표시).
fn print_themes() {
    let current = config::load_config().theme;
    println!("사용 가능한 테마 (현재: '{current}'):");
    for (name, tagline) in themes::catalog() {
        let marker = if *name == current { "*" } else { " " };
        println!("  {marker} {name:<6} {tagline}");
    }
}

/// 렌더 파이프라인을 실행하고 합성된 한 줄을 stdout에 출력한다.
///
/// 계획서 §D-1의 8단계를 순서대로 호출한다. 각 단계의 스텁(`todo!()`)은
/// 병렬 워커가 채운다. 본 함수는 실제 배선(stdin 읽기/단계 연결/출력)을 담당한다.
fn run_render_pipeline() {
    // (1) stdin 원본 보존(체이닝을 위해 raw 그대로 자식에 전달).
    let raw_stdin = read_stdin();

    // (2) Claude 세션 정보 파싱(누락/null/깨진 JSON 안전).
    let claude_input = claude::parse_claude_input(&raw_stdin);

    // 세션 캐시 격리 키를 한 곳에서 1회 살균한다(§11.3). session_id 부재/빈 값은 "default"로 폴백.
    let session_key = chain::sanitize_session_key(claude_input.session_id.as_deref().unwrap_or(""));

    // (5) 설정 로드(부재/깨짐 시 기본값).
    let cfg = config::load_config();

    // (3)(4) 시스템 스냅샷 수집(더블샘플 CPU + 메모리 + 배터리).
    let snapshot = system::sample_system(&cfg, &session_key);

    // 지각성 불변식(계획서 §H-5, AC4): 한 펄스 주기 안에 그려지는 프레임 수가 6 이상이어야
    // 색 출렁임이 끊기지 않는다. 릴리스 출력/성능에 영향을 주지 않도록 debug 빌드에서만 검증한다.
    debug_assert!(
        theme::samples_per_period(&cfg, cfg.refresh.interval_seconds) >= 6,
        "펄스 지각성 불변식 위반: samples_per_period < 6 (pulse_period={}s, refreshInterval={}s)",
        cfg.pulse.pulse_period_seconds,
        cfg.refresh.interval_seconds
    );

    // 히스테리시스: 직전 펄스 상태 읽기 → 게이트 → 다음 호출을 위해 기록.
    let prev_pulse_on = chain::read_prev_pulse_state(&session_key);
    let now_ms = now_millis();
    let pulse_on = theme::pulse_gate(snapshot.cpu_percent, prev_pulse_on, &cfg);
    chain::write_pulse_state(pulse_on, &session_key);

    // (6) understatus 자체 세그먼트 렌더.
    let self_segment = render::render(&claude_input, &snapshot, &cfg, now_ms, pulse_on);

    // (7) 체인 자식 실행(있으면). 타임아웃/캐시로 렌더 무블록.
    let chain_output = match cfg.chain.chain_command.as_deref() {
        Some(command) if !command.is_empty() => {
            chain::run_chain(command, &raw_stdin, &cfg, &session_key)
        }
        _ => String::new(),
    };

    // (8) self + chain 합성 후 한 줄 출력. 체인이 있으면 dim HUD seam("│")으로 소유권 경계 표시.
    let color_on = std::env::var_os("NO_COLOR").is_none() && cfg.color.mode != "none";
    let line = render::compose_with_seam(
        &self_segment,
        &chain_output,
        &cfg.chain.order,
        &cfg,
        color_on,
    );
    println!("{line}");
}

/// stdin을 끝까지 읽어 문자열로 반환한다.
///
/// # 반환
/// stdin 전체 내용. 읽기 실패 시 빈 문자열로 안전 저하한다(파이프라인은 빈 입력에도 무패닉, AC1).
fn read_stdin() -> String {
    let mut buffer = String::new();
    let _ = std::io::stdin().read_to_string(&mut buffer);
    buffer
}

/// 현재 시각을 UNIX epoch 기준 밀리초(ms)로 반환한다.
///
/// # 반환
/// epoch 이후 경과 밀리초. 시계 이상(epoch 이전) 시 0으로 안전 저하한다(펄스 위상 계산용).
fn now_millis() -> u128 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis())
        .unwrap_or(0)
}

/// 사용법(`--help`)을 stdout에 출력한다.
fn print_help() {
    println!(
        "understatus {} — AI 코딩 CLI용 macOS statusline 애드온\n\
         \n\
         사용법:\n\
         \x20 understatus [render]       stdin JSON을 읽어 statusline 한 줄을 출력(기본)\n\
         \x20 understatus install [옵션]  기존 statusLine을 보존(체이닝)하며 비파괴 설치\n\
         \x20 understatus uninstall      원본 설정을 정확 복원하며 제거\n\
         \x20 understatus theme <name>   설치 후 테마 교체(config.toml만 수정)\n\
         \x20 understatus themes         사용 가능한 테마 목록 출력\n\
         \x20 understatus --help         이 도움말 출력\n\
         \x20 understatus --version      버전 출력\n\
         \n\
         install 옵션:\n\
         \x20 --interval <N>   refreshInterval 초(정수 ≥ 1). 미지정 시 프롬프트/승계/기본 5.\n\
         \x20 --theme <name>   테마 이름. 미지정 시 프롬프트/기본 calm.\n\
         \x20 --yes, -y        프롬프트 생략(TTY여도). 플래그/승계/기본값 사용.",
        env!("CARGO_PKG_VERSION")
    );
}

/// 버전(`--version`)을 stdout에 출력한다.
fn print_version() {
    println!("understatus {}", env!("CARGO_PKG_VERSION"));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    /// 인자 슬라이스를 만든다(args[0] == "install").
    fn install_argv(rest: &[&str]) -> Vec<String> {
        let mut v = vec!["install".to_string()];
        v.extend(rest.iter().map(|s| s.to_string()));
        v
    }

    // --- parse_install_args ---

    #[test]
    fn parse_install_args_all_flags() {
        let args = install_argv(&["--interval", "10", "--theme", "vivid", "--yes"]);
        let parsed = parse_install_args(&args).expect("파싱 성공");
        assert_eq!(parsed.interval, Some(10));
        assert_eq!(parsed.theme.as_deref(), Some("vivid"));
        assert!(parsed.assume_yes);
    }

    #[test]
    fn parse_install_args_empty_is_all_none() {
        let parsed = parse_install_args(&install_argv(&[])).expect("파싱 성공");
        assert_eq!(parsed.interval, None);
        assert_eq!(parsed.theme, None);
        assert!(!parsed.assume_yes);
    }

    #[test]
    fn parse_install_args_short_yes() {
        let parsed = parse_install_args(&install_argv(&["-y"])).expect("파싱 성공");
        assert!(parsed.assume_yes);
    }

    #[test]
    fn parse_install_args_rejects_unknown_flag() {
        let result = parse_install_args(&install_argv(&["--bogus"]));
        assert!(result.is_err());
    }

    #[test]
    fn parse_install_args_rejects_missing_interval_value() {
        let result = parse_install_args(&install_argv(&["--interval"]));
        assert!(result.is_err());
    }

    // --- parse_interval ---

    #[test]
    fn parse_interval_accepts_positive() {
        assert_eq!(parse_interval("5"), Ok(5));
        assert_eq!(parse_interval(" 12 "), Ok(12));
    }

    #[test]
    fn parse_interval_rejects_zero_and_nonint() {
        assert!(parse_interval("0").is_err());
        assert!(parse_interval("-3").is_err());
        assert!(parse_interval("abc").is_err());
    }

    // --- parse_interval_prompt ---

    #[test]
    fn parse_interval_prompt_empty_is_none() {
        assert_eq!(parse_interval_prompt(""), Ok(None));
        assert_eq!(parse_interval_prompt("  \n"), Ok(None));
    }

    #[test]
    fn parse_interval_prompt_valid_is_some() {
        assert_eq!(parse_interval_prompt("8\n"), Ok(Some(8)));
    }

    #[test]
    fn parse_interval_prompt_invalid_is_err() {
        assert!(parse_interval_prompt("0").is_err());
        assert!(parse_interval_prompt("x").is_err());
    }

    // --- parse_theme_prompt ---

    #[test]
    fn parse_theme_prompt_empty_is_calm() {
        assert_eq!(parse_theme_prompt(""), Ok("calm".to_string()));
        assert_eq!(parse_theme_prompt("\n"), Ok("calm".to_string()));
    }

    #[test]
    fn parse_theme_prompt_number_maps_to_name() {
        // catalog 순서: 1=calm, 2=mono, 3=vivid, 4=ember, 5=emoji.
        assert_eq!(parse_theme_prompt("3"), Ok("vivid".to_string()));
        assert_eq!(parse_theme_prompt("5\n"), Ok("emoji".to_string()));
    }

    #[test]
    fn parse_theme_prompt_name_is_accepted() {
        assert_eq!(parse_theme_prompt("ember"), Ok("ember".to_string()));
    }

    #[test]
    fn parse_theme_prompt_invalid_is_err() {
        assert!(parse_theme_prompt("99").is_err());
        assert!(parse_theme_prompt("neon").is_err());
    }

    // --- resolve_install_params ---

    /// 비TTY/--yes는 즉시 폴백: 플래그값 사용, theme 미지정은 calm.
    #[test]
    fn resolve_install_params_flags_only_non_tty() {
        let args = InstallArgs {
            interval: Some(9),
            theme: Some("mono".to_string()),
            assume_yes: false,
        };
        let mut reader = Cursor::new(Vec::new());
        let mut writer = Vec::new();
        let (interval, theme) =
            resolve_install_params(&args, &mut reader, &mut writer, false, None);
        assert_eq!(interval, 9);
        assert_eq!(theme, "mono");
    }

    /// EOF + existing=None + TTY → 무한 루프 없이 기본값(5, calm).
    #[test]
    fn resolve_install_params_eof_falls_back() {
        let args = InstallArgs {
            interval: None,
            theme: None,
            assume_yes: false,
        };
        let mut reader = Cursor::new(Vec::new()); // 즉시 EOF.
        let mut writer = Vec::new();
        let (interval, theme) = resolve_install_params(&args, &mut reader, &mut writer, true, None);
        assert_eq!(interval, 5);
        assert_eq!(theme, "calm");
    }

    /// 혼합 모드: --interval만 플래그, --theme는 TTY 프롬프트(항목별 독립 분기, 권고 4).
    #[test]
    fn resolve_install_params_mixed_flag_and_prompt() {
        let args = InstallArgs {
            interval: Some(7),
            theme: None,
            assume_yes: false,
        };
        let mut reader = Cursor::new(b"vivid\n".to_vec());
        let mut writer = Vec::new();
        let (interval, theme) = resolve_install_params(&args, &mut reader, &mut writer, true, None);
        assert_eq!(interval, 7, "interval은 플래그값(프롬프트 안 함)");
        assert_eq!(theme, "vivid", "theme은 프롬프트값");
    }

    /// 비TTY 업그레이드 승계: interval 미지정 + existing=Some(10) → 10(5로 리셋 안 됨).
    #[test]
    fn resolve_install_params_inherits_interval_when_unset() {
        let args = InstallArgs {
            interval: None,
            theme: None,
            assume_yes: true,
        };
        let mut reader = Cursor::new(Vec::new());
        let mut writer = Vec::new();
        let (interval, _) =
            resolve_install_params(&args, &mut reader, &mut writer, false, Some(10));
        assert_eq!(interval, 10);
    }

    /// 플래그가 승계값보다 우선: interval=Some(3) + existing=Some(10) → 3.
    #[test]
    fn resolve_install_params_flag_overrides_existing() {
        let args = InstallArgs {
            interval: Some(3),
            theme: None,
            assume_yes: true,
        };
        let mut reader = Cursor::new(Vec::new());
        let mut writer = Vec::new();
        let (interval, _) =
            resolve_install_params(&args, &mut reader, &mut writer, false, Some(10));
        assert_eq!(interval, 3);
    }

    /// 플래그/승계 모두 없으면 최종 폴백 5.
    #[test]
    fn resolve_install_params_default_when_no_flag_no_existing() {
        let args = InstallArgs {
            interval: None,
            theme: None,
            assume_yes: true,
        };
        let mut reader = Cursor::new(Vec::new());
        let mut writer = Vec::new();
        let (interval, _) = resolve_install_params(&args, &mut reader, &mut writer, false, None);
        assert_eq!(interval, 5);
    }

    /// TTY 빈 입력은 승계값 적용: 빈 줄 + existing=Some(10) → 10.
    #[test]
    fn resolve_install_params_tty_empty_input_inherits() {
        let args = InstallArgs {
            interval: None,
            theme: Some("calm".to_string()), // theme 프롬프트 회피(interval 분기만 검증).
            assume_yes: false,
        };
        let mut reader = Cursor::new(b"\n".to_vec()); // interval 프롬프트에 빈 줄.
        let mut writer = Vec::new();
        let (interval, _) = resolve_install_params(&args, &mut reader, &mut writer, true, Some(10));
        assert_eq!(interval, 10);
    }
}
