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

use crate::config::{self, Config};
use crate::theme;
use crate::themes;

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
fn apply_install(
    settings: &mut Value,
    understatus_path: &str,
    refresh_interval: u64,
) -> InstallRecord {
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
/// # 인자
/// - `interval`: 주입할 refreshInterval 초. main이 플래그/프롬프트/기존값 승계까지 해석한 최종 값.
/// - `theme`: 적용 테마 이름. 진입부에서 [`validate_theme`]로 하드 검증한다(미지 테마면 즉시 중단).
///   `--theme` 플래그 경로(main의 `resolve_theme`)는 검증을 하지 않으므로 모든 경로를
///   여기서 일괄 커버한다(런타임 calm 폴백에 의존한 미지 테마 기록 방지).
///
/// # 동작
/// 1. `~/.claude/settings.json`의 기존 `statusLine.command`를 감지.
/// 2. 원본 전체를 백업(`~/.claude/settings.json.understatus.bak`, 멱등하게 1회만).
/// 3. 기존 명령을 understatus config의 `[chain].chain_command`로 보존(체이닝).
/// 4. `statusLine.command`를 understatus 바이너리 경로로 교체.
/// 5. `statusLine.refreshInterval = interval` 주입(기존 값/부재 상태는 백업에 보존).
/// 6. config.toml에 `theme`/`[refresh].interval_seconds`를 1회 병합 기록(기존 키 보존).
/// 7. 호흡 불변식 위반 시 stderr 경고(자동 보정 안 함, 사용자 pulse_period_seconds 반영).
///
/// # interval 미러 관계
/// config.toml `interval_seconds`와 settings.json `refreshInterval`을 동일 값으로 동기 기록한다.
/// 단 실제 렌더 주기는 settings.json `refreshInterval`이 결정(Claude Code가 그 값으로 호출),
/// config.toml `interval_seconds`는 install 미러 + main.rs debug_assert 소스. install 시점에만 일치 보장.
///
/// # 반환
/// 성공 시 `Ok(())`. I/O·JSON 파싱 실패 등은 [`anyhow::Error`]로 전파한다.
/// 멱등: 이미 설치된 상태에서 재실행해도 안전하다(AC8).
pub fn install(interval: u64, theme: &str) -> Result<()> {
    // 미지 테마는 어떤 디스크 쓰기보다 먼저 거부한다(settings.json/config.toml 미기록).
    validate_theme(theme)?;

    let settings_path = settings_json_path()?;
    let understatus_path = understatus_binary_path()?;

    let raw = std::fs::read_to_string(&settings_path)
        .with_context(|| format!("settings.json 읽기 실패: {}", settings_path.display()))?;
    let mut settings: Value = serde_json::from_str(&raw)
        .with_context(|| format!("settings.json JSON 파싱 실패: {}", settings_path.display()))?;

    // 백업: 아직 백업이 없을 때만 원본 전체를 보관(멱등 — 재설치가 백업을 덮지 않는다).
    // settings.json 교체 이전에 원본을 보존해야 라운드트립(uninstall 정확 복원)이 보장된다.
    let backup_path = backup_json_path(&settings_path);
    if !backup_path.exists() {
        std::fs::write(&backup_path, &raw)
            .with_context(|| format!("백업 파일 쓰기 실패: {}", backup_path.display()))?;
    }

    // 기존 settings.json의 statusLine.command를 chain_command로 보존(이미 설치된 경우 건너뜀 — 멱등).
    // settings.json을 교체하기 전에 원본 명령을 읽어 둔다.
    let original_command = settings
        .get(STATUS_LINE_KEY)
        .and_then(|status_line| status_line.get(COMMAND_KEY))
        .and_then(Value::as_str)
        .filter(|command| *command != understatus_path)
        .map(str::to_string);

    // 호흡 경고용: 기존 config.toml에서 사용자 pulse_period_seconds를 반영(BLOCKING-2).
    // config.toml을 수정하기 전 원본에서 사용자 값을 읽어야 한다.
    // 파일 부재/파싱 실패면 parse_config_str/unwrap_or_default가 Config::default()로 안전 저하.
    let cfg_for_warn = read_existing_config_str()
        .as_deref()
        .map(config::parse_config_str)
        .unwrap_or_default();

    // 쓰기 순서(부분 설치 방지): config.toml을 settings.json보다 **먼저** 기록한다.
    // config 단계가 실패하면 settings.json은 손대지 않은 상태로 에러를 전파해야 한다.
    // settings.json을 먼저 교체했다가 config 쓰기가 실패하면 settings는 understatus로
    // 바뀌었는데 config는 미기록인 부분 설치가 발생한다(HIGH 블로커).
    // config.toml 단일 read-modify-write로 chain(있으면)+theme+interval 1회 기록.
    edit_config_doc(|table| {
        if let Some(command) = &original_command {
            set_chain_command(table, command);
        }
        table.insert("theme".to_string(), toml::Value::String(theme.to_string()));
        set_refresh_interval(table, interval);
        Ok(())
    })?;

    // config.toml 기록 성공 확인 후에만 settings.json을 understatus로 교체+refreshInterval 주입.
    // 순수 변환 적용 후 2-space pretty JSON으로 기록(settings 측 interval 미러).
    let _record = apply_install(&mut settings, &understatus_path, interval);
    write_pretty_json(&settings_path, &settings)?;

    warn_if_pulse_period_too_short(&cfg_for_warn, interval);
    Ok(())
}

/// 설치 후 테마만 교체한다(config.toml 최상위 `theme` 키만, settings.json 무접근). interval 무변경.
///
/// # 인자
/// - `name`: 적용할 테마 이름. `is_known` 검증을 통과해야 한다(미지 테마면 하드 에러 + 목록).
///
/// # 반환
/// 성공 시 `Ok(())`. 미지 테마/I-O 실패는 [`anyhow::Error`]로 전파한다.
pub fn set_theme(name: &str) -> Result<()> {
    validate_theme(name)?;
    edit_config_doc(|table| {
        table.insert("theme".to_string(), toml::Value::String(name.to_string()));
        Ok(())
    })
}

/// 테마 이름이 출시 테마인지 하드 검증한다(순수 함수, I/O 없음 — 테스트 용이).
///
/// `install`/`set_theme`/(향후) 모든 테마 기록 경로가 공유하는 단일 검증점이다.
/// 미지 테마면 사용 가능 목록을 포함한 [`anyhow::Error`]를 돌려 호출부가 디스크 쓰기
/// 이전에 중단하게 한다(런타임 calm 폴백에 의존한 잘못된 기록 방지).
///
/// # 인자
/// - `name`: 검증할 테마 이름.
///
/// # 반환
/// 알려진 테마면 `Ok(())`, 미지면 유효 목록을 담은 `Err`.
fn validate_theme(name: &str) -> Result<()> {
    if themes::is_known(name) {
        return Ok(());
    }
    Err(anyhow!(
        "알 수 없는 테마 '{name}'. 사용 가능: {}",
        known_theme_names()
    ))
}

/// 설치 후 펄스 스타일만 교체한다(config.toml `[pulse].pulse_style`만, settings.json 무접근).
///
/// # 인자
/// - `style`: 적용할 펄스 스타일. [`validate_pulse_style`]를 통과해야 한다(미지면 하드 에러 + 목록).
///
/// # 반환
/// 성공 시 `Ok(())`. 미지 스타일/I-O 실패는 [`anyhow::Error`]로 전파한다.
pub fn set_pulse_style(style: &str) -> Result<()> {
    validate_pulse_style(style)?;
    edit_config_doc(|table| {
        set_pulse_style_key(table, style);
        Ok(())
    })
}

/// 펄스 스타일 이름이 출시 스타일인지 하드 검증한다(순수 함수, I/O 없음).
///
/// `set_pulse_style`(및 향후 모든 쓰기 경로)이 공유하는 단일 검증점이다. 미지 스타일이면
/// 사용 가능 목록을 담은 에러를 돌려 디스크 쓰기 이전에 중단하게 한다(render calm 폴백 의존 방지).
fn validate_pulse_style(style: &str) -> Result<()> {
    if theme::is_known_pulse_style(style) {
        return Ok(());
    }
    Err(anyhow!(
        "알 수 없는 펄스 스타일 '{style}'. 사용 가능: {}",
        theme::PULSE_STYLES.join(", ")
    ))
}

/// 출시 테마 이름을 쉼표로 이어 돌려준다(에러 메시지용).
fn known_theme_names() -> String {
    themes::catalog()
        .iter()
        .map(|(name, _)| *name)
        .collect::<Vec<_>>()
        .join(", ")
}

/// 기존 config.toml 원문을 읽는다(디스크 read만 격리). 부재/읽기 실패 시 `None`.
///
/// `config_toml_path()`는 `UNDERSTATUS_CONFIG` 오버라이드를 따르므로 테스트에서 경로 주입 가능하다.
pub(crate) fn read_existing_config_str() -> Option<String> {
    let path = config_toml_path()?;
    std::fs::read_to_string(&path).ok()
}

/// 기존 config 원문에서 `[refresh].interval_seconds`를 추출한다(승계용). **순수 함수.**
///
/// # 인자
/// - `existing`: 기존 config.toml 원문(없으면 `None`).
///
/// # 반환
/// 사용자가 **명시한** interval만 `Some`. 원문이 `None`이거나 파싱 실패, 또는
/// `[refresh].interval_seconds`가 없으면 `None`(호출부가 기본값 폴백).
///
/// # 주의
/// `config::parse_config_str`는 파싱 실패 시 `Config::default()`(interval=5)로 저하하므로,
/// "사용자가 명시한 interval"만 승계하려면 `toml::Value` has_key로 명시 여부를 직접 본다.
pub(crate) fn existing_interval(existing: Option<&str>) -> Option<u64> {
    let raw = existing?;
    let value: toml::Value = toml::from_str(raw).ok()?;
    value
        .get("refresh")?
        .get("interval_seconds")?
        .as_integer()
        // CLI `parse_interval`은 `>= 1`만 허용한다. 승계 경로도 동일 규약을 지켜
        // 음수(i64→u64 wrap으로 거대값)와 0(검증 우회)을 차단한다.
        // `>= 1`인 값만 승계하고, 나머지는 None으로 떨어뜨려 기본 5 폴백을 타게 한다.
        .filter(|n| *n >= 1)
        .map(|n| n as u64)
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
/// # config.toml 라운드트립(설계 결정)
/// uninstall은 settings.json만 복원하며 config.toml의 `theme`/`interval_seconds`는 보존한다
/// (사용자 설정). 이 잔존은 불변 제약 (b) 위반이 아니다(제약 (b)는 settings.json 한정).
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
///
/// config.rs `config_path`와 **대칭**으로 `UNDERSTATUS_CONFIG` 오버라이드를 우선한다
/// (테스트/측정용). 없으면 `~/.config/understatus/config.toml`. 이로써 install의
/// 읽기/쓰기/uninstall 폴백이 런타임 config 로드와 동일 경로 규칙을 공유한다(SSOT).
fn config_toml_path() -> Option<PathBuf> {
    if let Ok(override_path) = std::env::var("UNDERSTATUS_CONFIG") {
        return Some(PathBuf::from(override_path));
    }
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

/// config.toml 텍스트를 받아 편집한 새 TOML 텍스트를 돌려준다. **디스크 I/O 없음 — 순수 함수.**
///
/// # 인자
/// - `existing`: 기존 config.toml 원문. `None`이면 빈 테이블에서 시작.
/// - `edit`: 최상위 테이블을 in-place로 편집하는 클로저.
///
/// # 반환
/// 편집 후 직렬화한 TOML 문자열. 기존 키/섹션은 보존(병합)한다. 멱등.
///
/// # 주의
/// toml 0.8 `Value`는 주석/키 순서/포매팅을 보존하지 않는다(기존 merge_chain_command와 동일 동작).
fn edit_config_doc_str<F>(existing: Option<&str>, edit: F) -> Result<String>
where
    F: FnOnce(&mut toml::value::Table) -> Result<()>,
{
    let mut doc: toml::Value = match existing {
        Some(raw) => toml::from_str(raw).context("config.toml 파싱 실패")?,
        None => toml::Value::Table(toml::map::Map::new()),
    };
    let table = doc
        .as_table_mut()
        .ok_or_else(|| anyhow!("config.toml 최상위가 테이블이 아닙니다"))?;
    edit(table)?;
    toml::to_string_pretty(&doc).context("config.toml 직렬화 실패")
}

/// `edit_config_doc_str`의 디스크 래퍼: read → 변환 → 디렉터리 보장 → write.
///
/// 디스크 결합(read/create_dir_all/write)은 여기에만 격리한다. 변환 로직은 순수
/// `edit_config_doc_str`가 담당하므로 인메모리 단언이 가능하다. 디스크 경로는
/// `UNDERSTATUS_CONFIG` 오버라이드 덕에 `edit_config_doc_disk_roundtrip` 일반 테스트로 커버된다.
fn edit_config_doc<F>(edit: F) -> Result<()>
where
    F: FnOnce(&mut toml::value::Table) -> Result<()>,
{
    let Some(config_path) = config_toml_path() else {
        return Err(anyhow!("config.toml 경로를 확인할 수 없습니다"));
    };
    let existing = std::fs::read_to_string(&config_path).ok();
    let serialized = edit_config_doc_str(existing.as_deref(), edit)?;
    if let Some(dir) = config_path.parent() {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("config 디렉터리 생성 실패: {}", dir.display()))?;
    }
    std::fs::write(&config_path, serialized)
        .with_context(|| format!("config.toml 쓰기 실패: {}", config_path.display()))?;
    Ok(())
}

/// 최상위 테이블에 `[chain].chain_command`를 설정한다(다른 키 보존). 기존 merge_chain_command 본문 이식.
///
/// `[chain]`이 존재하나 테이블이 아니면(손상된 config) 조용히 무시하지 않고 새 테이블로
/// 교체한다. 그래야 install이 값을 반드시 기록한다(부분 설치/값 누락 방지). 조용한 no-op은
/// 기존 merge_chain_command 동작(에러)에서 회귀한 것이므로 install 성공을 보장하도록 복구한다.
fn set_chain_command(table: &mut toml::value::Table, command: &str) {
    let chain = table
        .entry("chain".to_string())
        .or_insert_with(|| toml::Value::Table(toml::map::Map::new()));
    // 섹션이 비-table(손상)이면 새 빈 테이블로 덮어써 키 기록을 보장한다.
    if !chain.is_table() {
        *chain = toml::Value::Table(toml::map::Map::new());
    }
    let chain_table = chain
        .as_table_mut()
        .expect("set_chain_command: 위에서 테이블로 보장했으므로 항상 Some");
    chain_table.insert(
        "chain_command".to_string(),
        toml::Value::String(command.to_string()),
    );
}

/// 최상위 테이블에 `[refresh].interval_seconds`를 설정한다(install 미러; 다른 키 보존).
///
/// `[refresh]`가 존재하나 테이블이 아니면(손상된 config) 조용히 무시하지 않고 새 테이블로
/// 교체한다. 그래야 install이 interval을 반드시 기록한다(부분 설치/값 누락 방지).
fn set_refresh_interval(table: &mut toml::value::Table, interval: u64) {
    let refresh = table
        .entry("refresh".to_string())
        .or_insert_with(|| toml::Value::Table(toml::map::Map::new()));
    // 섹션이 비-table(손상)이면 새 빈 테이블로 덮어써 키 기록을 보장한다.
    if !refresh.is_table() {
        *refresh = toml::Value::Table(toml::map::Map::new());
    }
    let refresh_table = refresh
        .as_table_mut()
        .expect("set_refresh_interval: 위에서 테이블로 보장했으므로 항상 Some");
    refresh_table.insert(
        "interval_seconds".to_string(),
        toml::Value::Integer(interval as i64),
    );
}

/// 최상위 테이블에 `[pulse].pulse_style`을 설정한다(다른 키 보존). `set_chain_command`와 동형.
///
/// `[pulse]`가 존재하나 테이블이 아니면(손상된 config) 새 테이블로 교체해 키 기록을 보장한다
/// (조용한 no-op로 인한 부분 기록 방지).
fn set_pulse_style_key(table: &mut toml::value::Table, style: &str) {
    let pulse = table
        .entry("pulse".to_string())
        .or_insert_with(|| toml::Value::Table(toml::map::Map::new()));
    if !pulse.is_table() {
        *pulse = toml::Value::Table(toml::map::Map::new());
    }
    let pulse_table = pulse
        .as_table_mut()
        .expect("set_pulse_style_key: 위에서 테이블로 보장했으므로 항상 Some");
    pulse_table.insert(
        "pulse_style".to_string(),
        toml::Value::String(style.to_string()),
    );
}

/// 호흡 불변식 위반(한 색 주기 안 프레임 < 6) 여부를 판정하는 **순수 함수**.
///
/// 판정은 런타임 debug_assert(main.rs)와 동일하게 `theme::samples_per_period`를 재사용한다(SSOT).
/// `interval==0`은 `samples_per_period` 내부 `.max(1)`로 1초 간주(theme.rs) → 런타임과 일치.
fn pulse_period_too_short(cfg: &Config, interval: u64) -> bool {
    theme::samples_per_period(cfg, interval) < 6
}

/// 호흡 불변식 위반 시 stderr 경고 1줄을 출력한다(자동 보정 안 함). 판정은 [`pulse_period_too_short`]에 위임.
///
/// 출력 전용 얇은 래퍼다. `cfg`는 호출부가 주입하므로(기존 config.toml의 사용자
/// `pulse_period_seconds` 반영) 사용자가 주기를 커스텀한 경우에도 경고가 사실과 일치한다(BLOCKING-2).
fn warn_if_pulse_period_too_short(cfg: &Config, interval: u64) {
    if pulse_period_too_short(cfg, interval) {
        eprintln!(
            "understatus: refreshInterval={interval}s에서는 테라코타 호흡이 끊길 수 있습니다\
             (현재 pulse_period_seconds={}, 권장 >= {}).",
            cfg.pulse.pulse_period_seconds,
            interval.saturating_mul(6)
        );
    }
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

    // --- config.toml 쓰기 변환(인메모리, 디스크 무접근) ---

    /// install이 chain+theme+interval을 단일 doc에 기록하고 기존 [chain]을 보존한다.
    #[test]
    fn install_writes_theme_and_interval_in_single_doc() {
        let existing = "[chain]\nchain_command = \"node old.mjs\"\n";
        let serialized = edit_config_doc_str(Some(existing), |table| {
            set_chain_command(table, "node old.mjs");
            table.insert(
                "theme".to_string(),
                toml::Value::String("ember".to_string()),
            );
            set_refresh_interval(table, 7);
            Ok(())
        })
        .expect("변환 성공");
        let parsed: toml::Value = toml::from_str(&serialized).expect("재파싱 성공");
        assert_eq!(
            parsed.get("theme").and_then(toml::Value::as_str),
            Some("ember")
        );
        assert_eq!(
            parsed
                .get("refresh")
                .and_then(|t| t.get("interval_seconds"))
                .and_then(toml::Value::as_integer),
            Some(7)
        );
        // 기존 [chain] 보존.
        assert_eq!(
            parsed
                .get("chain")
                .and_then(|t| t.get("chain_command"))
                .and_then(toml::Value::as_str),
            Some("node old.mjs")
        );
    }

    /// 손상된(비-table) [chain]/[refresh] 섹션이어도 install이 값을 확실히 기록한다(회귀 차단).
    ///
    /// `[chain]`/`[refresh]`가 문자열/정수 등 비-table로 들어오면 기존 구현은 조용히 no-op했다
    /// (값 미기록 → 부분 설치). 이제 새 테이블로 교체하고 키를 기록해야 한다.
    #[test]
    fn set_section_keys_overwrite_non_table_section() {
        // chain/refresh가 테이블이 아닌 스칼라로 손상된 입력.
        let existing = "chain = \"corrupted\"\nrefresh = 42\n";
        let serialized = edit_config_doc_str(Some(existing), |table| {
            set_chain_command(table, "node old.mjs");
            set_refresh_interval(table, 9);
            Ok(())
        })
        .expect("변환 성공");
        let parsed: toml::Value = toml::from_str(&serialized).expect("재파싱 성공");
        // chain_command가 새 테이블에 기록됨(조용한 no-op 아님).
        assert_eq!(
            parsed
                .get("chain")
                .and_then(|t| t.get("chain_command"))
                .and_then(toml::Value::as_str),
            Some("node old.mjs")
        );
        // interval_seconds도 새 테이블에 기록됨.
        assert_eq!(
            parsed
                .get("refresh")
                .and_then(|t| t.get("interval_seconds"))
                .and_then(toml::Value::as_integer),
            Some(9)
        );
    }

    /// existing=None이면 빈 테이블에서 시작해 theme/interval만 든 유효 TOML을 만든다.
    #[test]
    fn edit_config_doc_str_creates_from_none() {
        let serialized = edit_config_doc_str(None, |table| {
            table.insert(
                "theme".to_string(),
                toml::Value::String("vivid".to_string()),
            );
            set_refresh_interval(table, 5);
            Ok(())
        })
        .expect("변환 성공");
        let parsed: toml::Value = toml::from_str(&serialized).expect("재파싱 성공");
        assert_eq!(
            parsed.get("theme").and_then(toml::Value::as_str),
            Some("vivid")
        );
        assert_eq!(
            parsed
                .get("refresh")
                .and_then(|t| t.get("interval_seconds"))
                .and_then(toml::Value::as_integer),
            Some(5)
        );
    }

    /// 무관 키([cpu] load_glyphs)는 변환에서 보존되어야 한다.
    #[test]
    fn edit_config_doc_str_preserves_unrelated_keys() {
        let existing = "[cpu]\nload_glyphs = [\"a\", \"b\"]\n";
        let serialized = edit_config_doc_str(Some(existing), |table| {
            table.insert("theme".to_string(), toml::Value::String("mono".to_string()));
            Ok(())
        })
        .expect("변환 성공");
        let parsed: toml::Value = toml::from_str(&serialized).expect("재파싱 성공");
        assert_eq!(
            parsed.get("theme").and_then(toml::Value::as_str),
            Some("mono")
        );
        let glyphs = parsed
            .get("cpu")
            .and_then(|t| t.get("load_glyphs"))
            .and_then(toml::Value::as_array)
            .expect("load_glyphs 보존");
        assert_eq!(glyphs.len(), 2);
    }

    /// set_theme 변환은 theme 키만 교체하고 chain/refresh를 보존한다(인메모리 검증).
    #[test]
    fn set_theme_replaces_only_theme_key() {
        let existing =
            "theme = \"calm\"\n[chain]\nchain_command = \"x\"\n[refresh]\ninterval_seconds = 10\n";
        let serialized = edit_config_doc_str(Some(existing), |table| {
            table.insert(
                "theme".to_string(),
                toml::Value::String("vivid".to_string()),
            );
            Ok(())
        })
        .expect("변환 성공");
        let parsed: toml::Value = toml::from_str(&serialized).expect("재파싱 성공");
        assert_eq!(
            parsed.get("theme").and_then(toml::Value::as_str),
            Some("vivid")
        );
        // chain/refresh 보존.
        assert_eq!(
            parsed
                .get("chain")
                .and_then(|t| t.get("chain_command"))
                .and_then(toml::Value::as_str),
            Some("x")
        );
        assert_eq!(
            parsed
                .get("refresh")
                .and_then(|t| t.get("interval_seconds"))
                .and_then(toml::Value::as_integer),
            Some(10)
        );
    }

    /// set_theme은 미지 테마를 거부하고 에러에 사용 가능 목록을 포함한다.
    #[test]
    fn set_theme_rejects_unknown() {
        let result = set_theme("does-not-exist");
        let error = result.expect_err("미지 테마는 Err");
        let message = format!("{error}");
        assert!(message.contains("does-not-exist"), "에러에 테마 이름 포함");
        assert!(message.contains("calm"), "에러에 사용 가능 목록 포함");
    }

    /// install이 공유하는 순수 검증점 `validate_theme`는 미지 테마를 거부하고(목록 포함),
    /// 출시 테마는 통과시킨다. install의 진입부 검증이 디스크 쓰기 이전에 작동함을 보장한다.
    ///
    /// install()은 실제 `~/.claude/settings.json`을 건드리므로 단위 테스트에서 호출하지 않고,
    /// install이 진입부에서 호출하는 순수 검증 함수를 직접 검증한다(검증 함수 단위 갈음).
    #[test]
    fn install_rejects_unknown_theme() {
        let error = validate_theme("bogus").expect_err("미지 테마는 Err");
        let message = format!("{error}");
        assert!(message.contains("bogus"), "에러에 미지 테마 이름 포함");
        assert!(message.contains("calm"), "에러에 사용 가능 목록 포함");
        // 출시 테마는 전부 통과(검증이 정상 설치를 막지 않음).
        for (name, _) in themes::catalog() {
            assert!(validate_theme(name).is_ok(), "출시 테마 {name}은 통과해야");
        }
    }

    /// theme 교체는 interval_seconds를 바꾸지 않는다(변환 전후 interval 불변).
    #[test]
    fn theme_command_does_not_change_interval() {
        let existing = "theme = \"calm\"\n[refresh]\ninterval_seconds = 12\n";
        let serialized = edit_config_doc_str(Some(existing), |table| {
            table.insert(
                "theme".to_string(),
                toml::Value::String("ember".to_string()),
            );
            Ok(())
        })
        .expect("변환 성공");
        let parsed: toml::Value = toml::from_str(&serialized).expect("재파싱 성공");
        assert_eq!(
            parsed
                .get("refresh")
                .and_then(|t| t.get("interval_seconds"))
                .and_then(toml::Value::as_integer),
            Some(12),
            "theme 교체 후에도 interval 불변"
        );
    }

    /// interval이 settings.json/config.toml 양측에 동일 값으로 미러됨을 인메모리로 증명.
    #[test]
    fn install_mirrors_interval_to_both_files() {
        // settings 측: apply_install(...,5) 후 refreshInterval==5.
        let mut settings = real_settings();
        apply_install(&mut settings, UNDERSTATUS_PATH, 5);
        assert_eq!(
            settings
                .get("statusLine")
                .and_then(|s| s.get("refreshInterval"))
                .and_then(Value::as_i64),
            Some(5)
        );
        // config 측: set_refresh_interval(5) 후 interval_seconds==5.
        let serialized = edit_config_doc_str(None, |table| {
            set_refresh_interval(table, 5);
            Ok(())
        })
        .expect("변환 성공");
        let parsed: toml::Value = toml::from_str(&serialized).expect("재파싱 성공");
        assert_eq!(
            parsed
                .get("refresh")
                .and_then(|t| t.get("interval_seconds"))
                .and_then(toml::Value::as_integer),
            Some(5)
        );
    }

    // --- 호흡 경고 bool 순수 판정(블로킹 A·B + BLOCKING-2) ---

    /// 호흡 불변식 bool 판정: pp=30 기준 interval 조합. side-effect/출력 캡처 없음.
    ///
    /// 분모 가드(interval==0 → .max(1))는 theme.rs:samples_per_period_guards_zero_interval에
    /// 위임된다(본 테스트는 install 측 bool 래핑을 책임).
    #[test]
    fn pulse_period_too_short_cases() {
        let mut cfg = Config::default();
        cfg.pulse.pulse_period_seconds = 30;
        assert!(!pulse_period_too_short(&cfg, 5), "30/5=6 → false");
        assert!(pulse_period_too_short(&cfg, 6), "30/6=5 → true");
        assert!(pulse_period_too_short(&cfg, 10), "30/10=3 → true");
        assert!(!pulse_period_too_short(&cfg, 0), "30/1=30(.max(1)) → false");
        cfg.pulse.pulse_period_seconds = 60;
        assert!(!pulse_period_too_short(&cfg, 10), "60/10=6 → false");
    }

    /// 사용자 Config 주입이 판정을 바꿈을 고정(BLOCKING-2): 거짓 경고/경고 누락 양방향.
    #[test]
    fn pulse_period_too_short_uses_injected_cfg() {
        // pp=12 + iv=3 → 12/3=4 < 6 → true(default였으면 30/3=10 → false로 경고 누락).
        let mut custom = Config::default();
        custom.pulse.pulse_period_seconds = 12;
        assert!(pulse_period_too_short(&custom, 3));
        assert!(!pulse_period_too_short(&Config::default(), 3));
        // pp=60 + iv=8 → 60/8=7 ≥ 6 → false(default였으면 30/8=3 → true로 거짓 경고).
        let mut long = Config::default();
        long.pulse.pulse_period_seconds = 60;
        assert!(!pulse_period_too_short(&long, 8));
        assert!(pulse_period_too_short(&Config::default(), 8));
    }

    // --- interval 승계 순수 추출(BLOCKING-1) ---

    /// existing_interval은 명시된 interval만 추출하고, 부재/파싱 실패는 None으로 안전 저하한다.
    #[test]
    fn existing_interval_extracts_user_value() {
        assert_eq!(
            existing_interval(Some("[refresh]\ninterval_seconds = 10")),
            Some(10)
        );
        // refresh 섹션 없음 → None.
        assert_eq!(
            existing_interval(Some("[pulse]\npulse_period_seconds = 30")),
            None
        );
        // 빈 문자열 → None.
        assert_eq!(existing_interval(Some("")), None);
        // None → None.
        assert_eq!(existing_interval(None), None);
        // 파싱 실패 → None(안전 저하).
        assert_eq!(existing_interval(Some("not valid toml ===")), None);
    }

    /// 음수/0 interval_seconds는 승계되지 않고 None(→ 기본 5 폴백)이어야 한다(BLOCKING-2).
    ///
    /// i64→u64 직접 캐스트는 음수를 거대값으로 wrap하고 0도 통과시켜 CLI `>= 1` 검증을
    /// 우회한다. 승계 경로도 `>= 1`만 허용함을 고정한다.
    #[test]
    fn existing_interval_rejects_non_positive() {
        // 음수 → None(거대값 wrap 차단).
        assert_eq!(
            existing_interval(Some("[refresh]\ninterval_seconds = -1")),
            None
        );
        // 0 → None(CLI `>= 1` 검증 우회 차단).
        assert_eq!(
            existing_interval(Some("[refresh]\ninterval_seconds = 0")),
            None
        );
        // 경계값 1은 정상 승계.
        assert_eq!(
            existing_interval(Some("[refresh]\ninterval_seconds = 1")),
            Some(1)
        );
    }

    // --- 디스크 라운드트립(UNDERSTATUS_CONFIG 오버라이드 + CONFIG_PATH_LOCK 직렬, HOME 미변경) ---

    /// UNDERSTATUS_CONFIG 환경변수를 직렬화하는 전용 락(render.rs:561 ENV_LOCK 선례 따름).
    static CONFIG_PATH_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// 테스트용 고유 임시 config.toml 경로(프로세스 내 충돌 방지용 카운터 포함).
    fn unique_temp_config_path(tag: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();
        std::env::temp_dir().join(format!("understatus-test-{tag}-{pid}-{n}.toml"))
    }

    /// edit_config_doc 디스크 래퍼의 read→변환→write 경로를 검증한다(CONFIG_PATH_LOCK 직렬).
    #[test]
    fn edit_config_doc_disk_roundtrip() {
        let _guard = CONFIG_PATH_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let path = unique_temp_config_path("doc");
        let prev = std::env::var_os("UNDERSTATUS_CONFIG");
        std::env::set_var("UNDERSTATUS_CONFIG", &path);
        let _ = std::fs::remove_file(&path);

        // (1) 파일 부재에서 1회 write → 재read 시 키 존재.
        edit_config_doc(|table| {
            table.insert(
                "theme".to_string(),
                toml::Value::String("vivid".to_string()),
            );
            set_refresh_interval(table, 5);
            Ok(())
        })
        .expect("디스크 write 성공");
        let raw1 = std::fs::read_to_string(&path).expect("재read 성공");
        let parsed1: toml::Value = toml::from_str(&raw1).expect("재파싱");
        assert_eq!(
            parsed1.get("theme").and_then(toml::Value::as_str),
            Some("vivid")
        );

        // (2) 멱등 재write → 동일 결과.
        edit_config_doc(|table| {
            table.insert(
                "theme".to_string(),
                toml::Value::String("vivid".to_string()),
            );
            set_refresh_interval(table, 5);
            Ok(())
        })
        .expect("재write 성공");
        let raw2 = std::fs::read_to_string(&path).expect("재read 성공");
        assert_eq!(raw1, raw2, "멱등 재write는 동일 결과");

        // (3) 무관 키 보존: theme만 바꿔도 refresh 유지.
        edit_config_doc(|table| {
            table.insert(
                "theme".to_string(),
                toml::Value::String("ember".to_string()),
            );
            Ok(())
        })
        .expect("부분 변경 성공");
        let raw3 = std::fs::read_to_string(&path).expect("재read 성공");
        let parsed3: toml::Value = toml::from_str(&raw3).expect("재파싱");
        assert_eq!(
            parsed3.get("theme").and_then(toml::Value::as_str),
            Some("ember")
        );
        assert_eq!(
            parsed3
                .get("refresh")
                .and_then(|t| t.get("interval_seconds"))
                .and_then(toml::Value::as_integer),
            Some(5),
            "무관 키(refresh) 보존"
        );

        // env 복원 + 임시 파일 정리.
        let _ = std::fs::remove_file(&path);
        match prev {
            Some(value) => std::env::set_var("UNDERSTATUS_CONFIG", value),
            None => std::env::remove_var("UNDERSTATUS_CONFIG"),
        }
    }

    /// set_pulse_style 변환은 [pulse].pulse_style만 교체하고 다른 키를 보존한다(인메모리).
    #[test]
    fn set_pulse_style_replaces_only_pulse_style_key() {
        let existing =
            "theme = \"neon\"\n[pulse]\npulse_period_seconds = 30\n[refresh]\ninterval_seconds = 5\n";
        let serialized = edit_config_doc_str(Some(existing), |table| {
            set_pulse_style_key(table, "hue");
            Ok(())
        })
        .expect("변환 성공");
        let parsed: toml::Value = toml::from_str(&serialized).expect("재파싱 성공");
        assert_eq!(
            parsed
                .get("pulse")
                .and_then(|t| t.get("pulse_style"))
                .and_then(toml::Value::as_str),
            Some("hue")
        );
        assert_eq!(
            parsed
                .get("pulse")
                .and_then(|t| t.get("pulse_period_seconds"))
                .and_then(toml::Value::as_integer),
            Some(30)
        );
        assert_eq!(
            parsed.get("theme").and_then(toml::Value::as_str),
            Some("neon")
        );
        assert_eq!(
            parsed
                .get("refresh")
                .and_then(|t| t.get("interval_seconds"))
                .and_then(toml::Value::as_integer),
            Some(5)
        );
    }

    /// 손상된(비-table) [pulse] 섹션이어도 pulse_style을 확실히 기록한다(부분 기록 방지).
    #[test]
    fn set_pulse_style_key_overwrites_non_table_section() {
        let existing = "pulse = \"corrupted\"\n";
        let serialized = edit_config_doc_str(Some(existing), |table| {
            set_pulse_style_key(table, "flash");
            Ok(())
        })
        .expect("변환 성공");
        let parsed: toml::Value = toml::from_str(&serialized).expect("재파싱 성공");
        assert_eq!(
            parsed
                .get("pulse")
                .and_then(|t| t.get("pulse_style"))
                .and_then(toml::Value::as_str),
            Some("flash")
        );
    }

    /// set_pulse_style은 미지 스타일을 거부하고 에러에 사용 가능 목록을 포함한다.
    #[test]
    fn set_pulse_style_rejects_unknown() {
        let error = set_pulse_style("bogus").expect_err("미지 스타일은 Err");
        let message = format!("{error}");
        assert!(message.contains("bogus"), "에러에 입력 스타일 포함");
        assert!(message.contains("calm"), "에러에 사용 가능 목록 포함");
    }

    /// validate_pulse_style은 4개 출시 스타일을 통과시킨다.
    #[test]
    fn validate_pulse_style_accepts_known() {
        for style in ["calm", "flash", "hue", "swap"] {
            assert!(validate_pulse_style(style).is_ok(), "{style} 통과");
        }
    }

    /// 디스크 read 경로(read_existing_config_str)까지 interval 승계가 동작함을 검증한다.
    #[test]
    fn existing_interval_disk_roundtrip() {
        let _guard = CONFIG_PATH_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let path = unique_temp_config_path("interval");
        let prev = std::env::var_os("UNDERSTATUS_CONFIG");
        std::env::set_var("UNDERSTATUS_CONFIG", &path);

        std::fs::write(&path, "[refresh]\ninterval_seconds = 10\n").expect("fixture write");
        let raw = read_existing_config_str();
        assert_eq!(existing_interval(raw.as_deref()), Some(10));

        // env 복원 + 임시 파일 정리.
        let _ = std::fs::remove_file(&path);
        match prev {
            Some(value) => std::env::set_var("UNDERSTATUS_CONFIG", value),
            None => std::env::remove_var("UNDERSTATUS_CONFIG"),
        }
    }
}
