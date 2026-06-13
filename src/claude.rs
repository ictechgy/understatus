//! Claude Code stdin JSON 파서 (P1: Claude Code 전용, 하드코딩).
//!
//! 계획서 §G의 실제 stdin JSON 스키마를 누락/`null` 안전하게 파싱한다.
//! 모든 필드는 `Option`이며 파싱 자체가 실패해도 절대 패닉하지 않고
//! 전부 `None`인 빈 `ClaudeInput`으로 안전 저하한다(lenient).

use crate::codex::CodexExtras;
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
    /// 토큰 기반 컨텍스트 사용률% fallback(`current_usage` 토큰합/`context_window_size`,
    /// 없으면 `total_input_tokens`/size). Claude Code가 `used_percentage`를 일시적으로 누락하는
    /// 프레임에서도 ctx가 사라지지 않도록 두는 대체값이다. native(`used_percentage`)가 항상 우선하며,
    /// 실제 표시값 해석은 [`resolve_context_percent`]가 담당한다. lterm/codex 경로는 `None`.
    pub context_fallback_percentage: Option<f64>,
    /// 현재 작업 디렉터리 (`cwd` 또는 `workspace.current_dir`).
    pub cwd: Option<String>,
    /// `workspace.git_worktree`/`workspace.repo`에서 파생한 git 브랜치명.
    pub git_branch: Option<String>,
    /// 누적 비용 USD (`cost.total_cost_usd`). 라인에 표시.
    pub cost_usd: Option<f64>,
    /// 세션 식별자 (`session_id`).
    pub session_id: Option<String>,
    /// lterm 세션/페인 표시용(예 "codex/%3"). lterm 소스 전용, Claude 경로는 None.
    pub session_label: Option<String>,
    /// Codex 세션 심층판독으로 enrich된 추가 필드(5h/주간 한도·plan·effort). lterm/codex 소스 전용.
    /// Claude 경로는 항상 `None`(비트 동일 보장, spec §6). `crate::codex::maybe_enrich`가 채운다.
    pub codex: Option<CodexExtras>,
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
    // context_window에서 native(used_percentage)와 토큰 기반 fallback을 함께 도출한다.
    // Claude Code가 used_percentage를 간헐적으로 누락하는 프레임에서도 ctx를 추정할 수 있도록
    // fallback을 준비한다(표시 우선순위는 resolve_context_percent가 결정).
    let (context_used_percentage, context_fallback_percentage) = match raw_input.context_window {
        Some(window) => (window.used_percentage, compute_context_fallback(&window)),
        None => (None, None),
    };
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
        context_fallback_percentage,
        cwd: raw_input.cwd.or(cwd_from_workspace),
        git_branch,
        cost_usd,
        session_id: raw_input.session_id,
        // Claude 경로는 세션/페인 표시 라벨이 없다(lterm 소스 전용).
        session_label: None,
        // Claude 경로는 Codex enrich 대상이 아니다(비트 동일 보장, spec §6).
        codex: None,
    }
}

/// `context_window`의 토큰 정보로 컨텍스트 사용률% fallback을 계산한다(순수, 부재 안전).
///
/// Claude Code가 `used_percentage`를 일시적으로 누락하는 프레임에서도 ctx를 추정하기 위해,
/// omc HUD와 동일한 우선순위로 토큰 기반 비율을 산출한다:
///   1) `current_usage` 토큰합 / `context_window_size`
///   2) `total_input_tokens` / `context_window_size`
///
/// # 반환
/// 분모(창 크기)와 분자(토큰)가 모두 양수일 때만 `Some(0..=100)`. 크기 부재/0, 토큰 0/부재면
/// `None`을 반환해 호출부가 ctx 세그먼트를 생략(또는 직전 native 유지)하게 한다.
fn compute_context_fallback(window: &RawContextWindow) -> Option<f64> {
    let size = window.context_window_size?;
    if size <= 0.0 {
        return None;
    }
    // 1) current_usage 토큰합(입력 + 캐시 생성 + 캐시 읽기) 기준.
    let current_tokens = window
        .current_usage
        .as_ref()
        .map(RawCurrentUsage::total_tokens)
        .unwrap_or(0.0);
    if current_tokens > 0.0 {
        return Some(percent_of(current_tokens, size));
    }
    // 2) total_input_tokens 기준(네이티브 사용률을 0으로 보고하는 호환 프로바이더 대비).
    let total_input = window.total_input_tokens.unwrap_or(0.0);
    if total_input > 0.0 {
        return Some(percent_of(total_input, size));
    }
    None
}

/// 토큰 수를 창 크기 대비 백분율(0..=100, 정수 반올림)로 환산한다(순수).
///
/// `size`는 호출부에서 이미 양수임을 보장한다(0 분모 진입 불가). 결과는 표시 안정성을 위해
/// 정수로 반올림하고 0..=100으로 클램프한다(omc HUD `Math.min(100, Math.round(...))`와 동형).
fn percent_of(tokens: f64, size: f64) -> f64 {
    ((tokens / size) * 100.0).round().clamp(0.0, 100.0)
}

/// 표시·영속용 백분율을 0..=100으로 클램프한다(순수).
///
/// native(`used_percentage`)는 상류 값이라 이론상 0..100을 벗어날 수 있다. 토큰 fallback
/// ([`percent_of`])과 동일하게 클램프해 표시 일관성을 맞추고, 비정상 상한값(예: 120)이 세션
/// 캐시로 영속·전파되는 것을 막는다. 비유한 입력은 호출부에서 미리 차단한다.
fn clamp_percent(percent: f64) -> f64 {
    percent.clamp(0.0, 100.0)
}

/// 컨텍스트 사용률% 해석 결과: 이번 프레임에 표시할 값과, 양수 native를 본 경우 영속화할 값.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ContextResolution {
    /// ctx 세그먼트로 표시할 값. `None`이면 세그먼트를 생략한다.
    pub display: Option<f64>,
    /// 양수 native(`used_percentage` > 0)를 본 경우 세션 캐시에 기록할 값. `None`이면 기록하지 않는다.
    pub persist_native: Option<f64>,
}

