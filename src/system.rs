//! 시스템 지표 샘플링: REACTIVE 더블샘플 CPU% + loadavg 폴백 + 메모리 + 배터리.
//!
//! 계획서 §D-1/§H-1/AC3을 따른다. CPU는 렌더 시점에 두 스냅샷(~25ms 간격)을
//! self-contained로 떠서 진짜 순간 사용률(0–100%, 전 코어 평균)을 산출한다.
//! 영속 상태/데몬 없음(계획서 §A 원칙 1).

use crate::config::Config;

// macOS에는 `getloadavg(3)`이 libSystem에 항상 존재하지만 `libc` 0.2가
// 노출하지 않으므로 직접 선언한다. loadavg는 더블샘플 실패 시의 폴백 경로에서만
// 쓰인다(계획서 §A 원칙 2, AC3).
extern "C" {
    // 시스템 load average를 채운다(load1/load5/load15).
    // 인자: loadavg = 결과를 받을 f64 배열 포인터, nelem = 채울 원소 개수(최대 3).
    // 반환: 실제로 채운 원소 수(실패 시 -1).
    // (extern 블록 내부는 rustdoc 대상이 아니므로 `///` 대신 일반 주석을 쓴다.)
    fn getloadavg(loadavg: *mut f64, nelem: libc::c_int) -> libc::c_int;
}

/// 렌더 핫패스에서 허용하는 CPU 더블샘플 대기 상한(ms).
///
/// 기본 25ms 동작은 그대로 보존하되, config가 비정상적으로 큰 값을 지정해 statusline 렌더를 오래
/// 블록하지 못하게 한다. 100ms는 노이즈 완화 여지를 남기면서 한 프레임 지연 예산을 유한하게 만든다.
const MAX_CPU_SAMPLE_WINDOW_MS: u64 = 100;

/// 한 번의 렌더에서 측정한 시스템 상태 스냅샷.
///
/// 각 항목은 best-effort이며 실패/부재 시 안전 저하한다(배터리는 `Option`).
#[derive(Debug, Clone, PartialEq)]
pub struct SystemSnapshot {
    /// 진짜 순간 CPU 사용률(0–100, 전 코어 평균). 더블샘플 실패 시 loadavg 폴백값.
    pub cpu_percent: f64,
    /// 메모리 사용률(0–100).
    pub mem_percent: f64,
    /// 배터리 정보(P2, IOKit). 데스크톱/조회 실패 시 `None`.
    pub battery: Option<BatteryInfo>,
    /// 루트 볼륨 디스크 사용률(0–100, P2 statfs). 조회 실패 시 `None`.
    pub disk_percent: Option<f64>,
    /// 네트워크 throughput(P2, getifaddrs 델타). 첫 렌더(이전값 부재)/조회 실패 시 `None`.
    pub net: Option<NetThroughput>,
}

/// 배터리 상태(P2, IOKit `IOPSCopyPowerSourcesInfo` 기반, 30–60초 TTL 캐시).
#[derive(Debug, Clone, PartialEq)]
pub struct BatteryInfo {
    /// 충전 잔량(0–100).
    pub percent: f64,
    /// 충전 중 여부(AC 전원 연결).
    pub is_charging: bool,
}

/// 네트워크 throughput(초당 바이트). 비-루프백 인터페이스 카운터 델타로 산출한다(P2).
///
/// 절대 누적량이 아니라 직전 렌더 대비 변화율(rate)이다. 단기 TTL 캐시에 저장한
/// (rx_bytes, tx_bytes, now_ms)와의 델타로 계산하므로 첫 렌더에서는 `None`이다.
#[derive(Debug, Clone, PartialEq)]
pub struct NetThroughput {
    /// 수신 속도(bytes/sec).
    pub rx_bps: f64,
    /// 송신 속도(bytes/sec).
    pub tx_bps: f64,
}

/// 두 더블샘플 스냅샷의 틱 델타에서 순간 CPU%를 계산하는 순수 함수.
///
/// 본 계산을 FFI에서 분리해 단위 테스트 가능하게 한다(AC3). 전 코어 합산 델타를
/// 받아 `100 * busy_delta / total_delta`를 반환하며, 결과는 0..=100으로 클램프한다.
///
/// # 인자
/// - `busy_delta`: 두 스냅샷 사이 busy 틱(user+system+nice) 증가분 합.
/// - `total_delta`: 두 스냅샷 사이 전체 틱(busy+idle) 증가분 합.
///
/// # 반환
/// 0..=100 범위 CPU%. `total_delta == 0`(시간 미경과/측정 불가)이면 0.0.
fn cpu_percent_from_deltas(busy_delta: u64, total_delta: u64) -> f64 {
    if total_delta == 0 {
        return 0.0;
    }
    let percent = 100.0 * (busy_delta as f64) / (total_delta as f64);
    percent.clamp(0.0, 100.0)
}

/// loadavg 폴백을 0..=100 CPU%로 정규화하는 순수 함수.
///
/// 공식은 계획서 AC3의 `min(load1/ncpu*100, 100)`이다. load1은 ncpu를 초과할 수
/// 있으므로(예: load1=68, ncpu=12 → 567%) 반드시 0..=100으로 클램프한다.
///
/// # 인자
/// - `load1`: 1분 load average.
/// - `ncpu`: 논리 코어 수(0이면 측정 불가로 0.0 반환).
///
/// # 반환
/// 0..=100 범위 CPU% 근사값.
fn loadavg_to_percent(load1: f64, ncpu: u32) -> f64 {
    if ncpu == 0 {
        return 0.0;
    }
    let percent = load1 / (ncpu as f64) * 100.0;
    percent.clamp(0.0, 100.0)
}

/// 논리 코어 수(`hw.ncpu`)를 조회한다. 실패 시 1로 안전 저하한다.
///
/// # 반환
/// 논리 코어 수(>= 1). loadavg 폴백 정규화의 분모로 쓰인다.
fn cpu_count() -> u32 {
    // sysconf(_SC_NPROCESSORS_ONLN)는 온라인 코어 수를 반환한다. 음수/0이면 1로 저하.
    let count = unsafe { libc::sysconf(libc::_SC_NPROCESSORS_ONLN) };
    if count > 0 {
        count as u32
    } else {
        1
    }
}

