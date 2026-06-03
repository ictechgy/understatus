//! 체이닝 자식 스폰/합성 + 단기 TTL 캐시 + 히스테리시스 펄스 상태 보존.
//!
//! 계획서 §D-2/§H-6/AC8(CRITICAL-1)을 따른다. 기존 statusLine 명령을 자식 프로세스로
//! 실행하되 타임아웃과 단기 TTL 캐시(`~/Library/Caches/understatus/`)로 무거운 자식을
//! 디커플해 렌더를 절대 블록하지 않는다. 같은 캐시 디렉터리에 펄스 on/off boolean도 보존한다.
//!
//! 주의: 이 디스크 캐시는 영속 상태가 아니라 짧은 TTL 캐시 예외(§A 원칙 1의 명시적 예외,
//! F6/RC-8)이며 더블샘플 CPU 산식의 무상태성은 유지된다.

use crate::config::Config;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// 체인 자식 stdout 캐시 파일명(`~/Library/Caches/understatus/chain_output`).
const CHAIN_CACHE_FILE: &str = "chain_output";
/// 펄스 on/off 상태 파일명(`~/Library/Caches/understatus/pulse_state`).
const PULSE_STATE_FILE: &str = "pulse_state";
/// 펄스 상태 캐시의 단기 TTL(초). 직전 프레임의 히스테리시스 상태만 살아 있으면 된다.
const PULSE_STATE_TTL_SECONDS: u64 = 10;
/// 자식 종료 대기 폴링 간격(ms). 타임아웃 정밀도와 busy-wait 비용의 절충.
const POLL_INTERVAL_MS: u64 = 5;

// CONTRACT: signature is frozen — implement body only, do not change this signature
/// 체인 자식 명령을 실행하고 stdout을 반환한다(타임아웃 + 단기 TTL 캐시).
///
/// # 인자
/// - `chain_command`: 실행할 셸 명령(보존된 원본 statusLine 명령).
/// - `raw_stdin`: 자식 stdin으로 그대로 전달할 Claude raw JSON.
/// - `cfg`: `chain.chain_timeout_ms`(기본 500), `chain.chain_cache_ttl_seconds`(기본 3).
///
/// # 반환
/// 자식 stdout. TTL 내 캐시가 있으면 자식을 재스폰하지 않고 캐시를 반환한다.
/// 타임아웃/스폰 실패 시 마지막 캐시 출력으로, 그것도 없으면 빈 문자열로 저하한다.
/// **절대 렌더를 블록하지 않는다**(CRITICAL-1, AC8).
pub fn run_chain(chain_command: &str, raw_stdin: &str, cfg: &Config) -> String {
    let now_ms = now_millis();
    let cache_path = cache_file(CHAIN_CACHE_FILE);

    // (1) TTL 내 신선 캐시가 있으면 무거운 자식을 재스폰하지 않고 즉시 반환한다(디커플, D-2).
    if let Some(path) = cache_path.as_ref() {
        if let Some((written_ms, output)) = read_cache_entry(path) {
            if is_cache_fresh(written_ms, now_ms, cfg.chain.chain_cache_ttl_seconds) {
                return output;
            }
        }
    }

    // (2) 캐시 미스/만료 → 타임아웃으로 자식 스폰. 성공 시 캐시 갱신 후 반환.
    match spawn_with_timeout(chain_command, raw_stdin, cfg.chain.chain_timeout_ms) {
        Some(output) => {
            let trimmed = trim_trailing_newline(&output);
            if let Some(path) = cache_path.as_ref() {
                write_cache_entry(path, now_ms, &trimmed);
            }
            trimmed
        }
        // (3) 타임아웃/스폰 실패 → 만료 여부 무관하게 마지막 캐시, 없으면 빈 문자열로 저하.
        None => cache_path
            .as_ref()
            .and_then(read_cache_entry)
            .map(|(_, output)| output)
            .unwrap_or_default(),
    }
}

