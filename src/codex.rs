//! Codex 세션 심층판독(Phase 2-1, spec `2026-06-06-codex-session-deep-read-design.md`).
//!
//! `understatus render --source lterm`이 lterm payload의 `agent=="codex"`를 감지하면
//! `$CODEX_HOME/sessions/**/rollout-*.jsonl`을 직접 판독해 model·ctx%·5h한도%·주간한도%·
//! plan·effort를 statusline에 enrich한다. 세션을 못 찾거나 **모호**하면(동일 cwd·fresh 후보
//! ≥2개) enrich를 전면 생략해 기존 lterm 동작(model="codex")으로 정직하게 저하한다.
//!
//! 설계 원칙(spec §2):
//! 1. fail-safe + "모호한 성공도 실패로 취급"(잘못된 세션을 자신 있게 표시하는 fail-wrong 금지).
//! 2. 디스크 I/O·스캔·tail은 본 모듈에 격리(claude.rs::parse_lterm_input은 순수 유지).
//! 3. 바운디드 비용: 전체 파싱 금지(head 16KB + tail 256KB), 디스크 캐시로 정상상태 stat 1회.
//! 4. opt-out: `[codex] enabled=false`면 `~/.codex` 일절 안 읽음.
//! 5. 모든 필드 lenient(Option): 부재/타입 드리프트/cli_version 변동 시 무패닉 → 세그먼트 생략.

use crate::chain::{
    cache_now_millis, is_named_cache_fresh, read_session_named_cache_in,
    write_session_named_cache_in,
};
use crate::claude::ClaudeInput;
use crate::config::Config;
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// head(앞부분) 읽기 상한. 첫 줄 session_meta + 첫 turn_context(baseline model/effort)용(spec §8.1).
const HEAD_READ_BYTES: u64 = 16 * 1024;
/// 첫 줄(session_meta) 읽기 상한(매칭 선필터용). spec §8.1은 16KB로 가정했으나 **실측상 실제
/// session_meta 첫 줄은 inline `base_instructions` 때문에 ~33KB**에 달한다(16KB 가정은 실데이터와
/// 충돌). cwd/originator 매칭에 첫 줄 전체가 필요하므로 head 상한과 별도로 넉넉히 둔다.
const FIRST_LINE_READ_BYTES: u64 = 128 * 1024;
/// tail(뒷부분) 읽기 상한. 마지막 token_count + 최신 turn_context용(실측 gap max 14KB,
/// 단일 라인 max 132KB → 256KB 안전마진, spec §8.1).
const TAIL_READ_BYTES: u64 = 256 * 1024;
/// 최근 일자 디렉터리에서 한 번의 해소가 검사할 rollout 파일 총량 상한.
///
/// 상한을 넘기면 단일 후보를 확정하지 않고 fail-safe(ambiguous와 동일하게 enrich 생략)로
/// 저하한다. 그래야 큰 `sessions` 트리에서 statusline hot path가 무한히 파일을 stat/read하지
/// 않고, 부분 스캔으로 잘못된 단일 세션을 표시하는 fail-wrong도 피한다.
const MAX_ROLLOUT_SCAN_FILES: usize = 1024;
/// 디스크 캐시 파일명(세션별 격리, chain.rs 인프라 재사용, spec §8).
const CODEX_CACHE_FILE: &str = "codex_session";
/// 대화형(TUI) originator 화이트리스트 prefix. `codex_exec` 등 비대화형은 제외(spec §5).
/// exec 세션엔 token_count/turn_context가 없어 enrich 불가하므로 보수적으로 안전 저하한다.
const INTERACTIVE_ORIGINATOR_PREFIX: &str = "codex-tui";

/// Codex 세션에서 추출한 추가 표시 필드(rate 한도/plan/effort). lterm/codex 소스 전용.
///
/// model·ctx%는 [`ClaudeInput`]의 기존 슬롯(`model_display_name`/`context_used_percentage`)을
/// 재사용하므로 여기엔 두지 않는다. Claude 경로는 항상 `None`이라 비트 동일이 보장된다(spec §6).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CodexExtras {
    /// 5시간 한도 사용률 %(rate_limits 중 window_minutes==300인 객체의 used_percent).
    pub rate_5h_percent: Option<f64>,
    /// 주간 한도 사용률 %(rate_limits 중 window_minutes==10080인 객체의 used_percent).
    pub rate_weekly_percent: Option<f64>,
    /// 요금제(rate_limits.plan_type). 예 `"pro"`.
    pub plan: Option<String>,
    /// 추론 강도(turn_context.effort). 예 `"xhigh"`.
    pub effort: Option<String>,
}

/// 해소된 단일 Codex 세션의 표시 데이터 묶음(파싱 결과 + 캐시 본문).
///
/// `model`/`context_percentage`는 [`ClaudeInput`]의 기존 슬롯으로, `extras`는
/// [`CodexExtras`]로 흘러간다(spec §6 통합 표).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CodexSession {
    /// 최신 turn_context.model(tail 우선). 예 `"gpt-5.5"`.
    pub model: Option<String>,
    /// ctx% = last_token_usage.total_tokens / model_context_window × 100(window==0 가드).
    pub context_percentage: Option<f64>,
    /// rate 한도/plan/effort 추가 필드.
    pub extras: CodexExtras,
}

/// 후보 해소 결과(spec §5 모호성 판정).
///
/// - `Single`: 정확히 1개 후보 → 풀 enrich. 캐시 가능.
/// - `Ambiguous`: 동일 cwd·fresh 후보 ≥2개 → "이 페인의 codex 식별 불가" → enrich 전면 생략.
///   **캐시하지 않는다**(fail-wrong TTL 고착 차단, spec §8).
/// - `None`: 후보 0개 → 생략.
#[derive(Debug, Clone, PartialEq)]
pub enum Resolution {
    /// 단일 후보: 세션 데이터 + 해소된 rollout 경로.
    Single(CodexSession, PathBuf),
    /// 모호(≥2): enrich 생략(model="codex" 유지).
    Ambiguous,
    /// 후보 없음: enrich 생략.
    None,
}

/// 후보 스캔 결과와 캐시 재사용 안전성을 판정하기 위한 최근 rollout 트리 지문.
#[derive(Debug, Clone, PartialEq)]
struct CandidateScan {
    /// cwd/originator/freshness 선필터를 통과한 후보.
    candidates: Vec<PathBuf>,
    /// 스캔한 최근 일자 디렉터리의 파일 수/mtime 지문.
    fingerprint: CodexScanFingerprint,
}

/// 캐시된 단일 후보가 여전히 "유일한 후보"인지 재검증하기 위한 경량 지문.
///
/// 각 캐시 히트마다 rollout JSON 본문을 다시 읽지는 않지만, 최근 일자 디렉터리의 rollout 파일
/// 개수와 mtime 집계를 확인해 새 rollout 추가/삭제/수정을 감지한다. 지문이 바뀌면 캐시를
/// 바로 재사용하지 않고 후보 스캔으로 모호성을 다시 판정한다.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct CodexScanFingerprint {
    days: Vec<CodexDayFingerprint>,
    /// `MAX_ROLLOUT_SCAN_FILES`를 초과해 전체 후보 유일성을 증명할 수 없는 상태.
    budget_exceeded: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct CodexDayFingerprint {
    path: String,
    mtime_ms: Option<u128>,
    rollout_files_seen: usize,
    rollout_mtime_sum_ms: u128,
}

/// session_meta(첫 줄)에서 추출한 매칭 근거.
#[derive(Debug, Clone, PartialEq)]
struct SessionMeta {
    /// 세션 시작 cwd(payload.cwd). cwd 일치 매칭에 사용.
    cwd: Option<String>,
    /// originator(payload.originator). TUI/exec 구분(화이트리스트 prefix).
    originator: Option<String>,
}

/// token_count 이벤트에서 추출한 토큰/한도 스냅샷.
#[derive(Debug, Clone, PartialEq, Default)]
struct TokenSnapshot {
    /// last_token_usage.total_tokens(ctx% 분자). **total_token_usage 절대 금지**(누적값).
    last_total_tokens: Option<u64>,
    /// model_context_window(ctx% 분모). 0이면 ctx% None.
    context_window: Option<u64>,
    /// rate_limits.primary/secondary(named 객체, 배열 아님). window_minutes로 5h/주간 식별.
    rate_5h_percent: Option<f64>,
    /// 주간 한도(window_minutes==10080) used_percent.
    rate_weekly_percent: Option<f64>,
    /// rate_limits.plan_type.
    plan: Option<String>,
}

/// turn_context에서 추출한 model/effort.
#[derive(Debug, Clone, PartialEq, Default)]
struct TurnContext {
    /// turn_context.payload.model. 예 `"gpt-5.5"`.
    model: Option<String>,
    /// turn_context.payload.effort. 예 `"xhigh"`.
    effort: Option<String>,
}

// ============================== 순수 파서(spec §4/§7) ==============================

/// 첫 줄(session_meta)에서 cwd/originator를 추출한다(lenient, 무패닉).
///
/// # 인자
/// - `first_line`: rollout 파일의 첫 JSON 라인.
///
/// # 반환
/// `type=="session_meta"`이고 파싱되면 [`SessionMeta`]. 타입 불일치/깨짐/부재 시 `None`.
fn parse_session_meta(first_line: &str) -> Option<SessionMeta> {
    let value: serde_json::Value = serde_json::from_str(first_line.trim()).ok()?;
    // type 게이팅: session_meta가 아니면 매칭 근거로 쓰지 않는다.
    if value.get("type").and_then(|t| t.as_str()) != Some("session_meta") {
        return None;
    }
    let payload = value.get("payload")?;
    Some(SessionMeta {
        cwd: payload
            .get("cwd")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        originator: payload
            .get("originator")
            .and_then(|v| v.as_str())
            .map(str::to_string),
    })
}

/// turn_context 라인에서 model/effort를 추출한다(lenient, 무패닉).
///
/// # 인자
/// - `line`: rollout의 한 JSON 라인.
///
/// # 반환
/// `type=="turn_context"`이면 [`TurnContext`](부분/누락 안전). 그 외 `None`.
fn parse_turn_context(line: &str) -> Option<TurnContext> {
    let value: serde_json::Value = serde_json::from_str(line.trim()).ok()?;
    if value.get("type").and_then(|t| t.as_str()) != Some("turn_context") {
        return None;
    }
    let payload = value.get("payload")?;
    Some(TurnContext {
        model: payload
            .get("model")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        effort: payload
            .get("effort")
            .and_then(|v| v.as_str())
            .map(str::to_string),
    })
}