/// loadavg 폴백 경로: `getloadavg`로 load1을 읽어 0..=100 CPU%로 정규화한다.
///
/// 더블샘플(mach) 경로가 실패했을 때만 호출된다(계획서 §A 원칙 2, AC3).
/// 조회 실패 시 0.0으로 안전 저하하며 절대 패닉하지 않는다.
///
/// # 반환
/// 0..=100 범위 CPU% 근사값.
fn sample_cpu_loadavg_fallback() -> f64 {
    let mut loads = [0.0f64; 3];
    let filled = unsafe { getloadavg(loads.as_mut_ptr(), 3) };
    if filled < 1 {
        return 0.0;
    }
    loadavg_to_percent(loads[0], cpu_count())
}

/// 사용자 설정 CPU sample window를 렌더 핫패스 예산 안으로 정규화한다.
fn bounded_cpu_sample_window_ms(sample_window_ms: u64) -> u64 {
    sample_window_ms.min(MAX_CPU_SAMPLE_WINDOW_MS)
}

/// 전 코어 busy/total 틱 합계를 담는 한 번의 더블샘플 스냅샷.
struct CpuTickTotals {
    /// busy 틱(user + system + nice) 전 코어 합.
    busy: u64,
    /// 전체 틱(busy + idle) 전 코어 합.
    total: u64,
}

/// `host_processor_info(PROCESSOR_CPU_LOAD_INFO)`로 전 코어 틱 합계를 한 번 떠온다.
///
/// 반환 버퍼는 커널이 vm_allocate로 할당하므로 사용 후 `vm_deallocate`로 해제한다.
/// 어떤 단계든 실패하면 `None`을 반환해 호출부가 loadavg 폴백으로 저하하게 한다.
///
/// # 반환
/// 성공 시 전 코어 busy/total 틱 합([`CpuTickTotals`]), 실패 시 `None`.
fn snapshot_cpu_ticks() -> Option<CpuTickTotals> {
    // mach_host_self / vm_deallocate / mach_task_self_ 는 libc에서 deprecated 표시되어
    // 있으나 mach2 크레이트를 추가하지 않기 위해 직접 사용한다(계획서 §E 의존성 최소화).
    #[allow(deprecated)]
    unsafe {
        let host = libc::mach_host_self();
        let mut cpu_count: libc::natural_t = 0;
        let mut info_ptr: libc::processor_info_array_t = std::ptr::null_mut();
        let mut info_count: libc::mach_msg_type_number_t = 0;

        let result = libc::host_processor_info(
            host,
            libc::PROCESSOR_CPU_LOAD_INFO,
            &mut cpu_count,
            &mut info_ptr,
            &mut info_count,
        );
        if result != libc::KERN_SUCCESS || info_ptr.is_null() || cpu_count == 0 {
            return None;
        }

        // info_ptr은 [cpu_count][CPU_STATE_MAX] integer_t 평탄 배열이다.
        let states = libc::CPU_STATE_MAX as usize;
        let mut busy: u64 = 0;
        let mut total: u64 = 0;
        for core in 0..(cpu_count as usize) {
            let base = core * states;
            // 틱은 음수가 될 수 없는 카운터지만 integer_t(c_int)로 노출되므로 u64로 안전 변환.
            let user = *info_ptr.add(base + libc::CPU_STATE_USER as usize) as u32 as u64;
            let system = *info_ptr.add(base + libc::CPU_STATE_SYSTEM as usize) as u32 as u64;
            let nice = *info_ptr.add(base + libc::CPU_STATE_NICE as usize) as u32 as u64;
            let idle = *info_ptr.add(base + libc::CPU_STATE_IDLE as usize) as u32 as u64;
            busy += user + system + nice;
            total += user + system + nice + idle;
        }

        // 커널 할당 버퍼 해제(누수 방지). 실패해도 결과 산출에는 영향 없음.
        let dealloc_size = (info_count as usize) * std::mem::size_of::<libc::integer_t>();
        libc::vm_deallocate(
            libc::mach_task_self_,
            info_ptr as libc::vm_address_t,
            dealloc_size as libc::vm_size_t,
        );

        Some(CpuTickTotals { busy, total })
    }
}

// CONTRACT: signature is frozen — implement body only, do not change this signature
/// 진짜 순간 CPU 사용률(0–100, 전 코어 평균)을 더블샘플로 측정한다.
///
/// # 인자
/// - `sample_window_ms`: 두 스냅샷 사이 간격(ms, 기본 25).
///
/// # 반환
/// 0..=100 범위의 CPU%. 내부적으로 `host_processor_info` 스냅샷을 두 번 떠
/// 커널 CPU 틱 델타로 계산한다. 더블샘플 실패 시 loadavg 정규화로 저하하며,
/// 폴백 공식은 0–100 클램프 `min(load1/hw.ncpu*100, 100)`이다(AC3, 패닉 금지).
pub fn sample_cpu_reactive(sample_window_ms: u64) -> f64 {
    sample_cpu_reactive_with(
        sample_window_ms,
        snapshot_cpu_ticks,
        std::thread::sleep,
        sample_cpu_loadavg_fallback,
    )
}

/// [`sample_cpu_reactive`]의 테스트 가능한 코어.
///
/// FFI 스냅샷 함수와 sleeper를 주입해, 라이브 CPU 상태나 wall-clock에 의존하지 않고 sample window cap
/// 적용과 더블샘플 산식을 검증한다.
fn sample_cpu_reactive_with(
    sample_window_ms: u64,
    mut snapshot: impl FnMut() -> Option<CpuTickTotals>,
    mut sleep: impl FnMut(std::time::Duration),
    mut fallback: impl FnMut() -> f64,
) -> f64 {
    let sample_window_ms = bounded_cpu_sample_window_ms(sample_window_ms);

    // 1차 스냅샷 → sample_window_ms 만큼 대기 → 2차 스냅샷. 어느 한쪽이라도 실패하면
    // loadavg 폴백으로 저하한다.
    let first = match snapshot() {
        Some(snapshot) => snapshot,
        None => return fallback(),
    };

    sleep(std::time::Duration::from_millis(sample_window_ms));

    let second = match snapshot() {
        Some(snapshot) => snapshot,
        None => return fallback(),
    };

    // saturating_sub: 카운터 래핑/순서 역전 시 음수 델타를 0으로 방어.
    let busy_delta = second.busy.saturating_sub(first.busy);
    let total_delta = second.total.saturating_sub(first.total);

    // 윈도가 너무 짧아 틱이 전혀 증가하지 않은 경우(total_delta==0)에도 0.0이 나오는데,
    // 이는 더블샘플의 정상적 산출이므로 loadavg 폴백으로 넘기지 않는다.
    cpu_percent_from_deltas(busy_delta, total_delta)
}

