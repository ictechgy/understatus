//! 라인 조립 + 폭/색상 채널 적응 + self/chain 합성.
//!
//! 계획서 §H-7/AC5 + CALM 디자인을 따른다. understatus 자체 세그먼트를 ANSI 한
//! 줄로 조립한다. `NO_COLOR`/색상 모드/`max_width`/와이드 글리프(2칸)를 존중한다.
//!
//! CALM COLOR-ONCE 규칙: 색은 **부분별로** 입힌다.
//! - load glyph(예 ▄): 밴드 틴트(펄스 ON이면 테라코타 숨쉬기) — **색 있음**.
//! - 세그먼트 VALUE(58%, $1.23, 모델명, 브랜치명, cwd, 속도): **색 없음**(터미널 기본).
//! - 라벨/단위(mem, disk, ctx, ↓ ↑, $, git 마커 ⎇): dim `label_color`.
//! - 구분자(가운뎃점 " · "): 더 어두운 `separator_color`.
//! - self/chain 사이 HUD seam("│"): `separator_color`.
//!
//! BOLD 이스케이프("\x1b[1m")는 일절 쓰지 않는다(불투명도 단계 밝기만 사용).

use crate::claude::ClaudeInput;
use crate::config::Config;
use crate::system::{NetThroughput, SystemSnapshot};
use crate::theme::{band_index, band_tint, parse_hex_pub, pick_emoji, pulse_color, ColorSpec};

/// 세그먼트 한 조각: 폭 계산용 순수 텍스트 + ANSI 적용된 표시 텍스트 + 우선순위.
///
/// COLOR-ONCE 규칙상 한 세그먼트 안에서도 글리프/라벨/값이 서로 다른 색을 가지므로,
/// `colored`에 부분별 ANSI를 미리 합성해 둔다. 폭은 `plain`(이스케이프 없는 가시
/// 텍스트)으로 계산한다. `priority`가 낮을수록 먼저 버려진다(§H-7).
struct Segment {
    /// 폭 계산용 순수 텍스트(ANSI 이스케이프 없음).
    plain: String,
    /// 화면에 출력할, 부분별 ANSI가 적용된 텍스트.
    colored: String,
    /// 폭 초과 시 생략 우선순위(낮을수록 먼저 버림).
    priority: u8,
}

// CONTRACT: signature is frozen — implement body only, do not change this signature
/// understatus 자체 세그먼트(ANSI 한 줄)를 조립한다.
///
/// # 인자
/// - `input`: 파싱된 Claude 세션 정보(모델/비용/컨텍스트/git 등 세그먼트 소스).
/// - `snap`: CPU/메모리/배터리 스냅샷(이모지/펄스/지표 소스).
/// - `cfg`: 표시 토글(`display.*`), 색상 모드(`color.*`), 폭(`display.max_width`).
/// - `now_ms`: 현재 시각(ms). 펄스 위상 계산에 [`crate::theme`]로 전달.
/// - `pulse_on`: [`crate::theme::pulse_gate`]가 산출한 이번 프레임 펄스 on 여부.
///
/// # 반환
/// understatus 자체 세그먼트 문자열(개행 없음). `NO_COLOR` 설정 시 색상 제거,
/// `max_width` 초과 시 저우선 세그먼트 생략/축약, 각 정보원 부재 시 해당 세그먼트 생략(AC5).
pub fn render(
    input: &ClaudeInput,
    snap: &SystemSnapshot,
    cfg: &Config,
    now_ms: u128,
    pulse_on: bool,
) -> String {
    // 색상 활성 여부: NO_COLOR(존재만으로 비활성) 또는 mode "none"이면 ANSI 일절 없음.
    let color_on = std::env::var_os("NO_COLOR").is_none() && cfg.color.mode != "none";

    // 1) 표시할 세그먼트들을 (plain/colored/priority)와 함께 수집.
    let segments = collect_segments(input, snap, cfg, now_ms, pulse_on, color_on);

    // 2) max_width를 넘으면 저우선 세그먼트부터 제거(폭은 plain 기준 + 구분자).
    let kept = enforce_width(segments, cfg.display.max_width, &cfg.color.separator);

    // 3) dim 가운뎃점 구분자로 합친다(색상 on이면 separator_color 적용).
    let separator = render_separator(cfg, color_on);
    kept.iter()
        .map(|segment| segment.colored.as_str())
        .collect::<Vec<_>>()
        .join(&separator)
}

