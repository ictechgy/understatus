//! 비파괴 설치/제거: settings.json 백업·체이닝·refreshInterval 라운드트립.
//!
//! 계획서 §D-2/§H-6/AC8/AC9를 따른다. 기존 `~/.claude/settings.json`의 statusLine을
//! 감지→백업→`chain_command`로 보존(체이닝)→understatus으로 교체하고
//! `refreshInterval=1`을 주입한다(기존 값/부재 백업). uninstall은 원본을 정확 복원한다.
//!
//! # 안전성 (DATA-LOSS-GRADE)
//! 이 모듈은 사용자의 실제 `~/.claude/settings.json`을 수정한다. 따라서:
//! - 백업 없이 절대 덮어쓰지 않는다(백업 파일은 멱등하게 1회만 생성).
//! - statusLine 외의 키는 절대 건드리지 않는다(unknown 키 보존).
//! - 멱등: 두 번 설치해도 원본 chain_command를 잃거나 이중 래핑하지 않는다.
//! - uninstall은 백업 기록(`InstallRecord`)으로 주입 전 상태를 정확 복원한다.
//!
//! JSON 변환은 순수 헬퍼(`apply_install`/`apply_uninstall`)로 분리되어 실제 HOME을
//! 건드리지 않고 테스트 가능하다.

use std::path::PathBuf;

use anyhow::{anyhow, Context, Result};
use serde_json::{json, Map, Value};

use crate::config;

/// settings.json의 statusLine 키 이름.
const STATUS_LINE_KEY: &str = "statusLine";
/// statusLine 내부 명령 키 이름.
const COMMAND_KEY: &str = "command";
/// statusLine 내부 refreshInterval 키 이름.
const REFRESH_INTERVAL_KEY: &str = "refreshInterval";
/// statusLine 내부 type 키 이름.
const TYPE_KEY: &str = "type";
/// statusLine 내부 padding 키 이름.
const PADDING_KEY: &str = "padding";


/// 설치가 기록하는 라운드트립 복원 정보.
///
/// `apply_install`이 반환하고 `apply_uninstall`이 소비한다. uninstall이 주입 전
/// 상태(원본 명령 + refreshInterval 유무/값)를 **정확히** 복원할 수 있도록 필요한
/// 최소 정보만 담는다. 실제 디스크 백업 파일(`settings.json.understatus.bak`)에는
/// 원본 settings.json 전체를 별도로 보관하지만, 라운드트립 복원에 필요한 핵심은 이 기록이다.
#[derive(Debug, Clone, PartialEq, Eq)]
struct InstallRecord {
    /// 설치 전 `statusLine.command` 원본 값. statusLine 자체가 없었으면 `None`.
    /// uninstall이 이 값으로 명령을 정확 복원한다(없었으면 statusLine 키 자체 처리).
    original_command: Option<String>,
    /// 설치 전 statusLine 객체가 존재했는지 여부. `false`면 uninstall이 statusLine 키를 제거한다.
    had_status_line: bool,
    /// 설치 전 `refreshInterval` 값. 키가 없었으면 `None`(복원 시 키 삭제).
    original_refresh_interval: Option<Value>,
    /// 설치 전 `padding` 값. 키가 없었으면 `None`(복원 시 주입한 padding 키 삭제).
    original_padding: Option<Value>,
}