// CONTRACT: signature is frozen — implement body only, do not change this signature
/// 메모리 사용률(0–100)을 측정한다.
///
/// # 반환
/// 0..=100 범위의 사용 메모리 비율. 조회 실패 시 0.0으로 안전 저하(패닉 금지).
pub fn sample_memory() -> f64 {
    // host_statistics64(HOST_VM_INFO64)로 페이지 단위 VM 통계를 읽고,
    // 사용 페이지(active+wire+compressor)와 가용 페이지(free+inactive+speculative)로
    // 사용률을 산출한다. 페이지 크기는 sysconf(_SC_PAGESIZE).
    #[allow(deprecated)]
    let stats = unsafe {
        let host = libc::mach_host_self();
        let mut vm_stats: libc::vm_statistics64 = std::mem::zeroed();
        let mut count: libc::mach_msg_type_number_t = libc::HOST_VM_INFO64_COUNT;
        let result = libc::host_statistics64(
            host,
            libc::HOST_VM_INFO64,
            &mut vm_stats as *mut _ as libc::host_info64_t,
            &mut count,
        );
        if result != libc::KERN_SUCCESS {
            return 0.0;
        }
        vm_stats
    };

    // 사용 중 = active + wire + compressor(압축 메모리). 가용 = free + inactive + speculative.
    // inactive/speculative는 즉시 회수 가능하므로 "가용"으로 본다(Activity Monitor 근사).
    let used_pages =
        stats.active_count as u64 + stats.wire_count as u64 + stats.compressor_page_count as u64;
    let free_pages =
        stats.free_count as u64 + stats.inactive_count as u64 + stats.speculative_count as u64;
    let total_pages = used_pages + free_pages;
    if total_pages == 0 {
        return 0.0;
    }

    let percent = 100.0 * (used_pages as f64) / (total_pages as f64);
    percent.clamp(0.0, 100.0)
}

// === 배터리(P2): in-process IOKit IOPSCopyPowerSourcesInfo FFI + 30s TTL 캐시 ===
//
// 방식 선택(계획서 §F P2): pmset 셸아웃 대신 in-process IOKit FFI를 채택한다.
// IOKit/CoreFoundation은 build.rs가 프레임워크로 링크한다(정석). 배터리는 느리게
// 변하므로 30s TTL 디스크 캐시(chain.rs와 동일한 단기 TTL 예외)로 IOKit 재조회를
// 분당 ~2회로 제한한다 — 매 렌더(기본 5초)마다 IOKit를 두드리지 않는다.

/// 배터리 캐시 파일명(`~/Library/Caches/understatus/battery`).
const BATTERY_CACHE_FILE: &str = "battery";
/// 배터리 캐시 TTL(초). 배터리는 느리게 변하므로 30초면 충분하다(계획서 §F P2: 30–60s).
const BATTERY_CACHE_TTL_SECONDS: u64 = 30;

// CoreFoundation/IOKit FFI 선언. opaque 포인터(*mut/*const c_void)로 다루며,
// 키 문자열은 CFStringCreateWithCString로 만든다. build.rs가 두 프레임워크를 링크한다.
extern "C" {
    // IOPSCopyPowerSourcesInfo: 전원 소스 블롭(CFTypeRef)을 반환. 호출자가 CFRelease.
    fn IOPSCopyPowerSourcesInfo() -> *const libc::c_void;
    // IOPSCopyPowerSourcesList: 위 블롭에서 전원 소스 배열(CFArrayRef)을 만든다. 호출자가 CFRelease.
    fn IOPSCopyPowerSourcesList(blob: *const libc::c_void) -> *const libc::c_void;
    // IOPSGetPowerSourceDescription: 배열 원소(전원 소스)의 설명 딕셔너리(CFDictionaryRef)를 반환.
    // 반환값은 blob 소유이므로 CFRelease 금지(get 규칙).
    fn IOPSGetPowerSourceDescription(
        blob: *const libc::c_void,
        ps: *const libc::c_void,
    ) -> *const libc::c_void;

    // CFArray.
    fn CFArrayGetCount(array: *const libc::c_void) -> libc::c_long;
    fn CFArrayGetValueAtIndex(
        array: *const libc::c_void,
        index: libc::c_long,
    ) -> *const libc::c_void;

    // CFDictionary: 키로 값을 조회(get 규칙, CFRelease 금지).
    fn CFDictionaryGetValue(
        dict: *const libc::c_void,
        key: *const libc::c_void,
    ) -> *const libc::c_void;

    // CFNumber: i64로 값 추출. kCFNumberSInt64Type = 4.
    fn CFNumberGetValue(
        number: *const libc::c_void,
        the_type: libc::c_int,
        value_ptr: *mut libc::c_void,
    ) -> bool;

    // CFBoolean / CFString 비교용.
    fn CFBooleanGetValue(boolean: *const libc::c_void) -> bool;
    fn CFStringCreateWithCString(
        alloc: *const libc::c_void,
        c_str: *const libc::c_char,
        encoding: u32,
    ) -> *const libc::c_void;
    fn CFStringCompare(
        a: *const libc::c_void,
        b: *const libc::c_void,
        options: libc::c_ulong,
    ) -> libc::c_long;
    fn CFGetTypeID(cf: *const libc::c_void) -> libc::c_ulong;
    fn CFBooleanGetTypeID() -> libc::c_ulong;

    // 소유한 CFTypeRef 해제.
    fn CFRelease(cf: *const libc::c_void);
}

