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

/// Why the host refuses to start under WSL1 (ADR-007), printed on stderr with exit code 2.
pub const WSL1_UNSUPPORTED: &str = "WSL1 is not supported: it has no USB/serial devices. Use Windows natively, or WSL2 with usbipd-win.";

/// Whether the host may run on `platform` at all, decided before any port is listed or opened.
///
/// Pure, so the refusal is tested without WSL. Only WSL1 is refused: it has no USB or serial
/// devices, so every port would look absent and the user would get "no board" instead of the
/// real cause. [`Platform::Other`] is allowed: `serialport` may still work there.
pub fn startup_check(platform: Platform) -> Result<(), String> {
    match platform {
        Platform::Wsl(Wsl::V1) => Err(WSL1_UNSUPPORTED.to_owned()),
        Platform::Windows | Platform::Linux | Platform::Wsl(Wsl::V2) | Platform::MacOs => Ok(()),
        Platform::Other => Ok(()),
    }
}

/// Whether a terminal on this OS shows UTF-8 text (the report's `✓ ✕ ○ ─ └` glyphs) as such.
/// The caller asks only for a stream that *is* a terminal; a pipe or file always gets ASCII.
///
/// - **Windows:** yes. When stdout/stderr is a console, Rust's standard library writes it with
///   `WriteConsoleW` (UTF-16), so the console code page does not matter: Windows Terminal, the
///   VS Code terminal and the classic console all receive the right characters. (Redirected
///   output is raw UTF-8 bytes that Windows PowerShell 5.1 decodes with the OEM code page, hence
///   ASCII there.)
/// - **Elsewhere:** the locale decides, as for any POSIX program ([`locale_is_utf8`] with
///   `LC_ALL`, `LC_CTYPE` and `LANG`).
pub fn terminal_is_utf8() -> bool {
    terminal_is_utf8_os()
}

#[cfg(windows)]
fn terminal_is_utf8_os() -> bool {
    true
}

#[cfg(not(windows))]
fn terminal_is_utf8_os() -> bool {
    let var = |name: &str| std::env::var(name).ok();
    locale_is_utf8(
        var("LC_ALL").as_deref(),
        var("LC_CTYPE").as_deref(),
        var("LANG").as_deref(),
    )
}

/// The POSIX rule for the character set: the first of `LC_ALL`, `LC_CTYPE`, `LANG` that is set
/// and not empty wins, and it is UTF-8 when its codeset says so (`en_US.UTF-8`, `C.utf8`). No
/// locale at all is the `C` locale: ASCII. Pure, so it is tested on every OS.
pub fn locale_is_utf8(lc_all: Option<&str>, lc_ctype: Option<&str>, lang: Option<&str>) -> bool {
    [lc_all, lc_ctype, lang]
        .into_iter()
        .flatten()
        .find(|value| !value.is_empty())
        .is_some_and(|locale| {
            let locale = locale.to_ascii_lowercase();
            locale.contains("utf-8") || locale.contains("utf8")
        })
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

    #[test]
    fn wsl1_is_refused_before_any_port_is_opened() {
        // The refusal is decided from the platform alone; `ui::cli` checks it before listing
        // ports (its own test proves no port is touched).
        assert_eq!(
            startup_check(Platform::Wsl(Wsl::V1)),
            Err(WSL1_UNSUPPORTED.to_owned())
        );
        assert!(WSL1_UNSUPPORTED.starts_with("WSL1 is not supported: it has no USB/serial"));
        assert!(WSL1_UNSUPPORTED.ends_with("or WSL2 with usbipd-win."));
        assert!(!WSL1_UNSUPPORTED.contains("  "), "{WSL1_UNSUPPORTED}");
        for platform in [
            Platform::Windows,
            Platform::Linux,
            Platform::Wsl(Wsl::V2),
            Platform::MacOs,
            Platform::Other,
        ] {
            assert_eq!(startup_check(platform), Ok(()), "{platform:?}");
        }
    }

    #[test]
    fn locale_decides_utf8_outside_windows() {
        assert!(locale_is_utf8(None, None, Some("en_US.UTF-8")));
        assert!(locale_is_utf8(None, Some("C.utf8"), Some("C")));
        // LC_ALL wins over the others; an empty value is skipped.
        assert!(!locale_is_utf8(Some("C"), Some("en_US.UTF-8"), None));
        assert!(locale_is_utf8(Some(""), None, Some("pt_BR.UTF-8")));
        assert!(!locale_is_utf8(None, None, Some("pt_BR.ISO-8859-1")));
        assert!(!locale_is_utf8(None, None, None));
    }
}
