//! Operating-system differences (ADR-007).
//!
//! This is the only module allowed to use `#[cfg(windows)]`, `#[cfg(unix)]` or other
//! OS-specific APIs (e.g. WSL detection). Everything else stays portable. It must not import
//! `probe`, `runner`, `report`, `ui`, `ratatui` or `crossterm`.
//!
//! # Why WSL needs its own detection
//!
//! From the program's point of view WSL looks like Linux (`target_os = "linux"`), but the two
//! WSL generations are very different systems underneath:
//!
//! - **WSL1** has no Linux kernel at all. A Windows kernel driver (`lxcore.sys`) translates
//!   Linux syscalls into NT calls. There are no real USB devices nor `ttyACM`/`ttyUSB` nodes, so
//!   the board cannot be reached: **not supported**.
//! - **WSL2** runs a real Linux kernel inside a lightweight Hyper-V VM. USB devices only appear
//!   after being forwarded from Windows with `usbipd-win` (phase 2 of ADR-007), and the
//!   USB-Serial/JTAG reset drops that forwarding.
//!
//! The kernel release string tells them apart, e.g. WSL1 `4.4.0-19041-Microsoft` and WSL2
//! `5.15.153.1-microsoft-standard-WSL2`. Sources:
//! <https://github.com/microsoft/WSL/issues/423> and
//! <https://github.com/microsoft/WSL2-Linux-Kernel>.
//!
//! ADR-007 mentions `/proc/version`; we read `/proc/sys/kernel/osrelease` instead because it
//! holds only the release string (`/proc/version` also carries the compiler and build host).
//! The information used for the decision is the same.

/// Generation of the Windows Subsystem for Linux.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wsl {
    /// Syscall translation layer over the NT kernel; no real USB/serial devices. Not supported.
    V1,
    /// Real Linux kernel in a Hyper-V VM; USB via `usbipd-win` (ADR-007 phase 2).
    V2,
}

/// Operating system the host is running on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    /// Native Windows: reference platform until v1.0 (ADR-007 phase 1).
    Windows,
    /// Native Linux (not WSL): ADR-007 phase 3.
    Linux,
    /// Linux userland running under WSL on a Windows machine.
    Wsl(Wsl),
    /// macOS: ADR-007 phase 3.
    MacOs,
    /// Any other target; treated as unsupported but must not crash.
    Other,
}

/// Classifies a Linux kernel release string (`/proc/sys/kernel/osrelease`, `uname -r`).
///
/// Returns `None` for a non-WSL kernel. A release containing `microsoft` is WSL; it is WSL2 if
/// it also contains `wsl2` or `microsoft-standard` (Microsoft's WSL2 kernel flavour, kept by
/// custom builds), otherwise WSL1. Comparison is case-insensitive and the input is trimmed.
pub fn classify_kernel_release(release: &str) -> Option<Wsl> {
    let release = release.trim().to_ascii_lowercase();
    if !release.contains("microsoft") {
        return None;
    }
    if release.contains("wsl2") || release.contains("microsoft-standard") {
        Some(Wsl::V2)
    } else {
        Some(Wsl::V1)
    }
}

/// Detects the operating system the host is running on. Never panics.
///
/// On Linux, a failure to read `/proc/sys/kernel/osrelease` falls back to [`Platform::Linux`].
pub fn detect() -> Platform {
    detect_os()
}

#[cfg(target_os = "linux")]
fn detect_os() -> Platform {
    match std::fs::read_to_string("/proc/sys/kernel/osrelease") {
        Ok(release) => match classify_kernel_release(&release) {
            Some(wsl) => Platform::Wsl(wsl),
            None => Platform::Linux,
        },
        Err(_) => Platform::Linux,
    }
}

#[cfg(target_os = "windows")]
fn detect_os() -> Platform {
    Platform::Windows
}

#[cfg(target_os = "macos")]
fn detect_os() -> Platform {
    Platform::MacOs
}

#[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
fn detect_os() -> Platform {
    Platform::Other
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wsl1_release_is_v1() {
        assert_eq!(
            classify_kernel_release("4.4.0-19041-Microsoft"),
            Some(Wsl::V1)
        );
    }

    #[test]
    fn wsl2_release_is_v2() {
        assert_eq!(
            classify_kernel_release("5.15.153.1-microsoft-standard-WSL2"),
            Some(Wsl::V2)
        );
    }

    #[test]
    fn native_ubuntu_release_is_not_wsl() {
        assert_eq!(classify_kernel_release("6.8.0-49-generic"), None);
    }

    #[test]
    fn custom_wsl2_kernel_is_v2() {
        assert_eq!(
            classify_kernel_release("6.6.36.3-microsoft-standard-WSL2+"),
            Some(Wsl::V2)
        );
        assert_eq!(
            classify_kernel_release("5.10.16.3-microsoft-standard"),
            Some(Wsl::V2)
        );
    }

    #[test]
    fn empty_or_blank_release_is_not_wsl() {
        assert_eq!(classify_kernel_release(""), None);
        assert_eq!(classify_kernel_release("  \n"), None);
    }

    #[test]
    fn surrounding_whitespace_and_newline_are_ignored() {
        // /proc/sys/kernel/osrelease ends with a newline.
        assert_eq!(
            classify_kernel_release("  5.15.153.1-microsoft-standard-WSL2\n"),
            Some(Wsl::V2)
        );
        assert_eq!(
            classify_kernel_release("4.4.0-19041-Microsoft\n"),
            Some(Wsl::V1)
        );
    }

    #[test]
    fn detect_does_not_panic_and_matches_build_target() {
        let platform = detect();
        if cfg!(target_os = "windows") {
            assert_eq!(platform, Platform::Windows);
        } else if cfg!(target_os = "macos") {
            assert_eq!(platform, Platform::MacOs);
        } else if cfg!(target_os = "linux") {
            assert!(matches!(platform, Platform::Linux | Platform::Wsl(_)));
        }
    }
}