/// kCFNumberSInt64Type. CFNumberGetValue에 전달할 타입 코드.
const CF_NUMBER_SINT64_TYPE: libc::c_int = 4;
/// kCFStringEncodingUTF8. CFStringCreateWithCString 인코딩.
const CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;
/// CFComparisonResult::kCFCompareEqualTo.
const CF_COMPARE_EQUAL: libc::c_long = 0;

/// 키 문자열(C str)로 전원 소스 딕셔너리에서 CFNumber 값을 i64로 읽는다.
///
/// # 안전성
/// `dict`가 유효한 CFDictionaryRef라는 전제하에 호출한다. 키 CFString은 함수 내부에서
/// 생성/해제한다. 값 부재/타입 불일치 시 `None`.
unsafe fn dict_number(dict: *const libc::c_void, key: &std::ffi::CStr) -> Option<i64> {
    let cf_key = CFStringCreateWithCString(std::ptr::null(), key.as_ptr(), CF_STRING_ENCODING_UTF8);
    if cf_key.is_null() {
        return None;
    }
    let value = CFDictionaryGetValue(dict, cf_key);
    let result = if value.is_null() {
        None
    } else {
        let mut out: i64 = 0;
        let ok = CFNumberGetValue(
            value,
            CF_NUMBER_SINT64_TYPE,
            &mut out as *mut i64 as *mut libc::c_void,
        );
        if ok {
            Some(out)
        } else {
            None
        }
    };
    // 우리가 생성한 키 CFString은 소유하므로 해제한다(값은 get 규칙이라 해제 금지).
    CFRelease(cf_key);
    result
}

/// 전원 소스 딕셔너리의 "Power Source State"가 충전/AC 연결 상태인지 판정한다.
///
/// `kIOPSPowerSourceStateKey`("Power Source State") 값이 "AC Power"면 전원 연결(true).
/// 또는 `kIOPSIsChargingKey`("Is Charging") CFBoolean이 true면 충전 중으로 본다.
///
/// # 안전성
/// `dict`가 유효한 CFDictionaryRef라는 전제하에 호출한다.
unsafe fn dict_is_charging(dict: *const libc::c_void) -> bool {
    // (1) "Is Charging" CFBoolean이 true면 충전 중.
    if let Ok(key) = std::ffi::CString::new("Is Charging") {
        let cf_key =
            CFStringCreateWithCString(std::ptr::null(), key.as_ptr(), CF_STRING_ENCODING_UTF8);
        if !cf_key.is_null() {
            let value = CFDictionaryGetValue(dict, cf_key);
            let charging = !value.is_null()
                && CFGetTypeID(value) == CFBooleanGetTypeID()
                && CFBooleanGetValue(value);
            CFRelease(cf_key);
            if charging {
                return true;
            }
        }
    }

    // (2) "Power Source State" == "AC Power"면 전원 연결(충전 완료 포함)로 본다.
    dict_string_equals(dict, "Power Source State", "AC Power")
}

/// 딕셔너리에서 키의 CFString 값이 기대 문자열과 같은지 비교한다.
///
/// # 안전성
/// `dict`가 유효한 CFDictionaryRef라는 전제하에 호출한다.
unsafe fn dict_string_equals(dict: *const libc::c_void, key: &str, expected: &str) -> bool {
    let key_c = match std::ffi::CString::new(key) {
        Ok(c) => c,
        Err(_) => return false,
    };
    let expected_c = match std::ffi::CString::new(expected) {
        Ok(c) => c,
        Err(_) => return false,
    };
    let cf_key =
        CFStringCreateWithCString(std::ptr::null(), key_c.as_ptr(), CF_STRING_ENCODING_UTF8);
    if cf_key.is_null() {
        return false;
    }
    let cf_expected = CFStringCreateWithCString(
        std::ptr::null(),
        expected_c.as_ptr(),
        CF_STRING_ENCODING_UTF8,
    );
    let value = CFDictionaryGetValue(dict, cf_key);
    let equal = if !value.is_null() && !cf_expected.is_null() {
        CFStringCompare(value, cf_expected, 0) == CF_COMPARE_EQUAL
    } else {
        false
    };
    CFRelease(cf_key);
    if !cf_expected.is_null() {
        CFRelease(cf_expected);
    }
    equal
}

/// IOKit `IOPSCopyPowerSourcesInfo`로 배터리 상태를 in-process 조회한다(캐시 미스 경로).
///
/// # 반환
/// 첫 번째 배터리 전원 소스의 [`BatteryInfo`]. 배터리 없음(데스크톱)/조회 실패 시 `None`.
/// 모든 IOKit/CF 객체는 소유한 것만 CFRelease하며, 어떤 실패에도 패닉하지 않는다(AC5).
fn sample_battery_iokit() -> Option<BatteryInfo> {
    let current_key = std::ffi::CString::new("Current Capacity").ok()?;
    let max_key = std::ffi::CString::new("Max Capacity").ok()?;

    unsafe {
        let blob = IOPSCopyPowerSourcesInfo();
        if blob.is_null() {
            return None;
        }
        let list = IOPSCopyPowerSourcesList(blob);
        if list.is_null() {
            CFRelease(blob);
            return None;
        }

        let mut result: Option<BatteryInfo> = None;
        let count = CFArrayGetCount(list);
        for index in 0..count {
            let ps = CFArrayGetValueAtIndex(list, index);
            if ps.is_null() {
                continue;
            }
            // 설명 딕셔너리는 blob 소유(get 규칙) → CFRelease 금지.
            let dict = IOPSGetPowerSourceDescription(blob, ps);
            if dict.is_null() {
                continue;
            }

            // Current/Max Capacity로 퍼센트 산출. 둘 중 하나라도 없으면 이 소스는 건너뛴다.
            let current = dict_number(dict, &current_key);
            let max = dict_number(dict, &max_key);
            if let (Some(current), Some(max)) = (current, max) {
                if max > 0 {
                    let percent = (100.0 * current as f64 / max as f64).clamp(0.0, 100.0);
                    let is_charging = dict_is_charging(dict);
                    result = Some(BatteryInfo {
                        percent,
                        is_charging,
                    });
                    break; // 첫 배터리 소스만 사용.
                }
            }
        }

        // 우리가 Copy로 받은 두 객체를 해제(get 규칙의 dict/ps는 해제 금지).
        CFRelease(list);
        CFRelease(blob);
        result
    }
}

