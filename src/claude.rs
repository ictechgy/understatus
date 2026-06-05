//! Claude Code stdin JSON 파서 (P1: Claude Code 전용, 하드코딩).
//!
//! 계획서 §G의 실제 stdin JSON 스키마를 누락/`null` 안전하게 파싱한다.
//! 모든 필드는 `Option`이며 파싱 자체가 실패해도 절대 패닉하지 않고
//! 전부 `None`인 빈 `ClaudeInput`으로 안전 저하한다(lenient).

use serde::Deserialize;

/// understatus이 라인 렌더에 사용하는 Claude 세션 정보의 평탄화된 뷰.
///
/// 계획서 §G의 중첩 JSON(`model.display_name`, `cost.total_cost_usd`,
/// `context_window.used_percentage`, `workspace.*`)에서 필요한 필드만 추출한 결과다.
/// 모든 필드는 부재/`null`에 안전하도록 `Option`으로 둔다.
///
/// 주의: `git_branch`는 stdin의 직접 필드가 아니라 `workspace.git_worktree` /
/// `workspace.repo`에서 **파생(derive)**된 값이다(계획서 §G, AC2).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ClaudeInput {
    /// 모델 표시명 (`model.display_name`). 라인에 표시.
    pub model_display_name: Option<String>,
    /// 컨텍스트 사용률 % (`context_window.used_percentage`).
    /// 첫 API 호출 전 / `/compact` 직후 `null` → `None`이면 세그먼트 생략.
    pub context_used_percentage: Option<f64>,
    /// 현재 작업 디렉터리 (`cwd` 또는 `workspace.current_dir`).
    pub cwd: Option<String>,
    /// `workspace.git_worktree`/`workspace.repo`에서 파생한 git 브랜치명.
    pub git_branch: Option<String>,
    /// 누적 비용 USD (`cost.total_cost_usd`). 라인에 표시.
    pub cost_usd: Option<f64>,
    /// 세션 식별자 (`session_id`).
    pub session_id: Option<String>,
}

// CONTRACT: signature is frozen — implement body only, do not change this signature
/// raw stdin 문자열을 [`ClaudeInput`]으로 파싱한다.
///
/// # 인자
/// - `raw`: Claude Code가 stdin으로 전달한 JSON 한 줄(빈 문자열/깨진 JSON 가능).
///
/// # 반환
/// 파싱 가능한 필드를 채운 [`ClaudeInput`]. JSON이 비었거나 깨졌으면
/// 모든 필드가 `None`인 기본값을 반환한다(절대 패닉하지 않음, lenient).
///
/// # 주의
/// `git_branch`는 직접 필드가 아니라 `workspace.git_worktree`/`workspace.repo`에서
/// 파생한다(계획서 §G, AC2).
pub fn parse_claude_input(raw: &str) -> ClaudeInput {
    // LENIENT: 깨진/빈 JSON은 에러 대신 전부 None인 기본값으로 안전 저하한다.
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return ClaudeInput::default();
    }
    let raw_input: RawClaudeInput = match serde_json::from_str(trimmed) {
        Ok(parsed) => parsed,
        // 깨진 JSON → 패닉 금지, 전부 None.
        Err(_) => return ClaudeInput::default(),
    };

    // 중첩 객체를 평탄화한다. 각 단계는 Option을 그대로 흘려보내 부재/null에 견딘다.
    let model_display_name = raw_input.model.and_then(|model| model.display_name);
    let context_used_percentage = raw_input
        .context_window
        .and_then(|window| window.used_percentage);
    let cost_usd = raw_input.cost.and_then(|cost| cost.total_cost_usd);

    // cwd는 최상위 `cwd`를 우선하고, 없으면 workspace.current_dir로 폴백한다.
    // git_branch는 직접 필드가 아니라 workspace.git_worktree/repo에서 파생한다(§G, AC2).
    let (cwd_from_workspace, git_branch) = match raw_input.workspace {
        Some(workspace) => {
            let branch = derive_git_branch(&workspace);
            (workspace.current_dir, branch)
        }
        None => (None, None),
    };

    ClaudeInput {
        model_display_name,
        context_used_percentage,
        cwd: raw_input.cwd.or(cwd_from_workspace),
        git_branch,
        cost_usd,
        session_id: raw_input.session_id,
    }
}

