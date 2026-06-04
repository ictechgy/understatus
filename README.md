# understatus

A calm, unobtrusive statusline addon for Claude Code — live CPU, memory, battery, disk, network, and AI session info in a permanent bottom bar that stays out of your way.

![understatus](docs/understatus-calm-preview.png)

---

## Why

Most statusline widgets are noisy. understatus is designed around one principle: **permanent visibility without distraction**.

- **Calm glyph theme** — load stages render as `○ ▁ ▄ ▆ ◆`. Color touches the glyph only; numeric values stay uncolored. Labels and separators are dimmed. At ≥90% CPU the critical glyph `◆` breathes slowly in restrained terracotta (`#b87848` ↔ `#7a5030`) — not red, not flashing.
- **Reactive CPU** — every render takes two snapshots ~25 ms apart and computes true instantaneous CPU% across all cores. No stale load-average guessing.
- **Non-destructive chaining** — `understatus install` detects your existing `statusLine.command`, preserves it in config, and chains it. `uninstall` restores byte-for-byte. Your current setup is never lost.

> **macOS only.** Apple Silicon (arm64) + Intel (x86\_64). Linux is not supported.

---

## Install

### Homebrew (recommended)

```bash
brew install ictechgy/understatus/understatus
```

> The formula lives in the tap `ictechgy/understatus` (repo: `ictechgy/homebrew-understatus`).

### Cargo

```bash
cargo install understatus
```

Requires Rust 1.75+.

### npm

```bash
npm install -g understatus
```

The npm package shells out to the prebuilt macOS binary for your architecture (arm64 / x64). macOS only.

### From source

```bash
git clone https://github.com/ictechgy/understatus.git
cd understatus
cargo build --release
# binary at ./target/release/understatus
```

---

## Setup

### Install into Claude Code

```bash
understatus install [--interval N] [--theme NAME] [--yes]
```

This patches `~/.claude/settings.json` non-destructively:

1. Reads your current `statusLine.command` (if any).
2. Saves it as `chain_command` in `~/.config/understatus/config.toml`.
3. Replaces `statusLine.command` with the understatus binary path.
4. Injects `"refreshInterval": N` into `settings.json` and mirrors the same value to `config.toml [refresh].interval_seconds`.

**Flags:**

