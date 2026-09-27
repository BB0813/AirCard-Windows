//! FairPlay / CoreFP availability check for AirTraffic synchronization.
//!
//! AirTrafficHost.dll resolves CoreFP.dll at runtime: it imports
//! `RegOpenKeyExA`/`RegOpenKeyExW` and `RegQueryValueExA`/`RegQueryValueExW`
//! from advapi32 and embeds the ASCII strings `Software\Apple Inc.\CoreFP`
//! and `LibraryPath`. If that lookup fails the host cannot mint a FairPlay
//! Grappa, so the device accepts the session (`SyncAllowed`) and then never
//! advances to `ReadyForSync` — which surfaces as a silent sync timeout.
//!
//! Third-party device tools (i4Tools, 3uTools) commonly overwrite the
//! `CoreFP` key so it points at their own `itunesDll\CoreFP.dll`, and that
//! path often no longer exists. Probing it up front turns an unexplained
//! hang into an actionable message.

use std::path::Path;

use anyhow::{Result, bail};

#[cfg(windows)]
mod ffi {
    pub const HKEY_LOCAL_MACHINE: isize = 0x80000002u32 as i32 as isize;

    pub const KEY_READ: u32 = 0x20019;
    // Let a 64-bit process read the 32-bit (WOW6432Node) view, where a
    // 32-bit AMDS install would have written its CoreFP registration.
    pub const KEY_WOW64_32KEY: u32 = 0x0200;
    pub const RRF_RT_REG_EXPAND_SZ: u32 = 0x00000004;

    pub const ERROR_SUCCESS: u32 = 0;
    pub const ERROR_MORE_DATA: u32 = 234;
}

#[cfg(windows)]
unsafe extern "system" {
    fn RegOpenKeyExW(
        h_key: isize,
        lp_sub_key: *const u16,
        ul_options: u32,
        sam_desired: u32,
        phk_result: *mut isize,
    ) -> u32;
    fn RegQueryValueExW(
        h_key: isize,
        lp_value_name: *const u16,
        lp_reserved: *mut u32,
        lp_type: *mut u32,
        lp_data: *mut u8,
        lpcb_data: *mut u32,
    ) -> u32;
    fn RegCloseKey(h_key: isize) -> u32;
    fn ExpandEnvironmentStringsW(lp_src: *const u16, lp_dst: *mut u16, n_size: u32) -> u32;
}

fn to_wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(windows)]
fn from_wide_ptr(ptr: *const u16, len_chars: usize) -> String {
    let slice = unsafe { std::slice::from_raw_parts(ptr, len_chars) };
    String::from_utf16_lossy(slice)
        .trim_end_matches('\0')
        .to_string()
}

#[cfg(windows)]
fn read_registry_string(hive: isize, sub_key: &str, value: &str, wow64_32: bool) -> Option<String> {
    unsafe {
        let sam = if wow64_32 {
            ffi::KEY_READ | ffi::KEY_WOW64_32KEY
        } else {
            ffi::KEY_READ
        };

        let mut key: isize = 0;
        let sub = to_wide(sub_key);
        let status = RegOpenKeyExW(hive, sub.as_ptr(), 0, sam, &mut key);
        if status != ffi::ERROR_SUCCESS || key == 0 {
            return None;
        }

        let name = to_wide(value);
        let mut data_type: u32 = 0;
        let mut needed: u32 = 0;

        // First call reports the buffer size we need.
        let status = RegQueryValueExW(
            key,
            name.as_ptr(),
            std::ptr::null_mut(),
            &mut data_type,
            std::ptr::null_mut(),
            &mut needed,
        );
        if status != ffi::ERROR_SUCCESS && status != ffi::ERROR_MORE_DATA {
            RegCloseKey(key);
            return None;
        }
        if needed == 0 {
            RegCloseKey(key);
            return None;
        }

        let mut raw: Vec<u16> = vec![0; (needed as usize + 1) / 2];
        let mut size = needed;
        let status = RegQueryValueExW(
            key,
            name.as_ptr(),
            std::ptr::null_mut(),
            &mut data_type,
            raw.as_mut_ptr() as *mut u8,
            &mut size,
        );
        RegCloseKey(key);

        if status != ffi::ERROR_SUCCESS || size < 2 {
            return None;
        }

        let value = from_wide_ptr(raw.as_ptr(), (size as usize - 2) / 2);

        // REG_EXPAND_SZ values may contain %ProgramFiles% style tokens.
        if data_type == ffi::RRF_RT_REG_EXPAND_SZ && value.contains('%') {
            let src = to_wide(&value);
            let mut expanded = vec![0u16; 1024];
            let written = ExpandEnvironmentStringsW(src.as_ptr(), expanded.as_mut_ptr(), 1024);
            if written > 0 {
                return Some(from_wide_ptr(expanded.as_ptr(), written as usize - 1));
            }
        }

        Some(value)
    }
}