/// 표시 토글/스냅샷/세션 정보로 세그먼트 목록을 만든다(부분별 ANSI 적용).
///
/// 각 정보원이 부재(`None`)면 해당 세그먼트를 생략한다(AC5 우아한 저하).
/// COLOR-ONCE: 값엔 색 없음, 라벨/단위/마커엔 dim `label_color`, 글리프엔 밴드 틴트.
fn collect_segments(
    input: &ClaudeInput,
    snap: &SystemSnapshot,
    cfg: &Config,
    now_ms: u128,
    pulse_on: bool,
    color_on: bool,
) -> Vec<Segment> {
    let mut segments = Vec::new();

    // load glyph + CPU%: 가장 높은 우선순위(핵심, 마지막까지 유지).
    // 글리프는 밴드 틴트(펄스 ON이면 테라코타 숨쉬기), CPU% 값엔 색 없음.
    let glyph = pick_emoji(snap.cpu_percent, now_ms, pulse_on, cfg);
    let glyph_color = glyph_tint(snap.cpu_percent, now_ms, pulse_on, cfg);
    let cpu_value = format!("{:.0}%", snap.cpu_percent);
    segments.push(Segment {
        plain: format!("{glyph} {cpu_value}"),
        colored: format!(
            "{} {}",
            tinted(&glyph, glyph_color, cfg, color_on),
            cpu_value
        ),
        priority: 100,
    });

    // 메모리%: 라벨 "mem" dim + 값 색 없음.
    segments.push(label_value_segment(
        "mem",
        &format!("{:.0}%", snap.mem_percent),
        90,
        cfg,
        color_on,
    ));

    // 배터리(P2): 토글 on + 값 있을 때만. 라벨 마커 dim + 값 색 없음.
    if cfg.display.show_battery {
        if let Some(battery) = snap.battery.as_ref() {
            segments.push(label_value_segment(
                battery_marker(battery.percent, battery.is_charging),
                &format!("{:.0}%", battery.percent),
                85,
                cfg,
                color_on,
            ));
        }
    }

    // 디스크(P2): 토글 on + 값 있을 때만. 라벨 "disk" dim + 값 색 없음.
    if cfg.display.show_disk {
        if let Some(disk) = snap.disk_percent {
            segments.push(label_value_segment(
                "disk",
                &format!("{disk:.0}%"),
                80,
                cfg,
                color_on,
            ));
        }
    }

    // 네트워크(P2): 토글 on + 값 있을 때만. ↓ ↑ 화살표 dim + 속도 값 색 없음.
    if cfg.display.show_network {
        if let Some(net) = snap.net.as_ref() {
            segments.push(net_segment(net, 75, cfg, color_on));
        }
    }

    // 모델 표시명: 값(색 없음).
    if cfg.display.show_model {
        if let Some(model) = input.model_display_name.as_deref() {
            if !model.is_empty() {
                segments.push(value_segment(model, 60));
            }
        }
    }

    // 컨텍스트 사용률%: null이면 세그먼트 생략(AC2). 라벨 "ctx" dim + 값 색 없음.
    if cfg.display.show_context {
        if let Some(context) = input.context_used_percentage {
            segments.push(label_value_segment(
                "ctx",
                &format!("{context:.0}%"),
                50,
                cfg,
                color_on,
            ));
        }
    }

    // Codex 한도/요금제/effort(lterm+codex 소스 전용, spec §6). input.codex가 Some일 때만,
    // 그 안의 각 Option이 Some일 때만 추가한다(기존 None-skip 패턴 동형). Claude 경로는
    // codex=None이라 신규 세그먼트 0 → 기존 출력/바이트 보존.
    if let Some(codex) = input.codex.as_ref() {
        // 5h 한도%(priority 48): 라벨 "5h" dim + 값 색 없음.
        if let Some(rate_5h) = codex.rate_5h_percent {
            segments.push(label_value_segment(
                "5h",
                &format!("{rate_5h:.0}%"),
                48,
                cfg,
                color_on,
            ));
        }
        // 주간 한도%(priority 46): 라벨 "wk" dim + 값 색 없음.
        if let Some(rate_weekly) = codex.rate_weekly_percent {
            segments.push(label_value_segment(
                "wk",
                &format!("{rate_weekly:.0}%"),
                46,
                cfg,
                color_on,
            ));
        }
        // plan(priority 26): bare value(라벨 없음, 예 "pro").
        if let Some(plan) = codex.plan.as_deref() {
            if !plan.is_empty() {
                segments.push(value_segment(plan, 26));
            }
        }
        // effort(priority 24): bare value(라벨 없음, 예 "xhigh").
        if let Some(effort) = codex.effort.as_deref() {
            if !effort.is_empty() {
                segments.push(value_segment(effort, 24));
            }
        }
    }

    // cwd/git 브랜치: git_branch 우선. git 마커 ⎇ dim + 브랜치명 값 색 없음.
    if cfg.display.show_git {
        if let Some(branch) = input.git_branch.as_deref() {
            if !branch.is_empty() {
                segments.push(label_value_segment("⎇", branch, 40, cfg, color_on));
            }
        }
    }
    if let Some(cwd) = input.cwd.as_deref() {
        if let Some(dir) = cwd.rsplit('/').find(|part| !part.is_empty()) {
            segments.push(value_segment(dir, 30));
        }
    }

    // 누적 비용 USD: 가장 낮은 우선순위(먼저 버림). $ 마커 dim + 금액 값 색 없음.
    if cfg.display.show_cost {
        if let Some(cost) = input.cost_usd {
            segments.push(label_value_segment(
                "$",
                &format!("{cost:.2}"),
                20,
                cfg,
                color_on,
            ));
        }
    }

    segments
}

/// 글리프에 입힐 틴트(펄스 ON이면 테라코타 숨쉬기, OFF면 정적 밴드 틴트)를 고른다.
///
/// COLOR-ONCE: 이 틴트는 글리프 문자에만 적용되고 값/라벨엔 적용되지 않는다.
fn glyph_tint(cpu_percent: f64, now_ms: u128, pulse_on: bool, cfg: &Config) -> ColorSpec {
    pulse_color(cpu_percent, now_ms, pulse_on, cfg)
        .unwrap_or_else(|| band_tint(band_index(cpu_percent, cfg), cfg))
}

/// "라벨 값" 형태의 세그먼트를 만든다(라벨 dim, 값 색 없음).
///
/// 라벨(단위/마커)은 `label_color`로 dim 처리하고, 값은 ANSI 없이 그대로 둔다.
fn label_value_segment(
    label: &str,
    value: &str,
    priority: u8,
    cfg: &Config,
    color_on: bool,
) -> Segment {
    Segment {
        plain: format!("{label} {value}"),
        colored: format!("{} {value}", dim_label(label, cfg, color_on)),
        priority,
    }
}