/// 순수 헬퍼: 메모리상의 settings `Value`에 설치 변환을 적용한다(실제 HOME 무관).
///
/// statusLine 외 키는 절대 건드리지 않는다. statusLine.command가 이미 understatus을
/// 가리키면(=이미 설치됨) 멱등하게 동작하여 원본 chain_command를 보존하고 이중 래핑하지 않는다.
///
/// # 인자
/// - `settings`: 파싱된 settings.json. in-place로 statusLine만 변형한다.
/// - `understatus_path`: 교체로 주입할 understatus 바이너리 절대 경로.
/// - `refresh_interval`: 주입할 refreshInterval 값(설정된 `config.refresh.interval_seconds`).
///
/// # 반환
/// uninstall이 정확 복원에 사용할 [`InstallRecord`]. 멱등 재설치 시에는 원본(=직전 설치가
/// 보존한 chain_command/refreshInterval)을 그대로 담아 반환한다.
fn apply_install(settings: &mut Value, understatus_path: &str, refresh_interval: u64) -> InstallRecord {
    let root = ensure_object(settings);

    let existing_status_line = root.get(STATUS_LINE_KEY).cloned();
    let had_status_line = matches!(existing_status_line, Some(Value::Object(_)));

    // 이미 설치된 상태인지 감지: statusLine.command == understatus_path.
    let already_installed = existing_status_line
        .as_ref()
        .and_then(|status_line| status_line.get(COMMAND_KEY))
        .and_then(Value::as_str)
        == Some(understatus_path);

    // 멱등: 이미 설치되었다면 원본(이전 설치가 보존한 값)을 다시 계산할 수 없으므로,
    // 호출자가 백업 파일에서 InstallRecord를 복원해야 한다. 여기서는 설치 상태를 유지하되
    // 원본을 알 수 없으니 None/false로 채워 호출자가 백업을 신뢰하도록 한다.
    // 단, statusLine 객체 형태/refreshInterval은 멱등하게 다시 보장한다.
    let record = if already_installed {
        InstallRecord {
            original_command: None,
            had_status_line: true,
            original_refresh_interval: None,
            original_padding: None,
        }
    } else {
        // 신규 설치: 원본 command/refreshInterval/padding을 기록.
        let original_command = existing_status_line
            .as_ref()
            .and_then(|status_line| status_line.get(COMMAND_KEY))
            .and_then(Value::as_str)
            .map(str::to_string);
        let original_refresh_interval = existing_status_line
            .as_ref()
            .and_then(|status_line| status_line.get(REFRESH_INTERVAL_KEY))
            .cloned();
        let original_padding = existing_status_line
            .as_ref()
            .and_then(|status_line| status_line.get(PADDING_KEY))
            .cloned();
        InstallRecord {
            original_command,
            had_status_line,
            original_refresh_interval,
            original_padding,
        }
    };

    // statusLine 객체를 understatus 명령으로 구성/교체. 기존 statusLine의 그 외 키는
    // 보존(병합)하되 type/command/padding/refreshInterval만 understatus 값으로 설정한다.
    let mut status_line = match existing_status_line {
        Some(Value::Object(map)) => map,
        _ => Map::new(),
    };
    status_line.insert(TYPE_KEY.to_string(), json!("command"));
    status_line.insert(COMMAND_KEY.to_string(), json!(understatus_path));
    status_line.insert(PADDING_KEY.to_string(), json!(0));
    status_line.insert(
        REFRESH_INTERVAL_KEY.to_string(),
        json!(refresh_interval as i64),
    );

    root.insert(STATUS_LINE_KEY.to_string(), Value::Object(status_line));
    record
}

/// 순수 헬퍼: 메모리상의 settings `Value`에 제거 변환을 적용한다(실제 HOME 무관).
///
/// [`InstallRecord`]를 사용해 주입 전 상태를 정확 복원한다:
/// - `statusLine.command`를 원본으로 복원(없었으면 statusLine 키 자체 제거).
/// - `refreshInterval`을 주입 전 값으로 복원(없었으면 키 삭제).
/// - 설치가 주입한 `padding`은 신규 설치였을 경우에만 제거한다.
///
/// 멱등: 이미 제거된(설치 흔적이 없는) settings에 적용해도 안전하다.
fn apply_uninstall(settings: &mut Value, record: &InstallRecord) {
    let root = ensure_object(settings);

    if !record.had_status_line {
        // 설치 전 statusLine이 없었으므로 키 자체를 제거해 원상복구.
        root.remove(STATUS_LINE_KEY);
        return;
    }

    let Some(Value::Object(status_line)) = root.get_mut(STATUS_LINE_KEY) else {
        // statusLine이 사라졌거나 객체가 아니면 복원할 대상이 없다(멱등 안전).
        return;
    };

    // command 복원: 원본 값이 있으면 그대로, 없으면 command 키 제거.
    match &record.original_command {
        Some(command) => {
            status_line.insert(COMMAND_KEY.to_string(), json!(command));
        }
        None => {
            status_line.remove(COMMAND_KEY);
        }
    }

    // refreshInterval 복원: 주입 전 값이 있으면 그대로, 없었으면 키 삭제.
    match &record.original_refresh_interval {
        Some(value) => {
            status_line.insert(REFRESH_INTERVAL_KEY.to_string(), value.clone());
        }
        None => {
            status_line.remove(REFRESH_INTERVAL_KEY);
        }
    }

    // padding 복원: 설치 전 값이 있으면 그대로, 없었으면 주입한 padding 키 삭제.
    match &record.original_padding {
        Some(value) => {
            status_line.insert(PADDING_KEY.to_string(), value.clone());
        }
        None => {
            status_line.remove(PADDING_KEY);
        }
    }
}

