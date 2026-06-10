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
mod codex;
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
        None | Some("render") => match parse_render_args(&args) {
            Ok(render_args) => {
                run_render_pipeline(
                    render_args.source,
                    render_args.oneline,
                    render_args.surface_format,
                );
                ExitCode::SUCCESS
            }
            // 미지 플래그/미지 source 값은 기존 `Some(other)` 관례와 동일하게 FAILURE.
            Err(message) => {
                eprintln!("understatus: {message}");
                ExitCode::FAILURE
            }
        },
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
        Some("pulse") => run_pulse(&args),
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

/// 서브커맨드에 허용된 위치 인자(값 1개)를 초과했는지 판정한다.
/// `args[0]`=서브커맨드, `args[1]`=값. len>2면 잉여 인자.
///
/// 주의: render 경로는 플래그(`--source`/`--oneline`)를 받으므로 이 판정을 쓰지 않고
/// [`parse_render_args`]가 따로 검증한다(render 뒤 플래그를 잉여로 오판하지 않도록 분리).
fn has_extra_args(args: &[String]) -> bool {
    args.len() > 2
}

/// 렌더 입력 소스(spec §6.1). `--source <claude|lterm>`로 선택하며 기본은 claude.
///
/// - `Claude`: 기존 동작(Claude Code stdin JSON 파싱 + chain 가능).
/// - `Lterm`: lterm 합성 JSON 파싱(git 비활성, chain 기본 off).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Source {
    /// Claude Code stdin JSON(기본값).
    Claude,
    /// lterm 합성 JSON(`--source lterm`).
    Lterm,
}

/// 렌더 출력 표면(surface) 형식. `--surface-format <oneline|cmux-status>`로 선택하며 기본 Oneline.
///
/// `--surface-format`은 **출력 표면(텍스트 vs cmux JSON)** 선택이고, `--oneline`은 **그 텍스트
/// 표면을 1행 terse 모드(chain 미수행 + 후행 개행 없음)로 만들지** 여부다. 둘은 직교한다:
/// `--surface-format oneline`은 terse를 의미하지 않으며, terse는 오직 `--oneline`가 결정한다.
///
/// - `Oneline`: 비-cmux 텍스트 표면(SGR 한 줄). terse 여부는 별도 `--oneline`가 정한다. `--oneline`
///   없는 일반 render는 chain/compose + 후행 개행을 그대로 거치므로 기존 출력 바이트가 불변이다.
/// - `CmuxStatus`: cmux 네이티브 status pill JSON 1줄(설계 §3.3). lterm `CmuxStatusSink`가 소비.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SurfaceFormat {
    /// 비-cmux 텍스트 표면(기본값, SGR 한 줄). terse 여부는 별도 `--oneline` 플래그가 결정한다.
    /// 즉 `--surface-format oneline`만으로는 terse가 아니며, 기존 일반 render 동작을 그대로 보존한다.
    Oneline,
    /// cmux pill JSON 1줄(`--surface-format cmux-status`).
    CmuxStatus,
}

/// render 경로의 파싱된 플래그.
struct RenderArgs {
    /// `--source <claude|lterm>`. 미지정 시 [`Source::Claude`].
    source: Source,
    /// `--oneline`. true면 chain 미수행 + 후행 개행 없이 1행 출력(spec §6.3).
    oneline: bool,
    /// `--surface-format <oneline|cmux-status>`. 미지정 시 [`SurfaceFormat::Oneline`].
    /// `--surface-format`이 명시되면 `--oneline`보다 우선한다(설계 §5.1).
    surface_format: SurfaceFormat,
}