| Flag | Description |
|------|-------------|
| `--interval N` | Set refresh interval in seconds (integer ≥ 1). |
| `--theme NAME` | Set the theme (see [Themes](#themes) for valid names). |
| `--yes` / `-y` | Skip interactive prompts even in a TTY; use flags / inherited / default values. |

**Interactive prompts (TTY only, without `--yes`):** If a flag is omitted and stdin is a TTY, install asks for each missing value (up to 3 retries per item). Empty input accepts the inherited or default value.

**Interval inheritance on reinstall:** When `--interval` is not supplied, the existing `[refresh].interval_seconds` from `config.toml` is reused. Priority: `--interval` flag > existing config value > default (5 s). This prevents the interval from silently resetting to 5 when you reinstall to change only the theme.

### Uninstall

```bash
understatus uninstall
```

Restores `statusLine.command` and `refreshInterval` to their exact pre-install state. If `refreshInterval` was absent before install, the key is deleted.

---

### ⚠️ Global side-effect: `refreshInterval`

`understatus install` writes `"refreshInterval": N` into `settings.json` (default N = 5). This value applies to the **entire** statusLine subsystem — not just understatus:

- understatus itself spawns as a new process every N seconds.
- Any **chained command** (e.g. `lterm-omc-hud.mjs`) is also re-executed every N seconds.

To decouple heavy chain children, understatus caches their stdout via `chain_cache_ttl_seconds` (default 10 s). The chained child re-spawns at most once per TTL — not every N seconds.

**To save battery on laptops**, raise the interval:

```toml
# ~/.config/understatus/config.toml
[refresh]
interval_seconds = 10   # default: 5
```

Note: increasing `interval_seconds` proportionally slows the terracotta breath animation. Adjust `pulse_period_seconds` accordingly to keep it smooth (`pulse_period / interval >= 6`). If `pulse_period / interval < 6` at install time, understatus prints a warning to stderr.

`understatus uninstall` reverts `refreshInterval` precisely — no residue left behind.

---

## Themes

understatus ships nine built-in themes. Set the active theme in one line:

```toml
# ~/.config/understatus/config.toml
theme = "vivid"
```

Or switch after install without reinstalling:

```bash
understatus theme vivid        # switch to vivid; takes effect on next render
understatus theme              # show current theme and usage hint
understatus themes             # list all available themes
```

### Theme table

| Name | Glyph ramp (idle → crit) | Description |
|------|--------------------------|-------------|
| `calm` | `○ ▁ ▄ ▆ ◆` | Cool blue-grey ladder + terracotta breath at critical. **Default.** |
| `mono` | `○ ▁ ▄ ▆ ◆` | Greyscale only — zero hue across all bands. |
| `vivid` | `░ ▒ ▓ █ █` | Traffic-light colors (green → amber → red) with block-fill glyphs. |
| `ember` | `· ∙ • ● ◉` | Warm amber/terracotta monochromatic ladder with dot glyphs. |
| `emoji` | `😌 🙂 😅 🥵 🔥` | Emoji face ramp. Each glyph occupies 2 terminal columns. |
| `neon` | `░ ▒ ▓ █ █` | Neon cyberpunk — electric cyan → magenta with hue-cycling pulse. |
| `aurora` | `▁ ▃ ▅ ▆ █` | Aurora borealis — teal → purple gradient with flash pulse. |
| `sunset` | `· ∙ • ● ◉` | Sunset — gold → coral → purple with flash pulse. |
| `spectrum` | `▁ ▂ ▄ ▆ █` | Per-band rainbow — green → magenta with hue-cycling pulse. |

**COLOR-ONCE principle:** Color is applied to the glyph character only. Numeric values (CPU%, memory, cost, etc.) are always uncolored regardless of theme.

**Critical breath (≥90% CPU):** The critical-band glyph breathes between `pulse_palette[0]` (bright) and `pulse_palette[1]` (dim) over `pulse_period_seconds`. Hue never shifts — only brightness. The animation requires at least 6 render frames per period (`pulse_period / interval_seconds >= 6`); if this is not satisfied, install prints a warning.

**Per-key override:** `theme` fills only the keys not explicitly set in your config. Any of the eight theme-owned keys (`load_glyphs`, `band_tints`, `pulse_palette`, `label_color`, `separator`, `separator_color`, `hud_seam`, `pulse_style`) written in your config take precedence over the preset.

---

## Pulse styles

When the pulse is active (CPU ≥ `pulse_on_threshold`), `pulse_style` controls how the critical-band glyph animates. There are four styles:

| Style | Behavior |
|-------|----------|
| `calm` | Fixed glyph shape. Terracotta luminance breathing — hue never shifts, only brightness cycles between `pulse_palette[0]` (bright) and `pulse_palette[1]` (dim). Most subtle. **Default for original themes.** |
| `flash` | Fixed glyph shape. Same terracotta endpoints as `calm`, but uses a sharper sine curve — the midpoint brightness contrast is more pronounced (punchier breathing). |
| `hue` | Fixed glyph shape. The glyph tint cycles through the full hue wheel (rainbow shimmer) over one `pulse_period_seconds`. Saturation and value stay constant; only hue rotates 360°. |
| `swap` | Hue cycling (same as `hue`) **plus** glyph shape alternation: the critical glyph swaps between its filled and hollow forms (e.g. `◆` ↔ `◇`) on the second half of each period. Most eye-catching. |

**Flashy theme defaults:** The four bold themes ship with a fitting default style — `neon` and `spectrum` use `hue`; `aurora` and `sunset` use `flash`. The original five themes (`calm`, `mono`, `vivid`, `ember`, `emoji`) all use `calm`. All are overridable per the per-key override rule above.

**When pulse is OFF:** Regardless of `pulse_style`, when CPU is below `pulse_off_threshold` the pulse is inactive and the glyph renders with its static `band_tints` color — no animation of any kind.

### Trigger thresholds

```toml
# ~/.config/understatus/config.toml
[pulse]
pulse_on_threshold  = 90   # default: CPU% at which pulse activates
pulse_off_threshold = 80   # default: CPU% below which pulse deactivates (hysteresis)
```

Lower the thresholds to pulse at a lower CPU load:

```toml
[pulse]
pulse_on_threshold  = 75
pulse_off_threshold = 65
```

### Changing the pulse style

```toml
# ~/.config/understatus/config.toml
[pulse]
pulse_style = "hue"   # calm | flash | hue | swap
```

Or switch from the command line (takes effect on the next render):

```bash
understatus pulse hue    # change pulse style
understatus pulse        # print current style
```

---

## Configuration

File: `~/.config/understatus/config.toml`  
All keys are optional; omitting a key uses its default.

| Key | Default | Description |
|-----|---------|-------------|
| `theme` | `"calm"` | Active theme preset. Valid values: `calm`, `mono`, `vivid`, `ember`, `emoji`, `neon`, `aurora`, `sunset`, `spectrum`. The theme fills all eight visual keys not explicitly set in config; individual keys can still override it. |
| `[cpu] sample_window_ms` | `25` | Interval (ms) between the two CPU snapshots. Larger = less noise, more latency. |
| `[cpu] load_glyphs` | `["○","▁","▄","▆","◆"]` | Glyphs for idle→critical load stages. Color is applied to the glyph only. Filled by the active theme; override by writing this key explicitly. |
| `[pulse] pulse_on_threshold` | `90` | CPU% at which the critical glyph starts breathing. |
| `[pulse] pulse_off_threshold` | `80` | CPU% below which the breath turns off (hysteresis). |
| `[pulse] pulse_period_seconds` | `30` | One full breath cycle in seconds. Keep `period / interval_seconds >= 6` for smooth animation. |
| `[pulse] pulse_style` | `"calm"` | Pulse animation style when active. `"calm"` = terracotta luminance breath, hue-invariant (most subtle). `"flash"` = same endpoints, sharper midpoint contrast. `"hue"` = hue-wheel cycling (rainbow shimmer). `"swap"` = hue cycling + glyph shape alternation (most eye-catching). See [Pulse styles](#pulse-styles). |
| `[chain] chain_command` | `""` | Populated by `install`. The command that runs alongside understatus. |
| `[chain] order` | `"self_first"` | `"self_first"` or `"chain_first"` — which output appears on the left. |
| `[chain] chain_cache_ttl_seconds` | `10` | How long (s) to cache the chained command's stdout before re-spawning it. |
| `[chain] chain_timeout_ms` | `500` | Max ms to wait for the chained command. On timeout, cached or empty output is used. |
| `[display] max_width` | `80` | Maximum character width. Lower-priority segments are omitted when exceeded. |
| `[display] show_model` | `true` | Show Claude model name. |
| `[display] show_cost` | `true` | Show cumulative session cost. |
| `[display] show_context` | `true` | Show context usage %. Omitted automatically when null. |
| `[display] show_git` | `true` | Show git branch (derived from `workspace.git_worktree` / repo). |
| `[display] show_battery` | `true` | Show battery (IOKit, 30 s TTL cache). Silently omitted on desktops. |
| `[display] show_disk` | `true` | Show disk usage via `statfs("/")`. |
| `[display] show_network` | `true` | Show network throughput (getifaddrs counter delta). First render has no delta — omitted silently. |
| `[color] mode` | `"auto"` | `"auto"` \| `"truecolor"` \| `"256"` \| `"none"`. Respects `NO_COLOR`. |
| `[color] band_tints` | see below | Five hex colors for idle→critical glyph tint. Filled by the active theme; override by writing this key explicitly. |
| `[color] pulse_palette` | `["#b87848","#7a5030"]` | High/low brightness endpoints for the breath animation. Filled by the active theme; override by writing this key explicitly. |
| `[color] label_color` | `"#6b7280"` | Dimmed color for labels, units, arrows, and git marker. |
| `[color] separator` | `" · "` | Segment separator string. |
| `[color] separator_color` | `"#3b4048"` | Color for separator and HUD seam. |
| `[color] hud_seam` | `"│"` | Character placed between understatus output and the chained command output. |
| `[refresh] interval_seconds` | `5` | Value written to `settings.json` as `refreshInterval`. Set via `install --interval` or the interactive prompt. On reinstall the existing value is inherited unless `--interval` overrides it. ⚠️ Global side-effect — see above. |

**Default `band_tints`** (cool blue-grey brightness ladder, warm terracotta only at critical):

```toml
band_tints = ["#5a6878", "#6d8296", "#86a0b4", "#9fbfce", "#b87848"]
```

---

## How it works

```
Claude Code  (every refreshInterval seconds)
   │  stdin: one JSON line
   ▼
understatus binary  (new process per call — no daemon, no state files, no locks)
   ├─ parse stdin  → ClaudeInput  (session_id extracted here)
   ├─ double-sample CPU  → cpu_percent (0–100%, average across all cores)
   │     on failure → loadavg fallback: min(load1 / ncpu × 100, 100)
   ├─ memory (host_statistics64)
   ├─ battery (IOKit, 30 s TTL cache)        ← machine-global; omitted on desktops
   ├─ disk    (statfs("/"))
   ├─ network (getifaddrs counter delta)     ← omitted on first render
   ├─ glyph + band tint (color on glyph only, theme-driven)
   │   at ≥90% CPU → brightness breath on the critical-band glyph
   ├─ chain_command child (TTL cache + 500 ms timeout)
   └─ compose → stdout (single newline)
```

**CPU measurement:** Two `/proc`-equivalent snapshots are taken ~25 ms apart within the same process invocation. The delta gives true instantaneous utilization — not a smoothed load average. If the syscall fails (rare), `loadavg` serves as a silent fallback.

**Glyph + tint design (COLOR-ONCE):** `band_tints[0..3]` are cool blue-grey values of increasing brightness (idle to high load). `band_tints[4]` is the lone warm color — terracotta — reserved for the critical stage. Only the glyph character receives color; all numeric values and labels stay uncolored. The active theme fills these colors; individual config keys override the preset.

**Pulse animation:** When CPU stays at or above `pulse_on_threshold` (default 90%), the critical-band glyph animates according to `pulse_style`. The `"calm"` style cycles between `pulse_palette[0]` (brighter) and `pulse_palette[1]` (dimmer) with hue-invariant brightness breathing. `"flash"` uses the same endpoints but a sharper curve. `"hue"` rotates through the full hue wheel. `"swap"` adds glyph shape alternation on top of hue rotation. Smooth animation requires `pulse_period / interval_seconds >= 6` (6 or more render frames per cycle). See [Pulse styles](#pulse-styles).

**Session cache isolation:** Per-render caches (chain command output, pulse state, network counter delta) are keyed by `session_id`. Multiple terminal windows running understatus simultaneously do not share or corrupt each other's cached values. Battery state is machine-global and is shared across sessions.

---

## CLI Support Matrix

| CLI | Status | Notes |
|-----|--------|-------|
| **Claude Code** | ✅ Full support | Custom `statusLine.command`, stdin JSON, `refreshInterval` (default 5 s) — all supported. |
| **Gemini CLI** | ⏳ Stub / forward-looking | `/footer` and `/statusline` expose built-in items only; custom commands not yet supported (open issue). |
| **Codex CLI** | ⏳ Stub / forward-looking | `[tui].status_line` is a fixed built-in; custom commands not yet supported (open issue). |

Gemini and Codex integration is documented and stubbed (`CliAdapter` trait planned) but not yet functional — those CLIs do not currently expose a custom statusline command hook.

---

## Platform

- **macOS only.** Uses `host_processor_info`, `host_statistics64`, and `IOPSCopyPowerSourcesInfo` — all macOS-specific APIs.
- **Apple Silicon (arm64) + Intel (x86\_64).** Tested on macOS arm64 with a 12-core Apple Silicon chip.
- Linux: builds may succeed, but CPU double-sampling degrades silently to loadavg fallback. Not a supported target.
- **Rust edition 2021, MSRV 1.75+.**

---

## License

MIT — see [LICENSE](LICENSE).

---

## 한국어 안내

macOS용 AI 코딩 CLI statusline 애드온입니다. CPU%, 메모리, 배터리, 디스크, 네트워크, AI 세션 정보(모델명·비용·컨텍스트)를 Claude Code 하단 표시줄에 조용하고 자연스럽게 표시합니다.

**주요 특징**

- **9종 테마** — `calm`(기본), `mono`, `vivid`, `ember`, `emoji`, `neon`, `aurora`, `sunset`, `spectrum`. 테마는 8개 시각 키(글리프·색상 등)를 한 번에 설정하며, 개별 키를 config.toml에 명시하면 테마보다 우선합니다.
- **COLOR-ONCE 원칙** — 색은 글리프 문자에만 적용. 숫자 값(CPU%, 비용 등)은 항상 무색.
- **≥90% 호흡** — CPU가 90% 이상으로 유지되면 임계 밴드 글리프가 테라코타 명도로 천천히 숨쉽니다(hue 변화 없음). 부드러운 애니메이션에는 `pulse_period / interval_seconds >= 6` 조건이 필요하며, 위반 시 설치 시점에 경고가 출력됩니다.
- **반응형 CPU** — 매 렌더마다 두 스냅샷(~25ms 간격) 직접 측정. loadavg 아님.
- **비파괴 설치** — 기존 `statusLine.command`를 체이닝으로 보존하고 정확히 복원.
- **세션 캐시 격리** — 체인 출력·펄스 상태·네트워크 델타 캐시는 `session_id`별로 분리되어 여러 터미널을 동시에 열어도 값이 섞이지 않습니다. 배터리는 머신 전역.

**설치**

```bash
# Homebrew
brew install ictechgy/understatus/understatus

# Cargo
cargo install understatus

# npm
npm install -g understatus
```

**Claude Code에 적용**

```bash
understatus install [--interval N] [--theme NAME] [--yes]
understatus uninstall   # 원상 복원
```

`--interval`/`--theme` 미지정 + TTY 환경이면 각 항목을 대화형으로 묻습니다. `--yes`(또는 비TTY)이면 플래그·기존값·기본값을 그대로 사용합니다. 재설치 시 `--interval`을 지정하지 않으면 기존 `config.toml`의 interval이 그대로 승계됩니다(기본 5초로 초기화되지 않습니다).

> ⚠️ `install`은 `settings.json`에 `"refreshInterval": N`을 전역 주입합니다. 체이닝된 기존 명령도 N초마다 재실행 대상이 됩니다. 배터리 절약이 필요하면 `config.toml`에서 `interval_seconds = 10`으로 올리세요.

**테마 관리**

```bash
understatus theme vivid    # 테마 전환 (config.toml만 수정, 즉시 적용)
understatus theme          # 현재 테마 및 사용법 확인
understatus themes         # 사용 가능한 테마 목록
```

| 이름 | 글리프 램프 (idle → crit) | 설명 |
|------|--------------------------|------|
| `calm` | `○ ▁ ▄ ▆ ◆` | 차가운 blue-grey + 테라코타 호흡 (기본) |
| `mono` | `○ ▁ ▄ ▆ ◆` | 무채색, 제로 색상 |
| `vivid` | `░ ▒ ▓ █ █` | 신호등 색 + 블록 글리프 |
| `ember` | `· ∙ • ● ◉` | 따뜻한 앰버/테라코타 단색 + 도트 글리프 |
| `emoji` | `😌 🙂 😅 🥵 🔥` | 이모지 표정 램프 (각 글리프 2칸 폭) |
| `neon` | `░ ▒ ▓ █ █` | 네온 사이버펑크 (일렉트릭 시안→마젠타, hue 순환 펄스) |
| `aurora` | `▁ ▃ ▅ ▆ █` | 오로라 청록→보라 그라데이션 (flash 펄스) |
| `sunset` | `· ∙ • ● ◉` | 노을 골드→코랄→퍼플 (flash 펄스) |
| `spectrum` | `▁ ▂ ▄ ▆ █` | 밴드별 무지개 (초록→마젠타, hue 순환 펄스) |

**펄스 스타일**

CPU가 `pulse_on_threshold`(기본 90%) 이상이면 임계 밴드 글리프가 `pulse_style`에 따라 애니메이션됩니다.

| 스타일 | 동작 |
|--------|------|
| `calm` | 글리프 고정. 테라코타 명도 호흡 (hue 변화 없음). 가장 차분함. **기존 테마 기본.** |
| `flash` | 글리프 고정. `calm`과 같은 끝점이지만 중간 위상 대비가 더 가파름 (더 강렬한 호흡). |
| `hue` | 글리프 고정. 글리프 색이 한 주기(pulse_period_seconds) 동안 색상환 전체를 순환 (무지개 shimmer). |
| `swap` | `hue` 순환 **+** 글리프 모양 교대: 주기 후반부에 `◆`↔`◇` 등 채움/빈 형태를 번갈아 표시. 가장 눈에 띔. |

화려한 테마 기본: `neon`·`spectrum` = `hue`, `aurora`·`sunset` = `flash`. 기존 5종은 모두 `calm`. 개별 키로 재정의 가능.

펄스가 OFF(CPU < `pulse_off_threshold`)이면 스타일과 무관하게 정적 밴드 틴트로 표시됩니다 (애니메이션 없음).

```toml
# ~/.config/understatus/config.toml
[pulse]
pulse_on_threshold  = 90   # 펄스 발동 CPU% (기본)
pulse_off_threshold = 80   # 펄스 해제 CPU% — 히스테리시스 (기본)
pulse_style = "hue"        # calm | flash | hue | swap
```

```bash
understatus pulse hue    # 펄스 스타일 변경 (config.toml만 수정, 즉시 적용)
understatus pulse        # 현재 펄스 스타일 확인
```

설정 파일: `~/.config/understatus/config.toml` (없으면 모두 기본값)

```toml
theme = "vivid"   # 한 줄로 테마 지정; 개별 키 override 가능
```

macOS 전용 · Apple Silicon(arm64) + Intel(x86\_64) · Rust 1.75+ · MIT 라이선스