/// `Value`가 객체가 아니면 빈 객체로 치환하고 내부 `Map`에 대한 가변 참조를 돌려준다.
///
/// settings.json 최상위는 항상 객체여야 한다. 손상/비객체 입력에도 패닉 없이 안전 저하한다.
fn ensure_object(settings: &mut Value) -> &mut Map<String, Value> {
    if !settings.is_object() {
        *settings = Value::Object(Map::new());
    }
    settings
        .as_object_mut()
        .expect("ensure_object: 위에서 객체로 보장했으므로 항상 Some")
}

/// understatus을 비파괴로 설치한다.
///
/// # 동작
/// 1. `~/.claude/settings.json`의 기존 `statusLine.command`를 감지.
/// 2. 원본 전체를 백업(`~/.claude/settings.json.understatus.bak`, 멱등하게 1회만).
/// 3. 기존 명령을 understatus config의 `[chain].chain_command`로 보존(체이닝).
/// 4. `statusLine.command`를 understatus 바이너리 경로로 교체.
/// 5. `statusLine.refreshInterval = 1` 주입(기존 값/부재 상태는 백업에 보존).
///
/// # 반환
/// 성공 시 `Ok(())`. I/O·JSON 파싱 실패 등은 [`anyhow::Error`]로 전파한다.
/// 멱등: 이미 설치된 상태에서 재실행해도 안전하다(AC8).
pub fn install() -> Result<()> {
    let settings_path = settings_json_path()?;
    let understatus_path = understatus_binary_path()?;

    let raw = std::fs::read_to_string(&settings_path)
        .with_context(|| format!("settings.json 읽기 실패: {}", settings_path.display()))?;
    let mut settings: Value = serde_json::from_str(&raw)
        .with_context(|| format!("settings.json JSON 파싱 실패: {}", settings_path.display()))?;

    // 백업: 아직 백업이 없을 때만 원본 전체를 보관(멱등 — 재설치가 백업을 덮지 않는다).
    let backup_path = backup_json_path(&settings_path);
    if !backup_path.exists() {
        std::fs::write(&backup_path, &raw)
            .with_context(|| format!("백업 파일 쓰기 실패: {}", backup_path.display()))?;
    }

    // 기존 statusLine.command를 chain_command로 보존(이미 설치된 경우 건너뜀 — 멱등).
    let original_command = settings
        .get(STATUS_LINE_KEY)
        .and_then(|status_line| status_line.get(COMMAND_KEY))
        .and_then(Value::as_str)
        .filter(|command| *command != understatus_path)
        .map(str::to_string);
    if let Some(command) = original_command {
        merge_chain_command(&command)?;
    }

    // 설정된 refreshInterval을 읽어 주입한다(기본값 5초; 사용자 config로 변경 가능).
    let refresh_interval = config::load_config().refresh.interval_seconds;

    // 순수 변환 적용 후 2-space pretty JSON으로 기록.
    let _record = apply_install(&mut settings, &understatus_path, refresh_interval);
    write_pretty_json(&settings_path, &settings)?;

    Ok(())
}

