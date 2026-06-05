//! `render --source lterm --oneline`의 출력 계약 통합 테스트(spec §6.3, §10).
//!
//! 코어 1행을 stdout으로 쓰는 경로는 단위 테스트로 직접 관측하기 어렵다(직접 출력).
//! 따라서 빌드된 바이너리를 실제로 실행해 stdout 바이트를 검증한다:
//! - 정확히 1행(개행 0개), 후행 개행 없음.
//! - chain 미수행(체인 HUD seam "│"가 없음).
//! - cols 힌트가 강제 절단을 하지 않음(최종 폭 권위는 lterm).

use std::io::Write;
use std::process::{Command, Stdio};

/// chain 실행 여부를 stdout으로 직접 관측하기 위한 센티널. chain_command가 도는 경우에만
/// 자식 stdout에 합성되어 self 세그먼트와 함께 한 줄에 나타난다.
const CHAIN_SENTINEL: &str = "CHAINSENTINEL";

/// 빌드된 understatus 바이너리에 stdin/인자를 주어 실행하고 stdout 바이트를 반환한다.
///
/// # 인자
/// - `args`: render 서브커맨드 뒤 플래그(예: `["render", "--source", "lterm", "--oneline"]`).
/// - `stdin`: 자식 stdin으로 전달할 JSON 본문.
///
/// # 반환
/// 자식 stdout 바이트 전체. NO_COLOR=1로 색을 끄고, 설정은 부재 경로로 기본값을 강제한다.
fn run_understatus(args: &[&str], stdin: &str) -> Vec<u8> {
    // 존재하지 않는 설정 경로 → 전 항목 기본값(테스트 격리, chain_command 없음).
    run_understatus_with_config(args, stdin, "/nonexistent/understatus-test-config.toml")
}

