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

use std::io::Read;
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
        Some("install") => match install::install() {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("understatus: 설치 실패: {error:#}");
                ExitCode::FAILURE
            }
        },
        Some("uninstall") => match install::uninstall() {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("understatus: 제거 실패: {error:#}");
                ExitCode::FAILURE
            }
        },
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

/// 렌더 파이프라인을 실행하고 합성된 한 줄을 stdout에 출력한다.
///
/// 계획서 §D-1의 8단계를 순서대로 호출한다. 각 단계의 스텁(`todo!()`)은
/// 병렬 워커가 채운다. 본 함수는 실제 배선(stdin 읽기/단계 연결/출력)을 담당한다.
fn run_render_pipeline() {
    // (1) stdin 원본 보존(체이닝을 위해 raw 그대로 자식에 전달).
    let raw_stdin = read_stdin();

    // (2) Claude 세션 정보 파싱(누락/null/깨진 JSON 안전).
    let claude_input = claude::parse_claude_input(&raw_stdin);

    // (5) 설정 로드(부재/깨짐 시 기본값).
    let cfg = config::load_config();

    // (3)(4) 시스템 스냅샷 수집(더블샘플 CPU + 메모리 + 배터리).
    let snapshot = system::sample_system(&cfg);

    // 지각성 불변식(계획서 §H-5, AC4): 한 펄스 주기 안에 그려지는 프레임 수가 6 이상이어야
    // 색 출렁임이 끊기지 않는다. 릴리스 출력/성능에 영향을 주지 않도록 debug 빌드에서만 검증한다.
    debug_assert!(
        theme::samples_per_period(&cfg, cfg.refresh.interval_seconds) >= 6,
        "펄스 지각성 불변식 위반: samples_per_period < 6 (pulse_period={}s, refreshInterval={}s)",
        cfg.pulse.pulse_period_seconds,
        cfg.refresh.interval_seconds
    );

    // 히스테리시스: 직전 펄스 상태 읽기 → 게이트 → 다음 호출을 위해 기록.
    let prev_pulse_on = chain::read_prev_pulse_state();
    let now_ms = now_millis();
    let pulse_on = theme::pulse_gate(snapshot.cpu_percent, prev_pulse_on, &cfg);
    chain::write_pulse_state(pulse_on);

    // (6) understatus 자체 세그먼트 렌더.
    let self_segment = render::render(&claude_input, &snapshot, &cfg, now_ms, pulse_on);

    // (7) 체인 자식 실행(있으면). 타임아웃/캐시로 렌더 무블록.
    let chain_output = match cfg.chain.chain_command.as_deref() {
        Some(command) if !command.is_empty() => chain::run_chain(command, &raw_stdin, &cfg),
        _ => String::new(),
    };

    // (8) self + chain 합성 후 한 줄 출력. 체인이 있으면 dim HUD seam("│")으로 소유권 경계 표시.
    let color_on = std::env::var_os("NO_COLOR").is_none() && cfg.color.mode != "none";
    let line =
        render::compose_with_seam(&self_segment, &chain_output, &cfg.chain.order, &cfg, color_on);
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
         \x20 understatus [render]   stdin JSON을 읽어 statusline 한 줄을 출력(기본)\n\
         \x20 understatus install     기존 statusLine을 보존(체이닝)하며 비파괴 설치\n\
         \x20 understatus uninstall   원본 설정을 정확 복원하며 제거\n\
         \x20 understatus --help      이 도움말 출력\n\
         \x20 understatus --version   버전 출력",
        env!("CARGO_PKG_VERSION")
    );
}

/// 버전(`--version`)을 stdout에 출력한다.
fn print_version() {
    println!("understatus {}", env!("CARGO_PKG_VERSION"));
}