/// understatus을 제거하고 원본 설정을 정확 복원한다.
///
/// # 동작
/// 1. 백업(`settings.json.understatus.bak`)이 있으면 그 원본으로 settings.json을 통째 복원.
///    (이것이 statusLine.command/refreshInterval/padding을 바이트 단위로 정확 복원하는 가장 안전한 길.)
/// 2. 백업이 없으면 현재 config의 `[chain].chain_command`로 statusLine.command를 복원하고
///    refreshInterval/padding 주입분을 제거하는 best-effort 폴백을 수행한다.
/// 3. 백업 파일을 제거하고 캐시 디렉터리(`~/Library/Caches/understatus`)를 정리한다.
///
/// # 반환
/// 성공 시 `Ok(())`. 멱등: 설치되지 않은 상태에서 실행해도 안전하다(AC8/AC9).
pub fn uninstall() -> Result<()> {
    let settings_path = settings_json_path()?;
    let backup_path = backup_json_path(&settings_path);

    if backup_path.exists() {
        // 백업이 있으면 원본 전체를 그대로 되돌린다(정확/바이트 단위 복원).
        let backup_raw = std::fs::read_to_string(&backup_path)
            .with_context(|| format!("백업 읽기 실패: {}", backup_path.display()))?;
        std::fs::write(&settings_path, &backup_raw).with_context(|| {
            format!("settings.json 복원 쓰기 실패: {}", settings_path.display())
        })?;
        std::fs::remove_file(&backup_path)
            .with_context(|| format!("백업 제거 실패: {}", backup_path.display()))?;
    } else if settings_path.exists() {
        // 백업 부재 폴백: config의 chain_command로 best-effort 복원.
        let raw = std::fs::read_to_string(&settings_path)
            .with_context(|| format!("settings.json 읽기 실패: {}", settings_path.display()))?;
        let mut settings: Value = serde_json::from_str(&raw).with_context(|| {
            format!("settings.json JSON 파싱 실패: {}", settings_path.display())
        })?;
        let record = InstallRecord {
            original_command: read_chain_command_from_config(),
            had_status_line: settings
                .get(STATUS_LINE_KEY)
                .map(Value::is_object)
                .unwrap_or(false),
            original_refresh_interval: None,
            original_padding: None,
        };
        apply_uninstall(&mut settings, &record);
        write_pretty_json(&settings_path, &settings)?;
    }

    // 캐시 디렉터리 정리(존재 시). 영속 상태가 아닌 단기 TTL 캐시이므로 안전 제거.
    if let Some(cache_dir) = cache_dir_path() {
        if cache_dir.exists() {
            let _ = std::fs::remove_dir_all(&cache_dir);
        }
    }

    Ok(())
}

/// `~/.claude/settings.json` 절대 경로를 돌려준다.
fn settings_json_path() -> Result<PathBuf> {
    let home = home_dir().ok_or_else(|| anyhow!("HOME 환경변수를 찾을 수 없습니다"))?;
    Ok(home.join(".claude").join("settings.json"))
}

/// settings.json 백업 파일 경로(`<settings.json>.understatus.bak`)를 돌려준다.
fn backup_json_path(settings_path: &std::path::Path) -> PathBuf {
    let mut backup = settings_path.as_os_str().to_owned();
    backup.push(".understatus.bak");
    PathBuf::from(backup)
}

/// understatus 설정 디렉터리(`~/.config/understatus`) 절대 경로를 돌려준다.
fn config_dir_path() -> Option<PathBuf> {
    home_dir().map(|home| home.join(".config").join("understatus"))
}

/// understatus config.toml 절대 경로를 돌려준다.
fn config_toml_path() -> Option<PathBuf> {
    config_dir_path().map(|dir| dir.join("config.toml"))
}

/// understatus 캐시 디렉터리(`~/Library/Caches/understatus`) 절대 경로를 돌려준다.
fn cache_dir_path() -> Option<PathBuf> {
    home_dir().map(|home| home.join("Library").join("Caches").join("understatus"))
}

/// 설치된(혹은 빌드된) understatus 바이너리의 절대 경로를 돌려준다.
///
/// 현재 실행 중인 바이너리 경로(`std::env::current_exe`)를 사용한다. 이는 사용자가
/// `understatus install`을 실행한 그 바이너리이므로 statusLine.command로 그대로 주입한다.
fn understatus_binary_path() -> Result<String> {
    let exe = std::env::current_exe().context("현재 실행 바이너리 경로 확인 실패")?;
    let canonical = std::fs::canonicalize(&exe).unwrap_or(exe);
    canonical
        .to_str()
        .map(str::to_string)
        .ok_or_else(|| anyhow!("바이너리 경로에 비-UTF8 문자가 포함되어 있습니다"))
}

/// HOME 디렉터리를 돌려준다(환경변수 `HOME`).
fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// settings `Value`를 2-space pretty JSON으로 파일에 기록한다(끝에 개행 1개).
///
/// `serde_json::to_string_pretty`는 기본 2-space 들여쓰기를 사용한다(계획서 요구).
fn write_pretty_json(path: &std::path::Path, settings: &Value) -> Result<()> {
    let mut pretty = serde_json::to_string_pretty(settings).context("settings.json 직렬화 실패")?;
    pretty.push('\n');
    std::fs::write(path, pretty)
        .with_context(|| format!("settings.json 쓰기 실패: {}", path.display()))?;
    Ok(())
}