/// 값만 있는 세그먼트(모델명/cwd 등)를 만든다(색 없음, 터미널 기본 밝기).
fn value_segment(value: &str, priority: u8) -> Segment {
    Segment {
        plain: value.to_string(),
        colored: value.to_string(),
        priority,
    }
}

/// 네트워크 throughput 세그먼트를 만든다(↓ ↑ 화살표 dim + 속도 값 색 없음).
fn net_segment(net: &NetThroughput, priority: u8, cfg: &Config, color_on: bool) -> Segment {
    let rx = format_bytes_per_sec(net.rx_bps);
    let tx = format_bytes_per_sec(net.tx_bps);
    Segment {
        plain: format!("↓{rx} ↑{tx}"),
        colored: format!(
            "{}{rx} {}{tx}",
            dim_label("↓", cfg, color_on),
            dim_label("↑", cfg, color_on)
        ),
        priority,
    }
}

/// 배터리 잔량/충전 상태에 맞는 단일 셀 마커를 고른다(CALM, dim 라벨로 렌더).
///
/// 우선순위: 충전 중 → `bat+`, 비충전 & 잔량 ≤ 20% → `bat!`(저잔량 경고),
/// 그 외(비충전 & 충분) → `bat`. 마커는 라벨로 취급되어 `label_color`로 dim 처리된다.
///
/// # 인자
/// - `percent`: 배터리 잔량(0–100).
/// - `is_charging`: 충전/AC 연결 여부.
///
/// # 반환
/// 표시할 배터리 마커 문자열(차분한 텍스트, 이모지 아님).
fn battery_marker(percent: f64, is_charging: bool) -> &'static str {
    if is_charging {
        "bat+" // 충전 중.
    } else if percent <= 20.0 {
        "bat!" // 저잔량 경고(비충전).
    } else {
        "bat" // 배터리 구동 중(충분).
    }
}

/// 초당 바이트(rate)를 B/KB/MB 단위의 사람 친화 문자열로 환산한다(1024 기준).
///
/// # 인자
/// - `bps`: 초당 바이트.
///
/// # 반환
/// 예: `512B/s`, `12KB/s`, `3.4MB/s`. 음수/NaN은 `0B/s`로 안전 저하한다.
fn format_bytes_per_sec(bps: f64) -> String {
    if !bps.is_finite() || bps < 0.0 {
        return "0B/s".to_string();
    }
    const KB: f64 = 1024.0;
    const MB: f64 = 1024.0 * 1024.0;
    if bps < KB {
        format!("{:.0}B/s", bps)
    } else if bps < MB {
        format!("{:.0}KB/s", bps / KB)
    } else {
        format!("{:.1}MB/s", bps / MB)
    }
}

/// `max_width`를 넘으면 저우선 세그먼트부터 제거한다(와이드 글리프 2칸 계산).
///
/// 표시 순서는 보존하되, 폭 초과 시 `priority`가 가장 낮은 세그먼트를 하나씩 버린다.
/// 폭은 가시 텍스트(`plain`)와 구분자(`separator`) 표시 폭으로 계산한다(ANSI 제외).
/// 단일 핵심 세그먼트 하나는 항상 남긴다(빈 줄 방지).
fn enforce_width(mut segments: Vec<Segment>, max_width: usize, separator: &str) -> Vec<Segment> {
    let sep_width = display_width(separator);
    while segments.len() > 1 && composed_width(&segments, sep_width) > max_width {
        // 가장 낮은 priority(동률이면 뒤쪽) 세그먼트를 제거.
        let drop_index = segments
            .iter()
            .enumerate()
            .min_by_key(|(_, segment)| segment.priority)
            .map(|(index, _)| index)
            .unwrap_or(segments.len() - 1);
        segments.remove(drop_index);
    }
    segments
}

/// 세그먼트들을 구분자로 합쳤을 때의 표시 폭(와이드 문자 2칸)을 계산한다(ANSI 제외).
fn composed_width(segments: &[Segment], sep_width: usize) -> usize {
    let text_width: usize = segments
        .iter()
        .map(|segment| display_width(&segment.plain))
        .sum();
    // 세그먼트 사이 구분자 폭만큼.
    let separators = segments.len().saturating_sub(1);
    text_width + separators * sep_width
}

/// 문자열의 터미널 표시 폭을 계산한다(와이드/이모지 = 2칸, 그 외 1칸).
///
/// 정밀한 East Asian Width 테이블 대신, 이모지/넓은 문자 범위를 휴리스틱으로
/// 폭 2칸 처리한다(§H-7 — 와이드 이모지 2칸 계산 요구).
fn display_width(text: &str) -> usize {
    text.chars().map(char_width).sum()
}

/// 단일 문자의 표시 폭(1 또는 2)을 휴리스틱으로 판정한다.
///
/// 결합 문자(zero-width)는 0, 이모지/CJK 등 와이드 문자는 2, 나머지는 1.
fn char_width(c: char) -> usize {
    let code = c as u32;
    // Variation Selector / Zero-Width Joiner 등 결합 문자는 폭 0.
    if c == '\u{200d}' || (0xFE00..=0xFE0F).contains(&code) {
        return 0;
    }
    if is_wide(code) {
        2
    } else {
        1
    }
}