/// render 경로용 플래그를 파싱한다(순수 함수, clap 미사용 — 기존 수동 디스패치 스타일).
///
/// # 인자
/// - `args`: 전체 인자 슬라이스. `args[0]`이 `"render"`이면 건너뛰고, 무인자(`render` 생략)도 허용한다.
///
/// # 반환
/// 파싱된 [`RenderArgs`]. `--source`/`--oneline`/`--surface-format`은 순서 무관이며 모두 선택적이다.
/// 미지 플래그/미지 source 값/미지 surface-format 값/값 누락은 `Err(메시지)`(호출부가 `ExitCode::FAILURE`).
///
/// # 주의
/// `--source`/`--surface-format` 중복 지정 시 마지막 값이 이긴다(수동 파서의 일반 관례).
/// `--oneline` 중복은 무해(idempotent). `--surface-format`이 명시되면 그 값이 최종 surface_format이
/// 되어 `--oneline`의 Oneline 매핑보다 우선한다(설계 §5.1: 둘 다 있으면 --surface-format 우선).
fn parse_render_args(args: &[String]) -> Result<RenderArgs, String> {
    let mut source = Source::Claude;
    let mut oneline = false;
    // 명시된 --surface-format 값(없으면 None → --oneline 매핑/기본 Oneline으로 결정).
    let mut surface_format_flag: Option<SurfaceFormat> = None;

    // args[0]이 "render"면 건너뛴다(무인자 호출은 args가 비어 시작 인덱스 0).
    let mut index = if args.first().map(String::as_str) == Some("render") {
        1
    } else {
        0
    };
    while index < args.len() {
        match args[index].as_str() {
            "--source" => {
                let value = args
                    .get(index + 1)
                    .ok_or_else(|| "--source 뒤에 값이 필요합니다(claude|lterm).".to_string())?;
                source = parse_source(value)?;
                index += 2;
            }
            "--oneline" => {
                oneline = true;
                index += 1;
            }
            "--surface-format" => {
                let value = args.get(index + 1).ok_or_else(|| {
                    "--surface-format 뒤에 값이 필요합니다(oneline|cmux-status).".to_string()
                })?;
                surface_format_flag = Some(parse_surface_format(value)?);
                index += 2;
            }
            other => {
                return Err(format!("알 수 없는 render 옵션 '{other}'. --help 참조."));
            }
        }
    }

    // surface_format 결정: 명시 플래그가 최우선, 없으면 --oneline도 Oneline 표면이라 기본 Oneline.
    // (--oneline은 oneline 필드로 chain-skip/후행개행 제어를 따로 하고, 표면은 항상 Oneline에 매핑.)
    let surface_format = surface_format_flag.unwrap_or(SurfaceFormat::Oneline);

    Ok(RenderArgs {
        source,
        oneline,
        surface_format,
    })
}

/// `--surface-format` 값 문자열을 [`SurfaceFormat`]으로 해석한다(미지값은 에러).
fn parse_surface_format(value: &str) -> Result<SurfaceFormat, String> {
    match value {
        "oneline" => Ok(SurfaceFormat::Oneline),
        "cmux-status" => Ok(SurfaceFormat::CmuxStatus),
        other => Err(format!(
            "알 수 없는 surface-format '{other}'. 사용 가능: oneline|cmux-status."
        )),
    }
}