/// native·토큰 fallback·직전 native(hold)로부터 표시할 ctx%를 해석한다(순수, I/O 없음).
///
/// Claude Code는 `used_percentage`를 간헐적으로 누락하는데, 그 프레임에서 토큰 기반 fallback으로
/// 곧바로 전환하면 분모가 달라 값이 튄다(관측된 예: 86 ↔ 98). 이 튐은 실제 컨텍스트 증가가 아니라
/// 분모 불일치로 인한 체계적 노이즈이므로, native가 일시 누락된 동안에는 직전 native를 유지한다:
///   1) 양수 native가 있으면 그것을 표시하고 영속화한다(권위값, 0..=100 클램프).
///   2) native가 없고 TTL 내 직전 native(`held_native`)가 있으면 그 값을 유지한다(상승 노이즈 차단).
///      단, 토큰 fallback이 직전 native보다 `drop_tolerance`(config `[context].drop_tolerance`) 이상
///      *낮으면* 실제 감소(예: `/compact`)로 보고 유지를 깨 아래 3)에서 fallback을 반영한다.
///   3) 유지 안 함 → 토큰 fallback, 없으면 유한한 raw native(예: 0), 끝으로 `None`(생략).
///
/// 비대칭 가드 주의: omc HUD는 대칭 tolerance(`|fallback-native| > 3`)로 전환해 86↔98 *상승*
/// 노이즈에도 튀었다. 여기선 하강 방향만 통과시켜(토큰 fallback이 native 대비 과대추정이므로
/// fallback이 held보다 낮다는 건 노이즈가 아니라 실제 감소 신호) 그 회귀를 피하면서 급감은 따라간다.
///
/// # 인자
/// - `native`: 이번 프레임의 `used_percentage`(부재/0/NaN 가능).
/// - `fallback`: 이번 프레임의 토큰 기반 추정치([`compute_context_fallback`], 부재/양수).
/// - `held_native`: TTL 내 직전 양수 native(호출부가 세션 캐시에서 읽어 주입; 없으면 `None`).
/// - `drop_tolerance`: hold를 깨는 하강 임계치(%포인트, config `[context].drop_tolerance`).
///   fallback이 직전 native보다 이만큼 이상 낮으면 실제 감소로 보고 유지를 깬다.
///
/// 입력 방어: 본 함수는 `pub`이라 직접 호출자도 임의값을 넘길 수 있고, `held_native`는 변조 가능한
/// 캐시에서 올 수 있다. 따라서 세 입력 모두 표시/유지 전에 유한·`0..=100` 경계로 정규화한다
/// (native·held는 양수만 인정, 비유한·범위초과는 그 경로를 건너뛴다).
pub fn resolve_context_percent(
    native: Option<f64>,
    fallback: Option<f64>,
    held_native: Option<f64>,
    drop_tolerance: f64,
) -> ContextResolution {
    // fallback을 함수 진입 시 1회 정규화해 hold 해제 판정(2)과 표시(3)가 같은 값을 쓰게 한다.
    // 비유한/음수(직접 호출자의 잘못된 입력)는 제거해 hold를 잘못 깨지 않도록 하고, 범위는 0..=100으로
    // 클램프한다. 0%는 cold-start 빈 컨텍스트의 정당한 값이라 보존한다(실제 파이프라인의 fallback은
    // 항상 0..=100 양수라 무영향, 본 정규화는 pub-API 방어용).
    let fallback = fallback
        .filter(|p| p.is_finite() && *p >= 0.0)
        .map(clamp_percent);

    // 1) 양수 native 우선(권위값) — 표시 + 영속화. NaN/음수/0은 native로 인정하지 않는다.
    //    표시·영속 전 0..=100 클램프로 fallback과 일관성을 맞추고 비정상값의 캐시 전파를 막는다.
    if let Some(positive) = native.filter(|p| p.is_finite() && *p > 0.0) {
        let clamped = clamp_percent(positive);
        return ContextResolution {
            display: Some(clamped),
            persist_native: Some(clamped),
        };
    }
    // 2) native 부재/0 → TTL 내 직전 native 유지(상승 방향 분모 노이즈 차단). 재영속화하지 않아
    //    TTL 시계는 마지막 실제 native 시점부터 흐른다(누락이 TTL을 넘기면 자연히 fallback로 저하).
    //    단, 정규화된 fallback이 held보다 충분히 낮으면(실제 감소) 유지를 깨고 3)으로 떨어뜨린다.
    //    held는 변조 가능한 캐시 출처일 수 있으므로 유한·양수만 인정하고 0..=100으로 클램프한다.
    if let Some(held) = held_native
        .filter(|p| p.is_finite() && *p > 0.0)
        .map(clamp_percent)
    {
        let real_drop = fallback.is_some_and(|fb| fb <= held - drop_tolerance);
        if !real_drop {
            return ContextResolution {
                display: Some(held),
                persist_native: None,
            };
        }
    }
    // 3) cold-start 또는 실제 감소 감지: 정규화된 토큰 fallback, 없으면 유한한 raw native(0 등,
    //    클램프), 끝으로 생략.
    ContextResolution {
        display: fallback.or_else(|| native.filter(|p| p.is_finite()).map(clamp_percent)),
        persist_native: None,
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
/// - `cwd`는 **표시용 + git 도출용**으로 매핑한다. `$PWD` 폴백은 추가하지 않는다(spec §4.1/§6.2).
/// - `git_branch`는 `cwd`(또는 그 조상)가 유효 git repo일 때만 채워진다(조건부 — 절대 비활성이 아님).
///   non-git cwd/detached HEAD/조상 `.git` 부재면 `None`. canonical cwd 조상의 첫 `.git` 엔트리까지
///   walk-up해 repo 루트 branch를 도출한다(W-A v2, [`derive_git_branch_from_cwd`]).
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

    // 세션/페인 표시 라벨("session/pane"/"session"/"pane"/None)을 미리 합성해 둔다.
    // session_key 합성과 동일 규칙이므로 재사용해 synthesize_session_key 중복 호출을 없앤다.
    let session_label = synthesize_session_key(&raw_input.session, &raw_input.pane);

    // session_key는 명시값을 우선하고, 없으면 위에서 합성한 라벨을 재사용한다(캐시/펄스 격리).
    let session_key = raw_input
        .session_key
        .filter(|key| !key.is_empty())
        .or_else(|| session_label.clone());

    // cwd(또는 그 조상)에서 git 브랜치를 미리 도출한다(canonical 조상의 첫 .git까지 walk-up, W-A v2).
    // `as_deref()`는 불변 차용이라 이후 구조체의 `cwd: raw_input.cwd`(move)와 충돌하지 않는다.
    //
    // W-B(lterm payload에 `git_worktree` 필드 추가) 미채택 — lterm 레포 조사로 확정(블로커 해소):
    // lterm `Session.cwd`는 세션 시작 dir로 1회 고정되는 불변값이라 셸 `cd`를 추적하지 못한다
    // (비-Mutex String, create_session에서 1회 캡처 후 갱신 경로 부재 — OSC 7/proc cwd 폴링·cwd 갱신
    // RPC 전무). 게다가 lterm엔 git 인지 자체가 없다(git_worktree 미도출). 따라서 lterm이 줄 수 있는
    // git_worktree는 cwd와 동일한 부정확성을 공유하므로, false-positive를 빈 pill(관측 가능) 대신
    // 외부 repo의 틀린 branch(관측 불가)로 이전(displace)할 뿐이다. lterm이 proc_pidinfo(
    // PROC_PIDVNODEPATHINFO)로 live cwd를 실제 도출하도록 구현(lterm 측 별도 작업)하기 전까지는
    // cwd-only가 FP<FN 원칙상 최선. **후속 기여자는 RawLtermInput에 git_worktree를 추가하지 말 것.**
    let git_branch = raw_input
        .cwd
        .as_deref()
        .and_then(derive_git_branch_from_cwd);

    ClaudeInput {
        // 에이전트/모델 표시명: lterm payload의 `agent`를 모델 슬롯에 매핑(best-effort).
        model_display_name: raw_input.agent,
        context_used_percentage: None,
        // lterm 경로는 Claude context_window가 없다(ctx는 codex enrich 등 별도 경로).
        context_fallback_percentage: None,
        // cwd는 표시용 + git 도출용으로 사용한다($PWD 폴백 없음).
        cwd: raw_input.cwd,
        // cwd(또는 그 조상)가 유효 git repo일 때만 채워진다(조건부 — 위에서 walk-up 도출).
        git_branch,
        cost_usd: None,
        session_id: session_key,
        // lterm 세션/페인 표시 라벨(status row에 cwd 앞 표시용).
        session_label,
        // codex enrich는 호출부(main.rs)에서 Source::Lterm 한정으로 별도 수행한다(초기 None).
        codex: None,
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
    // 상대경로 거부: 상대 git_worktree/repo는 understatus 프로세스 cwd 기준으로 해석돼
    // 엉뚱한 repo의 branch를 도출하는 false-positive를 만든다(canonicalize가 cwd-상대 해석).
    // is_absolute 가드 자체는 cwd 경로(derive_git_branch_from_cwd)와 동일 철학이나, 후속 처리는 다르다
    // (이쪽=workspace 경로의 직접 .git read, 저쪽=cwd 조상 walk-up). is_absolute 가드만 양쪽 동기 유지.
    if !std::path::Path::new(base_path).is_absolute() {
        return None;
    }
    read_branch_from_git_dir(std::path::Path::new(base_path))
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

/// branch명 byte length 상한(방어적 상한, FP<FN).
///
/// loose ref는 파일시스템 경로 컴포넌트(`.git/refs/heads/<branch>`)로 저장되므로 정상 branch명은
/// APFS NAME_MAX(255)를 넘을 수 없어 256B를 절대 초과하지 않는다. 따라서 256B 초과는 정상 git이
/// 만들 수 없는 손상/조작된 `.git/HEAD` 신호이며, 표시를 거부해 false-positive(틀린 branch)보다
/// false-negative(빈 pill)를 택하는 방어적 상한이다(FP<FN).
const MAX_BRANCH_LEN: usize = 256;

/// walk-up 조상 순회 깊이 상한. 일반적 홈/프로젝트 중첩 기준 repo 조상 깊이는 수십 단계 이내라는
/// 경험적 추정에 근거해, 보수적으로 64를 상한으로 둔다(정상 회귀 0을 노리면서 mount loop·병적 깊이를
/// 잘라내는 방어적 상한, 초과=None, FP<FN. MAX_BRANCH_LEN과 동일 철학).
///
/// cap 의미: [`find_git_root_dir_capped`]의 `take(cap)`은 start 자신(ancestors index 0) +
/// (cap-1)개 조상을 방문 = start를 포함해 총 cap개 디렉터리를 본다. 따라서 64는 "ascent(상승) 횟수"가
/// 아니라 "방문 디렉터리 수"이다(start 1개 + 조상 63개).
const MAX_WALK_UP_DEPTH: usize = 64;

/// 주어진 git 작업트리 경로에서 `.git/HEAD`를 읽어 현재 브랜치명을 추출한다.
///
/// # 인자
/// - `base_path`: git 워크트리(또는 repo) 루트 경로(`&Path`). 비-UTF8 canonical 경로를
///   lossy 변환으로 다른 경로에 retarget하는 (이론적) FP를 피하기 위해 `&str`이 아닌
///   `&Path`로 받아 lossy 라운드트립을 제거한다.
///
/// # 반환
/// `ref: refs/heads/<branch>` 형식의 HEAD에서 추출한 `<branch>`. 다음 4원인 중 하나라도 해당하면
/// `None`을 반환한다(부재/실패에 안전 — 절대 패닉하지 않음):
/// 1. `.git/HEAD` 부재 또는 읽기 실패(canonicalize/read 실패).
/// 2. detached HEAD(`ref:` 접두 없이 SHA 직접 기록).
/// 3. 외부향 심볼릭 HEAD(canonicalize 결과가 `.git/HEAD`로 끝나지 않음 — 누출 방어 위반).
/// 4. branch명이 비었거나 제어문자를 포함하거나 [`MAX_BRANCH_LEN`]을 초과.
///
/// # 주의
/// - branch명은 제어문자 미포함만 허용한다(터미널/status 인젝션 방어). 신뢰 불가 `.git/HEAD`가
///   ESC/개행/CR 등 제어문자가 섞인 branch명을 담으면 oneline SGR/cmux pill로 그대로 렌더돼
///   인젝션이 되므로, 정상 git branch명이 절대 갖지 않는 제어문자를 source chokepoint에서 거부한다.
/// - 심볼릭 `.git` 추종은 의도된 표준 git 동작 — canonicalize 가드는 결과가 `.git/HEAD`로 끝나는지만
///   확인(외부향 누출 차단), 추종 자체는 허용한다.
/// - 동기 fs read는 의도적 — `<cwd>/.git/HEAD` 단일 소파일을 1회 read한다. 느린 네트워크 마운트
///   (NFS 등)에서 status 렌더가 블록될 수 있다(알려진 트레이드오프). 완화(timeout/캐시)는 future work.
fn read_branch_from_git_dir(base_path: &std::path::Path) -> Option<String> {
    use std::path::Path;
    // 표준 워크트리는 `<base>/.git/HEAD`(디렉터리). linked worktree(`git worktree add`)와 서브모듈은
    // `<base>/.git`가 `gitdir: <path>`를 담은 정규 파일이고 실제 HEAD는 그 gitdir(보통 main repo 하위
    // = `<base>` 밖)에 있다. 이 gitfile 추종은 v1 범위 밖 — `<base>/.git`가 정규 파일이라 그 하위
    // `<base>/.git/HEAD` 탐색이 불가(ENOTDIR)해 canonicalize가 Err를 내고 None으로 안전 저하한다
    // (의도된 false-negative=branch 미표시). gitdir 추종은 임의 위치 파일 read를 열어 공격면을
    // 키우므로(추종을 추가하면 gitfile 내용이 신뢰 불가 입력이 되어 공격면이 열리므로) 의도적으로 미지원.
    // 교차참조: walk-up([`find_git_root_dir`])이 그 위에서 첫 `.git` 엔트리(이 gitfile 포함) 정지로
    // linked worktree/서브모듈 경계를 강제하므로, 부모 main repo branch로 새어 올라가지 않는다.
    let head_path = base_path.join(".git").join("HEAD");
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
    // 인젝션 방어: 신뢰 불가 HEAD 내용에 제어문자(ESC/개행/CR/기타 C0·DEL)가 섞이면 거부한다.
    // 정상 git branch명은 제어문자를 절대 갖지 않으므로 정상 케이스 회귀는 0이다.
    // SECURITY: 256B 초과 branch명 = 손상/조작된 .git/HEAD 신호 → 표시 거부(FP<FN)
    if branch.is_empty() || branch.len() > MAX_BRANCH_LEN || branch.chars().any(char::is_control) {
        None
    } else {
        Some(branch.to_string())
    }
}

/// lterm payload의 `cwd`에서 현재 git 브랜치를 파생한다(FP-free walk-up, W-A v2).
///
/// # 인자
/// - `cwd`: lterm stdin payload의 `cwd`. 신뢰 불가 외부 입력 경계이므로 방어 검증을 거친다.
///
/// # 반환
/// canonical cwd 조상 중 첫 `.git` 엔트리를 가진 디렉터리의 `.git/HEAD`가 `ref: refs/heads/<branch>`이면
/// `Some("<branch>")`. traversal(`..`) cwd, 상대경로 cwd, detached HEAD, 어떤 조상에도 `.git` 부재,
/// 외부향 심볼릭 HEAD 등은 모두 `None`으로 안전 저하한다(패닉 금지).
///
/// # 주의
/// - **FP-free walk-up 허용(W-A v2)**: codex가 repo 하위 dir(예: `~/repo/src`)에서 시작해
///   `<cwd>/.git`가 없어도 부모로 올라가 repo 루트 branch를 도출한다. 세 불변식으로 false-positive를 막는다.
/// - 불변식① **canonicalize-once**: 루프 진입 전 cwd를 정확히 1회 canonicalize해 심볼릭 cwd/부모를
///   실경로로 치환한다 → 심볼릭이 가리키는 타 repo를 순회하지 않는다(threat#1 단일 방어선).
/// - 불변식② **첫 `.git` 정지**: [`find_git_root_dir`]이 `symlink_metadata`(lstat)로 첫 `.git` 엔트리
///   (파일/디렉터리/심볼릭 무차별) 존재만 보고 정지한다 → linked worktree 경계를 넘어 main repo branch를
///   표시하지 않는다. gitfile(`.git` 정규파일)은 여전히 미추종(첫 엔트리 정지 후 reader가 ENOTDIR→None으로
///   흡수, Task3 No-go 보존).
/// - 불변식③ **[`MAX_WALK_UP_DEPTH`] 상한**: 조상 순회를 64단계로 제한해 mount loop·병적 깊이를 차단한다.
/// - **후속 기여자 경고**: "부모 상승 금지"가 아니라 위 세 불변식(canonicalize-first 1회 / 첫 `.git`
///   정지 / depth cap)을 **제거하지 말 것**. 특히 ⓐ ancestors 루프 내 재canonicalize 금지(부모 심볼릭
///   재해소로 symlink FP가 부활한다), ⓑ 정지 분기에 `symlink_metadata().is_dir()`/`.is_file()` 사용 금지
///   (심볼릭 `.git` 디렉터리가 둘 다 false라 표준 git 추종이 깨진다 — 무조건 그 dir에 위임).
/// - canonicalize가 심볼릭을 해소하므로 사용자가 '심볼릭 경로로 의도한' repo와 다른 실체 repo의 branch를
///   볼 수 있으나, 이는 FP가 아니라 실제 디스크에 존재하는 repo의 정탐(git-consistent)이다.
/// - 외부 입력 cwd traversal 방어는 [`is_safe_base_path`], 심볼릭 HEAD 방어는 [`read_branch_from_git_dir`]의
///   canonicalize 가드가 담당한다(기존 Claude 경로와 동일 검증 재사용).
/// - 상대경로 cwd는 거부한다(프로세스 cwd 기준 false-positive 방지). `"."`/`"repo"`처럼 `..`가
///   없어 traversal 검사를 통과해도, understatus 프로세스 cwd 기준으로 해석돼 엉뚱한 위치의
///   branch를 도출하므로 절대경로만 허용한다.
fn derive_git_branch_from_cwd(cwd: &str) -> Option<String> {
    // traversal 차단: `..`가 섞인 cwd는 임의 위치 `.git/HEAD` 탐색을 노릴 수 있어 거부한다(기존 가드).
    if !is_safe_base_path(cwd) {
        return None;
    }
    // 상대경로 거부: 프로세스 cwd 기준 해석으로 엉뚱한 repo branch를 도출하는 false-positive를 막는다.
    if !std::path::Path::new(cwd).is_absolute() {
        return None;
    }
    // ★ 불변식①: canonicalize 정확히 1회, walk-up 루프 진입 전. 심볼릭 cwd/부모를 실경로로 치환한다.
    let start = std::fs::canonicalize(cwd).ok()?;
    find_git_root_dir(&start).and_then(|root| read_branch_from_git_dir(&root))
}

/// canonical 조상을 [`MAX_WALK_UP_DEPTH`] 내 순회하며 첫 `.git` 엔트리를 가진 디렉터리를 반환한다.
///
/// # 인자
/// - `start`: **canonicalize된 절대 실경로**(불변식 — 호출부가 루프 진입 전 1회 canonicalize). 루프
///   내부에서 재canonicalize하지 않으므로 부모 심볼릭 재해소로 인한 symlink false-positive가 부활하지 않는다.
///
/// # 반환
/// 첫 `.git` 엔트리가 존재하는 조상 디렉터리의 [`PathBuf`]. 깊이 상한 내 어떤 조상에도 `.git`이 없으면 `None`.
///
/// # 주의
/// - `symlink_metadata`(lstat)로 `.git` 엔트리 **존재만** 판정한다(추종/읽기 아님). dir/file/심볼릭을
///   구분하지 않고 첫 엔트리에서 정지한 뒤 그 디렉터리를 무조건 반환한다(분기 없음). `is_dir()`/`is_file()`
///   분기를 쓰면 심볼릭 `.git` 디렉터리가 둘 다 false라 표준 git 추종이 깨지므로 절대 사용하지 않는다.
/// - `ancestors()`는 루트에서 종료하고 `take(MAX_WALK_UP_DEPTH)`로 깊이가 유한하므로 무한 루프/패닉이 없다.
fn find_git_root_dir(start: &std::path::Path) -> Option<std::path::PathBuf> {
    find_git_root_dir_capped(start, MAX_WALK_UP_DEPTH)
}

/// [`find_git_root_dir`]의 깊이 상한 주입 변형(테스트 전용 cap 검증용 — prod는 항상 [`MAX_WALK_UP_DEPTH`]).
///
/// 65단계 디렉터리 생성 비용 없이 depth cap 동작을 검증하기 위해 cap을 인자로 분리했다(prod 공개 API
/// 오염 없음). 불변식·정지 규칙은 [`find_git_root_dir`]과 동일하다.
fn find_git_root_dir_capped(start: &std::path::Path, cap: usize) -> Option<std::path::PathBuf> {
    for dir in start.ancestors().take(cap) {
        // 불변식②: lstat로 첫 `.git` 엔트리 존재만 판정 → 무조건 이 dir 반환(파일/디렉터리/심볼릭 무차별).
        if std::fs::symlink_metadata(dir.join(".git")).is_ok() {
            return Some(dir.to_path_buf());
        }
    }
    None
}

/// Claude Code stdin JSON의 중첩 구조를 그대로 받는 내부 역직렬화 타입.
///
/// `#[serde(default)]`로 누락 필드를 안전 처리하고, 각 중첩 객체도 `Option`으로 둬
/// `null`/부재에 견딘다. [`parse_claude_input`]이 이 타입을 [`ClaudeInput`]으로 평탄화한다.
#[derive(Debug, Deserialize, Default)]
struct RawClaudeInput {
    // 표시/캐시키용 최상위 String 필드도 lenient로 받는다(`workspace.repo`처럼 Claude Code가 향후
    // 객체화해도 전체 파싱이 깨지지 않도록 — repo 회귀의 일반화 방어).
    #[serde(default, deserialize_with = "deserialize_lenient_string")]
    session_id: Option<String>,
    #[serde(default, deserialize_with = "deserialize_lenient_string")]
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
    #[serde(default, deserialize_with = "deserialize_lenient_string")]
    display_name: Option<String>,
    // 스키마 완전성을 위해 역직렬화하지만 라인 렌더에는 쓰지 않는다(§G).
    #[serde(default, deserialize_with = "deserialize_lenient_string")]
    #[allow(dead_code)]
    id: Option<String>,
}

/// `workspace` 중첩 객체. git 브랜치 파생 근거(`git_worktree`/`repo`)를 포함.
#[derive(Debug, Deserialize, Default)]
struct RawWorkspace {
    #[serde(default, deserialize_with = "deserialize_lenient_string")]
    current_dir: Option<String>,
    // 스키마 완전성을 위해 역직렬화하지만 라인 렌더에는 쓰지 않는다(§G).
    #[serde(default, deserialize_with = "deserialize_lenient_string")]
    #[allow(dead_code)]
    project_dir: Option<String>,
    #[serde(default, deserialize_with = "deserialize_lenient_string")]
    git_worktree: Option<String>,
    // `repo`는 Claude Code가 문자열→`{host,owner,name}` 객체로 바꿨다. lenient로 받아 객체면 `None`
    // (git 도출은 git_worktree 우선이라 자연 폴백)으로 흡수해 전체 파싱 실패를 막는다.
    #[serde(default, deserialize_with = "deserialize_lenient_string")]
    repo: Option<String>,
}

/// `cost` 중첩 객체.
#[derive(Debug, Deserialize, Default)]
struct RawCost {
    #[serde(default)]
    total_cost_usd: Option<f64>,
}

/// 숫자 자리에 문자열 등 다른 타입이 와도 전체 파싱을 깨지 않고 `None`으로 흡수하는 lenient f64
/// 역직렬화기(serde `deserialize_with`용).
///
/// [`parse_claude_input`]은 serde 에러 시 전체를 빈 `ClaudeInput`으로 저하하므로, 한 필드의 타입
/// 드리프트(예: 토큰 수가 문자열로 옴)가 model/cwd/cost 등 무관 세그먼트까지 함께 날리는 것을
/// 막는다. 어떤 JSON 값이든 [`serde_json::Value`]로 받아 숫자일 때만 `f64`를 추출한다
/// (문자열/배열/객체/불리언/null → `None`). lterm 경로의 forward-compat `Value` 수용과 같은 정신.
fn deserialize_lenient_f64<'de, D>(deserializer: D) -> Result<Option<f64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Option::<serde_json::Value>::deserialize(deserializer)?.and_then(|value| value.as_f64()))
}

/// 문자열 자리에 객체/숫자 등 다른 타입이 와도 전체 파싱을 깨지 않고 `None`으로 흡수하는 lenient
/// String 역직렬화기(serde `deserialize_with`용).
///
/// 실제 사례: Claude Code가 `workspace.repo`를 문자열에서 `{host, owner, name}` **객체**로 바꾸자,
/// `Option<String>` strict 역직렬화가 이를 거부해 `RawClaudeInput` **전체 파싱이 실패**하고
/// model/ctx/cost/git 세그먼트가 통째로 사라졌다([`parse_claude_input`]의 전부-None 저하). 표시용
/// String 필드를 이 헬퍼로 받으면, 어떤 JSON 값이 와도 문자열일 때만 추출하고 그 외(객체/숫자/배열/
/// 불리언/null)는 `None`으로 흡수해 무관 세그먼트를 보존한다([`deserialize_lenient_f64`]와 같은 정신).
fn deserialize_lenient_string<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Option::<serde_json::Value>::deserialize(deserializer)?
        .and_then(|value| value.as_str().map(str::to_string)))
}