/// understatus config.toml의 `[chain].chain_command`를 주어진 명령으로 병합 기록한다.
///
/// 기존 config.toml의 다른 키/섹션은 보존(병합)하며 `[chain].chain_command`만 설정한다.
/// 파일/디렉터리가 없으면 생성한다. 멱등: 이미 같은 값이면 그대로 유지된다.
fn merge_chain_command(command: &str) -> Result<()> {
    let Some(config_path) = config_toml_path() else {
        return Err(anyhow!("config.toml 경로를 확인할 수 없습니다"));
    };

    // 기존 config 로드(없으면 빈 테이블).
    let mut doc: toml::Value = match std::fs::read_to_string(&config_path) {
        Ok(raw) => toml::from_str(&raw)
            .with_context(|| format!("config.toml 파싱 실패: {}", config_path.display()))?,
        Err(_) => toml::Value::Table(toml::map::Map::new()),
    };

    // [chain] 테이블 확보 후 chain_command 설정(다른 키 보존).
    let table = doc
        .as_table_mut()
        .ok_or_else(|| anyhow!("config.toml 최상위가 테이블이 아닙니다"))?;
    let chain = table
        .entry("chain".to_string())
        .or_insert_with(|| toml::Value::Table(toml::map::Map::new()));
    let chain_table = chain
        .as_table_mut()
        .ok_or_else(|| anyhow!("config.toml [chain]이 테이블이 아닙니다"))?;
    chain_table.insert(
        "chain_command".to_string(),
        toml::Value::String(command.to_string()),
    );

    // 디렉터리 보장 후 기록.
    if let Some(dir) = config_path.parent() {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("config 디렉터리 생성 실패: {}", dir.display()))?;
    }
    let serialized = toml::to_string_pretty(&doc).context("config.toml 직렬화 실패")?;
    std::fs::write(&config_path, serialized)
        .with_context(|| format!("config.toml 쓰기 실패: {}", config_path.display()))?;
    Ok(())
}