// CONTRACT: signature is frozen — implement body only, do not change this signature
/// 직전 렌더의 펄스 on/off 상태를 단기 TTL 캐시에서 읽는다(히스테리시스용).
///
/// # 반환
/// 직전 펄스 on 여부. 캐시가 없거나 만료/읽기 실패 시 `false`로 안전 저하한다.
/// 이 boolean은 `~/Library/Caches/understatus/`에 저장되며 영속 상태가 아닌
/// 단기 TTL 캐시 예외다(§A 원칙 1, 더블샘플 무상태성 유지).
pub fn read_prev_pulse_state() -> bool {
    let path = match cache_file(PULSE_STATE_FILE) {
        Some(path) => path,
        None => return false,
    };
    match read_cache_entry(&path) {
        // 오래된 상태는 무시한다(직전 프레임만 유효).
        Some((written_ms, value))
            if is_cache_fresh(written_ms, now_millis(), PULSE_STATE_TTL_SECONDS) =>
        {
            value.trim() == "1"
        }
        _ => false,
    }
}

// CONTRACT: signature is frozen — implement body only, do not change this signature
/// 이번 렌더의 펄스 on/off 상태를 단기 TTL 캐시에 기록한다(다음 호출의 히스테리시스용).
///
/// # 인자
/// - `on`: 이번 프레임의 펄스 on 여부([`crate::theme::pulse_gate`] 결과).
///
/// 쓰기 실패는 무시한다(best-effort, 패닉 금지). 다음 호출이 만료된 상태를 읽으면
/// `false`로 저하할 뿐이다.
pub fn write_pulse_state(on: bool) {
    if let Some(path) = cache_file(PULSE_STATE_FILE) {
        write_cache_entry(&path, now_millis(), if on { "1" } else { "0" });
    }
}

// CONTRACT: signature is frozen — implement body only, do not change this signature
/// 임의 이름의 단기 TTL 캐시 항목을 읽는다(`~/Library/Caches/understatus/<name>`).
///
/// 네트워크 throughput 델타처럼 "직전 렌더 값"을 가로질러 보존해야 하는 P2 best-effort
/// 지표가, chain_output/pulse_state와 동일한 단기 TTL 캐시 예외(§A 원칙 1, F6/RC-8)를
/// 재사용하도록 노출한다. 데몬/영속 상태가 아니라 짧은 TTL 디스크 캐시일 뿐이다.
///
/// # 인자
/// - `name`: 캐시 파일명(예: `net_counters`).
///
/// # 반환
/// `(기록 시각 epoch ms, payload)`. `HOME` 미설정/파일 부재/포맷 불량 시 `None`.
pub fn read_named_cache(name: &str) -> Option<(u128, String)> {
    let path = cache_file(name)?;
    read_cache_entry(&path)
}

// CONTRACT: signature is frozen — implement body only, do not change this signature
/// 임의 이름의 단기 TTL 캐시 항목을 기록한다(best-effort, 실패 무시).
///
/// # 인자
/// - `name`: 캐시 파일명(예: `net_counters`).
/// - `now_ms`: 기록 시각(epoch ms).
/// - `payload`: 저장할 본문.
///
/// 쓰기 실패는 조용히 무시한다(패닉 금지). [`read_named_cache`]와 짝을 이룬다.
pub fn write_named_cache(name: &str, now_ms: u128, payload: &str) {
    if let Some(path) = cache_file(name) {
        write_cache_entry(&path, now_ms, payload);
    }
}

/// 현재 시각을 UNIX epoch 기준 밀리초(ms)로 반환한다(외부 모듈의 캐시 타임스탬프용).
///
/// # 반환
/// epoch 이후 경과 ms. 시계 이상 시 0으로 안전 저하한다.
pub fn cache_now_millis() -> u128 {
    now_millis()
}

// CONTRACT: signature is frozen — implement body only, do not change this signature
/// 캐시 항목이 TTL 내에 있는지 외부 모듈(예: 배터리 30s TTL)이 판정하도록 노출한다.
///
/// 내부 [`is_cache_fresh`]와 동일한 규칙(시계 역행/TTL 0은 stale)을 따른다.
///
/// # 인자
/// - `written_ms`: 캐시 기록 시각(epoch ms).
/// - `now_ms`: 현재 시각(epoch ms).
/// - `ttl_seconds`: 허용 신선도(초).
///
/// # 반환
/// TTL 내면 `true`, 아니면 `false`.
pub fn is_named_cache_fresh(written_ms: u128, now_ms: u128, ttl_seconds: u64) -> bool {
    is_cache_fresh(written_ms, now_ms, ttl_seconds)
}