/// `current_usage` 객체가 통째로 다른 타입(예: 문자열)으로 와도 전체 파싱을 깨지 않게 흡수하는
/// lenient 역직렬화기. 객체면 [`RawCurrentUsage`]로 best-effort 변환하고, 아니면 `None`.
fn deserialize_lenient_current_usage<'de, D>(
    deserializer: D,
) -> Result<Option<RawCurrentUsage>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Option::<serde_json::Value>::deserialize(deserializer)?
        .and_then(|value| serde_json::from_value(value).ok()))
}

/// `context_window` 중첩 객체. `used_percentage`는 `null` 가능.
///
/// `used_percentage`가 권위값이지만 Claude Code가 간헐적으로 누락하므로, 토큰 기반 fallback
/// 산출에 필요한 `context_window_size`/`total_input_tokens`/`current_usage`도 함께 받는다
/// (전부 부재/`null` 안전, lenient). 모든 수치 필드는 [`deserialize_lenient_f64`]로 받아, 한 필드의
/// 타입 드리프트가 statusline 전체를 무력화하지 않게 격리한다(`parse_claude_input`의 전부-None 저하
/// 차단). 토큰 수는 float 인코딩도 견디도록 `f64`로 받는다.
#[derive(Debug, Deserialize, Default)]
struct RawContextWindow {
    #[serde(default, deserialize_with = "deserialize_lenient_f64")]
    used_percentage: Option<f64>,
    #[serde(default, deserialize_with = "deserialize_lenient_f64")]
    context_window_size: Option<f64>,
    #[serde(default, deserialize_with = "deserialize_lenient_f64")]
    total_input_tokens: Option<f64>,
    #[serde(default, deserialize_with = "deserialize_lenient_current_usage")]
    current_usage: Option<RawCurrentUsage>,
}

/// `context_window.current_usage` 토큰 분해(입력 + 캐시 생성 + 캐시 읽기).
///
/// 컨텍스트를 점유하는 토큰합을 토큰 기반 ctx fallback 분자로 쓴다(omc HUD와 동형). 모든 필드는
/// 부재/`null`/타입 드리프트 안전([`deserialize_lenient_f64`])하며, 누락 필드는 0으로 본다.
#[derive(Debug, Deserialize, Default)]
struct RawCurrentUsage {
    #[serde(default, deserialize_with = "deserialize_lenient_f64")]
    input_tokens: Option<f64>,
    #[serde(default, deserialize_with = "deserialize_lenient_f64")]
    cache_creation_input_tokens: Option<f64>,
    #[serde(default, deserialize_with = "deserialize_lenient_f64")]
    cache_read_input_tokens: Option<f64>,
}

impl RawCurrentUsage {
    /// 컨텍스트 점유 토큰합(입력 + 캐시 생성 + 캐시 읽기). 부재 필드는 0으로 본다.
    fn total_tokens(&self) -> f64 {
        self.input_tokens.unwrap_or(0.0)
            + self.cache_creation_input_tokens.unwrap_or(0.0)
            + self.cache_read_input_tokens.unwrap_or(0.0)
    }
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

    /// 테스트용 고유 임시 디렉터리 경로 + RAII 정리 가드.
    /// Drop에서 remove_dir_all로 정리해 패닉(단언 실패) 시에도 누수 0.
    /// 주의: unwind 전제 — `[profile.test] panic="abort"` 도입 시 Drop 미실행으로 무효(현재 Cargo.toml은 unwind 기본).
    struct TestDir(std::path::PathBuf);
    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    impl std::ops::Deref for TestDir {
        type Target = std::path::Path;
        fn deref(&self) -> &std::path::Path {
            &self.0
        }
    }
    impl AsRef<std::path::Path> for TestDir {
        fn as_ref(&self) -> &std::path::Path {
            &self.0
        }
    }

    /// 호출마다 고유한 비존재 절대 temp 경로를 RAII 가드로 감싸 반환한다(테스트 격리).
    ///
    /// # 인자
    /// - `label`: 경로를 사람이 식별하기 위한 라벨(테스트 의도 표시).
    ///
    /// # 반환
    /// `<temp_dir>/understatus-<label>-<pid>-<seq>` 형태의 고유 절대 경로(미생성)를 담은 [`TestDir`].
    /// `Deref<Target=Path>`로 `PathBuf`처럼 `.join()`/`.to_string_lossy()` 등을 그대로 쓸 수 있다.
    ///
    /// # 주의
    /// `process::id()` 단독은 같은 프로세스 내 스레드 병렬 실행 시 prefix가 같으면 충돌·stale
    /// 누수 위험이 있다. `AtomicU64` 정적 카운터를 조합해 호출마다 고유 경로를 보장한다.
    /// 경로는 생성하지 않으므로, `.git` 없는 비존재 경로가 필요한 negative 테스트에도 그대로 쓴다.
    /// 반환된 [`TestDir`]가 drop될 때 디렉터리를 정리하므로 패닉(단언 실패) 시에도 누수가 없다.
    fn unique_test_dir(label: &str) -> TestDir {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
        TestDir(
            std::env::temp_dir().join(format!("understatus-{label}-{}-{seq}", std::process::id())),
        )
    }