/// 코드포인트가 폭 2칸(와이드/이모지)에 해당하는지 휴리스틱으로 판정한다.
fn is_wide(code: u32) -> bool {
    matches!(code,
        0x1100..=0x115F        // Hangul Jamo
        | 0x2600..=0x27BF      // Misc symbols / Dingbats (다수 이모지)
        | 0x2E80..=0x303E      // CJK Radicals ~ punctuation
        | 0x3041..=0x33FF      // Hiragana ~ CJK compat
        | 0x3400..=0x4DBF      // CJK Ext A
        | 0x4E00..=0x9FFF      // CJK Unified
        | 0xA000..=0xA4CF      // Yi
        | 0xAC00..=0xD7A3      // Hangul Syllables
        | 0xF900..=0xFAFF      // CJK Compat Ideographs
        | 0xFE30..=0xFE4F      // CJK Compat Forms
        | 0xFF00..=0xFF60      // Fullwidth Forms
        | 0xFFE0..=0xFFE6
        | 0x1F300..=0x1FAFF    // 이모지 평면(😌🙂😅🥵🔥 포함)
        | 0x20000..=0x3FFFD    // CJK Ext B+
    )
}

/// 글리프 문자에 밴드 틴트를 입힌다(COLOR-ONCE: 글리프에만, 값엔 안 입힘).
///
/// `color_on`이 false면 ANSI 없이 글리프 텍스트만 반환한다. 색상 모드 auto/
/// truecolor/256에 따라 ANSI 시퀀스 형식을 고르고 항상 리셋으로 닫는다(§H-7).
/// BOLD("\x1b[1m")는 절대 쓰지 않는다.
fn tinted(glyph: &str, color: ColorSpec, cfg: &Config, color_on: bool) -> String {
    if !color_on {
        return glyph.to_string();
    }
    format!("{}{glyph}\x1b[0m", ansi_fg(color, cfg))
}

/// 라벨/단위/마커를 dim `label_color`로 칠한다(값엔 적용하지 않는다).
///
/// `color_on`이 false면 ANSI 없이 라벨 텍스트만 반환한다.
fn dim_label(label: &str, cfg: &Config, color_on: bool) -> String {
    if !color_on {
        return label.to_string();
    }
    let color = parse_hex_pub(&cfg.color.label_color).unwrap_or(ColorSpec {
        r: 0x6b,
        g: 0x72,
        b: 0x80,
    });
    format!("{}{label}\x1b[0m", ansi_fg(color, cfg))
}

/// 세그먼트 사이에 들어갈 구분자를 만든다(dim 가운뎃점 " · ").
///
/// `color_on`이 false면 ANSI 없이 구분자 텍스트만 반환한다. 색상 on이면 양옆 공백은
/// 보존하고 가운뎃점 글리프에만 `separator_color`를 입힌다(공백 밝기 유지 불필요).
fn render_separator(cfg: &Config, color_on: bool) -> String {
    let separator = &cfg.color.separator;
    if !color_on {
        return separator.clone();
    }
    let color = separator_spec(cfg);
    format!("{}{separator}\x1b[0m", ansi_fg(color, cfg))
}

/// `separator_color`를 [`ColorSpec`]으로 파싱한다(기본 #3b4048으로 안전 저하).
fn separator_spec(cfg: &Config) -> ColorSpec {
    parse_hex_pub(&cfg.color.separator_color).unwrap_or(ColorSpec {
        r: 0x3b,
        g: 0x40,
        b: 0x48,
    })
}

/// 색상 모드에 맞는 전경색 ANSI 시퀀스를 만든다(truecolor 또는 256색).
///
/// - `truecolor`: `\x1b[38;2;R;G;Bm`.
/// - `256`: 가장 가까운 xterm-256 코드로 `\x1b[38;5;Nm`.
/// - `auto`: `COLORTERM=truecolor`면 truecolor, 아니면 256(§H-7).
fn ansi_fg(color: ColorSpec, cfg: &Config) -> String {
    if use_truecolor(cfg) {
        format!("\x1b[38;2;{};{};{}m", color.r, color.g, color.b)
    } else {
        format!("\x1b[38;5;{}m", nearest_xterm256(color))
    }
}

/// 현재 색상 모드/환경에서 truecolor를 써야 하는지 판정한다.
fn use_truecolor(cfg: &Config) -> bool {
    match cfg.color.mode.as_str() {
        "truecolor" => true,
        "256" => false,
        // auto: COLORTERM=truecolor 또는 24bit일 때만 truecolor.
        _ => std::env::var("COLORTERM")
            .map(|value| value == "truecolor" || value == "24bit")
            .unwrap_or(false),
    }
}

/// RGB를 가장 가까운 xterm-256 색 코드로 근사한다(6×6×6 컬러 큐브 + 그레이스케일).
fn nearest_xterm256(color: ColorSpec) -> u8 {
    // 6단계 큐브로 각 채널을 양자화(0,95,135,175,215,255).
    let cube = |value: u8| -> (u8, u8) {
        let steps = [0u8, 95, 135, 175, 215, 255];
        let mut best_index = 0usize;
        let mut best_distance = u16::MAX;
        for (index, &step) in steps.iter().enumerate() {
            let distance = (step as i16 - value as i16).unsigned_abs();
            if distance < best_distance {
                best_distance = distance;
                best_index = index;
            }
        }
        (best_index as u8, steps[best_index])
    };

    let (ri, _) = cube(color.r);
    let (gi, _) = cube(color.g);
    let (bi, _) = cube(color.b);
    16 + 36 * ri + 6 * gi + bi
}