/// 배터리 캐시 payload("percent is_charging")를 파싱하는 순수 헬퍼(테스트 가능).
///
/// # 인자
/// - `payload`: `"<percent> <0|1>"` 형식의 캐시 본문.
///
/// # 반환
/// 파싱된 [`BatteryInfo`]. 토큰 부족/형식 오류 시 `None`.
fn parse_battery_cache(payload: &str) -> Option<BatteryInfo> {
    let mut parts = payload.split_whitespace();
    let percent = parts.next()?.parse::<f64>().ok()?;
    let charging_flag = parts.next()?;
    let is_charging = charging_flag == "1";
    Some(BatteryInfo {
        percent: percent.clamp(0.0, 100.0),
        is_charging,
    })
}

/// 배터리를 30s TTL 단기 캐시로 조회한다(IOKit 재조회 빈도 제한).
///
/// 신선한 캐시가 있으면 IOKit를 두드리지 않고 캐시값을 반환하고(연속 호출 시 재조회 없음),
/// 없으면 IOKit로 조회한 뒤 캐시를 갱신한다. 데스크톱(배터리 없음)/실패 시 `None`(AC5).
///
/// 캐시는 chain.rs와 동일한 `~/Library/Caches/understatus/` 단기 TTL 예외를 재사용한다.
fn sample_battery() -> Option<BatteryInfo> {
    let now_ms = crate::chain::cache_now_millis();

    // (1) 신선한 캐시가 있으면 IOKit 재조회 없이 즉시 반환(TTL 내).
    if let Some((written_ms, payload)) = crate::chain::read_named_cache(BATTERY_CACHE_FILE) {
        if crate::chain::is_named_cache_fresh(written_ms, now_ms, BATTERY_CACHE_TTL_SECONDS) {
            return parse_battery_cache(&payload);
        }
    }

    // (2) 캐시 미스/만료 → IOKit 조회. 성공 시 캐시 갱신 후 반환.
    let info = sample_battery_iokit()?;
    let flag = if info.is_charging { "1" } else { "0" };
    crate::chain::write_named_cache(
        BATTERY_CACHE_FILE,
        now_ms,
        &format!("{} {}", info.percent, flag),
    );
    Some(info)
}

/// statfs 블록 통계에서 루트 볼륨 사용률(0–100)을 계산하는 순수 함수(테스트 가능).
///
/// 공식(계획서 §F P2): `used% = (blocks - bavail) / blocks * 100`.
/// `bavail`은 비특권 사용자가 실제로 쓸 수 있는 가용 블록이라 `bfree`(예약 포함)보다
/// Disk Utility/df 표시값에 가깝다. `bfree`는 시그니처 완전성을 위해 받지만 산식엔 쓰지 않는다.
///
/// # 인자
/// - `blocks`: 전체 블록 수(`f_blocks`).
/// - `bfree`: 슈퍼유저 가용 블록(`f_bfree`, 예약 포함).
/// - `bavail`: 비특권 가용 블록(`f_bavail`).
///
/// # 반환
/// 0..=100 범위 사용률. `blocks == 0`(측정 불가)이면 `None`.
fn disk_percent_from_statfs(blocks: u64, bfree: u64, bavail: u64) -> Option<f64> {
    // bfree는 산식에 직접 쓰지 않으나(df는 bavail 기준 사용률), 시그니처 완전성을 위해 받는다.
    let _ = bfree;
    if blocks == 0 {
        return None;
    }
    // used = blocks - bavail. bavail이 blocks를 초과하는 비정상 입력은 saturating으로 방어.
    let used = blocks.saturating_sub(bavail);
    let percent = 100.0 * (used as f64) / (blocks as f64);
    Some(percent.clamp(0.0, 100.0))
}

/// 루트 볼륨(`/`)의 디스크 사용률(0–100)을 `statfs("/")`로 측정한다(best-effort).
///
/// # 반환
/// 0..=100 범위 사용률. statfs 실패/blocks=0 시 `None`으로 안전 저하(패닉 금지, AC5).
fn sample_disk() -> Option<f64> {
    // statfs("/")로 루트 볼륨 블록 통계를 읽는다. 실패 시 None.
    let stats = unsafe {
        let mut buf: libc::statfs = std::mem::zeroed();
        // "/" 경로(NUL 종단)로 statfs 호출.
        let result = libc::statfs(c"/".as_ptr(), &mut buf);
        if result != 0 {
            return None;
        }
        buf
    };
    disk_percent_from_statfs(stats.f_blocks, stats.f_bfree, stats.f_bavail)
}

/// 두 카운터 스냅샷의 델타에서 초당 바이트(rate)를 계산하는 순수 함수(테스트 가능).
///
/// # 인자
/// - `prev_bytes`: 직전 렌더의 누적 바이트.
/// - `now_bytes`: 이번 렌더의 누적 바이트.
/// - `dt_ms`: 두 렌더 사이 경과(ms).
///
/// # 반환
/// `(now - prev) / dt_seconds`. `dt_ms <= 0`이면 `None`(0 나눗셈/시계역행 방어).
/// 카운터 래핑/리셋(now < prev)은 `saturating_sub`로 0 델타로 방어한다(음수 rate 금지).
fn throughput(prev_bytes: u64, now_bytes: u64, dt_ms: u64) -> Option<f64> {
    if dt_ms == 0 {
        return None;
    }
    // saturating_sub: 카운터 래핑(u32 오버플로)/인터페이스 리셋 시 음수 델타를 0으로 방어.
    let delta = now_bytes.saturating_sub(prev_bytes);
    let dt_seconds = (dt_ms as f64) / 1000.0;
    Some((delta as f64) / dt_seconds)
}