    /// AC-d1: [`TestDir`] 가드가 panic 언와인딩 중에도 디렉터리를 정리함을 증명한다.
    /// 클로저 안에서 디렉터리를 생성하고 경로를 외부 변수로 캡처한 뒤 의도적으로 panic을 일으키고,
    /// `catch_unwind` 복귀 후 그 경로가 부재함(=Drop이 cleanup 실행)을 단언한다.
    #[test]
    fn test_dir_guard_cleans_up_on_panic() {
        use std::sync::Mutex;
        // 패닉 클로저 밖으로 경로를 빼기 위한 캡처 변수(Mutex로 AssertUnwindSafe 충족).
        let captured_path: Mutex<Option<std::path::PathBuf>> = Mutex::new(None);

        // 의도적 panic을 catch_unwind로 감싼다. 클로저 안에서 가드 생성 + 디렉터리 실생성 후 panic.
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let tmp = unique_test_dir("panic-cleanup");
            std::fs::create_dir_all(&*tmp).expect("임시 디렉터리 생성 실패");
            assert!(tmp.exists(), "panic 전엔 디렉터리가 존재해야 한다");
            // 사후 검사를 위해 경로를 클로저 밖으로 캡처한다.
            *captured_path.lock().expect("락 획득 실패") = Some(tmp.to_path_buf());
            // 의도적 panic: 언와인딩이 시작되며 `tmp`(TestDir)의 Drop이 실행돼야 한다.
            panic!("의도적 패닉 — 가드 cleanup 증명용");
        }));

        // catch_unwind는 panic을 잡아 Err를 반환해야 한다(테스트가 죽지 않음).
        assert!(
            result.is_err(),
            "catch_unwind가 의도적 panic을 포착해야 한다"
        );

        // 캡처한 경로가 부재함을 단언 → Drop이 언와인딩에서도 cleanup을 실행했음을 증명.
        let path = captured_path
            .lock()
            .expect("락 획득 실패")
            .take()
            .expect("패닉 전 경로가 캡처돼야 한다");
        assert!(
            !path.exists(),
            "panic 언와인딩 후 가드 Drop이 디렉터리를 정리해 부재해야 한다: {path:?}"
        );
    }

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

    // === 토큰 기반 ctx fallback(compute_context_fallback via parse_claude_input) ===

    /// used_percentage 누락 + current_usage/size 존재 → 토큰합 비율 fallback을 산출한다.
    /// (입력 100k + 캐시생성 20k + 캐시읽기 320k = 440k) / 1,000,000 = 44%.
    #[test]
    fn fallback_from_current_usage_when_native_absent() {
        let raw = r#"{ "context_window": {
            "context_window_size": 1000000,
            "current_usage": { "input_tokens": 100000, "cache_creation_input_tokens": 20000, "cache_read_input_tokens": 320000 }
        } }"#;
        let input = parse_claude_input(raw);
        assert_eq!(
            input.context_used_percentage, None,
            "native 누락 → context_used_percentage None"
        );
        assert_eq!(input.context_fallback_percentage, Some(44.0));
    }

    /// current_usage 부재/0 → total_input_tokens/size로 fallback. 450k/1,000,000 = 45%.
    #[test]
    fn fallback_from_total_input_when_current_usage_zero() {
        let raw = r#"{ "context_window": {
            "context_window_size": 1000000,
            "total_input_tokens": 450000,
            "current_usage": { "input_tokens": 0 }
        } }"#;
        let input = parse_claude_input(raw);
        assert_eq!(input.context_fallback_percentage, Some(45.0));
    }

    /// context_window_size 부재 → 분모를 모르므로 fallback None(분자만으로는 비율 불가).
    #[test]
    fn fallback_none_without_window_size() {
        let raw = r#"{ "context_window": { "current_usage": { "input_tokens": 500000 } } }"#;
        let input = parse_claude_input(raw);
        assert_eq!(input.context_fallback_percentage, None);
    }

    /// context_window_size 0/음수 → 0 분모 진입 차단, fallback None.
    #[test]
    fn fallback_none_with_nonpositive_size() {
        let zero = parse_claude_input(
            r#"{ "context_window": { "context_window_size": 0, "total_input_tokens": 100 } }"#,
        );
        assert_eq!(zero.context_fallback_percentage, None);
        let negative = parse_claude_input(
            r#"{ "context_window": { "context_window_size": -5, "total_input_tokens": 100 } }"#,
        );
        assert_eq!(negative.context_fallback_percentage, None);
    }

    /// 토큰이 전부 0/부재면 fallback None(0%는 표시하지 않고 생략/유지에 맡긴다).
    #[test]
    fn fallback_none_with_zero_tokens() {
        let raw = r#"{ "context_window": { "context_window_size": 1000000 } }"#;
        let input = parse_claude_input(raw);
        assert_eq!(input.context_fallback_percentage, None);
    }

    /// native와 fallback이 공존하면 둘 다 채워진다(표시 우선순위는 resolve_context_percent가 결정).
    #[test]
    fn native_and_fallback_both_populated() {
        let raw = r#"{ "context_window": {
            "used_percentage": 86.0,
            "context_window_size": 1000000,
            "current_usage": { "input_tokens": 980000 }
        } }"#;
        let input = parse_claude_input(raw);
        assert_eq!(input.context_used_percentage, Some(86.0));
        assert_eq!(input.context_fallback_percentage, Some(98.0));
    }

    /// 토큰합이 창 크기를 초과해도 100%로 클램프한다.
    #[test]
    fn fallback_clamps_to_100() {
        let raw = r#"{ "context_window": {
            "context_window_size": 1000,
            "current_usage": { "input_tokens": 5000 }
        } }"#;
        let input = parse_claude_input(raw);
        assert_eq!(input.context_fallback_percentage, Some(100.0));
    }

    /// percent_of: 반올림(33.4%→33, 33.6%→34)과 0..=100 클램프를 보장한다.
    #[test]
    fn percent_of_rounds_and_clamps() {
        assert_eq!(percent_of(334.0, 1000.0), 33.0);
        assert_eq!(percent_of(336.0, 1000.0), 34.0);
        assert_eq!(percent_of(2.0, 1.0), 100.0);
        assert_eq!(percent_of(0.0, 1000.0), 0.0);
    }

    // === ctx 표시값 해석(resolve_context_percent) ===

    /// 테스트 헬퍼: 프로덕션 기본 drop tolerance로 ctx 해석을 호출한다(기존 3-인자 호출 보존용).
    fn resolve_with_default_tolerance(
        native: Option<f64>,
        fallback: Option<f64>,
        held: Option<f64>,
    ) -> ContextResolution {
        resolve_context_percent(
            native,
            fallback,
            held,
            crate::config::DEFAULT_CONTEXT_DROP_TOLERANCE,
        )
    }

    /// 양수 native가 있으면 그것을 표시하고 영속화 신호를 낸다(권위값 우선).
    #[test]
    fn resolve_prefers_positive_native_and_persists() {
        let r = resolve_with_default_tolerance(Some(86.0), Some(98.0), Some(50.0));
        assert_eq!(r.display, Some(86.0));
        assert_eq!(r.persist_native, Some(86.0));
    }

    /// native 부재 + TTL 내 직전 native(hold) → 유지하고 영속화하지 않는다(튐 차단의 핵심).
    #[test]
    fn resolve_holds_previous_native_on_transient_gap() {
        // 토큰 fallback이 98로 갈렸어도 직전 native 86을 유지해야 한다.
        let r = resolve_with_default_tolerance(None, Some(98.0), Some(86.0));
        assert_eq!(r.display, Some(86.0), "직전 native 유지로 86↔98 튐 차단");
        assert_eq!(r.persist_native, None, "유지 프레임은 재영속화하지 않음");
    }

    /// native·hold 모두 없으면 토큰 fallback을 표시한다(cold-start/비-native 프로바이더).
    #[test]
    fn resolve_uses_fallback_when_no_native_and_no_hold() {
        let r = resolve_with_default_tolerance(None, Some(45.0), None);
        assert_eq!(r.display, Some(45.0));
        assert_eq!(r.persist_native, None);
    }

    /// 표시할 근거가 전혀 없으면 None(세그먼트 생략, AC2 보존).
    #[test]
    fn resolve_yields_none_when_nothing_available() {
        let r = resolve_with_default_tolerance(None, None, None);
        assert_eq!(r.display, None);
        assert_eq!(r.persist_native, None);
    }

    /// native 0은 양수가 아니므로 hold 없을 때 토큰 fallback이 우선한다(스푸리어스 0% 회피).
    #[test]
    fn resolve_zero_native_defers_to_fallback() {
        let r = resolve_with_default_tolerance(Some(0.0), Some(45.0), None);
        assert_eq!(r.display, Some(45.0));
        assert_eq!(r.persist_native, None);
    }

    /// native 0 + fallback/hold 모두 없으면 마지막 수단으로 raw native(0%)를 표시한다.
    #[test]
    fn resolve_zero_native_shown_as_last_resort() {
        let r = resolve_with_default_tolerance(Some(0.0), None, None);
        assert_eq!(r.display, Some(0.0));
        assert_eq!(r.persist_native, None);
    }

    /// NaN native는 양수로 인정하지 않으며, 표시 후보에서도 제외한다(비유한 방어).
    #[test]
    fn resolve_rejects_nonfinite_native() {
        let r = resolve_with_default_tolerance(Some(f64::NAN), None, Some(70.0));
        assert_eq!(r.display, Some(70.0), "NaN native 무시 → hold 사용");
        assert_eq!(r.persist_native, None);
        let cold = resolve_with_default_tolerance(Some(f64::NAN), None, None);
        assert_eq!(cold.display, None, "NaN은 raw native 표시 후보에서도 제외");
    }

    /// 실제 급감(/compact): fallback이 held보다 tolerance 이상 낮으면 hold를 깨고 fallback 반영.
    #[test]
    fn resolve_breaks_hold_on_real_drop() {
        // held 86, /compact 후 토큰 fallback 20 → 86-12=74 이하이므로 유지를 깨고 20을 표시.
        let r = resolve_with_default_tolerance(None, Some(20.0), Some(86.0));
        assert_eq!(r.display, Some(20.0), "급감은 즉시 반영(stale-high 방지)");
        assert_eq!(r.persist_native, None, "토큰 fallback은 영속화하지 않음");
    }

    /// 작은 하강(tolerance 이내)은 노이즈로 보고 직전 native를 유지한다.
    #[test]
    fn resolve_holds_on_small_dip_within_tolerance() {
        // held 86, fallback 78 → 86-78=8 < 12 → 유지(상승 노이즈와 동급의 미세 하강은 흡수).
        let r = resolve_with_default_tolerance(None, Some(78.0), Some(86.0));
        assert_eq!(r.display, Some(86.0));
        assert_eq!(r.persist_native, None);
    }

    /// 하강 가드 경계: 정확히 held-tolerance면 깨고, 그보다 한 단계 위면 유지한다.
    #[test]
    fn resolve_drop_guard_boundary() {
        // 86-12=74: fallback 74는 '이하'라 깸, 75는 유지.
        assert_eq!(
            resolve_with_default_tolerance(None, Some(74.0), Some(86.0)).display,
            Some(74.0)
        );
        assert_eq!(
            resolve_with_default_tolerance(None, Some(75.0), Some(86.0)).display,
            Some(86.0)
        );
    }

    /// 토큰 fallback이 없으면(급감 판정 불가) 직전 native를 유지한다.
    #[test]
    fn resolve_holds_when_no_fallback_to_compare() {
        let r = resolve_with_default_tolerance(None, None, Some(86.0));
        assert_eq!(r.display, Some(86.0));
        assert_eq!(r.persist_native, None);
    }

    /// 비정상 상한 native(>100)는 표시·영속 전에 0..=100으로 클램프한다(캐시 전파 차단).
    #[test]
    fn resolve_clamps_out_of_range_native() {
        let r = resolve_with_default_tolerance(Some(150.0), None, None);
        assert_eq!(r.display, Some(100.0));
        assert_eq!(r.persist_native, Some(100.0), "클램프된 값만 영속화");
    }

    /// 음수 raw native(분기 3 마지막 수단)는 0%로 클램프되어 표시된다(하한 클램프 불변식 고정).
    #[test]
    fn resolve_clamps_negative_native_to_zero() {
        let r = resolve_with_default_tolerance(Some(-5.0), None, None);
        assert_eq!(r.display, Some(0.0));
        assert_eq!(r.persist_native, None);
    }

    // === 입력 방어: held/fallback 정규화(pub 함수·변조 가능 캐시 대비, quad-review 합의) ===

    /// held가 범위를 벗어나면(>100) 표시 전에 0..=100으로 클램프한다.
    #[test]
    fn resolve_clamps_out_of_range_held() {
        let r = resolve_with_default_tolerance(None, None, Some(150.0));
        assert_eq!(r.display, Some(100.0));
        assert_eq!(r.persist_native, None);
    }

    /// held가 비양수(≤0)/비유한이면 유지하지 않고 토큰 fallback으로 저하한다.
    #[test]
    fn resolve_rejects_nonpositive_or_nonfinite_held() {
        // held -5(손상 캐시) → 유지 안 함, fallback 45 표시.
        assert_eq!(
            resolve_with_default_tolerance(None, Some(45.0), Some(-5.0)).display,
            Some(45.0)
        );
        // held 0 → 유지 안 함, fallback도 없으면 None.
        assert_eq!(
            resolve_with_default_tolerance(None, None, Some(0.0)).display,
            None
        );
        // held NaN → 유지 안 함, fallback 30 표시.
        assert_eq!(
            resolve_with_default_tolerance(None, Some(30.0), Some(f64::NAN)).display,
            Some(30.0)
        );
    }

    /// fallback이 비유한이면(NaN/inf) 표시 후보에서 제외한다(분기 3 방어).
    #[test]
    fn resolve_rejects_nonfinite_fallback() {
        assert_eq!(
            resolve_with_default_tolerance(None, Some(f64::NAN), None).display,
            None
        );
        assert_eq!(
            resolve_with_default_tolerance(None, Some(f64::INFINITY), None).display,
            None
        );
    }

    /// fallback이 범위를 벗어나면(>100) 0..=100으로 클램프한다(직접 호출자 방어).
    #[test]
    fn resolve_clamps_out_of_range_fallback() {
        let r = resolve_with_default_tolerance(None, Some(150.0), None);
        assert_eq!(r.display, Some(100.0));
    }

    /// 음수/비유한 fallback(직접 호출자의 잘못된 입력)은 정규화로 제거되어 유효한 hold를 깨지 못한다.
    #[test]
    fn resolve_normalized_fallback_does_not_break_hold() {
        // fallback -5(잘못된 입력)는 정규화로 None이 되어 held 86을 깨지 않는다(폴리시: real_drop 전 정규화).
        let r = resolve_with_default_tolerance(None, Some(-5.0), Some(86.0));
        assert_eq!(r.display, Some(86.0), "음수 fallback은 hold를 깨지 못함");
        assert_eq!(r.persist_native, None);
        // 비유한 fallback도 동일.
        assert_eq!(
            resolve_with_default_tolerance(None, Some(f64::NAN), Some(86.0)).display,
            Some(86.0)
        );
    }

    /// 하강 가드는 *클램프된* held를 기준으로 비교한다(clamp-before-compare 순서 고정).
    #[test]
    fn resolve_drop_guard_uses_clamped_held() {
        // held 150 → 클램프 100. 임계는 100-12=88: fallback 89는 유지(100 표시), 87은 깸(87 표시).
        assert_eq!(
            resolve_with_default_tolerance(None, Some(89.0), Some(150.0)).display,
            Some(100.0),
            "89 > 88 → 클램프된 held(100) 유지",
        );
        assert_eq!(
            resolve_with_default_tolerance(None, Some(87.0), Some(150.0)).display,
            Some(87.0),
            "87 <= 88 → 유지를 깨고 fallback 표시",
        );
    }

    // === 타입 드리프트 leniency(신규 토큰 필드가 statusline 전체를 무력화하지 않음) ===

    /// 신규 토큰 필드가 문자열로 와도(타입 드리프트) 파싱이 통째로 깨지지 않고, 무관 필드는 보존된다.
    #[test]
    fn token_field_type_drift_preserves_other_fields() {
        let raw = r#"{
            "model": { "display_name": "Opus" },
            "context_window": { "context_window_size": 1000000, "used_percentage": 86.0, "total_input_tokens": "oops" }
        }"#;
        let input = parse_claude_input(raw);
        assert_eq!(
            input.model_display_name.as_deref(),
            Some("Opus"),
            "model 보존"
        );
        assert_eq!(
            input.context_used_percentage,
            Some(86.0),
            "used_percentage 보존"
        );
        // total_input_tokens가 문자열이라 fallback 분자로 못 쓰지만 패닉/전체 None 저하는 없다.
        assert_eq!(input.context_fallback_percentage, None);
    }

    /// current_usage 내부 토큰이 문자열이어도 흡수하고, 유효한 used_percentage는 보존한다.
    #[test]
    fn current_usage_token_drift_is_absorbed() {
        let raw = r#"{
            "model": { "display_name": "Opus" },
            "context_window": { "context_window_size": 1000000, "current_usage": { "input_tokens": "big" } }
        }"#;
        let input = parse_claude_input(raw);
        assert_eq!(input.model_display_name.as_deref(), Some("Opus"));
        assert_eq!(
            input.context_fallback_percentage, None,
            "문자열 토큰은 0 취급 → fallback 없음"
        );
    }

    /// current_usage 객체 자체가 다른 타입(문자열)으로 와도 전체 파싱이 깨지지 않는다.
    #[test]
    fn current_usage_wrong_object_type_is_absorbed() {
        let raw = r#"{
            "model": { "display_name": "Opus" },
            "context_window": { "context_window_size": 1000000, "total_input_tokens": 450000, "current_usage": "nope" }
        }"#;
        let input = parse_claude_input(raw);
        assert_eq!(input.model_display_name.as_deref(), Some("Opus"));
        // current_usage는 흡수(None), total_input_tokens fallback이 살아 45% 산출.
        assert_eq!(input.context_fallback_percentage, Some(45.0));
    }

    /// 권위 필드 used_percentage가 문자열로 드리프트해도 native만 None이 되고 무관 필드·fallback은 보존된다.
    #[test]
    fn used_percentage_drift_preserves_fallback_and_model() {
        let raw = r#"{
            "model": { "display_name": "Opus" },
            "context_window": { "used_percentage": "oops", "context_window_size": 1000000, "total_input_tokens": 450000 }
        }"#;
        let input = parse_claude_input(raw);
        assert_eq!(
            input.model_display_name.as_deref(),
            Some("Opus"),
            "model 보존"
        );
        assert_eq!(input.context_used_percentage, None, "문자열 native → None");
        assert_eq!(
            input.context_fallback_percentage,
            Some(45.0),
            "토큰 fallback 생존"
        );
    }

    /// 분모 context_window_size가 문자열로 드리프트하면 native는 보존되고 fallback은 분모 부재로 None.
    #[test]
    fn window_size_drift_preserves_native() {
        let raw = r#"{
            "context_window": { "used_percentage": 80.0, "context_window_size": "oops", "current_usage": { "input_tokens": 500000 } }
        }"#;
        let input = parse_claude_input(raw);
        assert_eq!(input.context_used_percentage, Some(80.0), "native 보존");
        assert_eq!(
            input.context_fallback_percentage, None,
            "분모 드리프트 → fallback 불가"
        );
    }

    /// 실제 회귀: Claude Code가 `workspace.repo`를 문자열→`{host,owner,name}` 객체로 바꿔도
    /// 전체 파싱이 안 깨지고 model/ctx/cwd가 보존된다(이 변경이 고치는 핵심 버그).
    #[test]
    fn workspace_repo_object_drift_preserves_all_segments() {
        let raw = r#"{
            "model": { "display_name": "Opus 4.8 (1M context)", "id": "claude-opus-4-8" },
            "cwd": "/Users/me/proj",
            "workspace": {
                "current_dir": "/Users/me/proj",
                "added_dirs": ["/a", "/b"],
                "repo": { "host": "github.com", "owner": "ictechgy", "name": "understatus" }
            },
            "cost": { "total_cost_usd": 33.9 },
            "context_window": { "context_window_size": 1000000, "used_percentage": 62 }
        }"#;
        let input = parse_claude_input(raw);
        assert_eq!(
            input.model_display_name.as_deref(),
            Some("Opus 4.8 (1M context)"),
            "model 보존(파싱 안 깨짐)"
        );
        assert_eq!(input.context_used_percentage, Some(62.0), "ctx 보존");
        assert_eq!(input.cwd.as_deref(), Some("/Users/me/proj"), "cwd 보존");
        assert_eq!(input.cost_usd, Some(33.9), "cost 보존");
        // repo가 객체라 git 도출 근거(경로)로 못 쓰지만 None으로 흡수 → 파싱은 정상.
        assert_eq!(input.git_branch, None);
    }

    /// model.display_name이 객체로 드리프트해도 흡수되고 다른 필드는 보존된다.
    #[test]
    fn model_display_name_object_drift_absorbed() {
        let raw = r#"{ "model": { "display_name": { "x": 1 } }, "context_window": { "used_percentage": 50 } }"#;
        let input = parse_claude_input(raw);
        assert_eq!(
            input.model_display_name, None,
            "객체 display_name → None 흡수"
        );
        assert_eq!(input.context_used_percentage, Some(50.0), "ctx 보존");
    }

    /// `workspace.repo`가 정상 문자열이면 git 도출 근거로 그대로 쓰인다(lenient가 기존 동작 보존).
    #[test]
    fn workspace_repo_string_still_used_for_git() {
        // repo가 문자열이면 derive_git_branch가 그 경로를 본다(존재 안 하면 None이지만 파싱은 정상).
        let raw = r#"{ "workspace": { "repo": "/nonexistent/repo/path" }, "context_window": { "used_percentage": 30 } }"#;
        let input = parse_claude_input(raw);
        assert_eq!(input.context_used_percentage, Some(30.0));
        assert_eq!(input.git_branch, None, "존재 않는 경로 → None(파싱은 정상)");
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
        let tmp = unique_test_dir("git-test");
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
    }

    /// repo가 객체(`{host,owner,name}`)로 드리프트해도 git_worktree가 정상이면 폴백 도출이 살아 있다.
    /// (repo lenient 흡수가 git_worktree 우선 폴백 체인을 깨지 않음을 직접 고정한다.)
    #[test]
    fn git_worktree_derives_branch_even_when_repo_is_object() {
        use std::io::Write;
        let tmp = unique_test_dir("git-repoobj");
        let git_dir = tmp.join(".git");
        std::fs::create_dir_all(&git_dir).expect("임시 .git 생성 실패");
        let mut file = std::fs::File::create(git_dir.join("HEAD")).expect("HEAD 생성 실패");
        writeln!(file, "ref: refs/heads/main").expect("HEAD 쓰기 실패");

        let raw = format!(
            r#"{{ "workspace": {{ "git_worktree": {:?}, "repo": {{ "host": "github.com", "owner": "x", "name": "y" }} }} }}"#,
            tmp.to_string_lossy()
        );
        let input = parse_claude_input(&raw);
        assert_eq!(
            input.git_branch.as_deref(),
            Some("main"),
            "repo 객체여도 git_worktree로 브랜치 도출"
        );
    }

    /// detached HEAD(직접 SHA)는 브랜치명이 없으므로 None이어야 한다.
    #[test]
    fn detached_head_yields_no_branch() {
        let tmp = unique_test_dir("git-detached");
        let git_dir = tmp.join(".git");
        std::fs::create_dir_all(&git_dir).expect("임시 .git 생성 실패");
        std::fs::write(git_dir.join("HEAD"), "0123456789abcdef\n").expect("HEAD 쓰기 실패");

        let raw = format!(
            r#"{{ "workspace": {{ "git_worktree": {:?} }} }}"#,
            tmp.to_string_lossy()
        );
        let input = parse_claude_input(&raw);
        assert_eq!(input.git_branch, None);
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

    // === derive_git_branch_from_cwd (cwd-only, AC1/AC2) ===

    /// 정상 git repo cwd(`.git/HEAD`=`ref: refs/heads/<b>`) → `Some("<b>")`(AC1).
    #[test]
    fn derive_from_cwd_ok_branch() {
        use std::io::Write;
        // 호출마다 고유 절대경로(AtomicU64 + pid)로 병렬 충돌·stale 누수를 차단한다.
        let tmp = unique_test_dir("lterm-git-ok");
        let git_dir = tmp.join(".git");
        std::fs::create_dir_all(&git_dir).expect("임시 .git 생성 실패");
        let mut file = std::fs::File::create(git_dir.join("HEAD")).expect("HEAD 생성 실패");
        writeln!(file, "ref: refs/heads/feature/x").expect("HEAD 쓰기 실패");

        let branch = derive_git_branch_from_cwd(&tmp.to_string_lossy());
        assert_eq!(branch.as_deref(), Some("feature/x"));
    }

    /// detached HEAD(직접 SHA)는 브랜치명이 없으므로 None이어야 한다(AC2).
    #[test]
    fn derive_from_cwd_detached_head_none() {
        let tmp = unique_test_dir("lterm-git-detached");
        let git_dir = tmp.join(".git");
        std::fs::create_dir_all(&git_dir).expect("임시 .git 생성 실패");
        std::fs::write(git_dir.join("HEAD"), "0123456789abcdef\n").expect("HEAD 쓰기 실패");

        assert_eq!(derive_git_branch_from_cwd(&tmp.to_string_lossy()), None);
    }

    /// `.git` 부재 cwd(존재하나 git이 아닌 디렉터리) → None(AC2).
    #[test]
    fn derive_from_cwd_no_git_dir_none() {
        let tmp = unique_test_dir("lterm-git-nogit");
        std::fs::create_dir_all(&*tmp).expect("임시 디렉터리 생성 실패");

        assert_eq!(derive_git_branch_from_cwd(&tmp.to_string_lossy()), None);
    }

    /// traversal cwd(`..` 포함) → is_safe_base_path가 거부해 None이어야 한다(AC2).
    #[test]
    fn derive_from_cwd_traversal_rejected() {
        assert_eq!(derive_git_branch_from_cwd("/some/repo/../../etc"), None);
    }

    /// 외부향 심볼릭 `.git/HEAD`(다른 파일을 가리킴) → canonicalize 가드로 None이어야 한다.
    /// (cwd-only라도 심볼릭 방어가 read_branch_from_git_dir 경유로 유효함을 고정한다.)
    #[test]
    #[cfg(unix)]
    fn derive_from_cwd_symlink_head_pointing_outside_none() {
        use std::os::unix::fs::symlink;
        let tmp = unique_test_dir("lterm-git-symlink");
        let git_dir = tmp.join(".git");
        std::fs::create_dir_all(&git_dir).expect("임시 .git 생성 실패");
        // `.git/HEAD`가 아닌 외부 파일을 유효 ref로 만든 뒤, HEAD를 그 파일로 심볼릭 링크한다.
        let outside = tmp.join("outside-ref");
        std::fs::write(&outside, "ref: refs/heads/leaked\n").expect("외부 ref 쓰기 실패");
        symlink(&outside, git_dir.join("HEAD")).expect("심볼릭 HEAD 생성 실패");

        // canonicalize 결과가 `.git/HEAD`로 끝나지 않으므로(outside-ref로 해소) None.
        assert_eq!(derive_git_branch_from_cwd(&tmp.to_string_lossy()), None);
    }

    /// 상보 테스트: `<cwd>/.git`이 심볼릭 디렉터리이고 그 실제 `.git/HEAD`가 정상 `ref:`이면
    /// `Some("<branch>")`를 반환한다(표준 git symlink 추종이 의도된 동작임을 고정 — 동작 변경 0).
    /// canonicalize 결과가 여전히 `.git/HEAD`로 끝나므로(실제 디렉터리도 `.git` 명) 누출 가드를 통과한다.
    #[test]
    #[cfg(unix)]
    fn derive_from_cwd_symlink_git_dir_follows_to_some() {
        use std::os::unix::fs::symlink;
        // 실제 repo: `<real>/.git/HEAD`에 정상 ref를 둔다.
        let real = unique_test_dir("lterm-git-symlink-real");
        let real_git = real.join(".git");
        std::fs::create_dir_all(&real_git).expect("실제 .git 생성 실패");
        std::fs::write(real_git.join("HEAD"), "ref: refs/heads/feature/linked\n")
            .expect("HEAD 쓰기 실패");

        // cwd: `<cwd>/.git`을 실제 `.git` 디렉터리로 심볼릭 링크한다(표준 git symlink 추종 케이스).
        let cwd = unique_test_dir("lterm-git-symlink-cwd");
        std::fs::create_dir_all(&*cwd).expect("cwd 생성 실패");
        symlink(&real_git, cwd.join(".git")).expect("심볼릭 .git 생성 실패");

        // canonicalize가 실제 `.git/HEAD`로 해소되고 `.git/HEAD`로 끝나므로 추종이 허용된다.
        assert_eq!(
            derive_git_branch_from_cwd(&cwd.to_string_lossy()).as_deref(),
            Some("feature/linked"),
            "심볼릭 .git 추종은 표준 git 동작 — 정상 ref면 branch 도출"
        );
    }

    /// BLOCKER 회귀 가드: 제어문자(ESC/개행)가 섞인 branch명은 거부돼 None이어야 한다.
    /// 악성 repo의 `.git/HEAD`가 `ref: refs/heads/main␛[31m…`처럼 인젝션 페이로드를 담아도
    /// source chokepoint(read_branch_from_git_dir)에서 차단됨을 실증한다.
    #[test]
    fn derive_from_cwd_rejects_control_chars() {
        let tmp = unique_test_dir("lterm-git-ctrl");
        let git_dir = tmp.join(".git");
        std::fs::create_dir_all(&git_dir).expect("임시 .git 생성 실패");
        // ESC(SGR) 인젝션 페이로드를 branch명에 심는다.
        std::fs::write(git_dir.join("HEAD"), "ref: refs/heads/main\x1b[31mX\n")
            .expect("HEAD 쓰기 실패");
        assert_eq!(derive_git_branch_from_cwd(&tmp.to_string_lossy()), None);

        // 개행 인젝션도 동일하게 거부돼야 한다.
        std::fs::write(git_dir.join("HEAD"), "ref: refs/heads/main\nINJECT\n")
            .expect("HEAD 쓰기 실패");
        assert_eq!(derive_git_branch_from_cwd(&tmp.to_string_lossy()), None);
    }

    /// 방어적 상한(FP<FN): MAX_BRANCH_LEN(256B) 초과 branch명은 손상/조작 신호로 보고 거부돼
    /// None이어야 한다. 경계값(정확히 256B는 허용, 257B는 거부)을 함께 고정한다.
    #[test]
    fn derive_from_cwd_rejects_overlong_branch() {
        let tmp = unique_test_dir("lterm-git-overlong");
        let git_dir = tmp.join(".git");
        std::fs::create_dir_all(&git_dir).expect("임시 .git 생성 실패");

        // 257B branch명(ASCII 1B/문자) → 상한 초과로 거부.
        let overlong = "a".repeat(MAX_BRANCH_LEN + 1);
        std::fs::write(
            git_dir.join("HEAD"),
            format!("ref: refs/heads/{overlong}\n"),
        )
        .expect("HEAD 쓰기 실패");
        assert_eq!(
            derive_git_branch_from_cwd(&tmp.to_string_lossy()),
            None,
            "256B 초과 branch명은 거부(FP<FN)"
        );

        // 정확히 256B branch명 → 허용(경계 미초과). 정상 케이스 회귀 0을 고정한다.
        let boundary = "b".repeat(MAX_BRANCH_LEN);
        std::fs::write(
            git_dir.join("HEAD"),
            format!("ref: refs/heads/{boundary}\n"),
        )
        .expect("HEAD 쓰기 실패");
        assert_eq!(
            derive_git_branch_from_cwd(&tmp.to_string_lossy()).as_deref(),
            Some(boundary.as_str()),
            "정확히 256B branch명은 허용(경계)"
        );
    }

    /// 상대경로 cwd는 프로세스 cwd 기준 false-positive를 막기 위해 거부돼 None이어야 한다.
    #[test]
    fn derive_from_cwd_rejects_relative_cwd() {
        assert_eq!(derive_git_branch_from_cwd("repo"), None);
        assert_eq!(derive_git_branch_from_cwd("./x"), None);
    }

    /// M-2 회귀 가드: worktree 경로도 상대 git_worktree/repo를 거부해야 한다(절대성 가드).
    /// 상대경로는 프로세스 cwd 기준 해석돼 엉뚱한 repo branch FP를 만들므로 None이어야 한다.
    /// derive_git_branch를 직접 호출해 is_absolute 가드가 fs 접근 전 즉시 거부함을 cwd-독립으로 검증
    /// (parse_claude_input 통합 테스트는 러너 cwd에 repo/.git이 없으면 false-green 위험).
    #[test]
    fn git_worktree_relative_path_rejected() {
        // git_worktree 상대경로 → None
        let ws_rel = RawWorkspace {
            git_worktree: Some("repo".to_string()),
            repo: None,
            current_dir: None,
            project_dir: None,
        };
        assert_eq!(
            derive_git_branch(&ws_rel),
            None,
            "상대 git_worktree는 절대성 가드로 거부"
        );

        // "." 상대 → None
        let ws_dot = RawWorkspace {
            git_worktree: Some(".".to_string()),
            repo: None,
            current_dir: None,
            project_dir: None,
        };
        assert_eq!(
            derive_git_branch(&ws_dot),
            None,
            "'.' 상대 git_worktree 거부"
        );

        // repo 분기(.or(repo))도 상대 거부
        let ws_repo = RawWorkspace {
            git_worktree: None,
            repo: Some("repo".to_string()),
            current_dir: None,
            project_dir: None,
        };
        assert_eq!(derive_git_branch(&ws_repo), None, "상대 repo도 거부");
    }

    // === parse_lterm_input (spec §6.2, §10) ===

    /// 정상 lterm JSON: 표시 필드가 정확히 매핑된다. git_branch는 cwd가 실존하지 않는
    /// non-git 경로라 None(조건부 부재 — 절대 비활성이 아님. 유효 git cwd 케이스는 별도 테스트).
    #[test]
    fn lterm_parses_normal_input() {
        // 비존재 고유 절대 temp 경로: `.git` 없으니 branch는 항상 None(host-독립).
        let cwd = unique_test_dir("lterm-normal");
        let cwd_str = cwd.to_string_lossy().into_owned();
        let raw = format!(
            r#"{{
            "source": "lterm",
            "version": 1,
            "session": "codex",
            "pane": "%3",
            "session_key": "codex/%3",
            "agent": "codex",
            "cwd": {cwd_str:?},
            "cols": 120,
            "rows": 40
        }}"#
        );
        let input = parse_lterm_input(&raw);
        assert_eq!(input.cwd.as_deref(), Some(cwd_str.as_str()));
        assert_eq!(input.model_display_name.as_deref(), Some("codex"));
        assert_eq!(input.session_id.as_deref(), Some("codex/%3"));
        // non-git cwd(실존하지 않는 경로)라 branch 없음(조건부 — 절대 비활성이 아님).
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
        // 비존재 고유 절대 temp 경로: `.git` 없으니 branch는 항상 None(host-독립).
        let cwd = unique_test_dir("lterm-unknown");
        let cwd_str = cwd.to_string_lossy().into_owned();
        let raw = format!(
            r#"{{
            "source": "lterm",
            "session": "s",
            "pane": "%1",
            "cwd": {cwd_str:?},
            "future_field": {{ "nested": [1, 2, 3] }},
            "another": "ignored"
        }}"#
        );
        let input = parse_lterm_input(&raw);
        assert_eq!(input.cwd.as_deref(), Some(cwd_str.as_str()));
        assert_eq!(input.session_id.as_deref(), Some("s/%1"));
        // non-git cwd(`.git` 없는 비존재 경로)라 branch 없음(조건부 부재).
        assert_eq!(input.git_branch, None);
    }

    /// 필드 누락: 부재 필드는 전부 None(또는 합성)으로 안전 저하해야 한다(무패닉).
    #[test]
    fn lterm_missing_fields_default_to_none() {
        // 비존재 고유 절대 temp 경로: `.git` 없으니 branch는 항상 None(host-독립).
        let cwd = unique_test_dir("lterm-missing");
        let cwd_str = cwd.to_string_lossy().into_owned();
        let raw = format!(r#"{{ "source": "lterm", "cwd": {cwd_str:?} }}"#);
        let input = parse_lterm_input(&raw);
        assert_eq!(input.cwd.as_deref(), Some(cwd_str.as_str()));
        assert_eq!(input.model_display_name, None);
        // session/pane 둘 다 부재 → session_key 합성 불가 → None.
        assert_eq!(input.session_id, None);
        // non-git cwd(`.git` 없는 비존재 경로)라 branch 없음(조건부 부재).
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

    /// session_label은 session/pane으로 "session/pane" 형식으로 합성된다(표시용).
    #[test]
    fn lterm_session_label_synthesized_from_session_and_pane() {
        let raw = r#"{ "session": "codex", "pane": "%3", "cwd": "/x/proj" }"#;
        let input = parse_lterm_input(raw);
        assert_eq!(input.session_label.as_deref(), Some("codex/%3"));
    }

    /// session_label 합성은 한쪽만 있으면 있는 쪽만 쓰고, 둘 다 없으면 None이다(session_key와 동일 규칙).
    #[test]
    fn lterm_session_label_partial_and_absent() {
        let only_session = parse_lterm_input(r#"{ "session": "codex" }"#);
        assert_eq!(only_session.session_label.as_deref(), Some("codex"));
        let only_pane = parse_lterm_input(r#"{ "pane": "%2" }"#);
        assert_eq!(only_pane.session_label.as_deref(), Some("%2"));
        let neither = parse_lterm_input(r#"{ "cwd": "/x" }"#);
        assert_eq!(neither.session_label, None);
    }

    /// 명시 session_key가 있어도 session_label은 session/pane 합성값을 따른다(별개 슬롯).
    #[test]
    fn lterm_session_label_independent_of_explicit_session_key() {
        let raw = r#"{ "session": "codex", "pane": "%7", "session_key": "stable-key" }"#;
        let input = parse_lterm_input(raw);
        assert_eq!(input.session_id.as_deref(), Some("stable-key"));
        assert_eq!(input.session_label.as_deref(), Some("codex/%7"));
    }

    /// 빈 lterm 객체와 Claude 입력은 session_label이 None이어야 한다(표시용 라벨 부재).
    #[test]
    fn session_label_none_for_empty_and_claude() {
        assert_eq!(parse_lterm_input("{}").session_label, None);
        let claude = parse_claude_input(r#"{ "session_id": "s", "cwd": "/x" }"#);
        assert_eq!(claude.session_label, None);
    }

    /// 미소비/forward-compat 필드(version/cols/rows)가 타입 드리프트(문자열 등)해도
    /// 전체 파싱이 실패하지 않고 session/pane/agent/cwd 등 useful 필드는 보존되어야 한다(무패닉).
    /// 과거: 이 필드들이 strict Option<u32>라 "version":"1" 등이 오면 from_str이 전체 실패해
    /// default로 저하되며 정상 필드까지 소실됐다.
    #[test]
    fn lterm_ignored_field_type_drift_preserves_useful_fields() {
        // 비존재 고유 절대 temp 경로: `.git` 없으니 branch는 항상 None(host-독립).
        let cwd = unique_test_dir("lterm-drift");
        let cwd_str = cwd.to_string_lossy().into_owned();
        let raw = format!(
            r#"{{
            "session": "codex",
            "pane": "%3",
            "agent": "codex",
            "cwd": {cwd_str:?},
            "version": "1",
            "cols": "120",
            "rows": "40"
        }}"#
        );
        let input = parse_lterm_input(&raw);
        // 타입 드리프트한 ignored 필드가 있어도 useful 필드가 살아남아야 한다.
        assert_eq!(input.session_id.as_deref(), Some("codex/%3"));
        assert_eq!(input.model_display_name.as_deref(), Some("codex"));
        assert_eq!(input.cwd.as_deref(), Some(cwd_str.as_str()));
        // non-git cwd(`.git` 없는 비존재 경로)라 branch 없음(조건부 부재).
        assert_eq!(input.git_branch, None);
    }

    /// 유효 git cwd(hermetic temp `.git`, create-before-call) → `git_branch == Some("<b>")`(AC9).
    /// T2의 신규 cwd→git 도출 계약을 우연 통과가 아니라 의도된 검증으로 고정한다.
    #[test]
    fn lterm_git_cwd_derives_branch() {
        use std::io::Write;
        let tmp = unique_test_dir("lterm-parse-git");
        let git_dir = tmp.join(".git");
        std::fs::create_dir_all(&git_dir).expect("임시 .git 생성 실패");
        let mut file = std::fs::File::create(git_dir.join("HEAD")).expect("HEAD 생성 실패");
        writeln!(file, "ref: refs/heads/main").expect("HEAD 쓰기 실패");

        let raw = format!(
            r#"{{ "source": "lterm", "session": "codex", "pane": "%3", "cwd": {:?} }}"#,
            tmp.to_string_lossy()
        );
        let input = parse_lterm_input(&raw);
        assert_eq!(
            input.git_branch.as_deref(),
            Some("main"),
            "유효 git cwd → branch 도출"
        );
    }

    /// 회귀 가드(Task3 No-go, mutation 저항): `<cwd>/.git`가 `gitdir:` 정규 파일이고 그 gitdir에
    /// 유효 HEAD(sentinel ref)가 실재해도, 현재는 추종하지 않아 None(안전 FN)이다. 누가 gitfile
    /// 추종을 추가하면 Some("<sentinel>")을 반환해 이 None 단언이 깨진다 = 추종 회귀를 실제로 포착.
    #[test]
    fn derive_from_cwd_gitfile_not_followed_none() {
        // (a) 절대 gitdir — 타깃 실재화 + sentinel HEAD
        let gitdir_abs = unique_test_dir("gitfile-target-abs");
        std::fs::create_dir_all(&*gitdir_abs).expect("gitdir 생성 실패");
        std::fs::write(gitdir_abs.join("HEAD"), "ref: refs/heads/sentinel-abs\n")
            .expect("타깃 HEAD 생성 실패");
        let base_abs = unique_test_dir("gitfile-abs");
        std::fs::create_dir_all(&*base_abs).expect("cwd 생성 실패");
        std::fs::write(
            base_abs.join(".git"),
            format!("gitdir: {}\n", gitdir_abs.to_string_lossy()),
        )
        .expect(".git 파일 생성 실패");
        assert_eq!(
            derive_git_branch_from_cwd(&base_abs.to_string_lossy()),
            None,
            "절대 gitdir gitfile 미추종 → None(추종 구현 시 Some(sentinel-abs)로 깨짐)"
        );

        // (b) 상대 gitdir — base 하위 타깃 실재화 + sentinel HEAD
        let base_rel = unique_test_dir("gitfile-rel");
        std::fs::create_dir_all(base_rel.join("realgit")).expect("상대 gitdir 생성 실패");
        std::fs::write(
            base_rel.join("realgit").join("HEAD"),
            "ref: refs/heads/sentinel-rel\n",
        )
        .expect("타깃 HEAD 생성 실패");
        std::fs::write(base_rel.join(".git"), "gitdir: realgit\n").expect(".git 파일 생성 실패");
        assert_eq!(
            derive_git_branch_from_cwd(&base_rel.to_string_lossy()),
            None,
            "상대 gitdir gitfile 미추종 → None(추종 구현 시 Some(sentinel-rel)로 깨짐)"
        );
    }

    /// 회귀 가드(Task3 No-go, mutation 저항): workspace git_worktree가 gitfile 워크트리를 가리키고
    /// 그 gitdir에 유효 HEAD가 실재해도 미추종 → None. cwd 경로와 대칭으로 양 진입점 고정.
    /// 추종이 구현되면 Some("sentinel-ws")를 반환해 이 None 단언이 깨진다 = 추종 회귀를 실제로 포착.
    #[test]
    fn derive_git_branch_gitfile_worktree_not_followed_none() {
        let gitdir = unique_test_dir("gitfile-ws-target");
        std::fs::create_dir_all(&*gitdir).expect("gitdir 생성 실패");
        std::fs::write(gitdir.join("HEAD"), "ref: refs/heads/sentinel-ws\n")
            .expect("타깃 HEAD 생성 실패");
        let base = unique_test_dir("gitfile-ws");
        std::fs::create_dir_all(&*base).expect("워크트리 생성 실패");
        std::fs::write(
            base.join(".git"),
            format!("gitdir: {}\n", gitdir.to_string_lossy()),
        )
        .expect(".git 파일 생성 실패");
        let ws = RawWorkspace {
            git_worktree: Some(base.to_string_lossy().into_owned()),
            repo: None,
            current_dir: None,
            project_dir: None,
        };
        assert_eq!(
            derive_git_branch(&ws),
            None,
            "gitfile 워크트리 미추종 → None(추종 구현 시 Some(sentinel-ws)로 깨짐)"
        );
    }

    /// [P1 핵심] repo 하위 dir에서 시작해도 부모로 walk-up해 repo 루트 branch를 도출한다.
    /// `<root>/.git/HEAD`=main + cwd=`<root>/src/deep` → Some("main").
    #[test]
    fn derive_from_cwd_walk_up_from_subdir_some() {
        let root = unique_test_dir("walkup-subdir");
        std::fs::create_dir_all(root.join(".git")).expect("root .git 생성 실패");
        std::fs::write(root.join(".git").join("HEAD"), "ref: refs/heads/main\n")
            .expect("HEAD 쓰기 실패");
        let deep = root.join("src").join("deep");
        std::fs::create_dir_all(&deep).expect("하위 dir 생성 실패");
        assert_eq!(
            derive_git_branch_from_cwd(&deep.to_string_lossy()).as_deref(),
            Some("main"),
            "repo 하위 dir에서 부모 walk-up으로 루트 branch 도출(P1)"
        );
    }

    /// walk-up은 첫 `.git` 디렉터리에서 정지한다(가장 가까운 repo 경계 존중).
    /// `<root>/.git`(outer) + `<root>/inner/.git`(inner) + cwd=`<root>/inner/x` → Some("inner").
    #[test]
    fn derive_from_cwd_walk_up_stops_at_first_git_dir() {
        let root = unique_test_dir("walkup-firststop");
        std::fs::create_dir_all(root.join(".git")).expect("outer .git 생성 실패");
        std::fs::write(root.join(".git").join("HEAD"), "ref: refs/heads/outer\n")
            .expect("outer HEAD 쓰기 실패");
        let inner = root.join("inner");
        std::fs::create_dir_all(inner.join(".git")).expect("inner .git 생성 실패");
        std::fs::write(inner.join(".git").join("HEAD"), "ref: refs/heads/inner\n")
            .expect("inner HEAD 쓰기 실패");
        let cwd = inner.join("x");
        std::fs::create_dir_all(&cwd).expect("cwd 생성 실패");
        assert_eq!(
            derive_git_branch_from_cwd(&cwd.to_string_lossy()).as_deref(),
            Some("inner"),
            "가장 가까운 첫 .git에서 정지 — 부모 outer로 올라가지 않음"
        );
    }

    /// [threat#2, 이중 mutation 저항] gitfile 정규파일 `.git` 경계에서 정지하되 미추종 → None.
    /// `<main>/.git`(디렉터리, outer-main) + `<main>/wt/.git`=gitfile(gitdir에 sentinel-wt 실재) +
    /// cwd=`<main>/wt/src` → None. gitfile 추종 추가 시 Some(sentinel-wt), 경계 무시 상승 추가 시
    /// Some(outer-main)로 깨진다 = 양방향 mutation을 모두 포착한다.
    #[test]
    fn derive_from_cwd_walk_stops_at_gitfile_boundary_none() {
        let main = unique_test_dir("walkup-gitfile-main");
        std::fs::create_dir_all(main.join(".git")).expect("main .git 생성 실패");
        std::fs::write(
            main.join(".git").join("HEAD"),
            "ref: refs/heads/outer-main\n",
        )
        .expect("main HEAD 쓰기 실패");
        // 실재 gitdir + sentinel HEAD(추종 회귀가 도입되면 Some(sentinel-wt)로 깨지도록 실재화).
        let gitdir = main.join(".git").join("worktrees").join("wt");
        std::fs::create_dir_all(&gitdir).expect("gitdir 생성 실패");
        std::fs::write(gitdir.join("HEAD"), "ref: refs/heads/sentinel-wt\n")
            .expect("gitdir HEAD 쓰기 실패");
        // linked worktree: `<main>/wt/.git`는 gitdir를 가리키는 정규파일.
        let wt = main.join("wt");
        std::fs::create_dir_all(wt.join("src")).expect("wt/src 생성 실패");
        std::fs::write(
            wt.join(".git"),
            format!("gitdir: {}\n", gitdir.to_string_lossy()),
        )
        .expect(".git gitfile 생성 실패");
        let cwd = wt.join("src");
        assert_eq!(
            derive_git_branch_from_cwd(&cwd.to_string_lossy()),
            None,
            "gitfile 경계에서 정지+미추종 → None(추종 추가 시 Some(sentinel-wt), 경계 무시 상승 시 Some(outer-main)로 깨짐)"
        );
    }

    /// 경계 `.git` 디렉터리에 HEAD가 부재해도 그 경계에서 정지한다(부모 real로 안 올라감) → None.
    /// `<root>/.git`(HEAD=real) + `<root>/sub/.git`(HEAD 부재) + cwd=`<root>/sub` → None.
    #[test]
    fn derive_from_cwd_walk_stops_at_empty_git_dir() {
        let root = unique_test_dir("walkup-emptygit");
        std::fs::create_dir_all(root.join(".git")).expect("root .git 생성 실패");
        std::fs::write(root.join(".git").join("HEAD"), "ref: refs/heads/real\n")
            .expect("root HEAD 쓰기 실패");
        let sub = root.join("sub");
        // 하위 `.git` 디렉터리는 존재하나 HEAD가 없다(엔트리 존재 → 정지, reader가 None).
        std::fs::create_dir_all(sub.join(".git")).expect("sub .git 생성 실패");
        assert_eq!(
            derive_git_branch_from_cwd(&sub.to_string_lossy()),
            None,
            "경계 .git이 HEAD 부재여도 정지 — 부모 real로 상승하지 않음"
        );
    }

    /// [threat#1, mutation 저항] canonicalize-once 불변식①을 실제로 고정한다.
    /// 구성: `<base>/repo/.git`(branch=leaked) 실재 + `<base>/repo/link`→`<base>/target` symlink +
    /// 실타깃 `<base>/target/sub`(.git 없음) 실재. cwd=`<base>/repo/link/sub`.
    /// canonicalize는 link를 `<base>/target`으로 해소하므로 그 조상(target/base)엔 `.git`이 없어 None.
    /// ★ 핵심: lexical 부모 `<base>/repo`에 함정 repo(.git=leaked)를 둔다. canonicalize를 제거하면
    /// lexical 조상 `<base>/repo/.git`을 만나 `Some("leaked")` = cross-repo false-positive로 단언이
    /// 깨진다(불변식① canonicalize-first mutation 포착). canonicalize가 살아 있을 때만 None.
    /// (주의: 이 테스트가 검증하는 건 "canonicalize 제거"이다. 루프 내 재canonicalize는 canonical
    /// 조상을 다시 canonicalize하는 no-op이라 실제로 본 단언을 깨지 않는다 — 재canonicalize 금지는
    /// 설계 의도(불변식①)로 doc에 유지하되 이 테스트의 직접 커버리지는 아니다.)
    #[test]
    #[cfg(unix)]
    fn derive_from_cwd_walk_up_symlink_cwd_no_cross_repo() {
        use std::os::unix::fs::symlink;
        let base = unique_test_dir("walkup-symlink");
        // lexical 부모에 함정 repo(.git=leaked): 미-canonicalize walk-up이 잘못 귀속할 대상.
        let repo = base.join("repo");
        std::fs::create_dir_all(repo.join(".git")).expect("repo/.git 생성 실패");
        std::fs::write(repo.join(".git").join("HEAD"), "ref: refs/heads/leaked\n")
            .expect("repo HEAD 쓰기 실패");
        // 심볼릭 실타깃: `.git` 없는 별도 트리(canonical 실경로 조상).
        std::fs::create_dir_all(base.join("target").join("sub")).expect("target/sub 생성 실패");
        // `<base>/repo/link` → `<base>/target`. cwd=`<base>/repo/link/sub`(canonical=`<base>/target/sub`).
        let link = repo.join("link");
        symlink(base.join("target"), &link).expect("심볼릭 link 생성 실패");
        let cwd = link.join("sub");
        assert_eq!(
            derive_git_branch_from_cwd(&cwd.to_string_lossy()),
            None,
            "canonical 실경로(target, .git 없음) 조상만 순회 → None. \
             canonicalize 제거 시 lexical repo/.git을 만나 Some(\"leaked\")로 깨짐(threat#1)"
        );
    }

    /// [ATTACK P 정탐 확대 고정] 추적 안 되는 하위 dir이라도 부모 repo work tree 안이면 branch 표시.
    /// `<repo>/.git/HEAD`=parent-main + cwd=`<repo>/untracked/sub` → Some("parent-main").
    /// "=git branch와 동일한 git-consistent 정탐, FP 아님"을 고정한다.
    #[test]
    fn derive_from_cwd_walk_up_parent_repo_positive() {
        let repo = unique_test_dir("walkup-parent-positive");
        std::fs::create_dir_all(repo.join(".git")).expect("repo .git 생성 실패");
        std::fs::write(
            repo.join(".git").join("HEAD"),
            "ref: refs/heads/parent-main\n",
        )
        .expect("HEAD 쓰기 실패");
        let cwd = repo.join("untracked").join("sub");
        std::fs::create_dir_all(&cwd).expect("untracked/sub 생성 실패");
        assert_eq!(
            derive_git_branch_from_cwd(&cwd.to_string_lossy()).as_deref(),
            Some("parent-main"),
            "추적 안 되는 하위 dir도 부모 work tree 안이면 branch 표시 = git-consistent 정탐(FP 아님)"
        );
    }

    /// [cwd-only 회귀 0] cwd=`<root>` 자신(depth 0)이면 기존 cwd-only 동작과 동일하게 Some("main").
    #[test]
    fn derive_from_cwd_walk_up_root_repo_unchanged() {
        let root = unique_test_dir("walkup-root-unchanged");
        std::fs::create_dir_all(root.join(".git")).expect("root .git 생성 실패");
        std::fs::write(root.join(".git").join("HEAD"), "ref: refs/heads/main\n")
            .expect("HEAD 쓰기 실패");
        assert_eq!(
            derive_git_branch_from_cwd(&root.to_string_lossy()).as_deref(),
            Some("main"),
            "depth 0(cwd 자신이 repo 루트) 동작은 기존 cwd-only와 동일"
        );
    }

    /// [walk-up 심볼릭 `.git` 엔트리, positive] walk-up이 부모에서 만나는 첫 `.git`이 심볼릭 링크여도
    /// 그 엔트리에서 정지·추종해 실 디렉터리의 branch를 도출한다(표준 git symlink 추종).
    /// 구성: `<base>/.git`(디렉터리, outer-main) + `<base>/inner/.git`→symlink→`<base>/realgit/.git`
    /// (실 디렉터리, inner-real) + cwd=`<base>/inner/sub`. → Some("inner-real").
    /// ★ 핵심: `find_git_root_dir`이 `is_dir()`/`is_file()` 분기를 쓰면 심볼릭 `.git`이 둘 다 false라
    /// skip돼 부모 `<base>/.git`까지 올라가 outer-main이 누출되며 깨진다(lstat 무차별 정지 mutation 포착).
    /// 심볼릭 타깃은 `.git`로 끝나야 reader의 `ends_with(.git/HEAD)` 누출 가드를 통과한다.
    #[test]
    #[cfg(unix)]
    fn derive_from_cwd_walk_up_symlink_git_dir_at_parent_some() {
        use std::os::unix::fs::symlink;
        let base = unique_test_dir("walkup-symlink-gitdir");
        // 부모 함정 repo: walk-up이 잘못 상승하면 누출될 outer-main.
        std::fs::create_dir_all(base.join(".git")).expect("base .git 생성 실패");
        std::fs::write(
            base.join(".git").join("HEAD"),
            "ref: refs/heads/outer-main\n",
        )
        .expect("base HEAD 쓰기 실패");
        // 심볼릭 타깃: `.git`로 끝나는 실 디렉터리(reader 누출 가드 통과 조건).
        let realgit = base.join("realgit").join(".git");
        std::fs::create_dir_all(&realgit).expect("realgit/.git 생성 실패");
        std::fs::write(realgit.join("HEAD"), "ref: refs/heads/inner-real\n")
            .expect("realgit HEAD 쓰기 실패");
        // inner: `<base>/inner/.git`을 실 `.git` 디렉터리로 심볼릭 링크(walk-up이 만나는 첫 `.git`).
        let inner = base.join("inner");
        std::fs::create_dir_all(inner.join("sub")).expect("inner/sub 생성 실패");
        symlink(&realgit, inner.join(".git")).expect("심볼릭 inner/.git 생성 실패");
        let cwd = inner.join("sub");
        assert_eq!(
            derive_git_branch_from_cwd(&cwd.to_string_lossy()).as_deref(),
            Some("inner-real"),
            "심볼릭 .git 엔트리에서 정지·추종 → inner-real. is_dir()/is_file() 분기 시 skip돼 부모 outer-main 누출로 깨짐"
        );
    }

    /// [walk-up 심볼릭 `.git` 엔트리, boundary] walk-up이 만나는 첫 `.git`이 dangling 심볼릭이면
    /// 그 엔트리에서 정지하되(`symlink_metadata` lstat가 심볼릭 존재를 봄) reader의 canonicalize가
    /// Err→None으로 흡수해 **부모로 계속 상승하지 않는다**.
    /// 구성: `<base>/.git`(디렉터리, outer-main) + `<base>/inner/.git`→symlink→`<base>/nonexistent`
    /// (dangling) + cwd=`<base>/inner/sub`. → None.
    /// ★ 핵심: dangling 심볼릭에서 정지 후 None을 반환해야 한다. 정지 대신 부모로 상승하면
    /// `<base>/.git`의 outer-main이 Some으로 누출돼 깨진다(첫 `.git` 정지 불변식 mutation 포착).
    #[test]
    #[cfg(unix)]
    fn derive_from_cwd_walk_up_dangling_git_symlink_stops_none() {
        use std::os::unix::fs::symlink;
        let base = unique_test_dir("walkup-dangling-gitsymlink");
        // 부모 함정 repo: 상승 회귀가 도입되면 누출될 outer-main.
        std::fs::create_dir_all(base.join(".git")).expect("base .git 생성 실패");
        std::fs::write(
            base.join(".git").join("HEAD"),
            "ref: refs/heads/outer-main\n",
        )
        .expect("base HEAD 쓰기 실패");
        // inner: `<base>/inner/.git`을 비존재 타깃으로 심볼릭 링크(dangling).
        let inner = base.join("inner");
        std::fs::create_dir_all(inner.join("sub")).expect("inner/sub 생성 실패");
        symlink(base.join("nonexistent"), inner.join(".git")).expect("dangling 심볼릭 생성 실패");
        let cwd = inner.join("sub");
        assert_eq!(
            derive_git_branch_from_cwd(&cwd.to_string_lossy()),
            None,
            "dangling .git 심볼릭에서 정지(lstat가 심볼릭 존재 인식)+reader canonicalize Err→None. 부모 outer-main 상승 시 Some(\"outer-main\")으로 깨짐"
        );
    }

    /// [threat#3] depth cap 초과 깊이에서는 None(병적 깊이/mount loop 차단).
    /// 65단계 fs 생성 비용을 회피하기 위해 작은 cap(3)을 `find_git_root_dir_capped`에 직접 주입해
    /// cap 초과 조상의 `.git`이 발견되지 않음을 검증한다(prod는 MAX_WALK_UP_DEPTH 경유).
    #[test]
    fn derive_from_cwd_walk_up_depth_cap_none() {
        let root = unique_test_dir("walkup-depthcap");
        std::fs::create_dir_all(root.join(".git")).expect("root .git 생성 실패");
        std::fs::write(root.join(".git").join("HEAD"), "ref: refs/heads/main\n")
            .expect("HEAD 쓰기 실패");
        // root에서 4단계 아래(cap=3 초과)인 깊은 cwd. canonicalize 위해 실생성한다.
        let deep = root.join("a").join("b").join("c").join("d");
        std::fs::create_dir_all(&deep).expect("깊은 cwd 생성 실패");
        let start = std::fs::canonicalize(&deep).expect("canonicalize 실패");
        // cap=3: start(=d) → c → b 까지만 순회(root .git 미도달, root는 5단계 위).
        assert_eq!(
            find_git_root_dir_capped(&start, 3),
            None,
            "cap 초과 깊이의 .git은 미발견 → None(threat#3 mount loop·병적 깊이 차단)"
        );
        // 대조: 넉넉한 cap이면 root .git을 발견한다(cap이 유일 변수임을 고정).
        // 깊이-인덱스 결합을 피해 canonical root와 직접 비교한다(deep 깊이 변경에 견고).
        let canonical_root = std::fs::canonicalize(&*root).expect("root canonicalize 실패");
        assert_eq!(
            find_git_root_dir_capped(&start, MAX_WALK_UP_DEPTH),
            Some(canonical_root),
            "넉넉한 cap이면 root .git 발견(cap만이 차이를 만드는 변수)"
        );
    }

    /// [threat#3, prod 경로] prod `derive_git_branch_from_cwd`가 실제로 `MAX_WALK_UP_DEPTH`(64) cap을
    /// 강제함을 직접 증명한다(`find_git_root_dir_capped`를 우회하지 않고 공개 진입점으로 검증).
    /// 구성: `<root>/.git`(deep-root) + `<root>` 아래 64단계 중첩 디렉터리(c1/c2/.../c64) 실생성,
    /// cwd=최심부(c64). root `.git`은 c64에서 보면 ancestors index 64(자신 c64=0, c63=1, ..., c1=63,
    /// root=64)라 `take(64)`=index 0..63에서 제외 = prod cap이 차단 → None.
    /// 비용: 중첩 dir create_dir_all 1회(작음).
    #[test]
    fn derive_from_cwd_walk_up_prod_depth_cap_none() {
        let root = unique_test_dir("walkup-prod-depthcap");
        std::fs::create_dir_all(root.join(".git")).expect("root .git 생성 실패");
        std::fs::write(
            root.join(".git").join("HEAD"),
            "ref: refs/heads/deep-root\n",
        )
        .expect("root HEAD 쓰기 실패");
        // `<root>` 아래 64단계 중첩(c1/.../c64)을 1회 생성. 최심부에서 root는 ancestors index 64.
        let mut deep = root.to_path_buf();
        for i in 1..=64 {
            deep = deep.join(format!("c{i}"));
        }
        std::fs::create_dir_all(&deep).expect("64단계 중첩 dir 생성 실패");
        // prod 진입점: take(64)는 index 0..63만 방문 → index 64인 root .git 제외 → None.
        assert_eq!(
            derive_git_branch_from_cwd(&deep.to_string_lossy()),
            None,
            "root .git이 ancestors index 64(MAX_WALK_UP_DEPTH 방문 범위 0..63 밖)라 prod cap이 차단 → None"
        );
    }
}