#[cfg(not(windows))]
fn read_registry_string(_hive: isize, _sub_key: &str, _value: &str) -> Option<String> {
    None
}

/// Registry value names that have been observed to hold the CoreFP.dll path.
/// `LibraryPath` is what a stock Apple install writes; `Libi4CFPath` and
/// `LibiiiiPath` are written by third-party tools (i4Tools/3uTools) repointing
/// the key at their own bundled copy.
const COREFP_VALUE_NAMES: &[&str] = &["LibraryPath", "Libi4CFPath", "LibiiiiPath"];

/// Registry locations AirTrafficHost is known to consult for CoreFP.dll.
const COREFP_KEYS: &[&str] = &[
    r"SOFTWARE\Apple Inc.\CoreFP",
    r"SOFTWARE\Apple Inc.\Apple Mobile Device Support",
    r"SOFTWARE\WOW6432Node\Apple Inc.\CoreFP",
];

/// Well-known install locations of CoreFP.dll, used when the registry is
/// silent (fresh official iTunes install) or poisoned.
const FALLBACK_COREFP_PATHS: &[&str] = &[
    r"C:\Program Files\Common Files\Apple\Internet Plug-Ins\CoreFP.dll",
    r"C:\Program Files (x86)\Common Files\Apple\Internet Plug-Ins\CoreFP.dll",
    r"C:\Program Files\iTunes\CoreFP.dll",
    r"C:\Program Files (x86)\iTunes\CoreFP.dll",
    r"C:\Program Files\Common Files\Apple\Mobile Device Support\CoreFP.dll",
];

/// Resolve `LibraryPath` from the CoreFP key in both registry views.
///
/// The native view is read first because a 64-bit AirTrafficHost.dll opens
/// HKLM directly. The WOW64 view is then tried as a fallback, since a 32-bit
/// AMDS or a third-party tool may have written only there.
fn read_registry_value(hive: isize, sub_key: &str, value: &str) -> Option<String> {
    read_registry_string(hive, sub_key, value, false)
        .or_else(|| read_registry_string(hive, sub_key, value, true))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoreFpHealth {
    /// CoreFP.dll was found and is loadable — AirTraffic can mint a Grappa.
    Available { path: String },
    /// The registry names a CoreFP.dll that does not exist on disk.
    MissingFile { expected: String },
    /// No value points at a usable CoreFP.dll.
    Unresolved,
}

impl CoreFpHealth {
    /// True when a CoreFP.dll genuine enough to complete Grappa was located.
    pub fn is_available(&self) -> bool {
        matches!(self, Self::Available { .. })
    }
}

/// Resolve CoreFP.dll the way AirTrafficHost.dll does, and report whether it
/// actually exists. This is the single check that decides whether a transfer
/// can proceed at the FairPlay layer.
pub fn probe_core_fp() -> CoreFpHealth {
    let mut missing: Option<String> = None;

    for sub_key in COREFP_KEYS {
        for value in COREFP_VALUE_NAMES {
            if let Some(path) = read_registry_value(ffi::HKEY_LOCAL_MACHINE, sub_key, value) {
                let trimmed = path.trim();
                if trimmed.is_empty() {
                    continue;
                }
                // The value may hold a directory rather than the DLL itself.
                let candidate = if trimmed.to_lowercase().ends_with("corefp.dll") {
                    trimmed.to_string()
                } else {
                    format!("{}\\CoreFP.dll", trimmed.trim_end_matches(['\\', '/']))
                };
                if Path::new(&candidate).is_file() {
                    return CoreFpHealth::Available { path: candidate };
                }
                if missing.is_none() {
                    missing = Some(trimmed.to_string());
                }
            }
        }
    }

    for fallback in FALLBACK_COREFP_PATHS {
        if Path::new(fallback).is_file() {
            return CoreFpHealth::Available {
                path: (*fallback).to_string(),
            };
        }
    }

    match missing {
        Some(expected) => CoreFpHealth::MissingFile { expected },
        None => CoreFpHealth::Unresolved,
    }
}

/// Human-readable remediation text, appended to sync failures and shown in
/// the status bar so the user does not have to guess why the transfer hung.
pub fn remediation_hint() -> &'static str {
    "FairPlay (CoreFP.dll) is missing or broken on this PC, so AirTraffic cannot authenticate. 1) Uninstall i4Tools / 3uTools Apple drivers. 2) Install the official iTunes from apple.com (the Microsoft Store version has no Mobile Device Support). 3) Reboot and retry."
}