/// lterm 합성 stdin JSON을 [`ClaudeInput`]으로 파싱한다([`parse_claude_input`]과 대칭, lenient).
///
/// # 인자
/// - `raw`: lterm이 stdin으로 전달한 JSON 한 줄(빈 `{}`/누락/미상 필드 가능). 계약(spec §4.1):
///   `source`/`version`/`session`/`pane`/`session_key`/`agent`/`cwd`/`cols`/`rows`.
///
/// # 반환
/// 표시에 필요한 필드를 채운 [`ClaudeInput`]. JSON이 비었거나 깨졌으면 전부 `None`인
/// 기본값으로 안전 저하한다(절대 패닉하지 않음, lenient — `parse_claude_input` 철학 동일).
///
/// # 주의
/// - `cwd`는 **표시용으로만** 매핑한다(git 도출 안 함). `$PWD` 폴백은 추가하지 않는다(spec §4.1/§6.2).
/// - `git_branch`는 항상 `None`으로 둔다 → 자연히 git 세그먼트 미표시(Phase 1 git 비활성).
/// - `session_key`는 캐시/펄스 격리용 안정 키다. 없으면 `"<session>/<pane>"`로 합성한다
///   (실제 경로 살균은 호출부 [`crate::chain::sanitize_session_key`]가 담당).
/// - `version`은 `version` 필드로 읽되 Phase 1은 분기 없이 무시한다(forward-compat).
pub fn parse_lterm_input(raw: &str) -> ClaudeInput {
    // LENIENT: 깨진/빈 JSON은 에러 대신 전부 None인 기본값으로 안전 저하한다.
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return ClaudeInput::default();
    }
    let raw_input: RawLtermInput = match serde_json::from_str(trimmed) {
        Ok(parsed) => parsed,
        // 깨진 JSON → 패닉 금지, 전부 None.
        Err(_) => return ClaudeInput::default(),
    };

    // session_key는 명시값을 우선하고, 없으면 "<session>/<pane>"로 합성한다(캐시/펄스 격리).
    let session_key = raw_input
        .session_key
        .filter(|key| !key.is_empty())
        .or_else(|| synthesize_session_key(&raw_input.session, &raw_input.pane));

    ClaudeInput {
        // 에이전트/모델 표시명: lterm payload의 `agent`를 모델 슬롯에 매핑(best-effort).
        model_display_name: raw_input.agent,
        context_used_percentage: None,
        // cwd는 표시용으로만 사용한다(git 도출 안 함, $PWD 폴백 없음).
        cwd: raw_input.cwd,
        // git 세그먼트 비활성: branch를 절대 채우지 않는다(Phase 1).
        git_branch: None,
        cost_usd: None,
        session_id: session_key,
    }
}

/// `session`/`pane`으로 안정 session_key를 합성한다(명시 `session_key` 부재 시).
///
/// # 인자
/// - `session`: lterm 세션 이름(예: `"codex"`).
/// - `pane`: lterm 페인 식별자(예: `"%3"`).
///
/// # 반환
/// `"<session>/<pane>"` 합성 키. 둘 다 부재면 `None`(호출부가 빈 키 → "default"로 폴백).
/// 한쪽만 있으면 있는 쪽만 사용한다(빈 세그먼트로 인한 무의미한 슬래시 방지).
fn synthesize_session_key(session: &Option<String>, pane: &Option<String>) -> Option<String> {
    let session = session.as_deref().filter(|value| !value.is_empty());
    let pane = pane.as_deref().filter(|value| !value.is_empty());
    match (session, pane) {
        (Some(session), Some(pane)) => Some(format!("{session}/{pane}")),
        (Some(session), None) => Some(session.to_string()),
        (None, Some(pane)) => Some(pane.to_string()),
        (None, None) => None,
    }
}