/// [`run_understatus`]와 동일하되 `UNDERSTATUS_CONFIG` 경로를 명시 주입한다.
///
/// chain_command가 설정된 임시 config를 주입해 chain 실행 여부를 stdout으로 직접 관측하기 위함이다.
///
/// # 인자
/// - `args`: render 서브커맨드 뒤 플래그.
/// - `stdin`: 자식 stdin으로 전달할 JSON 본문.
/// - `config_path`: `UNDERSTATUS_CONFIG`로 주입할 config.toml 경로.
fn run_understatus_with_config(args: &[&str], stdin: &str, config_path: &str) -> Vec<u8> {
    let mut child = Command::new(env!("CARGO_BIN_EXE_understatus"))
        .args(args)
        .env("NO_COLOR", "1")
        .env("UNDERSTATUS_CONFIG", config_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("understatus 바이너리 실행 실패");

    child
        .stdin
        .take()
        .expect("stdin 핸들 없음")
        .write_all(stdin.as_bytes())
        .expect("stdin 쓰기 실패");

    let output = child.wait_with_output().expect("자식 종료 대기 실패");
    assert!(
        output.status.success(),
        "종료 코드 비정상: {:?}",
        output.status
    );
    output.stdout
}

/// chain_command가 센티널을 출력하도록 설정한 임시 config.toml을 만들고 그 경로를 반환한다.
///
/// chain 자식은 `sh -c <command>`로 실행되므로 `printf CHAINSENTINEL`이 도는지로 chain
/// 실행 여부를 직접 검증한다. 테스트마다 고유 경로를 써서 캐시/병렬 간섭을 피한다.
///
/// # 인자
/// - `tag`: 파일명 고유화 태그(테스트별 충돌/캐시 격리용).
///
/// # 반환
/// 작성된 config.toml의 절대 경로 문자열.
fn write_chain_config(tag: &str) -> String {
    let path = std::env::temp_dir().join(format!(
        "understatus-chain-cfg-{}-{}.toml",
        std::process::id(),
        tag
    ));
    // [chain] chain_command가 sh -c로 실행된다. 센티널만 출력하는 최소 명령.
    let toml = format!("[chain]\nchain_command = \"printf {CHAIN_SENTINEL}\"\n");
    std::fs::write(&path, toml).expect("임시 config 작성 실패");
    path.to_string_lossy().into_owned()
}

/// --oneline은 정확히 1행을 후행 개행 없이 출력해야 한다(spec §6.3).
#[test]
fn oneline_emits_single_line_without_trailing_newline() {
    let stdout = run_understatus(
        &["render", "--source", "lterm", "--oneline"],
        r#"{"source":"lterm","session":"codex","pane":"%3","cwd":"/tmp/proj"}"#,
    );
    let text = String::from_utf8(stdout).expect("stdout는 UTF-8이어야 함");
    // 후행 개행 0개.
    assert!(!text.ends_with('\n'), "후행 개행이 있으면 안 됨: {text:?}");
    // 내부 개행 0개(정확히 1행).
    assert_eq!(
        text.matches('\n').count(),
        0,
        "정확히 1행이어야 함(개행 0): {text:?}"
    );
    // 코어 세그먼트(CPU% 등)가 비어 있지 않아야 한다.
    assert!(!text.is_empty(), "코어 출력이 비면 안 됨");
    assert!(text.contains('%'), "시스템 지표(%)가 있어야 함: {text:?}");
}

/// 기본 render(--oneline 없음)는 후행 개행이 붙는다(기존 동작 보존, 대조군).
#[test]
fn default_render_has_trailing_newline() {
    let stdout = run_understatus(&["render"], "{}");
    let text = String::from_utf8(stdout).expect("stdout는 UTF-8이어야 함");
    assert!(
        text.ends_with('\n'),
        "기본 render는 println!으로 후행 개행이 있어야 함: {text:?}"
    );
}

/// --oneline은 chain을 수행하지 않는다(실제 chain_command 설정 상태에서 직접 증명).
///
/// 기본 config 대신 chain_command(센티널 출력)가 설정된 임시 config를 주입해 chain 실행
/// 여부를 stdout 센티널로 직접 관측한다. 세 분기를 한 테스트에서 대조 검증한다:
/// - `--oneline`(claude source): chain 미수행 → 센티널 **없음**(수정 #2).
/// - 동일 config로 `--oneline` 없이(claude source): chain 수행 → 센티널 **있음**(대조군).
/// - `--source lterm`(--oneline 없이): chain 비활성 → 센티널 **없음**(수정 #3).
///
/// 각 분기는 서로 다른 session/pane(=session_key)을 써서 chain 캐시 교차 오염을 피한다.
#[test]
fn oneline_does_not_run_chain() {
    let config = write_chain_config("oneline-chain-skip");

    // (1) --oneline(claude source): chain 미수행 → 센티널 없음.
    let oneline_out = run_understatus_with_config(
        &["render", "--oneline"],
        r#"{"session_id":"oneline-skip-a"}"#,
        &config,
    );
    let oneline_text = String::from_utf8(oneline_out).expect("stdout는 UTF-8이어야 함");
    assert!(
        !oneline_text.contains(CHAIN_SENTINEL),
        "--oneline은 chain을 수행하면 안 됨(센티널 부재): {oneline_text:?}"
    );

    // (2) 대조군: 동일 config로 --oneline 없이(claude source) → chain 수행 → 센티널 있음.
    //   chain이 실제로 도는지 증명해 (1)의 부재가 chain-skip 때문임을 분리 검증한다.
    let control_out = run_understatus_with_config(
        &["render"],
        r#"{"session_id":"oneline-skip-control"}"#,
        &config,
    );
    let control_text = String::from_utf8(control_out).expect("stdout는 UTF-8이어야 함");
    assert!(
        control_text.contains(CHAIN_SENTINEL),
        "대조군(--oneline 없음, claude)은 chain이 실제로 돌아 센티널이 있어야 함: {control_text:?}"
    );

    // (3) --source lterm(--oneline 없이): chain 비활성 → 센티널 없음(수정 #3).
    let lterm_out = run_understatus_with_config(
        &["render", "--source", "lterm"],
        r#"{"source":"lterm","session":"oneline-skip-lterm","pane":"%1"}"#,
        &config,
    );
    let lterm_text = String::from_utf8(lterm_out).expect("stdout는 UTF-8이어야 함");
    assert!(
        !lterm_text.contains(CHAIN_SENTINEL),
        "--source lterm은 chain 기본 off여야 함(센티널 부재): {lterm_text:?}"
    );

    let _ = std::fs::remove_file(&config);
}

/// 작은 cols 힌트가 와도 강제 절단하지 않는다(최종 폭 권위는 lterm, spec §6.3).
///
/// cols=10처럼 작은 값을 주어도 출력이 그 폭으로 잘리지 않고 정상 세그먼트가 유지되어야 한다.
#[test]
fn oneline_cols_hint_does_not_force_truncation() {
    let stdout = run_understatus(
        &["render", "--source", "lterm", "--oneline"],
        r#"{"source":"lterm","session":"codex","pane":"%3","cwd":"/tmp/proj","cols":10,"rows":2}"#,
    );
    let text = String::from_utf8(stdout).expect("stdout는 UTF-8이어야 함");
    // cols=10보다 길게 나올 수 있어야 한다(강제 절단 금지). 기본 max_width(80)만 적용된다.
    // 최소한 cwd 디렉터리명("proj")이 살아 있어야 한다(좁은 cols로 인한 손실이 없음).
    assert!(
        text.contains("proj"),
        "cols 힌트가 cwd를 잘라내면 안 됨(폭 권위는 lterm): {text:?}"
    );
}

/// git 세그먼트는 lterm 소스에서 비활성이어야 한다(Phase 1, spec §6.2).
///
/// lterm payload엔 git 도출 입력이 없고 parse_lterm_input이 git_branch를 채우지 않으므로
/// git 마커(⎇)가 출력에 없어야 한다.
#[test]
fn oneline_lterm_has_no_git_segment() {
    let stdout = run_understatus(
        &["render", "--source", "lterm", "--oneline"],
        r#"{"source":"lterm","session":"codex","pane":"%3","cwd":"/tmp/proj"}"#,
    );
    let text = String::from_utf8(stdout).expect("stdout는 UTF-8이어야 함");
    assert!(
        !text.contains('⎇'),
        "lterm 소스는 git 세그먼트(⎇)가 없어야 함: {text:?}"
    );
}