// CONTRACT: signature is frozen — implement body only, do not change this signature
/// understatus 자체 세그먼트와 체인 자식 출력을 설정된 순서로 합성한다.
///
/// # 인자
/// - `self_segment`: [`render`]가 만든 understatus 자체 세그먼트.
/// - `chain_output`: 체인 자식 stdout(없으면 빈 문자열).
/// - `order`: `"self_first"` | `"chain_first"`(기본 self_first).
///
/// # 반환
/// 합성된 한 줄(개행 없음). 체인 출력이 비었으면 자체 세그먼트만 반환한다(AC8).
/// HUD seam이 필요하면 [`compose_with_seam`]을 사용한다(이 함수는 공백 1칸 합성).
///
/// 시그니처는 계약상 고정이라 보존한다. 런타임 경로는 [`compose_with_seam`]을 쓰지만
/// 공개 API/테스트가 이 함수를 참조하므로 dead_code 경고를 명시적으로 허용한다.
#[cfg_attr(not(test), allow(dead_code))]
pub fn compose(self_segment: &str, chain_output: &str, order: &str) -> String {
    compose_internal(self_segment, chain_output, order, " ")
}

/// [`compose`]에 dim HUD seam("│")을 끼워 소유권 경계를 표시한 변형이다.
///
/// # 인자
/// - `self_segment`/`chain_output`/`order`: [`compose`]와 동일.
/// - `cfg`: `color.hud_seam`(기본 "│") + `color.separator_color`(seam 색).
/// - `color_on`: 색상 활성 여부(NO_COLOR/mode none이면 false → ANSI 없음).
///
/// # 반환
/// `self … │ … chain`(order=chain_first면 `chain … │ … self`). 체인 출력이 비면
/// seam 없이 자체 세그먼트만 반환한다(후행 seam 금지, AC8).
pub fn compose_with_seam(
    self_segment: &str,
    chain_output: &str,
    order: &str,
    cfg: &Config,
    color_on: bool,
) -> String {
    // 체인 출력이 비면 seam 없이 자체 세그먼트만(후행 seam 금지).
    if chain_output.trim_end_matches(['\n', '\r']).is_empty() {
        return self_segment.to_string();
    }
    let seam = render_seam(cfg, color_on);
    let joiner = format!(" {seam} ");
    compose_internal(self_segment, chain_output, order, &joiner)
}

/// self/chain을 주어진 joiner로 합성하는 공통 내부 헬퍼.
///
/// 체인 출력 끝 개행은 한 줄 합성을 위해 제거하고, 비면 자체 세그먼트만 반환한다.
fn compose_internal(self_segment: &str, chain_output: &str, order: &str, joiner: &str) -> String {
    let chain = chain_output.trim_end_matches(['\n', '\r']);
    if chain.is_empty() {
        return self_segment.to_string();
    }
    match order {
        "chain_first" => format!("{chain}{joiner}{self_segment}"),
        // 기본 self_first.
        _ => format!("{self_segment}{joiner}{chain}"),
    }
}

