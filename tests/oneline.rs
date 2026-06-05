//! `render --source lterm --oneline`의 출력 계약 통합 테스트(spec §6.3, §10).
//!
//! 코어 1행을 stdout으로 쓰는 경로는 단위 테스트로 직접 관측하기 어렵다(직접 출력).
//! 따라서 빌드된 바이너리를 실제로 실행해 stdout 바이트를 검증한다:
//! - 정확히 1행(개행 0개), 후행 개행 없음.
//! - chain 미수행(체인 HUD seam "│"가 없음).
//! - cols 힌트가 강제 절단을 하지 않음(최종 폭 권위는 lterm).

use std::io::Write;
use std::process::{Command, Stdio};

/// 빌드된 understatus 바이너리에 stdin/인자를 주어 실행하고 stdout 바이트를 반환한다.
///
/// # 인자
/// - `args`: render 서브커맨드 뒤 플래그(예: `["render", "--source", "lterm", "--oneline"]`).
/// - `stdin`: 자식 stdin으로 전달할 JSON 본문.
///
/// # 반환
/// 자식 stdout 바이트 전체. NO_COLOR=1로 색을 끄고, 설정은 부재 경로로 기본값을 강제한다.
fn run_understatus(args: &[&str], stdin: &str) -> Vec<u8> {
    let mut child = Command::new(env!("CARGO_BIN_EXE_understatus"))
        .args(args)
        .env("NO_COLOR", "1")
        // 존재하지 않는 설정 경로 → 전 항목 기본값(테스트 격리, chain_command 없음).
        .env(
            "UNDERSTATUS_CONFIG",
            "/nonexistent/understatus-test-config.toml",
        )
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

/// --oneline은 chain을 수행하지 않는다(HUD seam "│"가 출력에 없어야 함).
///
/// 기본 설정엔 chain_command가 없지만, oneline 경로는 cfg와 무관하게 chain 분기를
/// 아예 건너뛴다. seam("│")은 chain 출력이 있을 때만 끼므로 부재로 간접 확인한다.
#[test]
fn oneline_does_not_run_chain() {
    let stdout = run_understatus(
        &["render", "--source", "lterm", "--oneline"],
        r#"{"source":"lterm","session":"s","pane":"%1"}"#,
    );
    let text = String::from_utf8(stdout).expect("stdout는 UTF-8이어야 함");
    assert!(
        !text.contains('│'),
        "chain seam(│)이 없어야 함(chain 미수행): {text:?}"
    );
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