/// 모든 비-루프백 인터페이스의 누적 (rx_bytes, tx_bytes)를 getifaddrs로 합산한다.
///
/// `AF_LINK` 엔트리의 `ifa_data`(`*mut libc::if_data`)에서 `ifi_ibytes`/`ifi_obytes`를
/// 읽어 합산한다. 루프백(`IFF_LOOPBACK`)은 제외한다(자기 트래픽 노이즈 배제).
///
/// # 반환
/// `(누적 rx_bytes, 누적 tx_bytes)`. getifaddrs 실패 시 `None`(안전 저하, 패닉 금지).
fn sample_net_counters() -> Option<(u64, u64)> {
    unsafe {
        let mut ifap: *mut libc::ifaddrs = std::ptr::null_mut();
        if libc::getifaddrs(&mut ifap) != 0 || ifap.is_null() {
            return None;
        }

        let mut rx_total: u64 = 0;
        let mut tx_total: u64 = 0;
        let mut cursor = ifap;
        while !cursor.is_null() {
            let entry = &*cursor;
            // AF_LINK 엔트리만 if_data 통계를 가진다. 루프백은 제외.
            if !entry.ifa_addr.is_null()
                && (*entry.ifa_addr).sa_family as libc::c_int == libc::AF_LINK
                && (entry.ifa_flags as libc::c_int & libc::IFF_LOOPBACK) == 0
                && !entry.ifa_data.is_null()
            {
                let data = &*(entry.ifa_data as *const libc::if_data);
                // macOS arm64(if_data b64)의 ibytes/obytes는 u32 카운터다.
                rx_total += data.ifi_ibytes as u64;
                tx_total += data.ifi_obytes as u64;
            }
            cursor = entry.ifa_next;
        }

        // getifaddrs가 할당한 연결 리스트를 해제(누수 방지).
        libc::freeifaddrs(ifap);
        Some((rx_total, tx_total))
    }
}

/// 네트워크 throughput(bytes/sec)을 직전 렌더 카운터와의 델타로 측정한다(best-effort).
///
/// chain_output/pulse_state와 동일한 단기 TTL 캐시 디렉터리에 `(rx,tx,now_ms)`를 저장하고,
/// 다음 렌더에서 그 직전값과 현재값의 델타로 rate를 산출한다(계획서 §F P2, 단기 TTL 예외).
/// 첫 렌더(이전값 부재)나 `dt<=0`에서는 `None`을 반환한다(데몬/영속 상태 아님).
///
/// # 인자
/// - `session_key`: 세션 캐시 격리 키. net_counters 델타를 세션(터미널)별로 분리한다.
///
/// # 반환
/// [`NetThroughput`] 또는 `None`(첫 렌더/카운터 조회 실패/dt<=0). 항상 무패닉(AC5).
fn sample_net(session_key: &str) -> Option<NetThroughput> {
    /// 네트워크 카운터 델타 캐시 파일명(`.../sessions/<key>/net_counters`).
    const NET_CACHE_FILE: &str = "net_counters";

    let (now_rx, now_tx) = sample_net_counters()?;
    let now_ms = crate::chain::cache_now_millis();

    // 직전 렌더 카운터를 읽는다(payload 포맷: "rx tx"). 다음 렌더를 위해 항상 현재값으로 갱신.
    // 세션 변형을 경유해 다른 세션의 prev가 이 세션 델타를 교란하지 않게 한다(§11.3).
    let prev = crate::chain::read_session_named_cache(session_key, NET_CACHE_FILE);
    crate::chain::write_session_named_cache(
        session_key,
        NET_CACHE_FILE,
        now_ms,
        &format!("{now_rx} {now_tx}"),
    );

    let (prev_ms, payload) = prev?;
    let (prev_rx, prev_tx) = parse_net_counters(&payload)?;
    // dt: 직전 기록 이후 경과(ms). 시계 역행은 0으로 → throughput이 None 처리.
    let dt_ms = (now_ms.saturating_sub(prev_ms)) as u64;

    let rx_bps = throughput(prev_rx, now_rx, dt_ms)?;
    let tx_bps = throughput(prev_tx, now_tx, dt_ms)?;
    Some(NetThroughput { rx_bps, tx_bps })
}

/// 캐시 payload("rx tx") 두 정수를 파싱하는 순수 헬퍼(테스트 가능).
///
/// # 반환
/// `(rx_bytes, tx_bytes)`. 토큰 부족/파싱 실패 시 `None`.
fn parse_net_counters(payload: &str) -> Option<(u64, u64)> {
    let mut parts = payload.split_whitespace();
    let rx = parts.next()?.parse::<u64>().ok()?;
    let tx = parts.next()?.parse::<u64>().ok()?;
    Some((rx, tx))
}