/// 캐시 항목이 TTL 내에 있으면 `true`를 반환하는 순수 헬퍼(테스트 가능).
///
/// # 인자
/// - `written_ms`: 캐시가 기록된 시각(epoch ms).
/// - `now_ms`: 현재 시각(epoch ms).
/// - `ttl_seconds`: 허용 신선도(초). `0`이면 항상 stale로 간주한다.
///
/// # 반환
/// `now_ms`가 `written_ms` 이후이고 경과가 `ttl_seconds` 이내면 `true`.
/// 시계 역행(`now < written`)이나 TTL 0은 `false`(보수적으로 stale 처리).
fn is_cache_fresh(written_ms: u128, now_ms: u128, ttl_seconds: u64) -> bool {
    if ttl_seconds == 0 || now_ms < written_ms {
        return false;
    }
    let elapsed_ms = now_ms - written_ms;
    elapsed_ms <= (ttl_seconds as u128) * 1000
}

/// `~/Library/Caches/understatus/` 디렉터리 경로를 반환한다.
///
/// # 반환
/// 캐시 디렉터리 경로. `HOME` 미설정 시 `None`(호출부에서 빈 문자열/false로 저하).
fn cache_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from)?;
    Some(home.join("Library").join("Caches").join("understatus"))
}

/// 캐시 디렉터리 내 특정 파일 경로를 반환한다(디렉터리 생성은 쓰기 시점에 시도).
///
/// # 인자
/// - `name`: 파일명(`chain_output`/`pulse_state`).
///
/// # 반환
/// 파일 경로. `HOME` 미설정 시 `None`.
fn cache_file(name: &str) -> Option<PathBuf> {
    cache_dir().map(|dir| dir.join(name))
}

/// 캐시 파일을 읽어 `(written_ms, payload)`로 분해한다.
///
/// 파일 포맷: 첫 줄 = epoch ms(타임스탬프), 그 이후(첫 개행 뒤) 전부 = payload.
///
/// # 반환
/// `(기록 시각 ms, payload 문자열)`. 파일 부재/포맷 불량/타임스탬프 파싱 실패 시 `None`.
fn read_cache_entry(path: &PathBuf) -> Option<(u128, String)> {
    let contents = std::fs::read_to_string(path).ok()?;
    // 첫 개행을 기준으로 타임스탬프 줄과 payload를 분리한다(payload는 개행 포함 가능).
    let (timestamp_line, payload) = match contents.split_once('\n') {
        Some((first, rest)) => (first, rest.to_string()),
        // 개행이 없으면 타임스탬프만 있고 payload는 빈 문자열이다.
        None => (contents.as_str(), String::new()),
    };
    let written_ms: u128 = timestamp_line.trim().parse().ok()?;
    Some((written_ms, payload))
}

/// `(now_ms, payload)`를 캐시 파일에 기록한다(best-effort, 실패 무시).
///
/// # 인자
/// - `path`: 캐시 파일 경로.
/// - `now_ms`: 기록 시각(epoch ms).
/// - `payload`: 저장할 본문.
///
/// 디렉터리가 없으면 생성을 시도하고, 모든 I/O 실패는 조용히 무시한다(패닉 금지).
fn write_cache_entry(path: &PathBuf, now_ms: u128, payload: &str) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(path, format!("{now_ms}\n{payload}"));
}