/// HUD seam 글리프("│")를 dim `separator_color`로 렌더한다(색상 off면 글리프만).
fn render_seam(cfg: &Config, color_on: bool) -> String {
    let seam = &cfg.color.hud_seam;
    if !color_on {
        return seam.clone();
    }
    format!("{}{seam}\x1b[0m", ansi_fg(separator_spec(cfg), cfg))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::claude::ClaudeInput;
    use crate::config::Config;
    use crate::system::{BatteryInfo, NetThroughput, SystemSnapshot};

    /// 테스트용 고정 Claude 세션 정보(모델/컨텍스트/git/cwd/cost).
    fn sample_input() -> ClaudeInput {
        ClaudeInput {
            model_display_name: Some("Opus".to_string()),
            context_used_percentage: Some(42.0),
            cwd: Some("/Users/dev/proj".to_string()),
            git_branch: Some("main".to_string()),
            cost_usd: Some(1.23),
            session_id: Some("sess-1".to_string()),
            codex: None,
        }
    }

    /// 테스트용 고정 시스템 스냅샷(P2 지표는 기본 None).
    fn sample_snap(cpu: f64) -> SystemSnapshot {
        SystemSnapshot {
            cpu_percent: cpu,
            mem_percent: 55.0,
            battery: None,
            disk_percent: None,
            net: None,
        }
    }

    /// 환경변수(NO_COLOR/COLORTERM)를 만지는 테스트들의 직렬화를 위한 프로세스 전역 락.
    ///
    /// `std::env::set_var`는 프로세스 전역 상태라 병렬 테스트에서 경합할 수 있으므로
    /// env를 변경하는 테스트는 이 락을 잡아 직렬 실행한다.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn render_no_color_env_has_no_escape_bytes() {
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        // NO_COLOR이 설정되면 truecolor 모드라도 ANSI를 일절 출력하지 않아야 한다.
        let mut cfg = Config::default();
        cfg.color.mode = "truecolor".to_string();
        // SAFETY: ENV_LOCK으로 직렬화된 단일 스레드 구간에서만 env를 변경한다.
        unsafe { std::env::set_var("NO_COLOR", "1") };
        let line = render(&sample_input(), &sample_snap(95.0), &cfg, 1_000, true);
        unsafe { std::env::remove_var("NO_COLOR") };
        assert!(
            !line.contains('\x1b'),
            "NO_COLOR 설정 시 ANSI ESC 바이트가 없어야 함: {line:?}"
        );
        assert!(line.contains("95%"));
    }

    #[test]
    fn render_no_color_mode_has_no_escape_bytes() {
        let mut cfg = Config::default();
        cfg.color.mode = "none".to_string();
        let line = render(&sample_input(), &sample_snap(95.0), &cfg, 1_000, true);
        assert!(
            !line.contains('\x1b'),
            "mode=none이면 ANSI ESC 바이트가 없어야 함: {line:?}"
        );
        // 핵심 텍스트는 그대로 존재.
        assert!(line.contains("95%"));
    }

    #[test]
    fn render_has_no_bold_escape() {
        // CALM: understatus 자체 세그먼트에 BOLD("\x1b[1m")가 절대 없어야 한다.
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        unsafe { std::env::remove_var("NO_COLOR") };
        let mut cfg = Config::default();
        cfg.color.mode = "truecolor".to_string();
        let mut snap = sample_snap(95.0);
        snap.battery = Some(BatteryInfo {
            percent: 80.0,
            is_charging: false,
        });
        snap.disk_percent = Some(63.0);
        snap.net = Some(NetThroughput {
            rx_bps: 2048.0,
            tx_bps: 512.0,
        });
        // 펄스 ON/OFF 양쪽 모두 BOLD가 없어야 한다.
        for pulse_on in [false, true] {
            let line = render(&sample_input(), &snap, &cfg, 1_000, pulse_on);
            assert!(
                !line.contains("\x1b[1m"),
                "BOLD 이스케이프(\\x1b[1m)가 없어야 함(pulse_on={pulse_on}): {line:?}"
            );
        }
    }

    #[test]
    fn render_truecolor_has_escape_bytes() {
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        // NO_COLOR이 다른 테스트에서 새지 않도록 보장.
        unsafe { std::env::remove_var("NO_COLOR") };
        let mut cfg = Config::default();
        cfg.color.mode = "truecolor".to_string();
        let line = render(&sample_input(), &sample_snap(95.0), &cfg, 1_000, true);
        assert!(
            line.contains('\x1b'),
            "truecolor 모드는 ANSI ESC 바이트를 포함해야 함: {line:?}"
        );
        // truecolor 시퀀스 형식 확인.
        assert!(
            line.contains("\x1b[38;2;"),
            "truecolor 38;2 시퀀스 필요: {line:?}"
        );
        // CALM: 색은 부분별로 입히므로 각 색 조각은 리셋으로 닫힌다(라인 끝은
        // 색 없는 값일 수 있어 더 이상 \x1b[0m으로 끝나지 않는다 — COLOR-ONCE).
        assert!(
            line.contains("\x1b[0m"),
            "색을 입힌 조각은 리셋으로 닫혀야 함: {line:?}"
        );
        // 마지막 가시 텍스트는 색 없는 값($1.23)이라 ESC로 끝나지 않아야 한다.
        assert!(
            line.ends_with("1.23"),
            "마지막 값은 색 없이 출력되어야 함: {line:?}"
        );
    }

    #[test]
    fn render_color_once_value_has_no_escape() {
        // COLOR-ONCE: 글리프엔 틴트가 붙지만 CPU% 값엔 ANSI가 붙지 않는다.
        // 밴드2(58%) truecolor 출력에서 "▄"는 색이 붙고 "58%"는 색이 안 붙어야 한다.
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        unsafe { std::env::remove_var("NO_COLOR") };
        let mut cfg = Config::default();
        cfg.color.mode = "truecolor".to_string();
        // 글리프 틴트는 밴드2(#86a0b4) truecolor 시퀀스로 시작해야 한다.
        let line = render(&sample_input(), &sample_snap(58.0), &cfg, 0, false);
        let expected_glyph = "\x1b[38;2;134;160;180m▄\x1b[0m 58%";
        assert!(
            line.starts_with(expected_glyph),
            "글리프엔 틴트, 값(58%)엔 색 없음: {line:?}"
        );
        // 값 바로 앞뒤에 색 escape가 끼지 않음(글리프 리셋 직후 ' 58%').
        assert!(
            line.contains("\x1b[0m 58%"),
            "CPU% 값은 리셋 직후 색 없이 출력: {line:?}"
        );
    }

    #[test]
    fn render_uses_dim_middot_separator() {
        // 세그먼트 구분자가 dim 가운뎃점 " · "(separator_color)여야 한다.
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        unsafe { std::env::remove_var("NO_COLOR") };
        let mut cfg = Config::default();
        cfg.color.mode = "truecolor".to_string();
        let line = render(&sample_input(), &sample_snap(10.0), &cfg, 0, false);
        // separator_color(#3b4048 = 59,64,72) + 가운뎃점.
        assert!(
            line.contains("\x1b[38;2;59;64;72m · \x1b[0m"),
            "dim 가운뎃점 구분자 필요: {line:?}"
        );
        // 옛 공백 단독 구분자(이모지 시절)는 더 이상 쓰지 않는다.
        assert!(
            !line.contains("% mem"),
            "구분자는 가운뎃점이어야 함: {line:?}"
        );
    }

    #[test]
    fn render_labels_are_dim_values_are_not() {
        // 라벨(mem)엔 label_color(#6b7280 = 107,114,128) dim, 값(55%)엔 색 없음.
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        unsafe { std::env::remove_var("NO_COLOR") };
        let mut cfg = Config::default();
        cfg.color.mode = "truecolor".to_string();
        let line = render(&sample_input(), &sample_snap(10.0), &cfg, 0, false);
        assert!(
            line.contains("\x1b[38;2;107;114;128mmem\x1b[0m 55%"),
            "라벨 dim + 값 색 없음: {line:?}"
        );
    }

    #[test]
    fn render_band_snapshots_glyph_and_tint() {
        // 5개 밴드 모두 글리프+틴트가 결정적으로 드러나도록 truecolor 스냅샷 검증(라이브 CPU 무관).
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        unsafe { std::env::remove_var("NO_COLOR") };
        let mut cfg = Config::default();
        cfg.color.mode = "truecolor".to_string();
        // (cpu%, 기대 글리프, 기대 밴드 틴트 truecolor 프리픽스).
        let cases = [
            (10.0, "○", "\x1b[38;2;90;104;120m"),  // idle  #5a6878
            (30.0, "▁", "\x1b[38;2;109;130;150m"), // low   #6d8296
            (58.0, "▄", "\x1b[38;2;134;160;180m"), // mid   #86a0b4
            (80.0, "▆", "\x1b[38;2;159;191;206m"), // high  #9fbfce
            (95.0, "◆", "\x1b[38;2;184;120;72m"),  // crit  #b87848 (warm)
        ];
        for (cpu, glyph, tint_prefix) in cases {
            // 펄스 OFF로 정적 밴드 틴트를 확인(crit도 pulse_on=false면 정적 틴트).
            let line = render(&sample_input(), &sample_snap(cpu), &cfg, 0, false);
            let expected = format!("{tint_prefix}{glyph}\x1b[0m {cpu:.0}%");
            assert!(
                line.starts_with(&expected),
                "밴드 {cpu}%: 글리프 {glyph} + 틴트 {tint_prefix:?} 필요\n  got: {line:?}"
            );
        }
    }

    #[test]
    fn render_crit_pulse_breathes_terracotta() {
        // crit 밴드 + pulse_on이면 글리프 틴트가 테라코타 숨쉬기로 칠해진다(글리프 모양 고정 ◆).
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        unsafe { std::env::remove_var("NO_COLOR") };
        let mut cfg = Config::default();
        cfg.color.mode = "truecolor".to_string();
        // wave=0(phase=0.75, 22500ms) → high 테라코타 #b87848.
        let high = render(&sample_input(), &sample_snap(95.0), &cfg, 22_500, true);
        assert!(
            high.starts_with("\x1b[38;2;184;120;72m◆\x1b[0m 95%"),
            "펄스 high 끝점 테라코타 + 고정 글리프 ◆: {high:?}"
        );
        // wave=1(phase=0.25, 7500ms) → low dim 테라코타 #7a5030.
        let low = render(&sample_input(), &sample_snap(95.0), &cfg, 7_500, true);
        assert!(
            low.starts_with("\x1b[38;2;122;80;48m◆\x1b[0m 95%"),
            "펄스 low 끝점 dim 테라코타 + 고정 글리프 ◆: {low:?}"
        );
    }

    #[test]
    fn render_omits_context_when_null() {
        let mut input = sample_input();
        input.context_used_percentage = None;
        let mut cfg = Config::default();
        cfg.color.mode = "none".to_string();
        let line = render(&input, &sample_snap(10.0), &cfg, 0, false);
        assert!(
            !line.contains("ctx"),
            "context null이면 ctx 세그먼트 생략: {line:?}"
        );
    }

    #[test]
    fn render_enforces_max_width_drops_low_priority() {
        let mut cfg = Config::default();
        cfg.color.mode = "none".to_string();
        cfg.display.max_width = 14; // 매우 좁게 → 저우선(cost 등) 제거
        let line = render(&sample_input(), &sample_snap(10.0), &cfg, 0, false);
        // 최저 우선순위인 cost($)가 가장 먼저 버려진다.
        assert!(
            !line.contains('$'),
            "max_width 초과 시 cost 세그먼트 제거: {line:?}"
        );
        // 핵심 CPU 세그먼트는 유지.
        assert!(line.contains("10%"), "핵심 CPU 세그먼트는 유지: {line:?}");
    }

    #[test]
    fn compose_self_first() {
        assert_eq!(compose("SELF", "CHAIN", "self_first"), "SELF CHAIN");
    }

    #[test]
    fn compose_chain_first() {
        assert_eq!(compose("SELF", "CHAIN", "chain_first"), "CHAIN SELF");
    }

    #[test]
    fn compose_empty_chain_returns_self_only() {
        assert_eq!(compose("SELF", "", "self_first"), "SELF");
        assert_eq!(compose("SELF", "\n", "self_first"), "SELF");
    }

    #[test]
    fn compose_with_seam_inserts_dim_seam() {
        // 체인 있음 + 색상 off → plain "│" seam이 양옆 공백과 함께 들어간다(self_first).
        let cfg = Config::default();
        assert_eq!(
            compose_with_seam("SELF", "CHAIN", "self_first", &cfg, false),
            "SELF │ CHAIN"
        );
        // chain_first면 순서가 뒤바뀐다.
        assert_eq!(
            compose_with_seam("SELF", "CHAIN", "chain_first", &cfg, false),
            "CHAIN │ SELF"
        );
    }

    #[test]
    fn compose_with_seam_colors_seam_when_color_on() {
        // 색상 on이면 seam("│")이 separator_color(#3b4048 = 59,64,72)로 칠해진다.
        let mut cfg = Config::default();
        cfg.color.mode = "truecolor".to_string();
        let line = compose_with_seam("SELF", "CHAIN", "self_first", &cfg, true);
        assert!(
            line.contains("\x1b[38;2;59;64;72m│\x1b[0m"),
            "dim seam 필요: {line:?}"
        );
    }

    #[test]
    fn compose_with_seam_no_trailing_seam_when_empty_chain() {
        // 체인 출력이 비면 seam 없이 자체 세그먼트만(후행 seam 금지, AC8).
        let cfg = Config::default();
        assert_eq!(
            compose_with_seam("SELF", "", "self_first", &cfg, true),
            "SELF"
        );
        assert_eq!(
            compose_with_seam("SELF", "\n", "self_first", &cfg, true),
            "SELF"
        );
    }

    #[test]
    fn battery_marker_states() {
        // 충전 중 → bat+(잔량 무관).
        assert_eq!(battery_marker(15.0, true), "bat+");
        assert_eq!(battery_marker(95.0, true), "bat+");
        // 비충전 + 저잔량(≤20%) → bat!.
        assert_eq!(battery_marker(20.0, false), "bat!");
        assert_eq!(battery_marker(5.0, false), "bat!");
        // 비충전 + 충분 → bat.
        assert_eq!(battery_marker(21.0, false), "bat");
        assert_eq!(battery_marker(80.0, false), "bat");
    }

    #[test]
    fn format_bytes_per_sec_units() {
        // 1024 미만 → B/s, 1024~1MB → KB/s, ≥1MB → MB/s.
        assert_eq!(format_bytes_per_sec(512.0), "512B/s");
        assert_eq!(format_bytes_per_sec(2048.0), "2KB/s");
        assert_eq!(format_bytes_per_sec(3.0 * 1024.0 * 1024.0), "3.0MB/s");
        // 음수/NaN은 0B/s로 안전 저하.
        assert_eq!(format_bytes_per_sec(-1.0), "0B/s");
        assert_eq!(format_bytes_per_sec(f64::NAN), "0B/s");
    }

    #[test]
    fn render_shows_battery_when_some_and_toggle_on() {
        let mut cfg = Config::default();
        cfg.color.mode = "none".to_string();
        let mut snap = sample_snap(10.0);
        snap.battery = Some(BatteryInfo {
            percent: 75.0,
            is_charging: true,
        });
        let line = render(&sample_input(), &snap, &cfg, 0, false);
        assert!(line.contains("bat+"), "충전 중 배터리 마커 표시: {line:?}");
        assert!(line.contains("75%"), "배터리 잔량 표시: {line:?}");
    }

    #[test]
    fn render_omits_battery_when_none() {
        let mut cfg = Config::default();
        cfg.color.mode = "none".to_string();
        // battery=None(데스크톱) → 세그먼트 없음.
        let line = render(&sample_input(), &sample_snap(10.0), &cfg, 0, false);
        assert!(!line.contains("bat"), "배터리 None이면 생략: {line:?}");
    }

    #[test]
    fn render_omits_battery_when_toggle_off() {
        let mut cfg = Config::default();
        cfg.color.mode = "none".to_string();
        cfg.display.show_battery = false;
        let mut snap = sample_snap(10.0);
        snap.battery = Some(BatteryInfo {
            percent: 50.0,
            is_charging: false,
        });
        let line = render(&sample_input(), &snap, &cfg, 0, false);
        assert!(
            !line.contains("bat"),
            "show_battery=false면 값이 있어도 생략: {line:?}"
        );
    }

    #[test]
    fn render_shows_disk_when_some_and_toggle_on() {
        let mut cfg = Config::default();
        cfg.color.mode = "none".to_string();
        let mut snap = sample_snap(10.0);
        snap.disk_percent = Some(63.0);
        let line = render(&sample_input(), &snap, &cfg, 0, false);
        assert!(line.contains("disk"), "디스크 라벨 표시: {line:?}");
        assert!(line.contains("63%"), "디스크 사용률 표시: {line:?}");
    }

    #[test]
    fn render_omits_disk_when_none_or_toggle_off() {
        let mut cfg = Config::default();
        cfg.color.mode = "none".to_string();
        // None → 생략.
        let line_none = render(&sample_input(), &sample_snap(10.0), &cfg, 0, false);
        assert!(
            !line_none.contains("disk"),
            "disk None이면 생략: {line_none:?}"
        );
        // toggle off → 생략.
        cfg.display.show_disk = false;
        let mut snap = sample_snap(10.0);
        snap.disk_percent = Some(63.0);
        let line_off = render(&sample_input(), &snap, &cfg, 0, false);
        assert!(
            !line_off.contains("disk"),
            "show_disk=false면 생략: {line_off:?}"
        );
    }

    #[test]
    fn render_shows_network_when_some_and_toggle_on() {
        let mut cfg = Config::default();
        cfg.color.mode = "none".to_string();
        let mut snap = sample_snap(10.0);
        snap.net = Some(NetThroughput {
            rx_bps: 2048.0,
            tx_bps: 512.0,
        });
        let line = render(&sample_input(), &snap, &cfg, 0, false);
        assert!(line.contains("↓2KB/s"), "수신 속도 표시: {line:?}");
        assert!(line.contains("↑512B/s"), "송신 속도 표시: {line:?}");
    }

    #[test]
    fn render_omits_network_when_none_or_toggle_off() {
        let mut cfg = Config::default();
        cfg.color.mode = "none".to_string();
        // None(첫 렌더) → 생략.
        let line_none = render(&sample_input(), &sample_snap(10.0), &cfg, 0, false);
        assert!(!line_none.contains('↓'), "net None이면 생략: {line_none:?}");
        // toggle off → 생략.
        cfg.display.show_network = false;
        let mut snap = sample_snap(10.0);
        snap.net = Some(NetThroughput {
            rx_bps: 2048.0,
            tx_bps: 512.0,
        });
        let line_off = render(&sample_input(), &snap, &cfg, 0, false);
        assert!(
            !line_off.contains('↓'),
            "show_network=false면 생략: {line_off:?}"
        );
    }

    #[test]
    fn display_width_counts_wide_glyph_as_two() {
        // 와이드 이모지 = 폭 2, CALM 글리프(◆ ○)/ASCII는 폭 1.
        assert_eq!(display_width("🔥"), 2);
        assert_eq!(display_width("a"), 1);
        assert_eq!(display_width("🔥a"), 3);
        // 가운뎃점 구분자 " · "는 폭 3(공백1 + 중점1 + 공백1).
        assert_eq!(display_width(" · "), 3);
    }
}