/// `--source` 값 문자열을 [`Source`]로 해석한다(미지값은 에러).
fn parse_source(value: &str) -> Result<Source, String> {
    match value {
        "claude" => Ok(Source::Claude),
        "lterm" => Ok(Source::Lterm),
        other => Err(format!(
            "알 수 없는 source '{other}'. 사용 가능: claude|lterm."
        )),
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
    if has_extra_args(args) {
        eprintln!(
            "understatus: theme 명령은 테마 이름 하나만 받습니다. 사용법: understatus theme <name>"
        );
        return ExitCode::FAILURE;
    }
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

/// pulse 서브커맨드를 실행한다(`pulse <style>` → 교체, 스타일 누락 → 현재 스타일 + 사용법).
fn run_pulse(args: &[String]) -> ExitCode {
    if has_extra_args(args) {
        eprintln!("understatus: pulse 명령은 스타일 인자 하나만 받습니다. 사용법: understatus pulse <calm|flash|hue|swap>");
        return ExitCode::FAILURE;
    }
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

/// ctx native hold 세션 캐시 파일명. used_percentage가 누락된 프레임에서 직전 양수 native를
/// 유지해 토큰 fallback으로의 값 튐을 막는다(pulse_state/net_counters와 동일한 단기 TTL 캐시 예외).
const CONTEXT_NATIVE_CACHE: &str = "ctx_native";

/// Claude 소스의 ctx 사용률%를 해석해 `claude_input.context_used_percentage`에 확정한다.
///
/// [`claude::resolve_context_percent`]로 native·토큰 fallback·직전 native(hold)를 종합한다.
/// 양수 native를 본 프레임에서는 그 값을 세션 캐시에 기록해 이후 누락 프레임이 유지(hold)에
/// 쓰도록 한다. 모든 캐시 I/O는 best-effort이며 실패해도 패닉하지 않는다.
///
/// hold 튜닝(TTL·하강 임계치)은 `[context]` 설정([`config::ContextConfig`])에서 주입한다.
/// Claude Code는 긴 세션에서 `used_percentage`와 토큰을 **모두 0/null로 보내는 프레임**을 지속할 수
/// 있어(라이브 관측: 양수 native 직후 ~98초간 0 지속), 그 구간엔 토큰 fallback도 불가하므로
/// 직전 native를 `hold_ttl_seconds`만큼 유지해 ctx가 사라지지 않게 한다. hold 프레임은 TTL을
/// 재시작하지 않으므로(양수 native를 본 프레임만 영속화) 시계는 **마지막 실제 native 시점부터** 흐른다.
///
/// # 인자
/// - `claude_input`: 해석 결과를 반영할 입력(in-place 갱신).
/// - `session_key`: 세션 캐시 격리 키(이미 살균됨).
/// - `now_ms`: 현재 시각(epoch ms). TTL 판정·캐시 타임스탬프에 사용.
/// - `ctx_cfg`: ctx hold 튜닝(`hold_ttl_seconds`/`drop_tolerance`).
fn resolve_claude_context(
    claude_input: &mut claude::ClaudeInput,
    session_key: &str,
    now_ms: u128,
    ctx_cfg: &config::ContextConfig,
) {
    let held_native = read_held_native_ctx(session_key, now_ms, ctx_cfg.hold_ttl_seconds);
    let resolution = claude::resolve_context_percent(
        claude_input.context_used_percentage,
        claude_input.context_fallback_percentage,
        held_native,
        ctx_cfg.drop_tolerance,
    );
    claude_input.context_used_percentage = resolution.display;
    // 양수 native를 본 프레임만 영속화한다(유지 프레임은 재기록하지 않아 TTL이 마지막 실제
    // native 시점부터 흐른다). f64 전체 정밀도로 저장하고 표시 시 반올림한다.
    if let Some(native) = resolution.persist_native {
        chain::write_session_named_cache(
            session_key,
            CONTEXT_NATIVE_CACHE,
            now_ms,
            &format!("{native}"),
        );
    }
}

/// 세션 캐시에서 TTL(`ttl_seconds`) 내 직전 양수 native ctx%를 읽는다.
///
/// I/O(세션 캐시 읽기)는 여기서 하고, TTL·파싱·유한성 판정은 순수 [`interpret_held_native_ctx`]에
/// 위임해 HOME 의존 없이 단위 테스트할 수 있게 한다.
///
/// # 인자
/// - `session_key`: 세션 캐시 격리 키.
/// - `now_ms`: 현재 시각(epoch ms). TTL 판정 기준.
/// - `ttl_seconds`: hold 유지 시간(초, config `[context].hold_ttl_seconds`).
///
/// # 반환
/// 신선한 직전 native가 있으면 `Some(percent)`. 항목 부재/stale/파싱 실패/비유한이면 `None`.
fn read_held_native_ctx(session_key: &str, now_ms: u128, ttl_seconds: u64) -> Option<f64> {
    let entry = chain::read_session_named_cache(session_key, CONTEXT_NATIVE_CACHE);
    interpret_held_native_ctx(entry, now_ms, ttl_seconds)
}

/// 세션 캐시 읽기 결과 `(written_ms, payload)`를 직전 native ctx%로 해석한다(순수, I/O 없음).
///
/// `read_held_native_ctx`의 판정 로직을 분리해 tempdir/HOME 스왑 없이 TTL 경계·`f64` 라운드트립
/// (`format!("{native}")` ↔ `parse::<f64>()`)·범위 방어를 테스트 가능하게 한다.
///
/// 캐시 payload는 우리가 기록한 양수·클램프 native지만(`resolve_context_percent` persist), 파일은
/// 사용자/외부가 변조할 수 있는 신뢰 경계다. 따라서 `held_native`의 계약(양수 0..=100)을 읽기 경계에서
/// 강제한다: 유한·`0 < v <= 100`을 벗어난 payload(예: `-5`, `0`, `150`, `1e24`)는 `None`으로 거부해
/// hold 대신 토큰 fallback으로 저하시킨다(손상 캐시가 그대로 표시되지 않도록).
///
/// # 인자
/// - `entry`: 캐시 읽기 결과. `None`이면 항목 부재.
/// - `now_ms`: 현재 시각(epoch ms). TTL 판정 기준.
/// - `ttl_seconds`: hold 유지 시간(초, config `[context].hold_ttl_seconds`).
///
/// # 반환
/// TTL(`ttl_seconds`) 내이고 `0 < v <= 100`인 유한 `f64`로 파싱되면 `Some(percent)`,
/// 아니면 `None`.
fn interpret_held_native_ctx(
    entry: Option<(u128, String)>,
    now_ms: u128,
    ttl_seconds: u64,
) -> Option<f64> {
    let (written_ms, payload) = entry?;
    if !chain::is_named_cache_fresh(written_ms, now_ms, ttl_seconds) {
        return None;
    }
    payload
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|percent| percent.is_finite() && *percent > 0.0 && *percent <= 100.0)
}

/// 렌더 파이프라인을 실행하고 합성된 한 줄을 stdout에 출력한다.
///
/// 계획서 §D-1의 8단계를 순서대로 호출한다. 본 함수는 실제 배선(stdin 읽기/단계 연결/출력)을 담당한다.
///
/// # 인자
/// - `source`: 입력 소스(claude=기존 동작, lterm=합성 JSON). lterm은 git 비활성·chain 기본 off.
/// - `oneline`: true면 chain을 수행하지 않고 코어 `render()` 1행만 **후행 개행 없이** 출력한다(spec §6.3).
/// - `surface_format`: 출력 표면(텍스트 vs cmux JSON). [`SurfaceFormat::CmuxStatus`]면 SGR 한 줄 대신
///   cmux pill JSON 1줄을 출력한다(설계 §3.3). [`SurfaceFormat::Oneline`]은 비-cmux 텍스트 표면일
///   뿐이며 terse(1행/chain-skip) 여부는 `oneline` 인자와 직교다. 즉 `--surface-format oneline`(=
///   `Oneline`)이라도 `oneline=false`면 chain/compose + 후행 개행을 거치는 기존 경로를 그대로 탄다.
///   수집부(parse + codex enrich + system sample)는 표면 분기와 무관하게 재사용된다.
fn run_render_pipeline(source: Source, oneline: bool, surface_format: SurfaceFormat) {
    // (1) stdin 원본 보존(체이닝을 위해 raw 그대로 자식에 전달).
    let raw_stdin = read_stdin();

    // (2) 소스별 세션 정보 파싱(누락/null/깨진 JSON 안전). lterm은 git 비활성.
    let mut claude_input = match source {
        Source::Claude => claude::parse_claude_input(&raw_stdin),
        Source::Lterm => claude::parse_lterm_input(&raw_stdin),
    };

    // 세션 캐시 격리 키를 한 곳에서 1회 살균한다(§11.3). session_id 부재/빈 값은 "default"로 폴백.
    let session_key = chain::sanitize_session_key(claude_input.session_id.as_deref().unwrap_or(""));

    // (5) 설정 로드(부재/깨짐 시 기본값).
    let cfg = config::load_config();

    // (5') Codex 세션 심층판독 enrich(spec §7). **Source::Lterm 한정**: Claude 경로에서 모델
    //   별칭이 우연히 codex 계열이어도 ~/.codex를 읽지 않도록 여기서 게이팅한다(비트 동일 보존).
    //   enrich는 session_id를 바꾸지 않으므로 위 session_key 도출/이후 파이프라인에 영향 없다.
    if source == Source::Lterm {
        codex::maybe_enrich(&mut claude_input, &cfg);
    }

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

    // (5'') ctx 해석(Claude 소스 한정): native(used_percentage) 우선, 일시 누락 시 TTL 내 직전
    //   native 유지, cold-start만 토큰 fallback. Claude Code가 used_percentage를 간헐적으로 누락해도
    //   ctx 세그먼트가 사라지지 않게 하고, native↔토큰 분모 차이로 인한 값 튐(예: 86↔98)을 막는다.
    //   lterm/codex 경로는 context_window가 없어 자연 no-op이므로 Claude로 게이팅한다.
    if source == Source::Claude {
        resolve_claude_context(&mut claude_input, &session_key, now_ms, &cfg.context);
    }

    // (6') --surface-format cmux-status: SGR 한 줄 대신 cmux pill JSON 1줄을 출력하고 종료한다.
    //   수집부(parse + codex enrich + system sample + ctx 해석)는 위에서 그대로 끝났으므로 재사용한다.
    //   oneline 분기 직전에 분기해 chain/compose 경로를 타지 않는다(설계 §5.2). 직렬화 실패는 무패닉
    //   no-op(빈 출력) — lterm sink가 non-JSON을 무해 처리(additive-optional 계약).
    if surface_format == SurfaceFormat::CmuxStatus {
        let pills = render::render_cmux_pills(&claude_input, &snapshot, &cfg, now_ms, pulse_on);
        if let Ok(json) = serde_json::to_string(&pills) {
            print!("{json}");
            let _ = std::io::stdout().flush();
        }
        return;
    }

    // (6) understatus 자체 세그먼트 렌더.
    let self_segment = render::render(&claude_input, &snapshot, &cfg, now_ms, pulse_on);

    // (8') --oneline: chain 미수행, 코어 render() 1행만 후행 개행 없이 출력(spec §6.3).
    //   status row(1행)용 경로로, 최종 폭 권위는 lterm이므로 cols 힌트로 강제 절단하지 않는다.
    if oneline {
        print!("{self_segment}");
        let _ = std::io::stdout().flush();
        return;
    }

    // (7) 체인 자식 실행(있으면). 타임아웃/캐시로 렌더 무블록.
    //   단, `--source lterm`은 chain 기본 off(spec §6.3): lterm JSON이 Claude용 chain으로
    //   전달되지 않도록 oneline 여부와 무관하게 chain을 미수행한다(정상 줄바꿈 출력은 유지).
    let chain_output = match (source, cfg.chain.chain_command.as_deref()) {
        (Source::Claude, Some(command)) if !command.is_empty() => {
            chain::run_chain(command, &raw_stdin, &cfg, &session_key)
        }
        _ => String::new(),
    };

    // (7b) 체인 출력의 ctx 표시 제거(strip_chain_ctx, 기본 on). understatus가 ctx를 권위있게
    //   표시하므로 체인 HUD가 같은/발명된 ctx를 중복 표시해 값이 튀는 것을 막는다(빈 출력엔 무영향).
    let chain_output = if cfg.chain.strip_chain_ctx {
        chain::strip_chained_context(&chain_output)
    } else {
        chain_output
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
         \x20 understatus                 stdin JSON을 읽어 statusline 한 줄을 출력(기본 render)\n\
         \x20 understatus render [옵션]   render 옵션과 함께 statusline 한 줄을 출력\n\
         \x20 understatus install [옵션]  기존 statusLine을 보존(체이닝)하며 비파괴 설치\n\
         \x20 understatus uninstall      원본 설정을 정확 복원하며 제거\n\
         \x20 understatus theme <name>   설치 후 테마 교체(config.toml만 수정)\n\
         \x20 understatus themes         사용 가능한 테마 목록 출력\n\
         \x20 understatus pulse <style>  펄스 스타일 교체(calm|flash|hue|swap, config.toml만 수정)\n\
         \x20 understatus --help         이 도움말 출력\n\
         \x20 understatus --version      버전 출력\n\
         \n\
         render 옵션(understatus render 뒤에 사용):\n\
         \x20 --source <s>          입력 소스(claude|lterm). 미지정 시 claude.\n\
         \x20 --oneline             chain 없이 코어 한 줄만 후행 개행 없이 출력(terse, status row용).\n\
         \x20 --surface-format <f>  출력 표면(oneline|cmux-status). 미지정 시 oneline.\n\
         \x20                       (--surface-format은 표면 선택, --oneline은 terse 여부 — 직교)\n\
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

    #[test]
    fn has_extra_args_detects_surplus() {
        assert!(!has_extra_args(&["pulse".to_string()]));
        assert!(!has_extra_args(&["pulse".to_string(), "hue".to_string()]));
        assert!(has_extra_args(&[
            "pulse".to_string(),
            "hue".to_string(),
            "typo".to_string()
        ]));
    }

    // --- parse_render_args (spec §6.1, §10) ---

    /// 인자 슬라이스를 만든다(args[0] == "render").
    fn render_argv(rest: &[&str]) -> Vec<String> {
        let mut v = vec!["render".to_string()];
        v.extend(rest.iter().map(|s| s.to_string()));
        v
    }

    /// 무인자(render 생략)는 기본값(claude, oneline off)으로 성공해야 한다.
    #[test]
    fn parse_render_args_empty_is_default() {
        let parsed = parse_render_args(&[]).expect("파싱 성공");
        assert_eq!(parsed.source, Source::Claude);
        assert!(!parsed.oneline);
    }

    /// `render` 단독(플래그 없이)도 기본값으로 성공해야 한다(기존 동작 보존).
    #[test]
    fn parse_render_args_bare_render_is_default() {
        let parsed = parse_render_args(&render_argv(&[])).expect("파싱 성공");
        assert_eq!(parsed.source, Source::Claude);
        assert!(!parsed.oneline);
    }

    /// `render --source lterm --oneline` → lterm + oneline 성공 진입.
    #[test]
    fn parse_render_args_lterm_oneline() {
        let parsed = parse_render_args(&render_argv(&["--source", "lterm", "--oneline"]))
            .expect("파싱 성공");
        assert_eq!(parsed.source, Source::Lterm);
        assert!(parsed.oneline);
    }

    /// 플래그 순서는 무관해야 한다(--oneline --source lterm).
    #[test]
    fn parse_render_args_order_independent() {
        let parsed = parse_render_args(&render_argv(&["--oneline", "--source", "lterm"]))
            .expect("파싱 성공");
        assert_eq!(parsed.source, Source::Lterm);
        assert!(parsed.oneline);
    }

    /// `--source claude`는 기본과 동일하게 해석되어야 한다.
    #[test]
    fn parse_render_args_explicit_claude() {
        let parsed = parse_render_args(&render_argv(&["--source", "claude"])).expect("파싱 성공");
        assert_eq!(parsed.source, Source::Claude);
        assert!(!parsed.oneline);
    }

    /// 미지 source 값은 에러(ExitCode::FAILURE로 이어짐).
    #[test]
    fn parse_render_args_rejects_unknown_source() {
        assert!(parse_render_args(&render_argv(&["--source", "bogus"])).is_err());
    }

    /// `--source` 값 누락은 에러여야 한다.
    #[test]
    fn parse_render_args_rejects_missing_source_value() {
        assert!(parse_render_args(&render_argv(&["--source"])).is_err());
    }

    /// 미지 플래그는 에러여야 한다(기존 `Some(other)` 관례와 동일).
    #[test]
    fn parse_render_args_rejects_unknown_flag() {
        assert!(parse_render_args(&render_argv(&["--bogus"])).is_err());
    }

    /// `--source`가 중복되면 마지막 값이 이긴다(last-wins 계약 고정).
    #[test]
    fn parse_render_args_duplicate_source_last_wins() {
        let parsed = parse_render_args(&render_argv(&["--source", "claude", "--source", "lterm"]))
            .expect("파싱 성공");
        assert_eq!(parsed.source, Source::Lterm);
    }

    // --- parse_render_args: --surface-format (설계 §5.1) ---

    /// 미지정 시 surface_format 기본은 Oneline(behavior-preserving).
    #[test]
    fn parse_render_args_default_surface_format_is_oneline() {
        let parsed = parse_render_args(&render_argv(&[])).expect("파싱 성공");
        assert_eq!(parsed.surface_format, SurfaceFormat::Oneline);
    }

    /// `--surface-format cmux-status` → CmuxStatus.
    #[test]
    fn parse_render_args_surface_format_cmux_status() {
        let parsed = parse_render_args(&render_argv(&["--surface-format", "cmux-status"]))
            .expect("파싱 성공");
        assert_eq!(parsed.surface_format, SurfaceFormat::CmuxStatus);
    }

    /// `--surface-format oneline` → Oneline(명시).
    #[test]
    fn parse_render_args_surface_format_oneline() {
        let parsed =
            parse_render_args(&render_argv(&["--surface-format", "oneline"])).expect("파싱 성공");
        assert_eq!(parsed.surface_format, SurfaceFormat::Oneline);
    }

    /// 미지 surface-format 값은 에러(ExitCode::FAILURE로 이어짐).
    #[test]
    fn parse_render_args_rejects_unknown_surface_format() {
        assert!(parse_render_args(&render_argv(&["--surface-format", "bogus"])).is_err());
    }

    /// `--surface-format` 값 누락은 에러여야 한다.
    #[test]
    fn parse_render_args_rejects_missing_surface_format_value() {
        assert!(parse_render_args(&render_argv(&["--surface-format"])).is_err());
    }

    /// `--surface-format`이 명시되면 `--oneline`보다 우선(둘 다 있어도 cmux-status 우선, 설계 §5.1).
    #[test]
    fn parse_render_args_surface_format_overrides_oneline() {
        let parsed = parse_render_args(&render_argv(&[
            "--oneline",
            "--surface-format",
            "cmux-status",
        ]))
        .expect("파싱 성공");
        assert_eq!(parsed.surface_format, SurfaceFormat::CmuxStatus);
        // --oneline 플래그 자체는 보존된다(chain-skip 제어용).
        assert!(parsed.oneline);
    }

    /// `--surface-format` 중복은 마지막 값이 이긴다(last-wins).
    #[test]
    fn parse_render_args_duplicate_surface_format_last_wins() {
        let parsed = parse_render_args(&render_argv(&[
            "--surface-format",
            "cmux-status",
            "--surface-format",
            "oneline",
        ]))
        .expect("파싱 성공");
        assert_eq!(parsed.surface_format, SurfaceFormat::Oneline);
    }

    /// parse_surface_format: 값 해석 + 미지값 거부.
    #[test]
    fn parse_surface_format_values() {
        assert_eq!(parse_surface_format("oneline"), Ok(SurfaceFormat::Oneline));
        assert_eq!(
            parse_surface_format("cmux-status"),
            Ok(SurfaceFormat::CmuxStatus)
        );
        assert!(parse_surface_format("bogus").is_err());
        assert!(parse_surface_format("").is_err());
    }

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
        assert!(parse_theme_prompt("invalid_theme").is_err());
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

    // === ctx hold 캐시 해석(interpret_held_native_ctx, 순수) ===

    /// 캐시 항목 부재면 None(유지값 없음).
    #[test]
    fn interpret_held_missing_entry_is_none() {
        assert_eq!(
            interpret_held_native_ctx(None, 10_000, config::DEFAULT_CONTEXT_HOLD_TTL_SECONDS),
            None
        );
    }

    /// TTL 내 정수/소수 payload는 그대로 복원된다(`format!`↔`parse` 라운드트립).
    #[test]
    fn interpret_held_roundtrips_writer_format() {
        // resolve_claude_context가 쓰는 직렬화(`format!("{native}")`)와 동일 경로를 검증.
        // 영속화되는 값은 항상 양수·0..=100 클램프(persist_native)이므로 그 범위만 라운드트립한다.
        for value in [86.0_f64, 33.7, 100.0, 12.5, 0.5] {
            let payload = format!("{value}");
            assert_eq!(
                interpret_held_native_ctx(
                    Some((1_000, payload)),
                    1_000,
                    config::DEFAULT_CONTEXT_HOLD_TTL_SECONDS
                ),
                Some(value),
                "값 {value} 라운드트립 실패",
            );
        }
    }

    /// TTL 경계: 정확히 TTL이면 신선(유지), 1ms 초과면 stale(None). 프로덕션 기본 TTL 기준으로 검증.
    #[test]
    fn interpret_held_respects_ttl_boundary() {
        let ttl_seconds = config::DEFAULT_CONTEXT_HOLD_TTL_SECONDS;
        let ttl_ms = (ttl_seconds as u128) * 1_000;
        let at_ttl =
            interpret_held_native_ctx(Some((1_000, "86".to_string())), 1_000 + ttl_ms, ttl_seconds);
        assert_eq!(at_ttl, Some(86.0), "경계(정확히 TTL)는 유지");
        let past_ttl = interpret_held_native_ctx(
            Some((1_000, "86".to_string())),
            1_000 + ttl_ms + 1,
            ttl_seconds,
        );
        assert_eq!(past_ttl, None, "TTL 초과는 stale → None");
    }

    /// 시계 역행(now < written)은 보수적으로 stale 처리한다.
    #[test]
    fn interpret_held_clock_skew_is_stale() {
        assert_eq!(
            interpret_held_native_ctx(
                Some((2_000, "86".to_string())),
                1_000,
                config::DEFAULT_CONTEXT_HOLD_TTL_SECONDS
            ),
            None
        );
    }

    /// 손상/비숫자/비유한 payload는 안전하게 None으로 저하한다(패닉 없음).
    #[test]
    fn interpret_held_rejects_garbage_and_nonfinite() {
        for bad in ["abc", "", "NaN", "inf", "-inf"] {
            assert_eq!(
                interpret_held_native_ctx(
                    Some((1_000, bad.to_string())),
                    1_000,
                    config::DEFAULT_CONTEXT_HOLD_TTL_SECONDS
                ),
                None,
                "payload {bad:?}는 None이어야 함",
            );
        }
    }

    /// 변조/손상으로 범위(0 < v <= 100)를 벗어난 캐시 payload는 거부해 토큰 fallback으로 저하시킨다.
    #[test]
    fn interpret_held_rejects_out_of_range() {
        for bad in ["150", "101", "0", "-5", "1e24"] {
            assert_eq!(
                interpret_held_native_ctx(
                    Some((1_000, bad.to_string())),
                    1_000,
                    config::DEFAULT_CONTEXT_HOLD_TTL_SECONDS
                ),
                None,
                "범위 밖 payload {bad:?}는 None이어야 함(손상 캐시 → fallback 저하)",
            );
        }
        // 경계: 100은 유효, 양의 소수도 유효.
        assert_eq!(
            interpret_held_native_ctx(
                Some((1_000, "100".to_string())),
                1_000,
                config::DEFAULT_CONTEXT_HOLD_TTL_SECONDS
            ),
            Some(100.0)
        );
        assert_eq!(
            interpret_held_native_ctx(
                Some((1_000, "0.5".to_string())),
                1_000,
                config::DEFAULT_CONTEXT_HOLD_TTL_SECONDS
            ),
            Some(0.5)
        );
    }

    /// 캐시 글루 라운드트립: resolve_claude_context가 쓰는 직렬화·캐시명·세션격리를 실제 세션 캐시
    /// 경로(chain `*_in` 주입)로 검증한다. 양수 native를 기록 → 동일 키로 읽어 interpret이 복원.
    #[test]
    fn held_native_cache_roundtrip_through_session_cache() {
        let base =
            std::env::temp_dir().join(format!("understatus-ctxnative-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let session_key = "ctx-roundtrip-session";
        let now_ms: u128 = 1_000_000;

        // 기록: resolve_claude_context와 동일한 직렬화(format!("{native}"))·캐시명을 사용.
        let native = 86.4_f64;
        chain::write_session_named_cache_in(
            &base,
            session_key,
            CONTEXT_NATIVE_CACHE,
            now_ms,
            &format!("{native}"),
        );

        // 읽기 + 해석: TTL 내이므로 기록값이 그대로 복원되어야 한다.
        let entry = chain::read_session_named_cache_in(&base, session_key, CONTEXT_NATIVE_CACHE);
        assert_eq!(
            interpret_held_native_ctx(entry, now_ms, config::DEFAULT_CONTEXT_HOLD_TTL_SECONDS),
            Some(native),
            "세션 캐시 라운드트립으로 직전 native가 복원되어야 함",
        );

        // 다른 세션 키로는 보이지 않는다(세션 격리 확인).
        let other =
            chain::read_session_named_cache_in(&base, "other-session", CONTEXT_NATIVE_CACHE);
        assert_eq!(
            interpret_held_native_ctx(other, now_ms, config::DEFAULT_CONTEXT_HOLD_TTL_SECONDS),
            None
        );

        let _ = std::fs::remove_dir_all(&base);
    }
}