/// config.toml에서 `[chain].chain_command`를 읽는다(백업 부재 폴백 복원용).
///
/// 파일/키 부재 또는 파싱 실패 시 `None`으로 안전 저하한다.
fn read_chain_command_from_config() -> Option<String> {
    let config_path = config_toml_path()?;
    let raw = std::fs::read_to_string(&config_path).ok()?;
    let doc: toml::Value = toml::from_str(&raw).ok()?;
    doc.get("chain")?
        .get("chain_command")?
        .as_str()
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// 실제 사용자 settings.json을 모사: statusLine.command = OMC HUD, refreshInterval 부재.
    fn real_settings() -> Value {
        json!({
            "model": "claude-opus-4",
            "permissions": { "allow": ["Bash"] },
            "statusLine": {
                "type": "command",
                "command": "node $HOME/.claude/hud/lterm-omc-hud.mjs"
            }
        })
    }

    const UNDERSTATUS_PATH: &str = "/usr/local/bin/understatus";
    /// 테스트에서 사용할 주입 refreshInterval 값(설정 기본값과 동일).
    const TEST_REFRESH_INTERVAL: u64 = 5;

    /// AC8/AC9: 실제 settings 라운드트립 — install 후 uninstall 시 원본 Value와 동일.
    #[test]
    fn install_then_uninstall_restores_exactly() {
        let original = real_settings();
        let mut settings = original.clone();

        let record = apply_install(&mut settings, UNDERSTATUS_PATH, TEST_REFRESH_INTERVAL);

        // install 후: command가 understatus을 가리키고 refreshInterval==5(설정값).
        let status_line = settings.get("statusLine").unwrap();
        assert_eq!(
            status_line.get("command").and_then(Value::as_str),
            Some(UNDERSTATUS_PATH)
        );
        assert_eq!(
            status_line.get("refreshInterval").and_then(Value::as_i64),
            Some(5)
        );
        // chain command(원본)가 record에 포착됨.
        assert_eq!(
            record.original_command.as_deref(),
            Some("node $HOME/.claude/hud/lterm-omc-hud.mjs")
        );
        assert!(record.had_status_line);
        // 원본에 refreshInterval이 없었으므로 None.
        assert_eq!(record.original_refresh_interval, None);
        // statusLine 외 키는 그대로 보존.
        assert_eq!(settings.get("model"), original.get("model"));
        assert_eq!(settings.get("permissions"), original.get("permissions"));

        // uninstall 후: 원본 Value와 정확히 동일(command 복원, refreshInterval/padding 부재).
        apply_uninstall(&mut settings, &record);
        assert_eq!(
            settings, original,
            "uninstall 후 settings는 원본과 정확히 같아야 한다"
        );
    }

    /// AC8: 멱등 — install을 두 번 적용해도 install 한 번과 동일한 결과.
    #[test]
    fn install_is_idempotent() {
        let mut once = real_settings();
        apply_install(&mut once, UNDERSTATUS_PATH, TEST_REFRESH_INTERVAL);

        let mut twice = real_settings();
        apply_install(&mut twice, UNDERSTATUS_PATH, TEST_REFRESH_INTERVAL);
        apply_install(&mut twice, UNDERSTATUS_PATH, TEST_REFRESH_INTERVAL);

        assert_eq!(
            once, twice,
            "두 번 설치한 결과는 한 번 설치한 결과와 같아야 한다(이중 래핑 금지)"
        );
        // 멱등 재설치가 chain_command(understatus 자기 경로)를 잘못 보존하지 않음:
        // 두 번째 apply_install의 record는 already_installed로 original_command=None.
        let mut detect = real_settings();
        apply_install(&mut detect, UNDERSTATUS_PATH, TEST_REFRESH_INTERVAL);
        let second = apply_install(&mut detect, UNDERSTATUS_PATH, TEST_REFRESH_INTERVAL);
        assert_eq!(second.original_command, None);
    }

    /// AC9: statusLine이 없던 settings에 install → uninstall 시 statusLine 키가 다시 사라짐.
    #[test]
    fn install_without_status_line_then_uninstall_removes_key() {
        let original = json!({ "model": "claude-opus-4" });
        let mut settings = original.clone();

        let record = apply_install(&mut settings, UNDERSTATUS_PATH, TEST_REFRESH_INTERVAL);
        assert!(!record.had_status_line);
        assert_eq!(record.original_command, None);

        // install 후 statusLine 주입 확인.
        let status_line = settings.get("statusLine").unwrap();
        assert_eq!(
            status_line.get("command").and_then(Value::as_str),
            Some(UNDERSTATUS_PATH)
        );
        assert_eq!(
            status_line.get("refreshInterval").and_then(Value::as_i64),
            Some(5)
        );

        // uninstall: statusLine 키 자체 제거 → 원본과 동일.
        apply_uninstall(&mut settings, &record);
        assert_eq!(
            settings, original,
            "statusLine이 없던 원본은 uninstall 후 statusLine 키가 없어야 한다"
        );
    }

    /// AC9: 기존 refreshInterval 값이 있던 경우 → uninstall이 원래 값으로 정확 복원.
    #[test]
    fn install_preserves_and_restores_existing_refresh_interval() {
        let original = json!({
            "statusLine": {
                "type": "command",
                "command": "node $HOME/.claude/hud/lterm-omc-hud.mjs",
                "refreshInterval": 5
            }
        });
        let mut settings = original.clone();

        let record = apply_install(&mut settings, UNDERSTATUS_PATH, TEST_REFRESH_INTERVAL);
        assert_eq!(record.original_refresh_interval, Some(json!(5)));
        // 주입값(5)으로 덮어씀 — 기존 값과 동일하므로 이 케이스는 실질적으로 무변화.
        assert_eq!(
            settings
                .get("statusLine")
                .and_then(|s| s.get("refreshInterval"))
                .and_then(Value::as_i64),
            Some(5)
        );

        apply_uninstall(&mut settings, &record);
        assert_eq!(
            settings, original,
            "기존 refreshInterval=5는 uninstall 후 정확히 5로 복원되어야 한다"
        );
    }

    /// statusLine에 알 수 없는 추가 키가 있어도 보존(병합)하며 understatus 값만 설정.
    #[test]
    fn install_preserves_unknown_status_line_keys() {
        let original = json!({
            "statusLine": {
                "type": "command",
                "command": "node old.mjs",
                "customExtra": "keep-me"
            }
        });
        let mut settings = original.clone();
        apply_install(&mut settings, UNDERSTATUS_PATH, TEST_REFRESH_INTERVAL);
        assert_eq!(
            settings
                .get("statusLine")
                .and_then(|s| s.get("customExtra"))
                .and_then(Value::as_str),
            Some("keep-me")
        );
    }
}