/// event_msg(payload.type=="token_count") 라인에서 토큰/한도 스냅샷을 추출한다(lenient).
///
/// **2단계 중첩 게이팅**(spec §4): `type=="event_msg"` AND `payload.type=="token_count"`.
/// 그 뒤 `payload.info`(last_token_usage/model_context_window)와 `payload.rate_limits`를 읽는다.
///
/// # 주의
/// - **`total_token_usage` 절대 사용 금지**(누적값이라 실측 100% 초과). `last_token_usage`만 사용.
/// - `rate_limits`는 **named 객체**(배열 아님): `primary`/`secondary` 각 `window_minutes`로
///   5h(300)/주간(10080)을 식별한다(`primary=5h` 단정 금지).
///
/// # 반환
/// 게이팅 통과 시 [`TokenSnapshot`]. 타입 불일치/깨짐 시 `None`. 개별 필드는 부재 시 `None`.
fn parse_token_count(line: &str) -> Option<TokenSnapshot> {
    let value: serde_json::Value = serde_json::from_str(line.trim()).ok()?;
    if value.get("type").and_then(|t| t.as_str()) != Some("event_msg") {
        return None;
    }
    let payload = value.get("payload")?;
    if payload.get("type").and_then(|t| t.as_str()) != Some("token_count") {
        return None;
    }

    let info = payload.get("info");
    // last_token_usage.total_tokens만 사용한다(total_token_usage는 누적이라 절대 금지).
    let last_total_tokens = info
        .and_then(|i| i.get("last_token_usage"))
        .and_then(|u| u.get("total_tokens"))
        .and_then(serde_json::Value::as_u64);
    let context_window = info
        .and_then(|i| i.get("model_context_window"))
        .and_then(serde_json::Value::as_u64);

    // rate_limits는 named 객체: primary/secondary 각각 window_minutes로 5h/주간을 식별한다.
    let rate_limits = payload.get("rate_limits");
    let (rate_5h_percent, rate_weekly_percent) = extract_rate_windows(rate_limits);
    let plan = rate_limits
        .and_then(|r| r.get("plan_type"))
        .and_then(|v| v.as_str())
        .map(str::to_string);

    Some(TokenSnapshot {
        last_total_tokens,
        context_window,
        rate_5h_percent,
        rate_weekly_percent,
        plan,
    })
}

/// rate_limits의 named 필드(primary/secondary)를 순회해 window_minutes로 5h/주간을 식별한다.
///
/// **`primary=5h` 단정 금지**(spec §4): 두 named 필드를 모두 검사하고 각 `window_minutes`가
/// 300이면 5h, 10080이면 주간 슬롯에 `used_percent`를 배정한다. 미상 window는 무시한다.
///
/// # 반환
/// `(5h%, 주간%)`. 해당 window 부재 시 각각 `None`.
fn extract_rate_windows(rate_limits: Option<&serde_json::Value>) -> (Option<f64>, Option<f64>) {
    let mut rate_5h = None;
    let mut rate_weekly = None;
    let Some(rate_limits) = rate_limits else {
        return (None, None);
    };
    // named 객체(배열 아님): primary/secondary 두 후보만 검사한다.
    for field in ["primary", "secondary"] {
        let Some(window) = rate_limits.get(field) else {
            continue;
        };
        let window_minutes = window
            .get("window_minutes")
            .and_then(serde_json::Value::as_u64);
        let used_percent = window
            .get("used_percent")
            .and_then(serde_json::Value::as_f64);
        match window_minutes {
            // 5시간 = 300분.
            Some(300) => rate_5h = used_percent,
            // 주간 = 10080분(7일).
            Some(10080) => rate_weekly = used_percent,
            // 미상 window는 무시(보수적 안전 저하).
            _ => {}
        }
    }
    (rate_5h, rate_weekly)
}

/// ctx% = total / window × 100을 계산한다(window==0 가드).
///
/// # 인자
/// - `total`: last_token_usage.total_tokens.
/// - `window`: model_context_window.
///
/// # 반환
/// `window > 0`이면 `Some(백분율)`. `window == 0`이면 `None`(0 나눗셈/무의미값 방지, spec §4).
fn compute_context_percentage(total: u64, window: u64) -> Option<f64> {
    if window == 0 {
        return None;
    }
    Some((total as f64 / window as f64) * 100.0)
}

/// originator가 대화형(TUI) 화이트리스트에 부합하는지 판정한다(spec §5).
///
/// 화이트리스트(prefix `codex-tui`) 채택 이유: 미래 새 originator는 보수적으로 안전 저하한다
/// (블랙리스트는 새 비대화형 originator를 놓칠 위험이 있음). `codex_exec` 등은 token_count/
/// turn_context가 없어 enrich가 불가하므로 제외한다.
fn is_interactive_originator(originator: Option<&str>) -> bool {
    originator
        .map(|o| o.starts_with(INTERACTIVE_ORIGINATOR_PREFIX))
        .unwrap_or(false)
}

/// cwd 비교용 정규화. 존재 경로는 canonicalize하고, 부재 경로는 trailing slash만 제거한다.
fn normalize_cwd_for_match(cwd: &str) -> PathBuf {
    std::fs::canonicalize(cwd).unwrap_or_else(|_| PathBuf::from(cwd.trim_end_matches('/')))
}

/// 반복 후보 스캔에서 target cwd canonicalize를 매 후보마다 반복하지 않기 위한 비교 헬퍼.
fn cwd_matches_normalized(candidate_cwd: &str, normalized_target_cwd: &Path) -> bool {
    normalize_cwd_for_match(candidate_cwd) == normalized_target_cwd
}

/// 두 cwd 문자열이 같은 디렉터리를 가리키는지 비교한다(정규화: canonicalize 실패 시 trim 비교).
///
/// 외부 입력(payload.cwd)은 **비교에만** 쓰고 파일경로 구성엔 쓰지 않는다(traversal 무관, spec §5).
/// trailing slash 등 표기 차이를 흡수하기 위해 canonicalize를 시도하고, 실패(부재 경로 등) 시
/// trim된 문자열 동치로 폴백한다.
#[cfg(test)]
fn cwd_matches(candidate_cwd: &str, target_cwd: &str) -> bool {
    cwd_matches_normalized(candidate_cwd, &normalize_cwd_for_match(target_cwd))
}

// ============================== 발견/IO(spec §5/§8.1) ==============================

/// 파일의 head(앞 16KB)와 tail(뒤 256KB)을 경계 정렬해 읽는다(전체 파싱 금지, spec §8.1).
///
/// # 인자
/// - `path`: rollout-*.jsonl 경로.
///
/// # 반환
/// `(head_text, tail_text)`. 읽기 실패 시 `None`. 비-UTF8은 `from_utf8_lossy`로 보존한다.
/// tail은 EOF 역방향이라 **첫 부분 라인을 폐기**(개행 경계 정렬)하고, head는 앞에서부터 읽되
/// 파일이 head보다 크면 마지막 부분 라인을 폐기한다(라인 경계 정렬).
fn read_head_tail(path: &Path) -> Option<(String, String)> {
    let mut file = File::open(path).ok()?;
    let file_len = file.metadata().ok()?.len();

    // head: 앞에서부터 최대 HEAD_READ_BYTES.
    let head_len = file_len.min(HEAD_READ_BYTES);
    let mut head_buf = vec![0u8; head_len as usize];
    file.seek(SeekFrom::Start(0)).ok()?;
    read_exact_lossy(&mut file, &mut head_buf)?;
    let mut head_text = String::from_utf8_lossy(&head_buf).into_owned();
    // 파일이 head보다 크면 마지막(잘린) 라인을 폐기해 경계를 정렬한다.
    if file_len > head_len {
        if let Some(idx) = head_text.rfind('\n') {
            head_text.truncate(idx);
        }
    }

    // tail: EOF 역방향 최대 TAIL_READ_BYTES.
    let tail_len = file_len.min(TAIL_READ_BYTES);
    let tail_start = file_len - tail_len;
    let mut tail_buf = vec![0u8; tail_len as usize];
    file.seek(SeekFrom::Start(tail_start)).ok()?;
    read_exact_lossy(&mut file, &mut tail_buf)?;
    let mut tail_text = String::from_utf8_lossy(&tail_buf).into_owned();
    // 파일 시작이 아니면 첫(부분) 라인을 폐기해 개행 경계로 정렬한다.
    if tail_start > 0 {
        if let Some(idx) = tail_text.find('\n') {
            tail_text = tail_text[idx + 1..].to_string();
        }
    }

    Some((head_text, tail_text))
}

/// 버퍼를 끝까지 읽되 EOF/단축 읽기를 안전 처리한다(부분 읽기도 보존).
///
/// `read_exact`는 EOF에서 에러를 내지만, 동시 쓰기로 파일이 줄어든 경우에도 읽은 만큼은
/// 보존해야 하므로 루프로 채우고 더 못 읽으면 버퍼를 그만큼 잘라 반환한다(무패닉).
///
/// `ErrorKind::Interrupted`(EINTR)는 시그널(예: 자식 프로세스 종료로 인한 SIGCHLD)에 의한
/// 일시 중단이라 **데이터 손실이 아니다**. 이를 fatal `None`으로 취급하면 시스템 부하가 높을 때
/// rollout 읽기가 간헐적으로 실패할 수 있으므로, `Interrupted`는 재시도하고 그 외 진짜 I/O
/// 에러만 `None`으로 안전 저하한다(fail-safe 일관성).
fn read_exact_lossy(file: &mut File, buf: &mut Vec<u8>) -> Option<()> {
    let mut filled = 0usize;
    while filled < buf.len() {
        match file.read(&mut buf[filled..]) {
            Ok(0) => break,
            Ok(n) => filled += n,
            // EINTR: 시그널에 의한 일시 중단 → 데이터 손실 아님, 재시도.
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => return None,
        }
    }
    buf.truncate(filled);
    Some(())
}

/// 단일 rollout 파일에서 [`CodexSession`]을 추출한다(head+tail 결합, spec §8.1).
///
/// head에서 baseline model/effort(첫 turn_context)를, tail에서 최신 token_count + 더 최신
/// turn_context를 얻는다(tail 우선). token_count 전무(신생/exec)면 ctx/rate는 `None`(부분/생략).
///
/// # 반환
/// 추출 결과. 읽기 실패 시 `None`. 깨진 라인은 개별적으로 무시한다(무패닉).
fn extract_from_file(path: &Path) -> Option<CodexSession> {
    let (head_text, tail_text) = read_head_tail(path)?;

    // 1) baseline turn_context: head 앞쪽부터 첫 번째.
    let mut turn = head_text
        .lines()
        .find_map(parse_turn_context)
        .unwrap_or_default();
    // 2) tail에서 더 최신 turn_context가 있으면 그것이 우선(세션 중 /model 변경 반영).
    if let Some(latest_turn) = tail_text.lines().rev().find_map(parse_turn_context) {
        // 최신 값이 Some이면 덮어쓰되, 누락 필드는 baseline을 보존한다.
        if latest_turn.model.is_some() {
            turn.model = latest_turn.model;
        }
        if latest_turn.effort.is_some() {
            turn.effort = latest_turn.effort;
        }
    }

    // 3) 최신 token_count: tail 역방향 첫 번째(가장 최근).
    let snapshot = tail_text
        .lines()
        .rev()
        .find_map(parse_token_count)
        .unwrap_or_default();

    let context_percentage = match (snapshot.last_total_tokens, snapshot.context_window) {
        (Some(total), Some(window)) => compute_context_percentage(total, window),
        _ => None,
    };

    Some(CodexSession {
        model: turn.model,
        context_percentage,
        extras: CodexExtras {
            rate_5h_percent: snapshot.rate_5h_percent,
            rate_weekly_percent: snapshot.rate_weekly_percent,
            plan: snapshot.plan,
            effort: turn.effort,
        },
    })
}