/// `workspace.git_worktree`(우선) 또는 `workspace.repo`에서 현재 git 브랜치를 파생한다.
///
/// # 인자
/// - `workspace`: Claude stdin의 `workspace` 중첩 객체.
///
/// # 반환
/// 워크트리 경로의 `.git/HEAD`를 읽어 `ref: refs/heads/<branch>`에서 추출한 브랜치명.
/// 경로/파일 부재, detached HEAD, 읽기 실패 시 `None`으로 안전 저하한다(패닉 금지, §G/AC2).
fn derive_git_branch(workspace: &RawWorkspace) -> Option<String> {
    // git_worktree를 우선 근거로, 없으면 repo 경로를 사용한다.
    let base_path = workspace
        .git_worktree
        .as_deref()
        .or(workspace.repo.as_deref())?;
    // 외부 입력 경로 검증(traversal 차단): stdin으로 들어온 신뢰 불가 경로이므로
    // `..` 상위 디렉터리 이동이 섞인 입력은 임의 위치 `.git/HEAD` 탐색을 노릴 수 있어 거부한다.
    if !is_safe_base_path(base_path) {
        return None;
    }
    read_branch_from_git_dir(base_path)
}

/// 외부 입력으로 받은 git 워크트리 경로가 안전한지(상위 디렉터리 이동이 없는지) 검사한다.
///
/// # 인자
/// - `base_path`: stdin의 `workspace.git_worktree`/`repo`에서 온 신뢰 불가 경로 문자열.
///
/// # 반환
/// 경로가 비어 있지 않고 `..`(상위 디렉터리) 컴포넌트를 포함하지 않으면 `true`.
///
/// # 주의
/// 외부 입력 경로 검증(traversal 차단): `../`로 의도하지 않은 상위 경로의 `.git/HEAD`를
/// 읽는 path traversal 정보 탐색을 막기 위함이다. 절대경로 자체는 허용하되(정상 워크트리
/// 보존), 심볼릭 링크 차단은 호출 측의 canonicalize 검증과 함께 다층 방어로 동작한다.
fn is_safe_base_path(base_path: &str) -> bool {
    use std::path::{Component, Path};
    if base_path.trim().is_empty() {
        return false;
    }
    // `..` 컴포넌트가 하나라도 있으면 traversal 시도로 보고 거부한다.
    !Path::new(base_path)
        .components()
        .any(|component| matches!(component, Component::ParentDir))
}

/// 주어진 git 작업트리 경로에서 `.git/HEAD`를 읽어 현재 브랜치명을 추출한다.
///
/// # 인자
/// - `base_path`: git 워크트리(또는 repo) 루트 경로.
///
/// # 반환
/// `ref: refs/heads/<branch>` 형식의 HEAD에서 추출한 `<branch>`. detached HEAD(직접 SHA)나
/// 읽기 실패 시 `None`. 부재/실패에 안전(절대 패닉하지 않음).
fn read_branch_from_git_dir(base_path: &str) -> Option<String> {
    use std::path::Path;
    // 표준 워크트리는 `<base>/.git/HEAD`. (linked worktree의 gitfile 케이스는 v1 범위 밖.)
    let head_path = Path::new(base_path).join(".git").join("HEAD");
    // 외부 입력 경로 검증(심볼릭 차단): canonicalize로 심볼릭 링크/`.` 등을 해소한 실제
    // 경로가 여전히 `.git/HEAD`로 끝나는지 확인한다. 심볼릭 링크가 다른 파일을 가리키면
    // 끝이 달라져 거부되고, 경로가 없으면 canonicalize가 Err → None으로 안전 저하한다.
    // (정상 워크트리의 실재 `.git/HEAD`는 문제없이 해소되므로 정상 동작은 보존된다.)
    let canonical = std::fs::canonicalize(&head_path).ok()?;
    if !canonical.ends_with(Path::new(".git").join("HEAD")) {
        return None;
    }
    let contents = std::fs::read_to_string(&canonical).ok()?;
    let trimmed = contents.trim();
    // 심볼릭 ref만 브랜치명을 가진다: "ref: refs/heads/main".
    let branch = trimmed.strip_prefix("ref: refs/heads/")?;
    if branch.is_empty() {
        None
    } else {
        Some(branch.to_string())
    }
}

/// Claude Code stdin JSON의 중첩 구조를 그대로 받는 내부 역직렬화 타입.
///
/// `#[serde(default)]`로 누락 필드를 안전 처리하고, 각 중첩 객체도 `Option`으로 둬
/// `null`/부재에 견딘다. [`parse_claude_input`]이 이 타입을 [`ClaudeInput`]으로 평탄화한다.
#[derive(Debug, Deserialize, Default)]
struct RawClaudeInput {
    #[serde(default)]
    session_id: Option<String>,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    model: Option<RawModel>,
    #[serde(default)]
    workspace: Option<RawWorkspace>,
    #[serde(default)]
    cost: Option<RawCost>,
    #[serde(default)]
    context_window: Option<RawContextWindow>,
}