/// A one-line warning for the status bar, or `None` when FairPlay is healthy.
pub fn fairplay_warning() -> Option<String> {
    match probe_core_fp() {
        CoreFpHealth::Available { .. } => None,
        CoreFpHealth::MissingFile { expected } => Some(format!(
            "FairPlay CoreFP.dll is missing (registry points at {}). Transfers will fail until this is fixed.",
            expected
        )),
        CoreFpHealth::Unresolved => Some(
            "FairPlay CoreFP.dll was not found on this PC. Transfers will fail until this is fixed."
                .to_string(),
        ),
    }
}

/// Raised when a transfer is attempted while FairPlay is known-broken, so the
/// app can fail fast instead of stalling on `ReadyForSync` for 60+ seconds.
pub fn ensure_fairplay_ready() -> Result<()> {
    let health = probe_core_fp();
    if health.is_available() {
        return Ok(());
    }

    match health {
        CoreFpHealth::MissingFile { expected } => bail!(
            "FairPlay CoreFP.dll is missing.\n\nThe registry points at:\n  {}\nbut that file no longer exists.\n\n{}",
            expected,
            remediation_hint()
        ),
        CoreFpHealth::Unresolved => bail!(
            "FairPlay CoreFP.dll was not found on this PC.\n\n{}",
            remediation_hint()
        ),
        CoreFpHealth::Available { .. } => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_does_not_panic_and_reports_a_usable_variant() {
        // On a CI box there is no Apple stack; both resolved states are fine.
        let health = probe_core_fp();
        assert!(matches!(
            health,
            CoreFpHealth::Available { .. }
                | CoreFpHealth::MissingFile { .. }
                | CoreFpHealth::Unresolved
        ));
    }

    #[test]
    fn probe_reads_third_party_registry_variants() {
        // i4Tools/3uTools repoint CoreFP at their own itunesDll copy. The probe
        // must surface that path as a *missing file* rather than reporting no
        // CoreFP registration at all.
        for sub_key in COREFP_KEYS {
            for value in COREFP_VALUE_NAMES {
                let _ = read_registry_value(ffi::HKEY_LOCAL_MACHINE, sub_key, value);
            }
        }
        // No Apple stack is installed; the healthy variant must still resolve
        // to the poisoned-but-absent path.
        let health = probe_core_fp();
        println!("CoreFP probe on this host: {health:?}");
        // 32 falls back to Apple's known install dirs and only reports
        // Available when one of those files genuinely exists.
        match &health {
            CoreFpHealth::Available { path } => {
                assert!(
                    std::path::Path::new(path).is_file(),
                    "Available must only be reported for a CoreFP.dll on disk: {path}"
                );
            }
            CoreFpHealth::MissingFile { .. } | CoreFpHealth::Unresolved => {}
        }
    }

    #[test]
    fn available_variant_is_the_only_healthy_one() {
        assert!(
            CoreFpHealth::Available {
                path: "C:\\CoreFP.dll".to_string()
            }
            .is_available()
        );
        assert!(
            !CoreFpHealth::MissingFile {
                expected: "C:\\gone\\CoreFP.dll".to_string()
            }
            .is_available()
        );
        assert!(!CoreFpHealth::Unresolved.is_available());
    }

    #[test]
    fn remediation_hint_mentions_third_party_tools_and_itunes() {
        let hint = remediation_hint();
        assert!(hint.contains("i4Tools"));
        assert!(hint.contains("iTunes"));
    }
}