/// 최근 `scan_days` 일자 디렉터리에서 cwd+freshness+originator에 부합하는 후보를 찾는다(spec §5).
///
/// # 인자(주입 — 테스트 격리)
/// - `base`: `$CODEX_HOME`(런타임은 [`codex_home`], 테스트는 tempdir).
/// - `cwd`: 매칭 대상 cwd(lterm payload).
/// - `now`: 현재 시각(SystemTime). freshness 비교 기준.
/// - `freshness`: mtime 신선도 상한(분).
/// - `scan_days`: 스캔할 최근 일자 디렉터리 수.
///
/// # 반환
/// 부합 후보 경로들. 1) 최근 scan_days 일자만(전체 4400+ 회피) 2) mtime freshness 선필터
/// 3) 첫 줄 cwd 정규화 일치 AND originator 화이트리스트. 외부 cwd는 비교에만 사용한다.
#[cfg(test)]
fn find_codex_candidates(
    base: &Path,
    cwd: &str,
    now: SystemTime,
    freshness: u64,
    scan_days: usize,
) -> Vec<PathBuf> {
    scan_codex_candidates(base, cwd, now, freshness, scan_days).candidates
}

/// 후보 스캔 + rollout 트리 지문 계산을 한 번에 수행한다.
///
/// 지문은 캐시 히트 시 새 rollout 추가/삭제/수정 여부를 확인하는 데 쓰이고, `budget_exceeded`
/// 는 부분 스캔으로 단일 후보를 확정하지 않도록 fail-safe 판정에 쓰인다.
fn scan_codex_candidates(
    base: &Path,
    cwd: &str,
    now: SystemTime,
    freshness: u64,
    scan_days: usize,
) -> CandidateScan {
    let (rollout_paths, fingerprint) = collect_rollout_scan(base, scan_days, true);
    let freshness_secs = freshness.saturating_mul(60);
    let normalized_target_cwd = normalize_cwd_for_match(cwd);
    let mut candidates = Vec::new();

    if fingerprint.budget_exceeded {
        return CandidateScan {
            candidates,
            fingerprint,
        };
    }

    for path in rollout_paths {
        // (2) cheap stat 선필터: mtime이 freshness 이내인 후보만.
        if !is_fresh(&path, now, freshness_secs) {
            continue;
        }
        // (3) 첫 줄 session_meta만 읽어 cwd 일치 + 대화형 originator를 확인한다.
        if let Some(meta) = read_first_line_meta(&path) {
            let cwd_ok = meta
                .cwd
                .as_deref()
                .map(|c| cwd_matches_normalized(c, &normalized_target_cwd))
                .unwrap_or(false);
            let originator_ok = is_interactive_originator(meta.originator.as_deref());
            if cwd_ok && originator_ok {
                candidates.push(path);
            }
        }
    }
    CandidateScan {
        candidates,
        fingerprint,
    }
}

/// 후보 본문을 읽지 않고 최근 rollout 트리의 캐시 검증용 지문만 계산한다.
fn current_scan_fingerprint(base: &Path, scan_days: usize) -> CodexScanFingerprint {
    collect_rollout_scan(base, scan_days, false).1
}

/// 최근 일자 디렉터리의 rollout 파일 경로(선택)와 스캔 지문을 수집한다.
fn collect_rollout_scan(
    base: &Path,
    scan_days: usize,
    collect_paths: bool,
) -> (Vec<PathBuf>, CodexScanFingerprint) {
    let sessions_dir = base.join("sessions");
    let day_dirs = recent_day_dirs(&sessions_dir, scan_days);
    let mut rollout_paths = Vec::new();
    let mut days = Vec::new();
    let mut files_seen = 0usize;
    let mut budget_exceeded = false;

    for day_dir in day_dirs {
        let mut day = CodexDayFingerprint {
            path: day_dir.to_string_lossy().into_owned(),
            mtime_ms: file_mtime_ms(&day_dir),
            rollout_files_seen: 0,
            rollout_mtime_sum_ms: 0,
        };
        let entries = match std::fs::read_dir(&day_dir) {
            Ok(entries) => entries,
            Err(_) => {
                days.push(day);
                continue;
            }
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !is_rollout_file(&path) {
                continue;
            }
            if files_seen >= MAX_ROLLOUT_SCAN_FILES {
                budget_exceeded = true;
                break;
            }
            files_seen += 1;
            day.rollout_files_seen += 1;
            if let Some(mtime) = file_mtime_ms(&path) {
                day.rollout_mtime_sum_ms = day.rollout_mtime_sum_ms.wrapping_add(mtime);
            }
            if collect_paths {
                rollout_paths.push(path);
            }
        }
        days.push(day);
        if budget_exceeded {
            break;
        }
    }

    (
        rollout_paths,
        CodexScanFingerprint {
            days,
            budget_exceeded,
        },
    )
}

/// `sessions/<year>/<month>/<day>` 트리에서 최근 `scan_days`개 일자 디렉터리를 내림차순으로 모은다.
///
/// 연도 desc → 월 desc → 일 desc로 정렬해 최신부터 `scan_days`개만 취한다(전체 풀스캔 회피, spec §5).
/// 폴더는 세션 시작시각 기준이라 scan_days 밖 장기 활성 세션은 미발견(알려진 한계, spec §10 S1).
fn recent_day_dirs(sessions_dir: &Path, scan_days: usize) -> Vec<PathBuf> {
    let mut result = Vec::new();
    if scan_days == 0 {
        return result;
    }
    // 연도 디렉터리 내림차순.
    for year_dir in sorted_subdirs_desc(sessions_dir) {
        for month_dir in sorted_subdirs_desc(&year_dir) {
            for day_dir in sorted_subdirs_desc(&month_dir) {
                result.push(day_dir);
                if result.len() >= scan_days {
                    return result;
                }
            }
        }
    }
    result
}

/// 디렉터리의 하위 디렉터리를 이름 내림차순으로 반환한다(`2026` > `2025`, `06` > `05`).
///
/// 이름이 zero-padded 날짜(`06`/`05`/`30`)라 문자열 desc 정렬이 곧 시간 desc와 일치한다.
fn sorted_subdirs_desc(dir: &Path) -> Vec<PathBuf> {
    let mut subdirs: Vec<PathBuf> = match std::fs::read_dir(dir) {
        Ok(entries) => entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect(),
        Err(_) => return Vec::new(),
    };
    // 이름 내림차순(최신 우선). file_name 기준으로 비교한다.
    subdirs.sort_by(|a, b| b.file_name().cmp(&a.file_name()));
    subdirs
}

/// 경로가 `rollout-*.jsonl` 형식인지 판정한다.
fn is_rollout_file(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    let name = match path.file_name().and_then(|n| n.to_str()) {
        Some(name) => name,
        None => return false,
    };
    name.starts_with("rollout-") && name.ends_with(".jsonl")
}

/// 파일 mtime이 `now`로부터 `freshness_secs` 이내인지 판정한다(cheap stat 선필터).
///
/// 시계 이상/메타데이터 부재 시 보수적으로 `false`(제외). 미래 mtime(now보다 나중)은 fresh로 본다.
fn is_fresh(path: &Path, now: SystemTime, freshness_secs: u64) -> bool {
    let modified = match path.metadata().and_then(|m| m.modified()) {
        Ok(m) => m,
        Err(_) => return false,
    };
    match now.duration_since(modified) {
        // mtime이 과거: 경과가 freshness 이내면 fresh.
        Ok(elapsed) => elapsed.as_secs() <= freshness_secs,
        // mtime이 미래(now보다 나중): 동시 쓰기 등 → fresh로 본다.
        Err(_) => true,
    }
}

/// rollout 파일의 첫 줄(session_meta)만 읽어 cwd/originator를 파싱한다(매칭 선필터용).
///
/// 첫 줄은 inline `base_instructions` 때문에 실측 ~33KB에 달하므로([`FIRST_LINE_READ_BYTES`]
/// 참조), 그 상한까지 읽되 **개행으로 완결된 첫 줄이 잡힐 때만** 파싱한다. 개행 미발견(상한 내
/// 첫 줄 미완결)이면 부분 JSON을 파싱하지 않고 `None`(무패닉, 보수적 제외). 읽기/파싱 실패도 `None`.
fn read_first_line_meta(path: &Path) -> Option<SessionMeta> {
    let file = File::open(path).ok()?;
    let reader = BufReader::new(file);
    // `read_until`은 개행을 만날 때까지만 버퍼를 키운다. 이전처럼 후보마다 고정 128KiB를
    // 선할당하지 않으므로 다수 후보 스캔에서 불필요한 메모리 churn을 줄인다.
    let mut limited = reader.take(FIRST_LINE_READ_BYTES);
    let mut buf = Vec::with_capacity(4 * 1024);
    let bytes_read = limited.read_until(b'\n', &mut buf).ok()?;
    if bytes_read == 0 {
        return None;
    }
    let has_newline = buf.last() == Some(&b'\n');
    if !has_newline && bytes_read as u64 >= FIRST_LINE_READ_BYTES {
        // 상한에 걸린 미완성 JSON은 파싱하지 않는다(부분 라인 신뢰 금지).
        return None;
    }
    if has_newline {
        buf.pop();
        if buf.last() == Some(&b'\r') {
            buf.pop();
        }
    }
    let first_line = String::from_utf8_lossy(&buf);
    parse_session_meta(&first_line)
}

/// 후보를 스캔하고 모호성을 판정해 [`Resolution`]으로 해소한다(spec §5).
///
/// # 반환
/// - 후보 정확히 1개 → `Single`(추출 성공 시). 추출 실패 시 `None`.
/// - 후보 ≥2개 → `Ambiguous`(enrich 전면 생략).
/// - 후보 0개 → `None`.
fn read_codex_session(
    base: &Path,
    cwd: &str,
    now: SystemTime,
    freshness: u64,
    scan_days: usize,
) -> Resolution {
    let scan = scan_codex_candidates(base, cwd, now, freshness, scan_days);
    resolution_from_scan(&scan)
}