/// `model` 중첩 객체.
#[derive(Debug, Deserialize, Default)]
struct RawModel {
    #[serde(default)]
    display_name: Option<String>,
    // 스키마 완전성을 위해 역직렬화하지만 라인 렌더에는 쓰지 않는다(§G).
    #[serde(default)]
    #[allow(dead_code)]
    id: Option<String>,
}

/// `workspace` 중첩 객체. git 브랜치 파생 근거(`git_worktree`/`repo`)를 포함.
#[derive(Debug, Deserialize, Default)]
struct RawWorkspace {
    #[serde(default)]
    current_dir: Option<String>,
    // 스키마 완전성을 위해 역직렬화하지만 라인 렌더에는 쓰지 않는다(§G).
    #[serde(default)]
    #[allow(dead_code)]
    project_dir: Option<String>,
    #[serde(default)]
    git_worktree: Option<String>,
    #[serde(default)]
    repo: Option<String>,
}

/// `cost` 중첩 객체.
#[derive(Debug, Deserialize, Default)]
struct RawCost {
    #[serde(default)]
    total_cost_usd: Option<f64>,
}

/// `context_window` 중첩 객체. `used_percentage`는 `null` 가능.
#[derive(Debug, Deserialize, Default)]
struct RawContextWindow {
    #[serde(default)]
    used_percentage: Option<f64>,
}