// CONTRACT 해제(§11.3 버그 수정): net_counters 세션 격리를 위해 `session_key`를 추가한다.
/// 설정에 따라 시스템 전체 스냅샷을 한 번에 수집한다.
///
/// # 인자
/// - `cfg`: 샘플 윈도(`cpu.sample_window_ms`), 표시 토글(`display.show_battery/show_disk/show_network`) 등.
/// - `session_key`: 세션 캐시 격리 키(net_counters 델타에만 전달, battery는 전역 유지).
///
/// # 반환
/// CPU/메모리/배터리/디스크/네트워크를 채운 [`SystemSnapshot`]. 각 항목은 실패 시 안전 저하하며,
/// 표시 토글이 꺼져 있으면 해당 샘플링 작업 자체를 건너뛴다(불필요한 syscall 회피).
pub fn sample_system(cfg: &Config, session_key: &str) -> SystemSnapshot {
    SystemSnapshot {
        cpu_percent: sample_cpu_reactive(cfg.cpu.sample_window_ms),
        mem_percent: sample_memory(),
        // 배터리(P2, IOKit + 30s TTL 캐시). 토글 off면 샘플링 생략, 데스크톱/실패 시 None.
        battery: if cfg.display.show_battery {
            sample_battery()
        } else {
            None
        },
        // 디스크(P2, statfs). 토글 off면 생략, 실패 시 None.
        disk_percent: if cfg.display.show_disk {
            sample_disk()
        } else {
            None
        },
        // 네트워크(P2, getifaddrs 델타). 토글 off면 생략, 첫 렌더/실패 시 None.
        net: if cfg.display.show_network {
            sample_net(session_key)
        } else {
            None
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 틱 델타 산식: busy_delta=300, total_delta=1000 → 30.0% (AC3 핵심 케이스).
    #[test]
    fn cpu_percent_from_deltas_basic() {
        assert_eq!(cpu_percent_from_deltas(300, 1000), 30.0);
    }

    /// total_delta=0(시간 미경과)이면 0.0으로 안전 저하한다.
    #[test]
    fn cpu_percent_from_deltas_zero_total() {
        assert_eq!(cpu_percent_from_deltas(0, 0), 0.0);
        assert_eq!(cpu_percent_from_deltas(500, 0), 0.0);
    }

    /// busy_delta == total_delta → 100.0% (완전 포화).
    #[test]
    fn cpu_percent_from_deltas_full() {
        assert_eq!(cpu_percent_from_deltas(1000, 1000), 100.0);
    }

    /// busy가 total을 초과하는 비정상 입력도 100.0으로 클램프한다.
    #[test]
    fn cpu_percent_from_deltas_clamps_high() {
        assert_eq!(cpu_percent_from_deltas(1500, 1000), 100.0);
    }

    /// loadavg 폴백: load1=3.0, ncpu=12 → 25.0% (AC3).
    #[test]
    fn loadavg_to_percent_normal() {
        assert_eq!(loadavg_to_percent(3.0, 12), 25.0);
    }

    /// loadavg 폴백 클램프: load1=68.0, ncpu=12 → 567%가 아니라 100.0으로 클램프 (AC3).
    #[test]
    fn loadavg_to_percent_clamps() {
        assert_eq!(loadavg_to_percent(68.0, 12), 100.0);
    }

    /// ncpu=0(측정 불가)이면 0.0으로 안전 저하(0 나눗셈 방지).
    #[test]
    fn loadavg_to_percent_zero_ncpu() {
        assert_eq!(loadavg_to_percent(5.0, 0), 0.0);
    }

    /// CPU double-sample window는 기본/정상 값은 보존하고 비정상 큰 값만 렌더 예산 안으로 제한한다.
    #[test]
    fn cpu_sample_window_is_capped_for_hot_path() {
        assert_eq!(bounded_cpu_sample_window_ms(0), 0);
        assert_eq!(bounded_cpu_sample_window_ms(25), 25);
        assert_eq!(
            bounded_cpu_sample_window_ms(MAX_CPU_SAMPLE_WINDOW_MS),
            MAX_CPU_SAMPLE_WINDOW_MS
        );
        assert_eq!(
            bounded_cpu_sample_window_ms(MAX_CPU_SAMPLE_WINDOW_MS + 1),
            MAX_CPU_SAMPLE_WINDOW_MS
        );
        assert_eq!(
            bounded_cpu_sample_window_ms(u64::MAX),
            MAX_CPU_SAMPLE_WINDOW_MS
        );
    }

    /// `sample_cpu_reactive` 코어가 실제 sleep 직전에 cap을 적용해야 한다. helper 단독 테스트만으로는
    /// 호출부 배선 회귀를 못 잡으므로, 주입형 코어에서 관측된 sleep duration과 산출 CPU%를 함께 검증한다.
    #[test]
    fn sample_cpu_reactive_core_applies_capped_sleep() {
        let mut calls = 0;
        let mut sleeps = Vec::new();

        let cpu = sample_cpu_reactive_with(
            u64::MAX,
            || {
                calls += 1;
                match calls {
                    1 => Some(CpuTickTotals {
                        busy: 1_000,
                        total: 2_000,
                    }),
                    2 => Some(CpuTickTotals {
                        busy: 1_050,
                        total: 2_200,
                    }),
                    _ => panic!("CPU snapshot called too many times"),
                }
            },
            |duration| sleeps.push(duration),
            || panic!("fallback should not be used for successful snapshots"),
        );

        assert_eq!(calls, 2);
        assert_eq!(
            sleeps,
            vec![std::time::Duration::from_millis(MAX_CPU_SAMPLE_WINDOW_MS)]
        );
        assert_eq!(cpu, 25.0, "busy_delta=50,total_delta=200 → 25%");
    }

    /// 실측 더블샘플/메모리 경로가 항상 0..=100 범위를 지키는지 무패닉으로 확인한다.
    /// (FFI 산식 자체는 위 순수 함수 테스트가 검증하며, 여기서는 라이브 경로의
    /// 범위 불변식과 무패닉만 보장한다.)
    #[test]
    fn live_paths_stay_in_range_without_panic() {
        let cpu = sample_cpu_reactive(5);
        let mem = sample_memory();
        let load = sample_cpu_loadavg_fallback();
        assert!((0.0..=100.0).contains(&cpu), "cpu out of range: {cpu}");
        assert!((0.0..=100.0).contains(&mem), "mem out of range: {mem}");
        assert!(
            (0.0..=100.0).contains(&load),
            "loadavg out of range: {load}"
        );
    }

    // === P2 디스크 순수 함수 ===

    /// 디스크 사용률: blocks=100, bavail=25 → used=75 → 75% (계획서 §F P2 산식).
    #[test]
    fn disk_percent_basic() {
        assert_eq!(disk_percent_from_statfs(100, 30, 25), Some(75.0));
    }

    /// blocks=0(측정 불가)이면 None으로 안전 저하한다.
    #[test]
    fn disk_percent_zero_blocks_is_none() {
        assert_eq!(disk_percent_from_statfs(0, 0, 0), None);
    }

    /// 가득 찬 디스크(bavail=0) → 100%.
    #[test]
    fn disk_percent_full() {
        assert_eq!(disk_percent_from_statfs(1000, 0, 0), Some(100.0));
    }

    /// bavail이 blocks를 초과하는 비정상 입력은 0%로 saturating 방어(음수 사용률 금지).
    #[test]
    fn disk_percent_bavail_exceeds_blocks() {
        assert_eq!(disk_percent_from_statfs(100, 200, 200), Some(0.0));
    }

    // === P2 네트워크 throughput 순수 함수 ===

    /// rate: prev=1000, now=3048, dt=1000ms(1초) → (2048)/1 = 2048 bytes/sec.
    #[test]
    fn throughput_basic_rate() {
        assert_eq!(throughput(1000, 3048, 1000), Some(2048.0));
    }

    /// dt=500ms(0.5초)면 같은 델타라도 rate는 2배: 1024/0.5 = 2048.
    #[test]
    fn throughput_half_second_doubles_rate() {
        assert_eq!(throughput(0, 1024, 500), Some(2048.0));
    }

    /// dt<=0(0ms, 시계역행)이면 None(0 나눗셈 방어).
    #[test]
    fn throughput_zero_dt_is_none() {
        assert_eq!(throughput(0, 1000, 0), None);
    }

    /// 카운터 래핑/리셋(now < prev)은 saturating_sub로 0 델타 → rate 0(음수 금지).
    #[test]
    fn throughput_counter_wrap_saturates_to_zero() {
        assert_eq!(throughput(5000, 100, 1000), Some(0.0));
    }

    /// 캐시 payload "rx tx" 파싱.
    #[test]
    fn parse_net_counters_roundtrip() {
        assert_eq!(parse_net_counters("123 456"), Some((123, 456)));
        // 토큰 부족/형식 오류는 None.
        assert_eq!(parse_net_counters("123"), None);
        assert_eq!(parse_net_counters(""), None);
        assert_eq!(parse_net_counters("abc def"), None);
    }

    // === P2 배터리 캐시 파싱 + TTL 신선도 ===

    /// 배터리 캐시 payload "percent flag" 파싱: 충전(1)/비충전(0).
    #[test]
    fn parse_battery_cache_charging_flag() {
        let charging = parse_battery_cache("82.5 1").expect("파싱 성공");
        assert_eq!(charging.percent, 82.5);
        assert!(charging.is_charging);

        let not_charging = parse_battery_cache("40 0").expect("파싱 성공");
        assert_eq!(not_charging.percent, 40.0);
        assert!(!not_charging.is_charging);
    }

    /// 배터리 캐시 파싱은 잔량을 0..=100으로 클램프하고, 형식 오류는 None.
    #[test]
    fn parse_battery_cache_clamps_and_guards() {
        assert_eq!(parse_battery_cache("150 1").unwrap().percent, 100.0);
        assert_eq!(parse_battery_cache("not-a-number 1"), None);
        assert_eq!(parse_battery_cache(""), None);
    }

    /// 배터리 30s TTL 신선도: TTL 내면 fresh(캐시 사용, IOKit 재조회 없음), 초과면 stale.
    /// 신선도 게이트는 chain::is_named_cache_fresh로 재사용되므로 그 동작을 확인한다.
    #[test]
    fn battery_cache_freshness_gate() {
        // 기록 0ms, 현재 29초 → TTL 30초 내 → fresh(IOKit 스킵).
        assert!(crate::chain::is_named_cache_fresh(
            0,
            29_000,
            BATTERY_CACHE_TTL_SECONDS
        ));
        // 정확히 30초 경계는 fresh(<=).
        assert!(crate::chain::is_named_cache_fresh(
            0,
            30_000,
            BATTERY_CACHE_TTL_SECONDS
        ));
        // 31초는 stale(IOKit 재조회).
        assert!(!crate::chain::is_named_cache_fresh(
            0,
            31_000,
            BATTERY_CACHE_TTL_SECONDS
        ));
    }

    /// 라이브 디스크 경로는 항상 0..=100 범위거나 None이며 무패닉이어야 한다.
    #[test]
    fn live_disk_path_in_range_or_none() {
        if let Some(disk) = sample_disk() {
            assert!((0.0..=100.0).contains(&disk), "disk out of range: {disk}");
        }
    }

    /// 라이브 네트워크 카운터 조회는 무패닉이어야 한다(첫 호출은 카운터만 수집).
    /// (rate 산출은 캐시 델타에 의존하므로 단발 호출의 결과값은 단언하지 않는다.)
    #[test]
    fn live_net_counters_no_panic() {
        let _ = sample_net_counters();
    }

    /// net_counters 세션 독립성(§11.3): 한 세션 키의 prev가 다른 세션 키 델타에 영향을 주지
    /// 않아야 한다. `sample_net`이 경유하는 세션 변형(read/write_session_named_cache)을 직접
    /// 검증한다. 키마다 다른 prev를 써도 각 키 read가 자기 값만 돌려주면 델타가 교란되지 않는다.
    /// 충돌/오염 방지를 위해 프로세스 고유 키를 쓰고 끝나면 정리한다(HOME은 macOS 전용 보장).
    #[test]
    fn net_delta_session_independent() {
        // HOME 기반 세션 캐시를 write→read 라운드트립하지만, 프로세스 고유 키(pid 접미사)만 쓰고
        // 더는 어떤 테스트도 HOME을 swap하지 않으므로(codex 통합 테스트가 base 주입으로 전환됨)
        // 베이스 경로 교란이 없어 직렬화 락이 불필요하다.
        const NET_CACHE_FILE: &str = "net_counters";
        let pid = std::process::id();
        let key_a = format!("netindep-A-{pid}");
        let key_b = format!("netindep-B-{pid}");

        // 두 세션에 서로 다른 카운터(prev)를 기록한다.
        crate::chain::write_session_named_cache(&key_a, NET_CACHE_FILE, 1_000, "100 200");
        crate::chain::write_session_named_cache(&key_b, NET_CACHE_FILE, 1_000, "999 888");

        // 각 세션 read가 자기 값만 돌려줘야 한다(상호 오염 없음).
        let a = crate::chain::read_session_named_cache(&key_a, NET_CACHE_FILE);
        let b = crate::chain::read_session_named_cache(&key_b, NET_CACHE_FILE);
        assert_eq!(a.as_ref().map(|(_, p)| p.as_str()), Some("100 200"));
        assert_eq!(b.as_ref().map(|(_, p)| p.as_str()), Some("999 888"));

        // 정리: 세션 디렉터리 제거(런타임 GC 없음 → 테스트가 직접 청소).
        if let Some(home) = std::env::var_os("HOME") {
            let root = std::path::PathBuf::from(home)
                .join("Library")
                .join("Caches")
                .join("understatus")
                .join("sessions");
            let _ = std::fs::remove_dir_all(root.join(&key_a));
            let _ = std::fs::remove_dir_all(root.join(&key_b));
        }
    }
}