/// 완료된 후보 스캔을 단일/모호/없음 해소 결과로 변환한다.
fn resolution_from_scan(scan: &CandidateScan) -> Resolution {
    if scan.fingerprint.budget_exceeded {
        return Resolution::Ambiguous;
    }
    match scan.candidates.len() {
        0 => Resolution::None,
        1 => {
            let path = &scan.candidates[0];
            match extract_from_file(path) {
                Some(session) => Resolution::Single(session, path.clone()),
                None => Resolution::None,
            }
        }
        // 동일 cwd·fresh 후보 ≥2 → "이 페인의 codex 식별 불가" → fail-wrong→fail-safe.
        _ => Resolution::Ambiguous,
    }
}

// ============================== 디스크 캐시(spec §8) ==============================

/// 캐시 본문(serde_json 1라인 직렬화). 해소된 rollout 경로 + mtime + 파싱 결과.
///
/// 정상상태(경로 mtime 불변 & freshness 이내)에는 stat 1회로 재사용된다(spec §8). 역직렬화
/// 실패(스키마 드리프트)는 lenient로 무시 → 풀 재해소(캐시 버저닝 불필요).
#[derive(Debug, Clone, Serialize, Deserialize)]
struct CodexCacheEntry {
    /// 해소된 rollout 파일 경로.
    path: String,
    /// 그 파일의 mtime(epoch ms). 불변이면 재파싱 없이 재사용.
    mtime_ms: u128,
    /// 단일 후보를 확정했던 시점의 최근 rollout 트리 지문.
    #[serde(default)]
    scan_fingerprint: Option<CodexScanFingerprint>,
    /// 캐시된 파싱 결과.
    session: CodexSession,
}

/// 파일 mtime을 epoch ms로 반환한다(캐시 무효화 키). 실패 시 `None`.
fn file_mtime_ms(path: &Path) -> Option<u128> {
    let modified = path.metadata().and_then(|m| m.modified()).ok()?;
    modified
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|d| d.as_millis())
}

/// 캐시 히트 시 해소된 파일의 mtime(epoch ms)이 여전히 freshness 이내인지 판정한다(spec §5/§8).
///
/// 캐시 신선도 게이트([`is_named_cache_fresh`])는 **캐시 기록 시각**(`written_ms`) 기준이라,
/// 세션 종료 후 파일 mtime이 고정돼도 마지막 캐시 write로부터 freshness 동안 stale 세션을
/// 계속 표시하는 결함이 있다. 이를 막기 위해 캐시 히트 재사용/재독 전에 **파일 자체의 mtime**이
/// freshness 이내인지 [`find_codex_candidates`]의 선필터(`is_fresh`)와 동일 기준으로 재검증한다.
/// 이미 `file_mtime_ms`로 stat한 결과를 그대로 받으므로 추가 syscall은 없다(핫패스 비용 불변).
///
/// # 인자
/// - `mtime_ms`: 해소된 rollout 파일의 mtime(epoch ms).
/// - `now`: 현재 시각(SystemTime). freshness 비교 기준.
/// - `freshness_secs`: 신선도 상한(초).
///
/// # 반환
/// 미래 mtime(now보다 나중, 동시 쓰기 등)은 fresh로 본다. `now`의 epoch 변환 실패 시 보수적
/// 으로 `false`(캐시 무시 → 풀 재해소).
fn is_mtime_fresh(mtime_ms: u128, now: SystemTime, freshness_secs: u64) -> bool {
    let now_ms = match now.duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_millis(),
        Err(_) => return false,
    };
    // 미래 mtime(now보다 나중): 동시 쓰기 등 → fresh로 본다(is_fresh와 동일 정책).
    if mtime_ms >= now_ms {
        return true;
    }
    let elapsed_secs = (now_ms - mtime_ms) / 1000;
    elapsed_secs <= freshness_secs as u128
}

/// 세션 데이터를 디스크 캐시에서 조회/갱신해 [`CodexSession`]을 반환한다(spec §8 매 틱 로직).
///
/// 1) 캐시 히트 & rollout 트리 지문 불변 & 경로 mtime 불변 & **파일 mtime freshness 이내**
///    → 재사용(후보 본문 재독해 없음).
/// 2) 캐시 히트 & mtime 변동 & **파일 mtime freshness 이내** → 그 파일만 tail 재독 → 캐시 갱신.
/// 3) 미스/트리 지문 변동/경로 stale/**파일 mtime stale**/없음 → 풀 후보스캔 재해소.
///    **Ambiguous는 캐시하지 않는다**.
///
/// **파일 freshness 재검증(spec §5 일관성)**: 캐시 신선도는 기록 시각 기준이라, 캐시 히트 시
/// 해소된 파일의 mtime이 여전히 freshness 이내인지([`is_mtime_fresh`]) 추가 검증한다. stale이면
/// 캐시를 무시하고 (3) 풀 재해소로 떨어진다 — 종료된 세션은 freshness 경과 후 더는 표시되지 않는다.
///
/// **모호성 재검증**: 캐시된 파일 자체가 그대로여도 같은 cwd의 새 rollout이 생기면 이전 단일
/// 후보 판단은 무효다. 최근 rollout 트리 지문이 바뀌면 캐시를 바로 쓰지 않고 후보 스캔을 다시
/// 수행해 새 모호성을 fail-safe로 반영한다.
///
/// # 반환
/// 단일 해소 시 `Some(session)`. 모호/없음 시 `None`(무변경 신호).
fn resolve_with_cache(
    base: &Path,
    cache_base: &Path,
    session_key: &str,
    cwd: &str,
    now: SystemTime,
    freshness: u64,
    scan_days: usize,
) -> Option<CodexSession> {
    let now_ms = cache_now_millis();
    let freshness_secs = freshness.saturating_mul(60);

    // 캐시 조회.
    if let Some((written_ms, payload)) =
        read_session_named_cache_in(cache_base, session_key, CODEX_CACHE_FILE)
    {
        if is_named_cache_fresh(written_ms, now_ms, freshness_secs) {
            if let Ok(entry) = serde_json::from_str::<CodexCacheEntry>(&payload) {
                let current_fingerprint = current_scan_fingerprint(base, scan_days);
                let fingerprint_stable = entry
                    .scan_fingerprint
                    .as_ref()
                    .map(|cached| cached == &current_fingerprint)
                    .unwrap_or(false);
                if fingerprint_stable && !current_fingerprint.budget_exceeded {
                    let cached_path = PathBuf::from(&entry.path);
                    match file_mtime_ms(&cached_path) {
                        // 파일 mtime이 stale(freshness 경과)이면 캐시를 무시하고 풀 재해소로 폴백한다
                        // (종료된 세션의 stale 표시 차단, find_codex_candidates 선필터와 일관).
                        Some(current_mtime)
                            if !is_mtime_fresh(current_mtime, now, freshness_secs) => {}
                        // (1) 트리 지문/경로 mtime 불변 & fresh → 재사용(후보 본문 재독해 없음).
                        Some(current_mtime) if current_mtime == entry.mtime_ms => {
                            return Some(entry.session);
                        }
                        // (2) mtime 변동(파일 존재) & fresh → 그 파일만 tail 재독 → 캐시 갱신.
                        Some(current_mtime) => {
                            if let Some(session) = extract_from_file(&cached_path) {
                                write_cache_entry(
                                    cache_base,
                                    session_key,
                                    &cached_path,
                                    current_mtime,
                                    &session,
                                    &current_fingerprint,
                                    now_ms,
                                );
                                return Some(session);
                            }
                        }
                        // 경로 소실 → 풀 재해소로 폴백.
                        None => {}
                    }
                }
            }
        }
    }

    // (3) 미스/stale/경로 소실 → 풀 후보스캔 재해소.
    let scan = scan_codex_candidates(base, cwd, now, freshness, scan_days);
    match resolution_from_scan(&scan) {
        Resolution::Single(session, path) => {
            // 단일 해소만 캐시한다(모호는 비캐시 — TTL 고착 차단).
            if let Some(mtime) = file_mtime_ms(&path) {
                write_cache_entry(
                    cache_base,
                    session_key,
                    &path,
                    mtime,
                    &session,
                    &scan.fingerprint,
                    now_ms,
                );
            }
            Some(session)
        }
        // Ambiguous/None → 캐시하지 않고 무변경 신호.
        Resolution::Ambiguous | Resolution::None => None,
    }
}

/// 해소 결과를 디스크 캐시에 1라인 직렬화로 기록한다(best-effort, 실패 무시).
///
/// `cache_base`는 캐시 루트(런타임은 `chain::cache_dir()`, 테스트는 주입 tempdir)로,
/// process-global `HOME` 비의존 hermetic 기록을 위해 [`write_session_named_cache_in`]에 위임한다.
fn write_cache_entry(
    cache_base: &Path,
    session_key: &str,
    path: &Path,
    mtime_ms: u128,
    session: &CodexSession,
    scan_fingerprint: &CodexScanFingerprint,
    now_ms: u128,
) {
    let entry = CodexCacheEntry {
        path: path.to_string_lossy().into_owned(),
        mtime_ms,
        scan_fingerprint: Some(scan_fingerprint.clone()),
        session: session.clone(),
    };
    if let Ok(payload) = serde_json::to_string(&entry) {
        write_session_named_cache_in(cache_base, session_key, CODEX_CACHE_FILE, now_ms, &payload);
    }
}

// ============================== 통합(spec §7) ==============================

/// `$CODEX_HOME`(env) 또는 `~/.codex`를 반환한다.
///
/// # 반환
/// `CODEX_HOME` 환경변수가 있으면 그 경로, 없으면 `$HOME/.codex`. HOME 미설정 시 `None`.
fn codex_home() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("CODEX_HOME") {
        return Some(PathBuf::from(path));
    }
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(".codex"))
}

/// model_display_name이 codex 계열인지 판정한다(prefix/정규화 매칭, 정확 동등 아님 — spec §7 A-2).
///
/// lterm payload의 agent를 model 슬롯에 매핑하므로(`parse_lterm_input`), 보통 정확히 `"codex"`다.
/// 다만 미래 변형(`codex-...`)도 받도록 소문자 prefix로 관대하게 매칭한다.
fn is_codex_model(model: &str) -> bool {
    model.trim().to_ascii_lowercase().starts_with("codex")
}

/// Codex 세션을 판독해 [`ClaudeInput`]을 in-place로 enrich한다(spec §7 게이팅).
///
/// # 게이팅(이중 + observability)
/// - 호출부([`crate::main`])에서 **`Source::Lterm`·`Source::Codex`로 한정**해 호출한다(Claude 경로 오발동 차단).
/// - 추가로 `cfg.codex.enabled` && model이 codex 계열 && `input.cwd=Some` && `codex_home()` 존재.
///
/// 단일 해소면 `model_display_name`/`context_used_percentage`/`codex`를 설정한다. 모호/없음/실패
/// 시 무변경(기존 lterm 출력 보존). 실패/모호 시 `LTERM_STATUS_DEBUG` 설정 하에 stderr 1줄.
///
/// # 인자
/// - `input`: enrich 대상(이미 parse_lterm_input으로 채워진 상태).
/// - `cfg`: `[codex]` 토글/freshness/scan_days.
pub fn maybe_enrich(input: &mut ClaudeInput, cfg: &Config) {
    // 프로덕션 경로: codex base/cache base override를 주입하지 않는다(None → 런타임 기본).
    // None일 때 동작은 리팩터 이전과 100% 동일하다(codex base=codex_home(), cache base=cache_dir()).
    maybe_enrich_in(input, cfg, None, None);
}

