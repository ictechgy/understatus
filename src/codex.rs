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
    cache_now_millis, is_named_cache_fresh, read_session_named_cache, write_session_named_cache,
};
use crate::claude::ClaudeInput;
use crate::config::Config;
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// head(앞부분) 읽기 상한. 첫 줄 session_meta + 첫 turn_context(baseline model/effort)용(spec §8.1).
const HEAD_READ_BYTES: u64 = 16 * 1024;
/// tail(뒷부분) 읽기 상한. 마지막 token_count + 최신 turn_context용(실측 gap max 14KB,
/// 단일 라인 max 132KB → 256KB 안전마진, spec §8.1).
const TAIL_READ_BYTES: u64 = 256 * 1024;
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

/// 두 cwd 문자열이 같은 디렉터리를 가리키는지 비교한다(정규화: canonicalize 실패 시 trim 비교).
///
/// 외부 입력(payload.cwd)은 **비교에만** 쓰고 파일경로 구성엔 쓰지 않는다(traversal 무관, spec §5).
/// trailing slash 등 표기 차이를 흡수하기 위해 canonicalize를 시도하고, 실패(부재 경로 등) 시
/// trim된 문자열 동치로 폴백한다.
fn cwd_matches(candidate_cwd: &str, target_cwd: &str) -> bool {
    let normalize = |p: &str| -> PathBuf {
        std::fs::canonicalize(p).unwrap_or_else(|_| PathBuf::from(p.trim_end_matches('/')))
    };
    normalize(candidate_cwd) == normalize(target_cwd)
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
fn read_exact_lossy(file: &mut File, buf: &mut Vec<u8>) -> Option<()> {
    let mut filled = 0usize;
    while filled < buf.len() {
        match file.read(&mut buf[filled..]) {
            Ok(0) => break,
            Ok(n) => filled += n,
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
fn find_codex_candidates(
    base: &Path,
    cwd: &str,
    now: SystemTime,
    freshness: u64,
    scan_days: usize,
) -> Vec<PathBuf> {
    let sessions_dir = base.join("sessions");
    let day_dirs = recent_day_dirs(&sessions_dir, scan_days);

    let freshness_secs = freshness.saturating_mul(60);
    let mut candidates = Vec::new();
    for day_dir in day_dirs {
        let entries = match std::fs::read_dir(&day_dir) {
            Ok(entries) => entries,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !is_rollout_file(&path) {
                continue;
            }
            // (2) cheap stat 선필터: mtime이 freshness 이내인 후보만.
            if !is_fresh(&path, now, freshness_secs) {
                continue;
            }
            // (3) 첫 줄 session_meta만 읽어 cwd 일치 + 대화형 originator를 확인한다.
            if let Some(meta) = read_first_line_meta(&path) {
                let cwd_ok = meta
                    .cwd
                    .as_deref()
                    .map(|c| cwd_matches(c, cwd))
                    .unwrap_or(false);
                let originator_ok = is_interactive_originator(meta.originator.as_deref());
                if cwd_ok && originator_ok {
                    candidates.push(path);
                }
            }
        }
    }
    candidates
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

/// rollout 파일의 첫 줄만 읽어 session_meta를 파싱한다(매칭 선필터용, 전체 읽기 금지).
///
/// 첫 줄은 항상 session_meta이고 보통 작지 않을 수 있으나(base_instructions 포함), head 상한
/// 안에서 첫 개행까지만 취하면 충분하다. 읽기/파싱 실패 시 `None`(무패닉).
fn read_first_line_meta(path: &Path) -> Option<SessionMeta> {
    let mut file = File::open(path).ok()?;
    // 첫 줄(session_meta)은 base_instructions로 커질 수 있으므로 head 상한까지 읽는다.
    let mut buf = vec![0u8; HEAD_READ_BYTES as usize];
    read_exact_lossy(&mut file, &mut buf)?;
    let text = String::from_utf8_lossy(&buf);
    let first_line = text.lines().next()?;
    parse_session_meta(first_line)
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
    let candidates = find_codex_candidates(base, cwd, now, freshness, scan_days);
    match candidates.len() {
        0 => Resolution::None,
        1 => {
            let path = &candidates[0];
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

/// 세션 데이터를 디스크 캐시에서 조회/갱신해 [`CodexSession`]을 반환한다(spec §8 매 틱 로직).
///
/// 1) 캐시 히트 & 경로 mtime 불변 & freshness 이내 → 재사용(스캔 0, stat 1회).
/// 2) 캐시 히트 & mtime 변동 & freshness 이내 → 그 파일만 tail 재독 → 캐시 갱신.
/// 3) 미스/경로 stale/없음 → 풀 후보스캔 재해소. **Ambiguous는 캐시하지 않는다**.
///
/// # 반환
/// 단일 해소 시 `Some(session)`. 모호/없음 시 `None`(무변경 신호).
fn resolve_with_cache(
    base: &Path,
    session_key: &str,
    cwd: &str,
    now: SystemTime,
    freshness: u64,
    scan_days: usize,
) -> Option<CodexSession> {
    let now_ms = cache_now_millis();
    let freshness_secs = freshness.saturating_mul(60);

    // 캐시 조회.
    if let Some((written_ms, payload)) = read_session_named_cache(session_key, CODEX_CACHE_FILE) {
        if is_named_cache_fresh(written_ms, now_ms, freshness_secs) {
            if let Ok(entry) = serde_json::from_str::<CodexCacheEntry>(&payload) {
                let cached_path = PathBuf::from(&entry.path);
                match file_mtime_ms(&cached_path) {
                    // (1) 경로 mtime 불변 → 재사용(stat 1회).
                    Some(current_mtime) if current_mtime == entry.mtime_ms => {
                        return Some(entry.session);
                    }
                    // (2) mtime 변동(파일 존재) → 그 파일만 tail 재독 → 캐시 갱신.
                    Some(current_mtime) => {
                        if let Some(session) = extract_from_file(&cached_path) {
                            write_cache_entry(
                                session_key,
                                &cached_path,
                                current_mtime,
                                &session,
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

    // (3) 미스/stale/경로 소실 → 풀 후보스캔 재해소.
    match read_codex_session(base, cwd, now, freshness, scan_days) {
        Resolution::Single(session, path) => {
            // 단일 해소만 캐시한다(모호는 비캐시 — TTL 고착 차단).
            if let Some(mtime) = file_mtime_ms(&path) {
                write_cache_entry(session_key, &path, mtime, &session, now_ms);
            }
            Some(session)
        }
        // Ambiguous/None → 캐시하지 않고 무변경 신호.
        Resolution::Ambiguous | Resolution::None => None,
    }
}

/// 해소 결과를 디스크 캐시에 1라인 직렬화로 기록한다(best-effort, 실패 무시).
fn write_cache_entry(
    session_key: &str,
    path: &Path,
    mtime_ms: u128,
    session: &CodexSession,
    now_ms: u128,
) {
    let entry = CodexCacheEntry {
        path: path.to_string_lossy().into_owned(),
        mtime_ms,
        session: session.clone(),
    };
    if let Ok(payload) = serde_json::to_string(&entry) {
        write_session_named_cache(session_key, CODEX_CACHE_FILE, now_ms, &payload);
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
/// - 호출부([`crate::main`])에서 **`Source::Lterm`으로 한정**해 호출한다(Claude 경로 오발동 차단).
/// - 추가로 `cfg.codex.enabled` && model이 codex 계열 && `input.cwd=Some` && `codex_home()` 존재.
///
/// 단일 해소면 `model_display_name`/`context_used_percentage`/`codex`를 설정한다. 모호/없음/실패
/// 시 무변경(기존 lterm 출력 보존). 실패/모호 시 `LTERM_STATUS_DEBUG` 설정 하에 stderr 1줄.
///
/// # 인자
/// - `input`: enrich 대상(이미 parse_lterm_input으로 채워진 상태).
/// - `cfg`: `[codex]` 토글/freshness/scan_days.
pub fn maybe_enrich(input: &mut ClaudeInput, cfg: &Config) {
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
    // 게이팅 4: CODEX_HOME 존재.
    let base = match codex_home() {
        Some(base) if base.exists() => base,
        _ => {
            debug_log("codex_home 부재 — enrich 생략");
            return;
        }
    };

    // session_key는 캐시 격리용(lterm payload 유래). 부재 시 cwd 기반으로 안정화한다.
    let session_key = input.session_id.clone().unwrap_or_else(|| cwd.clone());

    let resolved = resolve_with_cache(
        &base,
        &session_key,
        &cwd,
        SystemTime::now(),
        cfg.codex.freshness_minutes,
        cfg.codex.scan_days,
    );

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