/// 셸 명령을 타임아웃으로 스폰하고 stdout을 반환한다(렌더 무블록).
///
/// # 인자
/// - `command`: `sh -c`로 실행할 셸 명령.
/// - `raw_stdin`: 자식 stdin으로 전달할 본문.
/// - `timeout_ms`: 최대 대기 시간(ms). 초과 시 자식을 강제 종료(kill)한다.
///
/// # 반환
/// - `Some(stdout)`: 자식이 타임아웃 내에 정상 종료한 경우의 stdout(개행 미정리).
/// - `None`: 스폰 실패, 타임아웃 초과(자식 kill), stdin 쓰기/출력 수집 실패.
///
/// 구현: stdin을 먼저 써서 닫고, `try_wait` 폴링으로 타임아웃을 강제한다(외부 의존 없음).
fn spawn_with_timeout(command: &str, raw_stdin: &str, timeout_ms: u64) -> Option<String> {
    let mut child = Command::new("sh")
        .arg("-c")
        .arg(command)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;

    // stdin을 모두 쓰고 즉시 닫아(EOF) 자식이 입력 대기로 멈추지 않게 한다.
    // take()로 핸들을 꺼내 스코프 종료 시 drop → EOF 전달.
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(raw_stdin.as_bytes());
        // stdin은 여기서 drop되어 닫힌다.
    }

    // try_wait 폴링으로 타임아웃을 강제한다(스레드 join 없이 메인에서 폴링).
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    loop {
        match child.try_wait() {
            Ok(Some(_status)) => {
                // 자식 종료 → 남은 stdout 수집.
                return collect_stdout(child);
            }
            Ok(None) => {
                if Instant::now() >= deadline {
                    // 타임아웃 → 강제 종료 후 None(캐시/빈 문자열로 저하).
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
                std::thread::sleep(Duration::from_millis(POLL_INTERVAL_MS));
            }
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}

/// 종료된 자식의 stdout을 끝까지 읽어 UTF-8 문자열로 반환한다.
///
/// # 반환
/// stdout 내용. 핸들 부재/읽기 실패 시 `None`(비-UTF8은 손실 변환으로 보존).
fn collect_stdout(mut child: std::process::Child) -> Option<String> {
    use std::io::Read;
    let mut stdout = child.stdout.take()?;
    let mut buffer = Vec::new();
    stdout.read_to_end(&mut buffer).ok()?;
    Some(String::from_utf8_lossy(&buffer).into_owned())
}

/// 문자열 끝의 개행(`\n`/`\r\n`)을 한 번 제거한다.
///
/// # 인자
/// - `value`: 자식 stdout.
///
/// # 반환
/// 후행 개행을 제거한 새 문자열(중간 개행은 보존, AC8).
fn trim_trailing_newline(value: &str) -> String {
    value
        .strip_suffix('\n')
        .map(|stripped| stripped.strip_suffix('\r').unwrap_or(stripped))
        .unwrap_or(value)
        .to_string()
}

/// 현재 시각을 UNIX epoch 기준 밀리초(ms)로 반환한다.
///
/// # 반환
/// epoch 이후 경과 ms. 시계 이상 시 0으로 안전 저하한다.
fn now_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// TTL 내 캐시는 fresh(true), 초과/0/시계역행은 stale(false)이어야 한다.
    #[test]
    fn cache_freshness_respects_ttl() {
        // 기록 0ms, 현재 2000ms, TTL 3초 → 경과 2초 ≤ 3초 → fresh.
        assert!(is_cache_fresh(0, 2_000, 3));
        // 정확히 TTL 경계(3초)는 fresh(<=).
        assert!(is_cache_fresh(0, 3_000, 3));
        // TTL 초과(3.001초)는 stale.
        assert!(!is_cache_fresh(0, 3_001, 3));
        // TTL 0은 항상 stale.
        assert!(!is_cache_fresh(0, 0, 0));
        // 시계 역행(now < written)은 보수적으로 stale.
        assert!(!is_cache_fresh(5_000, 1_000, 3));
    }

    /// 후행 개행만 제거하고 중간 개행은 보존해야 한다.
    #[test]
    fn trims_only_trailing_newline() {
        assert_eq!(trim_trailing_newline("hello\n"), "hello");
        assert_eq!(trim_trailing_newline("hello\r\n"), "hello");
        assert_eq!(trim_trailing_newline("a\nb\n"), "a\nb");
        assert_eq!(trim_trailing_newline("no-newline"), "no-newline");
        assert_eq!(trim_trailing_newline(""), "");
    }

    /// 캐시 항목 라운드트립: write → read가 같은 payload/timestamp를 돌려줘야 한다.
    #[test]
    fn cache_entry_roundtrip() {
        let path =
            std::env::temp_dir().join(format!("understatus-cache-rt-{}", std::process::id()));
        write_cache_entry(&path, 12_345, "line1\nline2");
        let (written_ms, payload) = read_cache_entry(&path).expect("캐시 읽기 실패");
        assert_eq!(written_ms, 12_345);
        assert_eq!(payload, "line1\nline2");
        let _ = std::fs::remove_file(&path);
    }

    /// 빈 payload(타임스탬프만)도 안전하게 라운드트립되어야 한다.
    #[test]
    fn cache_entry_empty_payload() {
        let path =
            std::env::temp_dir().join(format!("understatus-cache-empty-{}", std::process::id()));
        write_cache_entry(&path, 999, "");
        let (written_ms, payload) = read_cache_entry(&path).expect("캐시 읽기 실패");
        assert_eq!(written_ms, 999);
        assert_eq!(payload, "");
        let _ = std::fs::remove_file(&path);
    }

    /// 부재 캐시 파일은 None을 반환해야 한다(패닉 금지).
    #[test]
    fn missing_cache_returns_none() {
        let path = std::env::temp_dir().join("understatus-cache-does-not-exist-xyz");
        let _ = std::fs::remove_file(&path);
        assert!(read_cache_entry(&path).is_none());
    }

    /// 정상 명령은 stdout을 반환하고(개행 정리), 캐시를 채워야 한다.
    #[test]
    fn spawn_returns_stdout() {
        let output = spawn_with_timeout("printf 'hi-there'", "", 2_000);
        assert_eq!(output.as_deref(), Some("hi-there"));
    }

    /// stdin이 자식에게 전달되어야 한다(cat으로 에코).
    #[test]
    fn spawn_passes_stdin() {
        let output = spawn_with_timeout("cat", "payload-123", 2_000);
        assert_eq!(output.as_deref(), Some("payload-123"));
    }

    /// 타임아웃을 초과하는 명령(`sleep 5`)은 200ms 안에 None으로 저하해야 한다(렌더 무블록).
    #[test]
    fn spawn_times_out_quickly() {
        let started = Instant::now();
        let output = spawn_with_timeout("sleep 5", "", 200);
        let elapsed = started.elapsed();
        assert_eq!(output, None, "타임아웃 시 None이어야 함");
        // 200ms 타임아웃 + 폴링/kill 여유. 5초 sleep을 기다리지 않았음을 보증(상한 2초).
        assert!(
            elapsed < Duration::from_secs(2),
            "타임아웃이 렌더를 블록함: {elapsed:?}"
        );
    }

    /// 스폰 자체가 실패하지 않는 셸이라도 비정상 명령은 빈 stdout을 정상 반환한다.
    #[test]
    fn spawn_nonzero_exit_still_returns_stdout() {
        // 종료코드 1이어도 stdout은 수집되어야 한다(체인 자식 실패 격리).
        let output = spawn_with_timeout("printf 'partial'; exit 1", "", 2_000);
        assert_eq!(output.as_deref(), Some("partial"));
    }

    /// run_chain 디커플 로직(신선 캐시 → 재스폰 없음)을 재구성한 캐시 우선순위 검증.
    ///
    /// `run_chain`은 process-global `HOME`에 의존하므로 병렬 테스트 안전을 위해
    /// 전역 env를 변경하지 않고, 핵심 결정 로직(신선 캐시면 자식 미실행)을 동일하게 재현한다.
    /// 자식 미실행은 `is_cache_fresh` 게이트로 결정되므로 그 게이트 + 캐시 읽기로 검증한다.
    #[test]
    fn fresh_cache_decision_skips_spawn() {
        let path =
            std::env::temp_dir().join(format!("understatus-skip-spawn-{}", std::process::id()));
        let now = now_millis();
        write_cache_entry(&path, now, "CACHED-OUTPUT");

        // run_chain과 동일한 결정: 신선 캐시면 자식을 스폰하지 않고 캐시를 반환한다.
        let ttl = Config::default().chain.chain_cache_ttl_seconds; // 3
        let entry = read_cache_entry(&path).expect("캐시 읽기 실패");
        let (written_ms, output) = entry;
        let would_skip_spawn = is_cache_fresh(written_ms, now_millis(), ttl);

        assert!(would_skip_spawn, "신선 캐시는 자식 스폰을 건너뛰어야 함");
        assert_eq!(output, "CACHED-OUTPUT");
        let _ = std::fs::remove_file(&path);
    }

    /// 만료된 캐시는 자식 스폰 결정으로 이어져야 한다(디커플의 반대 케이스).
    #[test]
    fn stale_cache_decision_triggers_spawn() {
        let path =
            std::env::temp_dir().join(format!("understatus-stale-spawn-{}", std::process::id()));
        let now = now_millis();
        // 15초 전에 기록(기본 TTL 10초 초과) → stale.
        write_cache_entry(&path, now.saturating_sub(15_000), "OLD");

        let ttl = Config::default().chain.chain_cache_ttl_seconds;
        let (written_ms, _output) = read_cache_entry(&path).expect("캐시 읽기 실패");
        assert!(
            !is_cache_fresh(written_ms, now, ttl),
            "만료 캐시는 자식 스폰을 트리거해야 함"
        );
        let _ = std::fs::remove_file(&path);
    }
}