/// [`maybe_enrich`]의 base 주입 변형(프로덕션 호출은 None override, 테스트는 tempdir 주입).
///
/// process-global `HOME`/`CODEX_HOME` env를 만지지 않고 codex sessions base와 캐시 base를 직접
/// 주입할 수 있게 해, in-process 통합 테스트가 hermetic하게(전역 env 무변경, 락 불필요) 돌게 한다.
/// 코드베이스의 기존 주입 패턴([`find_codex_candidates`]의 base, [`crate::chain::write_session_named_cache_in`])과 일관된다.
///
/// # 인자
/// - `input`/`cfg`: [`maybe_enrich`]와 동일.
/// - `codex_base_override`: codex sessions 루트. `None`이면 런타임 [`codex_home`].
/// - `cache_base_override`: 캐시 루트. `None`이면 런타임 [`crate::chain::cache_dir`].
///
/// # 동작 불변
/// 두 override가 모두 `None`이면 게이팅·캐시 위치·스캔이 리팩터 이전과 100% 동일하다.
/// 캐시 base 부재(`HOME` 미설정)는 enrich 생략으로 안전 저하한다(이전엔 캐시 read/write가 no-op).
fn maybe_enrich_in(
    input: &mut ClaudeInput,
    cfg: &Config,
    codex_base_override: Option<&Path>,
    cache_base_override: Option<&Path>,
) {
    // 게이팅 1: opt-out.
    if !cfg.codex.enabled {
        return;
    }
    // 게이팅 2: model이 codex 계열이 아니면 무접촉.
    let is_codex = input
        .model_display_name
        .as_deref()
        .map(is_codex_model)
        .unwrap_or(false);
    if !is_codex {
        return;
    }
    // 게이팅 3: cwd 부재면 매칭 불가.
    let cwd = match input.cwd.as_deref() {
        Some(cwd) if !cwd.is_empty() => cwd.to_string(),
        _ => return,
    };
    // 게이팅 4: CODEX_HOME(또는 주입 base) 존재.
    let base = match codex_base_override.map(PathBuf::from).or_else(codex_home) {
        Some(base) if base.exists() => base,
        _ => {
            debug_log("codex_home 부재 — enrich 생략");
            return;
        }
    };
    // session_key는 캐시 격리용(lterm payload 유래). 부재 시 cwd 기반으로 안정화한다.
    let session_key = input.session_id.clone().unwrap_or_else(|| cwd.clone());

    // 캐시 base 결정(주입 우선, 없으면 런타임 cache_dir).
    let cache_base = cache_base_override
        .map(PathBuf::from)
        .or_else(crate::chain::cache_dir);

    let resolved = match cache_base {
        // 캐시 base 존재 → 정상 경로(캐시 read/write 경유).
        Some(cache_base) => resolve_with_cache(
            &base,
            &cache_base,
            &session_key,
            &cwd,
            SystemTime::now(),
            cfg.codex.freshness_minutes,
            cfg.codex.scan_days,
        ),
        // 캐시 base 부재(HOME 미설정 등): 캐시 없이 직접 해소한다. 리팩터 이전에도 cache_dir이
        // None이면 캐시 read/write가 no-op이라 캐시 없이 해소했으므로 동작이 100% 동일하다.
        None => match read_codex_session(
            &base,
            &cwd,
            SystemTime::now(),
            cfg.codex.freshness_minutes,
            cfg.codex.scan_days,
        ) {
            Resolution::Single(session, _path) => Some(session),
            Resolution::Ambiguous | Resolution::None => None,
        },
    };

    match resolved {
        Some(session) => {
            // 단일 해소: model/ctx는 기존 슬롯, 나머지는 CodexExtras로.
            if let Some(model) = session.model {
                input.model_display_name = Some(model);
            }
            input.context_used_percentage = session.context_percentage;
            input.codex = Some(session.extras);
        }
        None => {
            // 모호/없음/실패: 무변경(기존 lterm 출력 = model "codex").
            debug_log("Codex 세션 해소 실패/모호 — enrich 생략");
        }
    }
}

