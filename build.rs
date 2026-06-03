//! 빌드 스크립트: 배터리 샘플링용 IOKit/CoreFoundation 프레임워크 링크.
//!
//! P2 배터리는 in-process IOKit `IOPSCopyPowerSourcesInfo` FFI를 사용한다(계획서 §E/§F P2).
//! 이 심볼들은 IOKit/CoreFoundation 프레임워크에 있으므로, macOS 표준 방식대로
//! `cargo:rustc-link-lib=framework=...`로 링크해야 한다(우회가 아닌 정석).

fn main() {
    // macOS에서만 프레임워크를 링크한다(타 플랫폼에서는 배터리 FFI를 컴파일하지 않음).
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        // IOKit: IOPSCopyPowerSourcesInfo / IOPSCopyPowerSourcesList / IOPSGetPowerSourceDescription.
        println!("cargo:rustc-link-lib=framework=IOKit");
        // CoreFoundation: CFArray/CFDictionary/CFNumber/CFBoolean/CFString 접근 + CFRelease.
        println!("cargo:rustc-link-lib=framework=CoreFoundation");
    }
}