/// lterm 합성 stdin JSON(평탄 구조)을 그대로 받는 내부 역직렬화 타입(spec §4.1 계약).
///
/// `#[serde(default)]`로 누락/미상 필드를 안전 처리하고, 빈 `{}`에도 견딘다([`RawClaudeInput`]과
/// 동일 철학). [`parse_lterm_input`]이 이 타입을 [`ClaudeInput`]으로 매핑한다.
#[derive(Debug, Deserialize, Default)]
struct RawLtermInput {
    // 스키마 완전성을 위해 역직렬화하지만 라인 렌더에는 쓰지 않는다(`source`는 호출부 분기로 결정됨).
    // forward-compat: 미소비 필드는 타입에 관대하게(Value) 받아, 타입 드리프트(예: 숫자 대신
    // 문자열)가 와도 from_str이 실패하지 않게 한다. 이 필드의 타입 어긋남이 session/pane 등
    // 정상 필드 매핑까지 깨뜨려 전체 payload가 default로 저하되는 것을 막는다.
    #[serde(default)]
    #[allow(dead_code)]
    source: Option<serde_json::Value>,
    // 버전 협상용. Phase 1은 읽되 분기 없이 무시한다(forward-compat, spec §4.1).
    // lterm이 "version":"1"처럼 문자열로 보내도 파싱 전체가 실패하지 않도록 Value로 받는다.
    #[serde(default)]
    #[allow(dead_code)]
    version: Option<serde_json::Value>,
    #[serde(default)]
    session: Option<String>,
    #[serde(default)]
    pane: Option<String>,
    #[serde(default)]
    session_key: Option<String>,
    #[serde(default)]
    agent: Option<String>,
    #[serde(default)]
    cwd: Option<String>,
    // 폭 맞춤 힌트. 최종 폭 권위는 lterm이므로 understatus는 참고만 한다(현재 미소비).
    // Phase 1은 미소비이므로 Value로 관대하게 받는다(타입 드리프트 격리). 추후 소비 시
    // 숫자 변환은 그 시점에 별도로 처리한다.
    #[serde(default)]
    #[allow(dead_code)]
    cols: Option<serde_json::Value>,
    #[serde(default)]
    #[allow(dead_code)]
    rows: Option<serde_json::Value>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 정상 JSON: 모든 필드가 올바르게 평탄화되어야 한다(AC2).
    #[test]
    fn parses_normal_input() {
        let raw = r#"{
            "session_id": "sess-123",
            "cwd": "/Users/me/proj",
            "model": { "display_name": "Claude Opus", "id": "claude-opus" },
            "workspace": { "current_dir": "/Users/me/proj", "repo": "myrepo" },
            "cost": { "total_cost_usd": 0.42 },
            "context_window": { "used_percentage": 37.5 }
        }"#;
        let input = parse_claude_input(raw);
        assert_eq!(input.session_id.as_deref(), Some("sess-123"));
        assert_eq!(input.cwd.as_deref(), Some("/Users/me/proj"));
        assert_eq!(input.model_display_name.as_deref(), Some("Claude Opus"));
        assert_eq!(input.cost_usd, Some(0.42));
        assert_eq!(input.context_used_percentage, Some(37.5));
    }

    /// `context_window`가 null이면 컨텍스트 사용률은 None이어야 한다(AC2, 패닉 금지).
    #[test]
    fn null_context_window_yields_none() {
        let raw = r#"{ "model": { "display_name": "M" }, "context_window": null }"#;
        let input = parse_claude_input(raw);
        assert_eq!(input.context_used_percentage, None);
        assert_eq!(input.model_display_name.as_deref(), Some("M"));
    }

    /// `context_window.used_percentage`가 null이어도 None으로 안전 저하해야 한다.
    #[test]
    fn null_used_percentage_yields_none() {
        let raw = r#"{ "context_window": { "used_percentage": null } }"#;
        let input = parse_claude_input(raw);
        assert_eq!(input.context_used_percentage, None);
    }

    /// 필드 누락: 부재 필드는 전부 None이어야 한다(에러/패닉 없음).
    #[test]
    fn missing_fields_default_to_none() {
        let raw = r#"{ "session_id": "only-session" }"#;
        let input = parse_claude_input(raw);
        assert_eq!(input.session_id.as_deref(), Some("only-session"));
        assert_eq!(input.cwd, None);
        assert_eq!(input.model_display_name, None);
        assert_eq!(input.context_used_percentage, None);
        assert_eq!(input.cost_usd, None);
        assert_eq!(input.git_branch, None);
    }

    /// 빈 객체는 전부 None인 기본값을 반환해야 한다.
    #[test]
    fn empty_object_is_all_none() {
        let input = parse_claude_input("{}");
        assert_eq!(input, ClaudeInput::default());
    }

    /// 깨진 JSON은 패닉 없이 전부 None으로 저하해야 한다(LENIENT, AC1/AC2).
    #[test]
    fn broken_json_returns_default() {
        for raw in ["", "   ", "not json", "{ \"model\": ", "[1,2,3]"] {
            let input = parse_claude_input(raw);
            assert_eq!(input, ClaudeInput::default(), "입력: {raw:?}");
        }
    }

    /// cwd 부재 시 workspace.current_dir로 폴백해야 한다.
    #[test]
    fn cwd_falls_back_to_workspace_current_dir() {
        let raw = r#"{ "workspace": { "current_dir": "/ws/dir" } }"#;
        let input = parse_claude_input(raw);
        assert_eq!(input.cwd.as_deref(), Some("/ws/dir"));
    }

    /// 실제 .git/HEAD가 심볼릭 ref이면 브랜치명을 파생해야 한다(AC2).
    #[test]
    fn derives_git_branch_from_head() {
        use std::io::Write;
        // 임시 워크트리에 .git/HEAD를 만들어 브랜치 파생을 검증한다.
        let tmp = std::env::temp_dir().join(format!("understatus-git-test-{}", std::process::id()));
        let git_dir = tmp.join(".git");
        std::fs::create_dir_all(&git_dir).expect("임시 .git 생성 실패");
        let head = git_dir.join("HEAD");
        let mut file = std::fs::File::create(&head).expect("HEAD 생성 실패");
        writeln!(file, "ref: refs/heads/feature/my-branch").expect("HEAD 쓰기 실패");

        let raw = format!(
            r#"{{ "workspace": {{ "git_worktree": {:?} }} }}"#,
            tmp.to_string_lossy()
        );
        let input = parse_claude_input(&raw);
        assert_eq!(input.git_branch.as_deref(), Some("feature/my-branch"));

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// detached HEAD(직접 SHA)는 브랜치명이 없으므로 None이어야 한다.
    #[test]
    fn detached_head_yields_no_branch() {
        let tmp =
            std::env::temp_dir().join(format!("understatus-git-detached-{}", std::process::id()));
        let git_dir = tmp.join(".git");
        std::fs::create_dir_all(&git_dir).expect("임시 .git 생성 실패");
        std::fs::write(git_dir.join("HEAD"), "0123456789abcdef\n").expect("HEAD 쓰기 실패");

        let raw = format!(
            r#"{{ "workspace": {{ "git_worktree": {:?} }} }}"#,
            tmp.to_string_lossy()
        );
        let input = parse_claude_input(&raw);
        assert_eq!(input.git_branch, None);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// git_worktree 경로가 존재하지 않으면 브랜치 파생은 None으로 안전 저하한다.
    #[test]
    fn nonexistent_worktree_yields_no_branch() {
        let raw = r#"{ "workspace": { "git_worktree": "/nonexistent/path/xyz" } }"#;
        let input = parse_claude_input(raw);
        assert_eq!(input.git_branch, None);
    }

    /// 외부 입력 경로 검증(traversal 차단): `..`가 섞인 git_worktree는 거부되어 None이어야 한다.
    /// 악의적 stdin이 상위 경로의 `.git/HEAD`를 탐색하지 못하게 막는다.
    #[test]
    fn git_worktree_with_parent_traversal_rejected() {
        let raw = r#"{ "workspace": { "git_worktree": "/some/repo/../../etc" } }"#;
        let input = parse_claude_input(raw);
        assert_eq!(input.git_branch, None);
    }

    /// 외부 입력 경로 검증: `/etc` 같은 임의 절대경로는 `.git/HEAD` 부재로 None이어야 한다.
    /// (절대경로 자체는 허용하되 의도한 HEAD 파일이 없으므로 안전하게 None으로 저하한다.)
    #[test]
    fn absolute_system_path_yields_no_branch() {
        let raw = r#"{ "workspace": { "git_worktree": "/etc" } }"#;
        let input = parse_claude_input(raw);
        assert_eq!(input.git_branch, None);
    }

    // === parse_lterm_input (spec §6.2, §10) ===

    /// 정상 lterm JSON: 표시 필드가 정확히 매핑되고 git은 비활성이어야 한다.
    #[test]
    fn lterm_parses_normal_input() {
        let raw = r#"{
            "source": "lterm",
            "version": 1,
            "session": "codex",
            "pane": "%3",
            "session_key": "codex/%3",
            "agent": "codex",
            "cwd": "/Users/me/dev/app",
            "cols": 120,
            "rows": 40
        }"#;
        let input = parse_lterm_input(raw);
        assert_eq!(input.cwd.as_deref(), Some("/Users/me/dev/app"));
        assert_eq!(input.model_display_name.as_deref(), Some("codex"));
        assert_eq!(input.session_id.as_deref(), Some("codex/%3"));
        // git 세그먼트 비활성: branch는 항상 None.
        assert_eq!(input.git_branch, None);
        // lterm 계약엔 컨텍스트/비용이 없으므로 None.
        assert_eq!(input.context_used_percentage, None);
        assert_eq!(input.cost_usd, None);
    }

    /// 빈 객체는 전부 None인 기본값을 반환해야 한다(무패닉).
    #[test]
    fn lterm_empty_object_is_all_none() {
        let input = parse_lterm_input("{}");
        assert_eq!(input, ClaudeInput::default());
    }

    /// 미상/추가 필드가 섞여도 무시하고 정상 매핑해야 한다(lenient, 무패닉).
    #[test]
    fn lterm_unknown_fields_ignored() {
        let raw = r#"{
            "source": "lterm",
            "session": "s",
            "pane": "%1",
            "cwd": "/tmp/x",
            "future_field": { "nested": [1, 2, 3] },
            "another": "ignored"
        }"#;
        let input = parse_lterm_input(raw);
        assert_eq!(input.cwd.as_deref(), Some("/tmp/x"));
        assert_eq!(input.session_id.as_deref(), Some("s/%1"));
        assert_eq!(input.git_branch, None);
    }

    /// 필드 누락: 부재 필드는 전부 None(또는 합성)으로 안전 저하해야 한다(무패닉).
    #[test]
    fn lterm_missing_fields_default_to_none() {
        let raw = r#"{ "source": "lterm", "cwd": "/only/cwd" }"#;
        let input = parse_lterm_input(raw);
        assert_eq!(input.cwd.as_deref(), Some("/only/cwd"));
        assert_eq!(input.model_display_name, None);
        // session/pane 둘 다 부재 → session_key 합성 불가 → None.
        assert_eq!(input.session_id, None);
        assert_eq!(input.git_branch, None);
        assert_eq!(input.context_used_percentage, None);
        assert_eq!(input.cost_usd, None);
    }

    /// session_key 부재 시 "<session>/<pane>"로 합성해야 한다.
    #[test]
    fn lterm_synthesizes_session_key_from_session_and_pane() {
        let raw = r#"{ "session": "codex", "pane": "%7" }"#;
        let input = parse_lterm_input(raw);
        assert_eq!(input.session_id.as_deref(), Some("codex/%7"));
    }

    /// 명시 session_key가 있으면 합성하지 않고 그 값을 그대로 쓴다.
    #[test]
    fn lterm_explicit_session_key_takes_precedence() {
        let raw = r#"{ "session": "codex", "pane": "%7", "session_key": "stable-key" }"#;
        let input = parse_lterm_input(raw);
        assert_eq!(input.session_id.as_deref(), Some("stable-key"));
    }

    /// session_key 합성 시 한쪽만 있으면 있는 쪽만 사용해 무의미한 슬래시를 만들지 않는다.
    #[test]
    fn lterm_session_key_synthesis_partial() {
        let only_session = parse_lterm_input(r#"{ "session": "codex" }"#);
        assert_eq!(only_session.session_id.as_deref(), Some("codex"));
        let only_pane = parse_lterm_input(r#"{ "pane": "%2" }"#);
        assert_eq!(only_pane.session_id.as_deref(), Some("%2"));
    }

    /// 빈 session_key 문자열은 무시하고 session/pane으로 합성해야 한다.
    #[test]
    fn lterm_empty_session_key_falls_back_to_synthesis() {
        let raw = r#"{ "session": "s", "pane": "%1", "session_key": "" }"#;
        let input = parse_lterm_input(raw);
        assert_eq!(input.session_id.as_deref(), Some("s/%1"));
    }

    /// 깨진 JSON은 패닉 없이 전부 None으로 저하해야 한다(LENIENT).
    #[test]
    fn lterm_broken_json_returns_default() {
        for raw in ["", "   ", "not json", "{ \"session\": ", "[1,2,3]"] {
            let input = parse_lterm_input(raw);
            assert_eq!(input, ClaudeInput::default(), "입력: {raw:?}");
        }
    }

    /// version은 읽되 분기 없이 무시한다(forward-compat): version 유무로 결과가 달라지지 않아야 한다.
    #[test]
    fn lterm_version_is_ignored() {
        let with_version = parse_lterm_input(r#"{ "session": "s", "pane": "%1", "version": 99 }"#);
        let without_version = parse_lterm_input(r#"{ "session": "s", "pane": "%1" }"#);
        assert_eq!(with_version, without_version);
    }

    /// 미소비/forward-compat 필드(version/cols/rows)가 타입 드리프트(문자열 등)해도
    /// 전체 파싱이 실패하지 않고 session/pane/agent/cwd 등 useful 필드는 보존되어야 한다(무패닉).
    /// 과거: 이 필드들이 strict Option<u32>라 "version":"1" 등이 오면 from_str이 전체 실패해
    /// default로 저하되며 정상 필드까지 소실됐다.
    #[test]
    fn lterm_ignored_field_type_drift_preserves_useful_fields() {
        let raw = r#"{
            "session": "codex",
            "pane": "%3",
            "agent": "codex",
            "cwd": "/Users/me/dev/app",
            "version": "1",
            "cols": "120",
            "rows": "40"
        }"#;
        let input = parse_lterm_input(raw);
        // 타입 드리프트한 ignored 필드가 있어도 useful 필드가 살아남아야 한다.
        assert_eq!(input.session_id.as_deref(), Some("codex/%3"));
        assert_eq!(input.model_display_name.as_deref(), Some("codex"));
        assert_eq!(input.cwd.as_deref(), Some("/Users/me/dev/app"));
        assert_eq!(input.git_branch, None);
    }
}