/// `LTERM_STATUS_DEBUG` 환경변수가 설정된 경우에만 stderr에 진단 1줄을 출력한다(silent off 방지).
///
/// 정상 핫패스에선 무출력(env 미설정 시 no-op). 실패/모호 경로의 가시성만 제공한다(spec §7).
fn debug_log(message: &str) {
    if std::env::var_os("LTERM_STATUS_DEBUG").is_some() {
        eprintln!("understatus[codex]: {message}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::time::Duration;

    // ===== 픽스처 헬퍼: 실측 jsonl 포맷(spec §4 검증본)과 동일 구조 =====

    /// session_meta 첫 줄(cwd/originator) 픽스처.
    fn session_meta_line(cwd: &str, originator: &str) -> String {
        format!(
            r#"{{"timestamp":"2026-06-05T11:41:50.379Z","type":"session_meta","payload":{{"id":"abc","cwd":"{cwd}","originator":"{originator}","cli_version":"0.137.0"}}}}"#
        )
    }

    /// 실데이터 회귀용: inline base_instructions로 16KB를 넘는 거대 session_meta 첫 줄 픽스처.
    ///
    /// 실측상 실제 첫 줄은 ~33KB라 16KB head 상한으로는 매칭에 실패했다(회귀 차단).
    fn big_session_meta_line(cwd: &str, originator: &str) -> String {
        // 32KB짜리 base_instructions 본문(첫 줄을 head 16KB 한참 너머로 키운다).
        let big_instructions = "A".repeat(32 * 1024);
        format!(
            r#"{{"timestamp":"2026-06-05T11:41:50.379Z","type":"session_meta","payload":{{"id":"abc","cwd":"{cwd}","originator":"{originator}","cli_version":"0.137.0","base_instructions":{{"text":"{big_instructions}"}}}}}}"#
        )
    }

    /// turn_context 라인(model/effort) 픽스처.
    fn turn_context_line(model: &str, effort: &str) -> String {
        format!(
            r#"{{"type":"turn_context","payload":{{"turn_id":"t1","model":"{model}","effort":"{effort}","summary":"auto"}}}}"#
        )
    }

    /// 표준 token_count 이벤트(info 중첩 + rate_limits named 객체) 픽스처.
    fn token_count_line(
        last_total: u64,
        window: u64,
        total_cumulative: u64,
        rate_5h: f64,
        rate_weekly: f64,
        plan: &str,
    ) -> String {
        format!(
            r#"{{"type":"event_msg","payload":{{"type":"token_count","info":{{"total_token_usage":{{"total_tokens":{total_cumulative}}},"last_token_usage":{{"input_tokens":1,"total_tokens":{last_total}}},"model_context_window":{window}}},"rate_limits":{{"limit_id":"codex","primary":{{"used_percent":{rate_5h},"window_minutes":300,"resets_at":1}},"secondary":{{"used_percent":{rate_weekly},"window_minutes":10080,"resets_at":2}},"plan_type":"{plan}"}}}}}}"#
        )
    }

    /// 고유 임시 디렉터리를 만든다(테스트별 격리, 호출자가 정리).
    fn unique_tmp(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "understatus-codex-{tag}-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).expect("임시 디렉터리 생성 실패");
        dir
    }

    /// `<base>/sessions/2026/06/05/rollout-<tag>.jsonl`에 주어진 라인들을 기록한다.
    fn write_rollout(base: &Path, tag: &str, lines: &[String]) -> PathBuf {
        let day_dir = base.join("sessions").join("2026").join("06").join("05");
        std::fs::create_dir_all(&day_dir).expect("일자 디렉터리 생성 실패");
        let path = day_dir.join(format!("rollout-2026-06-05T20-40-45-{tag}.jsonl"));
        let mut file = std::fs::File::create(&path).expect("rollout 파일 생성 실패");
        for line in lines {
            writeln!(file, "{line}").expect("rollout 라인 쓰기 실패");
        }
        path
    }

    // ============== Unit: 순수 파서(AC-X2/X3/X4/X7) ==============

    /// AC-X3: last_token_usage.total_tokens(info 중첩) / model_context_window 정확 파싱.
    /// 11000/40000 = 27.5%.
    #[test]
    fn parse_token_count_nested_info_27_5_percent() {
        let line = token_count_line(11_000, 40_000, 9_999_999, 3.0, 21.0, "pro");
        let snap = parse_token_count(&line).expect("token_count 파싱");
        assert_eq!(snap.last_total_tokens, Some(11_000));
        assert_eq!(snap.context_window, Some(40_000));
        let ctx = compute_context_percentage(11_000, 40_000).expect("ctx%");
        assert!((ctx - 27.5).abs() < 1e-9, "ctx%는 27.5여야 함: {ctx}");
    }

    /// AC-X2: total_token_usage(누적값)는 절대 사용하지 않는다(210% fixture 회귀).
    /// total_token_usage=84000/window=40000=210%지만 last_token_usage=11000=27.5%만 써야 한다.
    #[test]
    fn parse_token_count_ignores_total_token_usage() {
        let line = token_count_line(11_000, 40_000, 84_000, 3.0, 21.0, "pro");
        let snap = parse_token_count(&line).expect("token_count 파싱");
        // 누적값(84000)이 아니라 last_total(11000)만 읽혀야 한다.
        assert_eq!(snap.last_total_tokens, Some(11_000));
        assert_ne!(
            snap.last_total_tokens,
            Some(84_000),
            "total_token_usage(누적) 오용 금지"
        );
        let ctx = compute_context_percentage(snap.last_total_tokens.unwrap(), 40_000).unwrap();
        assert!(ctx < 100.0, "100% 초과 불가(누적값 미사용): {ctx}");
    }

    /// AC-X4: rate_limits의 window_minutes로 5h(300)/주간(10080)을 식별한다(primary=5h 단정 금지).
    #[test]
    fn parse_token_count_identifies_rate_windows_by_minutes() {
        let line = token_count_line(100, 1000, 0, 3.0, 21.0, "pro");
        let snap = parse_token_count(&line).expect("token_count 파싱");
        assert_eq!(snap.rate_5h_percent, Some(3.0));
        assert_eq!(snap.rate_weekly_percent, Some(21.0));
        assert_eq!(snap.plan.as_deref(), Some("pro"));
    }

    /// rate_limits에서 primary/secondary의 window_minutes가 뒤바뀌어도 minutes로 정확히 식별한다.
    /// (primary가 주간, secondary가 5h여도 300→5h/10080→주간으로 배정되어야 함.)
    #[test]
    fn parse_token_count_window_swap_still_identified() {
        let line = r#"{"type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"total_tokens":5},"model_context_window":100},"rate_limits":{"primary":{"used_percent":55.0,"window_minutes":10080},"secondary":{"used_percent":7.0,"window_minutes":300}}}}"#;
        let snap = parse_token_count(line).expect("token_count 파싱");
        // primary가 주간이어도 window_minutes로 식별 → 5h=7.0, 주간=55.0.
        assert_eq!(snap.rate_5h_percent, Some(7.0));
        assert_eq!(snap.rate_weekly_percent, Some(55.0));
    }

    /// rate_limits 부재 → rate/plan 모두 None(부분 추출, 무패닉).
    #[test]
    fn parse_token_count_missing_rate_limits_is_none() {
        let line = r#"{"type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"total_tokens":5},"model_context_window":100}}}"#;
        let snap = parse_token_count(line).expect("token_count 파싱");
        assert_eq!(snap.rate_5h_percent, None);
        assert_eq!(snap.rate_weekly_percent, None);
        assert_eq!(snap.plan, None);
        assert_eq!(snap.last_total_tokens, Some(5));
    }

    /// event_msg가 아니거나 payload.type!=token_count면 None(게이팅).
    #[test]
    fn parse_token_count_gating_rejects_non_token_count() {
        // type이 event_msg가 아님.
        assert!(parse_token_count(&turn_context_line("gpt-5.5", "high")).is_none());
        // event_msg지만 payload.type이 다름.
        let other = r#"{"type":"event_msg","payload":{"type":"agent_message","text":"hi"}}"#;
        assert!(parse_token_count(other).is_none());
    }

    /// compute_context_percentage: window==0 → None(0 나눗셈 가드).
    #[test]
    fn compute_context_percentage_window_zero_is_none() {
        assert_eq!(compute_context_percentage(100, 0), None);
        assert_eq!(compute_context_percentage(0, 100), Some(0.0));
        let half = compute_context_percentage(50, 100).unwrap();
        assert!((half - 50.0).abs() < 1e-9);
    }

    /// parse_session_meta: cwd/originator 추출 + type 게이팅.
    #[test]
    fn parse_session_meta_extracts_cwd_and_originator() {
        let line = session_meta_line("/Users/me/proj", "codex-tui");
        let meta = parse_session_meta(&line).expect("session_meta 파싱");
        assert_eq!(meta.cwd.as_deref(), Some("/Users/me/proj"));
        assert_eq!(meta.originator.as_deref(), Some("codex-tui"));
        // type이 session_meta가 아니면 None.
        assert!(parse_session_meta(&turn_context_line("m", "e")).is_none());
    }

    /// parse_turn_context: model/effort 추출 + 부분 누락 안전.
    #[test]
    fn parse_turn_context_extracts_model_effort() {
        let full = parse_turn_context(&turn_context_line("gpt-5.5", "xhigh")).expect("파싱");
        assert_eq!(full.model.as_deref(), Some("gpt-5.5"));
        assert_eq!(full.effort.as_deref(), Some("xhigh"));
        // effort 누락도 안전(model만).
        let partial =
            parse_turn_context(r#"{"type":"turn_context","payload":{"model":"gpt-5.5"}}"#)
                .expect("부분 파싱");
        assert_eq!(partial.model.as_deref(), Some("gpt-5.5"));
        assert_eq!(partial.effort, None);
    }

    /// is_interactive_originator: codex-tui prefix만 통과, exec 등 제외.
    #[test]
    fn interactive_originator_whitelist() {
        assert!(is_interactive_originator(Some("codex-tui")));
        assert!(is_interactive_originator(Some("codex-tui-0.137")));
        assert!(!is_interactive_originator(Some("codex_exec")));
        assert!(!is_interactive_originator(Some("codex-exec")));
        assert!(!is_interactive_originator(None));
    }

    /// AC-X7: 깨진/미상 cli_version 변형/타입 드리프트 → None, 무패닉.
    #[test]
    fn drifted_or_broken_lines_no_panic() {
        // 완전히 깨진 JSON.
        assert!(parse_token_count("{not json").is_none());
        assert!(parse_session_meta("garbage").is_none());
        assert!(parse_turn_context("[1,2,3]").is_none());
        // 빈 줄.
        assert!(parse_token_count("").is_none());
        // 타입 드리프트: total_tokens가 문자열이면 as_u64 실패 → None(전체 무패닉).
        let drift = r#"{"type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"total_tokens":"oops"},"model_context_window":"big"}}}"#;
        let snap = parse_token_count(drift).expect("게이팅은 통과");
        assert_eq!(snap.last_total_tokens, None);
        assert_eq!(snap.context_window, None);
        // 미상 cli_version/추가 필드는 무시되고 정상 필드는 보존된다.
        let versioned = r#"{"type":"turn_context","payload":{"model":"gpt-6","effort":"max","new_field_v999":{"x":1}}}"#;
        let tc = parse_turn_context(versioned).expect("드리프트 무패닉");
        assert_eq!(tc.model.as_deref(), Some("gpt-6"));
    }

    /// is_mtime_fresh: 파일 mtime의 freshness 판정(과거 경과/미래/경계).
    #[test]
    fn is_mtime_fresh_judges_by_elapsed() {
        let now = SystemTime::now();
        let now_ms = now.duration_since(UNIX_EPOCH).unwrap().as_millis();
        let freshness_secs = 240 * 60; // 240분.
                                       // 1분 전 mtime → fresh.
        assert!(is_mtime_fresh(now_ms - 60_000, now, freshness_secs));
        // 5시간 전 mtime → stale(240분 초과).
        assert!(!is_mtime_fresh(
            now_ms - 5 * 3600 * 1000,
            now,
            freshness_secs
        ));
        // 정확히 freshness 경계(240분) → 이내로 본다.
        assert!(is_mtime_fresh(
            now_ms - freshness_secs as u128 * 1000,
            now,
            freshness_secs
        ));
        // 미래 mtime(now보다 나중, 동시 쓰기 등) → fresh로 본다.
        assert!(is_mtime_fresh(now_ms + 60_000, now, freshness_secs));
    }

    /// cwd_matches: trailing slash 정규화(존재 경로는 canonicalize, 부재는 trim 비교).
    #[test]
    fn cwd_matches_normalizes_trailing_slash() {
        // 부재 경로는 trim 문자열 비교로 폴백.
        assert!(cwd_matches("/no/such/dir", "/no/such/dir/"));
        assert!(cwd_matches("/no/such/dir/", "/no/such/dir"));
        assert!(!cwd_matches("/no/such/dir", "/other/dir"));
    }

    // ============== Unit(IO): find/extract(AC-X1/X5) ==============

    /// 단일 정상 후보 → 1개 발견.
    #[test]
    fn find_candidates_single_match() {
        let base = unique_tmp("find-single");
        let cwd = "/Users/me/projA";
        write_rollout(
            &base,
            "single",
            &[
                session_meta_line(cwd, "codex-tui"),
                turn_context_line("gpt-5.5", "high"),
                token_count_line(100, 1000, 0, 3.0, 21.0, "pro"),
            ],
        );
        let found = find_codex_candidates(&base, cwd, SystemTime::now(), 240, 3);
        assert_eq!(found.len(), 1, "단일 후보여야 함");
        let _ = std::fs::remove_dir_all(&base);
    }

    /// 회귀: 거대(>16KB) session_meta 첫 줄도 매칭에 성공해야 한다(실데이터 ~33KB 첫 줄).
    ///
    /// 실데이터의 첫 줄은 inline base_instructions로 ~33KB라, 16KB head 상한으로 첫 줄을
    /// 잘라 파싱하면 매칭이 항상 실패한다(피처 무력화). FIRST_LINE_READ_BYTES로 첫 줄 전체를
    /// 읽어 cwd/originator를 정확히 추출함을 박제한다.
    #[test]
    fn find_candidates_with_huge_first_line() {
        let base = unique_tmp("bigmeta");
        let cwd = "/Users/me/projBigMeta";
        write_rollout(
            &base,
            "bigmeta",
            &[
                big_session_meta_line(cwd, "codex-tui"),
                turn_context_line("gpt-5.5", "high"),
                token_count_line(275, 1000, 0, 3.0, 21.0, "pro"),
            ],
        );
        let found = find_codex_candidates(&base, cwd, SystemTime::now(), 240, 3);
        assert_eq!(found.len(), 1, "거대 첫 줄도 cwd/originator 매칭 성공");
        // 전체 추출도 무패닉.
        let session = extract_from_file(&found[0]).expect("추출");
        assert_eq!(session.extras.rate_5h_percent, Some(3.0));
        let _ = std::fs::remove_dir_all(&base);
    }

    /// AC-X1: 동일 cwd·fresh 후보 2개 → Ambiguous → enrich 생략(ctx/rate 미표시).
    #[test]
    fn ambiguous_two_same_cwd_candidates() {
        let base = unique_tmp("ambiguous");
        let cwd = "/Users/me/projDup";
        for tag in ["dup1", "dup2"] {
            write_rollout(
                &base,
                tag,
                &[
                    session_meta_line(cwd, "codex-tui"),
                    turn_context_line("gpt-5.5", "high"),
                    token_count_line(100, 1000, 0, 3.0, 21.0, "pro"),
                ],
            );
        }
        let found = find_codex_candidates(&base, cwd, SystemTime::now(), 240, 3);
        assert_eq!(found.len(), 2, "동일 cwd 2 후보");
        // read_codex_session은 Ambiguous를 반환해야 한다(fail-wrong→fail-safe).
        let resolution = read_codex_session(&base, cwd, SystemTime::now(), 240, 3);
        assert_eq!(resolution, Resolution::Ambiguous);
        let _ = std::fs::remove_dir_all(&base);
    }

    /// stale(freshness 초과 mtime) 후보는 제외된다.
    #[test]
    fn stale_candidate_excluded() {
        let base = unique_tmp("stale");
        let cwd = "/Users/me/projStale";
        let path = write_rollout(
            &base,
            "stale",
            &[
                session_meta_line(cwd, "codex-tui"),
                token_count_line(100, 1000, 0, 3.0, 21.0, "pro"),
            ],
        );
        // mtime을 2시간 전으로 설정하고 freshness=60분 → stale.
        let two_hours_ago = SystemTime::now() - Duration::from_secs(2 * 3600);
        set_file_mtime(&path, two_hours_ago);
        let found = find_codex_candidates(&base, cwd, SystemTime::now(), 60, 3);
        assert_eq!(found.len(), 0, "stale 후보는 제외");
        let _ = std::fs::remove_dir_all(&base);
    }

    /// scan_days 밖 일자 디렉터리는 스캔되지 않는다.
    #[test]
    fn scan_days_limits_directories() {
        let base = unique_tmp("scandays");
        let cwd = "/Users/me/projScan";
        // 06/05(최신)와 06/01(오래됨) 두 일자에 각각 후보를 둔다.
        let new_day = base.join("sessions").join("2026").join("06").join("05");
        let old_day = base.join("sessions").join("2026").join("06").join("01");
        std::fs::create_dir_all(&new_day).unwrap();
        std::fs::create_dir_all(&old_day).unwrap();
        let lines = [
            session_meta_line(cwd, "codex-tui"),
            token_count_line(100, 1000, 0, 3.0, 21.0, "pro"),
        ];
        for (dir, tag) in [(&new_day, "new"), (&old_day, "old")] {
            let path = dir.join(format!("rollout-2026-06-05T20-40-45-{tag}.jsonl"));
            let mut file = std::fs::File::create(&path).unwrap();
            for line in &lines {
                writeln!(file, "{line}").unwrap();
            }
        }
        // scan_days=1 → 최신 일자(06/05)만 스캔 → old(06/01) 미발견.
        let found = find_codex_candidates(&base, cwd, SystemTime::now(), 240, 1);
        assert_eq!(found.len(), 1, "scan_days=1은 최신 일자만 스캔");
        let _ = std::fs::remove_dir_all(&base);
    }

    /// 스캔 파일 총량 상한을 넘으면 부분 결과를 단일 후보로 신뢰하지 않고 fail-safe로 저하한다.
    #[test]
    fn scan_budget_exceeded_fails_safe() {
        let base = unique_tmp("scan-budget");
        let cwd = "/Users/me/projBudget";
        let day_dir = base.join("sessions").join("2026").join("06").join("05");
        std::fs::create_dir_all(&day_dir).unwrap();

        for idx in 0..=MAX_ROLLOUT_SCAN_FILES {
            let path = day_dir.join(format!("rollout-2026-06-05T20-40-45-{idx:04}.jsonl"));
            std::fs::File::create(path).unwrap();
        }

        let scan = scan_codex_candidates(&base, cwd, SystemTime::now(), 240, 3);
        assert!(
            scan.fingerprint.budget_exceeded,
            "rollout 파일이 상한을 넘으면 지문에 budget_exceeded가 기록되어야 함"
        );
        let resolution = read_codex_session(&base, cwd, SystemTime::now(), 240, 3);
        assert_eq!(
            resolution,
            Resolution::Ambiguous,
            "부분 스캔으로 단일/없음을 확정하지 않고 enrich 생략 경로로 저하"
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    /// AC-X5: codex_exec(비대화형 originator)는 제외된다.
    #[test]
    fn exec_originator_excluded() {
        let base = unique_tmp("exec");
        let cwd = "/Users/me/projExec";
        write_rollout(
            &base,
            "exec",
            &[
                session_meta_line(cwd, "codex_exec"),
                turn_context_line("gpt-5.5", "high"),
            ],
        );
        let found = find_codex_candidates(&base, cwd, SystemTime::now(), 240, 3);
        assert_eq!(found.len(), 0, "exec originator는 제외");
        let _ = std::fs::remove_dir_all(&base);
    }

    /// cwd 정규화: trailing slash가 달라도 매칭된다(존재하는 임시 디렉터리로 canonicalize 경로).
    #[test]
    fn cwd_normalization_trailing_slash_matches() {
        let base = unique_tmp("cwdnorm");
        // 실제 존재하는 cwd 디렉터리를 만들어 canonicalize 경로로도 일치하게 한다.
        let real_cwd = base.join("realcwd");
        std::fs::create_dir_all(&real_cwd).unwrap();
        let cwd_str = real_cwd.to_string_lossy().into_owned();
        write_rollout(
            &base,
            "cwdnorm",
            &[
                session_meta_line(&cwd_str, "codex-tui"),
                token_count_line(100, 1000, 0, 3.0, 21.0, "pro"),
            ],
        );
        // target에 trailing slash를 붙여도 매칭되어야 한다.
        let target = format!("{cwd_str}/");
        let found = find_codex_candidates(&base, &target, SystemTime::now(), 240, 3);
        assert_eq!(found.len(), 1, "trailing slash 정규화 매칭");
        let _ = std::fs::remove_dir_all(&base);
    }

    /// extract_from_file: head(baseline) + tail(최신) 결합. tail의 더 최신 turn_context/token_count 우선.
    #[test]
    fn extract_combines_head_and_tail() {
        let base = unique_tmp("extract");
        let cwd = "/Users/me/projExtract";
        let path = write_rollout(
            &base,
            "extract",
            &[
                session_meta_line(cwd, "codex-tui"),
                turn_context_line("gpt-5.5", "low"), // baseline
                token_count_line(50, 1000, 0, 1.0, 5.0, "pro"),
                turn_context_line("gpt-5.5", "xhigh"), // 더 최신 effort
                token_count_line(275, 1000, 0, 3.0, 21.0, "pro"), // 최신 → 27.5%
            ],
        );
        let session = extract_from_file(&path).expect("추출");
        assert_eq!(session.model.as_deref(), Some("gpt-5.5"));
        let ctx = session.context_percentage.expect("ctx%");
        assert!((ctx - 27.5).abs() < 1e-9, "최신 token_count 우선: {ctx}");
        assert_eq!(session.extras.effort.as_deref(), Some("xhigh"));
        assert_eq!(session.extras.rate_5h_percent, Some(3.0));
        assert_eq!(session.extras.rate_weekly_percent, Some(21.0));
        assert_eq!(session.extras.plan.as_deref(), Some("pro"));
        let _ = std::fs::remove_dir_all(&base);
    }

    /// 거대 레코드(132KB 단일 라인) 상한 안전: tail 256KB 안에서 무패닉 처리.
    #[test]
    fn extract_huge_record_within_bounds() {
        let base = unique_tmp("huge");
        let cwd = "/Users/me/projHuge";
        // 132KB짜리 거대 turn_context 라인(spec §8.1 상한 검증).
        let big_summary = "x".repeat(132 * 1024);
        let huge_line = format!(
            r#"{{"type":"turn_context","payload":{{"model":"gpt-5.5","effort":"high","blob":"{big_summary}"}}}}"#
        );
        let path = write_rollout(
            &base,
            "huge",
            &[
                session_meta_line(cwd, "codex-tui"),
                huge_line,
                token_count_line(100, 1000, 0, 3.0, 21.0, "pro"),
            ],
        );
        // 무패닉으로 추출되어야 한다(최신 token_count는 tail 256KB 안에 있음).
        let session = extract_from_file(&path).expect("거대 레코드 무패닉 추출");
        assert_eq!(session.extras.rate_5h_percent, Some(3.0));
        let _ = std::fs::remove_dir_all(&base);
    }

    /// 비-UTF8 바이트가 섞여도 from_utf8_lossy로 무패닉 처리한다.
    #[test]
    fn extract_non_utf8_lossy() {
        let base = unique_tmp("nonutf8");
        let cwd = "/Users/me/projUtf";
        let day_dir = base.join("sessions").join("2026").join("06").join("05");
        std::fs::create_dir_all(&day_dir).unwrap();
        let path = day_dir.join("rollout-2026-06-05T20-40-45-utf.jsonl");
        let mut file = std::fs::File::create(&path).unwrap();
        writeln!(file, "{}", session_meta_line(cwd, "codex-tui")).unwrap();
        // 깨진 UTF-8 바이트(0xFF)를 한 줄에 섞는다.
        file.write_all(&[0xFF, 0xFE, b'\n']).unwrap();
        writeln!(file, "{}", token_count_line(100, 1000, 0, 3.0, 21.0, "pro")).unwrap();
        // 무패닉으로 추출(깨진 라인은 개별 무시).
        let session = extract_from_file(&path).expect("비-UTF8 무패닉");
        assert_eq!(session.extras.rate_5h_percent, Some(3.0));
        let _ = std::fs::remove_dir_all(&base);
    }

    /// token_count 전무(신생 세션) → ctx/rate None(부분/생략, AC-X5 변형).
    #[test]
    fn extract_no_token_count_partial() {
        let base = unique_tmp("notoken");
        let cwd = "/Users/me/projNew";
        let path = write_rollout(
            &base,
            "notoken",
            &[
                session_meta_line(cwd, "codex-tui"),
                turn_context_line("gpt-5.5", "high"),
            ],
        );
        let session = extract_from_file(&path).expect("추출");
        assert_eq!(session.model.as_deref(), Some("gpt-5.5"));
        assert_eq!(
            session.context_percentage, None,
            "token_count 전무 → ctx None"
        );
        assert_eq!(session.extras.rate_5h_percent, None);
        assert_eq!(session.extras.effort.as_deref(), Some("high"));
        let _ = std::fs::remove_dir_all(&base);
    }

    // ============== Integration: maybe_enrich / 캐시(AC1/AC-X6) ==============
    //
    // 통합 테스트는 process-global `HOME`/`CODEX_HOME` env를 만지지 않는다(hermetic).
    // codex sessions base와 캐시 base를 [`maybe_enrich_in`]에 고유 temp dir로 직접 주입하므로
    // 전역 상태 경합이 없어 직렬화 락이 불필요하다(이전 `with_codex_env`/`ENV_LOCK` 제거).

    /// agent≠codex → 무변경(IO 0).
    #[test]
    fn enrich_non_codex_no_change() {
        let mut input = ClaudeInput {
            model_display_name: Some("claude".to_string()),
            cwd: Some("/tmp/x".to_string()),
            session_id: Some("k1".to_string()),
            ..Default::default()
        };
        let before = input.clone();
        maybe_enrich(&mut input, &Config::default());
        assert_eq!(input, before, "non-codex는 무변경");
    }

    /// enabled=false → 무변경(IO 0).
    #[test]
    fn enrich_disabled_no_change() {
        let mut input = ClaudeInput {
            model_display_name: Some("codex".to_string()),
            cwd: Some("/tmp/x".to_string()),
            session_id: Some("k2".to_string()),
            ..Default::default()
        };
        let mut cfg = Config::default();
        cfg.codex.enabled = false;
        let before = input.clone();
        maybe_enrich(&mut input, &cfg);
        assert_eq!(input, before, "disabled는 무변경");
    }

    /// 단일 후보 → model/ctx/codex 설정(AC1). codex/cache base를 temp 주입으로 격리(env 무변경).
    #[test]
    fn enrich_single_candidate_sets_fields() {
        let base = unique_tmp("enrich-single");
        let cache_base = unique_tmp("enrich-single-cache");
        let cwd = "/Users/me/projEnrich";
        write_rollout(
            &base,
            "enrich",
            &[
                session_meta_line(cwd, "codex-tui"),
                turn_context_line("gpt-5.5", "xhigh"),
                token_count_line(275, 1000, 0, 3.0, 21.0, "pro"),
            ],
        );
        let mut input = ClaudeInput {
            model_display_name: Some("codex".to_string()),
            cwd: Some(cwd.to_string()),
            session_id: Some("enrich-single-key".to_string()),
            ..Default::default()
        };
        maybe_enrich_in(
            &mut input,
            &Config::default(),
            Some(&base),
            Some(&cache_base),
        );
        // 단일 해소: model=실모델, ctx=27.5%, extras 채워짐.
        assert_eq!(input.model_display_name.as_deref(), Some("gpt-5.5"));
        let ctx = input.context_used_percentage.expect("ctx%");
        assert!((ctx - 27.5).abs() < 1e-9, "ctx 27.5: {ctx}");
        let extras = input.codex.expect("codex extras");
        assert_eq!(extras.rate_5h_percent, Some(3.0));
        assert_eq!(extras.rate_weekly_percent, Some(21.0));
        assert_eq!(extras.plan.as_deref(), Some("pro"));
        assert_eq!(extras.effort.as_deref(), Some("xhigh"));
        // 캐시도 temp(cache_base) 하위에 격리되어 실캐시 오염이 없다.
        let _ = std::fs::remove_dir_all(&base);
        let _ = std::fs::remove_dir_all(&cache_base);
    }

    /// 모호(≥2) → 무변경(model="codex" 유지, AC2/AC-X1).
    #[test]
    fn enrich_ambiguous_no_change() {
        let base = unique_tmp("enrich-amb");
        let cache_base = unique_tmp("enrich-amb-cache");
        let cwd = "/Users/me/projAmb";
        for tag in ["a1", "a2"] {
            write_rollout(
                &base,
                tag,
                &[
                    session_meta_line(cwd, "codex-tui"),
                    token_count_line(275, 1000, 0, 3.0, 21.0, "pro"),
                ],
            );
        }
        let mut input = ClaudeInput {
            model_display_name: Some("codex".to_string()),
            cwd: Some(cwd.to_string()),
            session_id: Some("enrich-amb-key".to_string()),
            ..Default::default()
        };
        let before = input.clone();
        maybe_enrich_in(
            &mut input,
            &Config::default(),
            Some(&base),
            Some(&cache_base),
        );
        assert_eq!(input, before, "모호는 무변경(model=codex 유지)");
        // 모호는 캐시되지 않아야 한다(TTL 고착 차단). cache_base 격리로 캐시는 temp에만 존재한다.
        let _ = std::fs::remove_dir_all(&base);
        let _ = std::fs::remove_dir_all(&cache_base);
    }

    /// AC-X6: 캐시 정상상태 — 2회차는 경로 mtime 불변이면 재해소 없이 캐시를 재사용한다.
    ///
    /// 1회차에 캐시를 채운 뒤, 2회차는 매칭 불가 cwd로 호출한다. 캐시 재사용이면 같은 키로
    /// 캐시 히트 → 경로 mtime 불변 → 재사용되어 여전히 enrich가 성공해야 한다(풀 재해소면 실패).
    #[test]
    fn cache_steady_state_reuses_without_rescan() {
        let base = unique_tmp("cache-steady");
        let cache_base = unique_tmp("cache-steady-cache");
        let cwd = "/Users/me/projCache";
        let key = "cache-steady-key";
        write_rollout(
            &base,
            "cache",
            &[
                session_meta_line(cwd, "codex-tui"),
                turn_context_line("gpt-5.5", "high"),
                token_count_line(275, 1000, 0, 3.0, 21.0, "pro"),
            ],
        );

        // 1회차: 캐시 채움.
        let mut first = ClaudeInput {
            model_display_name: Some("codex".to_string()),
            cwd: Some(cwd.to_string()),
            session_id: Some(key.to_string()),
            ..Default::default()
        };
        maybe_enrich_in(
            &mut first,
            &Config::default(),
            Some(&base),
            Some(&cache_base),
        );
        assert_eq!(first.model_display_name.as_deref(), Some("gpt-5.5"));

        // 2회차: 같은 캐시 키지만 매칭 불가 cwd. 풀스캔이면 0 발견이지만 캐시 히트 →
        // 경로 mtime 불변 → 재사용되어 여전히 성공해야 한다(정상상태 stat 1회).
        let mut second = ClaudeInput {
            model_display_name: Some("codex".to_string()),
            cwd: Some("/no/match/here".to_string()),
            session_id: Some(key.to_string()),
            ..Default::default()
        };
        maybe_enrich_in(
            &mut second,
            &Config::default(),
            Some(&base),
            Some(&cache_base),
        );
        assert_eq!(
            second.model_display_name.as_deref(),
            Some("gpt-5.5"),
            "정상상태는 캐시 재사용(재스캔 없이 stat 1회)"
        );
        // cache_base 격리로 캐시는 temp 하위에만 존재하므로 실캐시 청소가 불필요하다.
        let _ = std::fs::remove_dir_all(&base);
        let _ = std::fs::remove_dir_all(&cache_base);
    }

    /// 캐시 히트라도 같은 cwd의 새 fresh rollout이 생기면 이전 단일 후보를 재사용하지 않는다.
    ///
    /// 1회차 단일 후보로 캐시를 채운 뒤 2회차 전에 같은 cwd 후보를 추가한다. 캐시가 rollout
    /// 트리 지문 변화를 무시하면 stale 단일 세션을 계속 표시하지만, 올바른 동작은 재스캔 후
    /// Ambiguous로 안전 저하되어 input을 그대로 두는 것이다.
    #[test]
    fn cache_hit_rechecks_new_same_cwd_rollout_before_reuse() {
        let base = unique_tmp("cache-amb");
        let cache_base = unique_tmp("cache-amb-cache");
        let cwd = "/Users/me/projCacheAmb";
        let key = "cache-amb-key";
        write_rollout(
            &base,
            "cacheamb1",
            &[
                session_meta_line(cwd, "codex-tui"),
                turn_context_line("gpt-5.5", "high"),
                token_count_line(275, 1000, 0, 3.0, 21.0, "pro"),
            ],
        );

        let mut first = ClaudeInput {
            model_display_name: Some("codex".to_string()),
            cwd: Some(cwd.to_string()),
            session_id: Some(key.to_string()),
            ..Default::default()
        };
        maybe_enrich_in(
            &mut first,
            &Config::default(),
            Some(&base),
            Some(&cache_base),
        );
        assert_eq!(first.model_display_name.as_deref(), Some("gpt-5.5"));

        write_rollout(
            &base,
            "cacheamb2",
            &[
                session_meta_line(cwd, "codex-tui"),
                turn_context_line("gpt-5.4-mini", "medium"),
                token_count_line(100, 1000, 0, 2.0, 10.0, "pro"),
            ],
        );

        let mut second = ClaudeInput {
            model_display_name: Some("codex".to_string()),
            cwd: Some(cwd.to_string()),
            session_id: Some(key.to_string()),
            ..Default::default()
        };
        let before = second.clone();
        maybe_enrich_in(
            &mut second,
            &Config::default(),
            Some(&base),
            Some(&cache_base),
        );
        assert_eq!(
            second, before,
            "새 같은-cwd 후보가 생긴 뒤에는 캐시 재사용 대신 Ambiguous로 안전 저하"
        );

        let _ = std::fs::remove_dir_all(&base);
        let _ = std::fs::remove_dir_all(&cache_base);
    }

    /// M2: 캐시 히트라도 해소된 파일 mtime이 freshness를 넘기면 캐시를 무시하고 재해소한다.
    ///
    /// 캐시 신선도는 기록 시각 기준이라, 세션 종료 후 파일 mtime이 고정돼도 마지막 캐시 write로부터
    /// freshness 동안 stale 세션을 계속 표시하는 결함을 박제한다(spec §5 "fresh 후보만" 일관성).
    /// 1회차로 캐시를 채운 뒤 해소된 파일 mtime을 freshness보다 오래되게 만들고, 2회차는 매칭 불가
    /// cwd로 호출한다. 캐시가 stale로 무시되면 풀 재해소가 0 후보 → None(model="codex" 유지)이어야 한다.
    /// (이전 동작은 stale 캐시를 재사용해 enrich를 유지했으므로 이 단언이 회귀 가드 역할을 한다.)
    #[test]
    fn cache_ignored_when_resolved_file_is_stale() {
        let base = unique_tmp("cache-stale");
        let cache_base = unique_tmp("cache-stale-cache");
        let cwd = "/Users/me/projStaleCache";
        let key = "cache-stale-key";
        let rollout_path = write_rollout(
            &base,
            "cachestale",
            &[
                session_meta_line(cwd, "codex-tui"),
                turn_context_line("gpt-5.5", "high"),
                token_count_line(275, 1000, 0, 3.0, 21.0, "pro"),
            ],
        );

        // 1회차: 캐시 채움(파일이 fresh이므로 enrich 성공).
        let mut first = ClaudeInput {
            model_display_name: Some("codex".to_string()),
            cwd: Some(cwd.to_string()),
            session_id: Some(key.to_string()),
            ..Default::default()
        };
        maybe_enrich_in(
            &mut first,
            &Config::default(),
            Some(&base),
            Some(&cache_base),
        );
        assert_eq!(
            first.model_display_name.as_deref(),
            Some("gpt-5.5"),
            "1회차는 fresh 파일이라 enrich 성공"
        );

        // 해소된 파일의 mtime을 freshness(기본 240분)보다 한참 오래되게(5시간 전) 만든다.
        let five_hours_ago = SystemTime::now() - Duration::from_secs(5 * 3600);
        set_file_mtime(&rollout_path, five_hours_ago);

        // 2회차: 같은 캐시 키지만 매칭 불가 cwd. 캐시 히트하더라도 파일 mtime이 stale이므로
        // 캐시를 무시하고 풀 재해소 → 0 후보 → None(무변경, model="codex" 유지)이어야 한다.
        let mut second = ClaudeInput {
            model_display_name: Some("codex".to_string()),
            cwd: Some("/no/match/here".to_string()),
            session_id: Some(key.to_string()),
            ..Default::default()
        };
        let before = second.clone();
        maybe_enrich_in(
            &mut second,
            &Config::default(),
            Some(&base),
            Some(&cache_base),
        );
        assert_eq!(
            second, before,
            "stale 캐시는 무시되어 종료된 세션이 더는 표시되지 않아야 함(model=codex 유지)"
        );
        assert_eq!(
            second.model_display_name.as_deref(),
            Some("codex"),
            "stale 후 재해소 0 후보 → model 슬롯 미변경(bare codex)"
        );
        let _ = std::fs::remove_dir_all(&base);
        let _ = std::fs::remove_dir_all(&cache_base);
    }

    // ===== 캐시 테스트 헬퍼 =====

    /// 파일 mtime을 지정 시각으로 설정한다(stale 테스트용, libc utimes).
    fn set_file_mtime(path: &Path, time: SystemTime) {
        let secs = time
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0) as i64;
        let times = [
            libc::timeval {
                tv_sec: secs,
                tv_usec: 0,
            },
            libc::timeval {
                tv_sec: secs,
                tv_usec: 0,
            },
        ];
        let c_path = std::ffi::CString::new(path.to_string_lossy().as_bytes()).unwrap();
        // SAFETY: 유효한 경로/timeval 포인터로 utimes 호출(실패해도 무패닉).
        unsafe {
            libc::utimes(c_path.as_ptr(), times.as_ptr());
        }
    }
}
