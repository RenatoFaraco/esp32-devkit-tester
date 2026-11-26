//! Bench tool: reads every ROM fact of the board on one port, then hard-resets it.
//!
//! ```powershell
//! cargo run --manifest-path host/Cargo.toml --example probe_port -- COM6
//! ```
//!
//! Read-only (ADR-009): only the volatile writes listed there happen, and the board goes back to
//! its application at the end. The port name is only used to find the port in the OS list; the
//! facts are printed with `{:?}`, so nothing the board sends reaches the terminal raw. Errors are
//! printed, never panicked on.

use std::process::ExitCode;
use std::time::Instant;

use esp32_devkit_tester::chips::ChipIdentity;
use esp32_devkit_tester::probe::{EspflashProbe, Probe, ProbeError};
use esp32_devkit_tester::transport::{self, Mac, PortDesc};

fn main() -> ExitCode {
    let Some(name) = std::env::args().nth(1) else {
        eprintln!("usage: probe_port <port>   (e.g. COM6, /dev/ttyACM0)");
        return ExitCode::from(2);
    };
    let ports = match transport::enumerate() {
        Ok(ports) => ports,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::from(2);
        }
    };
    let Some(port) = find(&ports, &name) else {
        eprintln!("port {name:?} not found; ports seen by the OS:");
        for port in &ports {
            eprintln!("  {:?} {:?}", port.name, transport::classify(port));
        }
        return ExitCode::from(2);
    };
    println!("port: {port:?}");
    println!("class: {:?}", transport::classify(port));

    let mut probe = EspflashProbe::new();
    let ok = probe_all(&mut probe, port);
    let reset = step("hard_reset", || probe.hard_reset());
    if ok && reset.is_some() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn find<'a>(ports: &'a [PortDesc], name: &str) -> Option<&'a PortDesc> {
    ports.iter().find(|port| port.name == name).or_else(|| {
        ports
            .iter()
            .find(|port| port.name.eq_ignore_ascii_case(name))
    })
}

/// Runs one step, printing its value (or error) and duration.
fn step<T: std::fmt::Debug>(label: &str, f: impl FnOnce() -> Result<T, ProbeError>) -> Option<T> {
    let start = Instant::now();
    let result = f();
    let ms = start.elapsed().as_millis();
    match result {
        Ok(value) => {
            println!("{label:<16} {ms:>5} ms  {value:?}");
            Some(value)
        }
        Err(error) => {
            println!("{label:<16} {ms:>5} ms  ERROR {error:?} ({error})");
            None
        }
    }
}

/// Every fact of the `Probe` trait; false if any step failed.
fn probe_all(probe: &mut EspflashProbe, port: &PortDesc) -> bool {
    if step("connect", || probe.connect(port)).is_none() {
        return false;
    }
    let mut ok = true;
    let chip = step("chip", || probe.chip());
    ok &= chip.is_some();
    ok &= step("identity", || probe.identity()).is_some();
    ok &= step("revision", || probe.revision()).is_some();
    ok &= step("crystal_mhz", || probe.crystal_mhz()).is_some();
    let mac = step("mac", || probe.mac());
    ok &= mac.is_some();
    if let Some(chip) = chip {
        let reg = chip.caps().mac_efuse_reg.value;
        let lo = step("read_reg mac lo", || probe.read_reg(reg));
        let hi = step("read_reg mac hi", || probe.read_reg(reg + 4));
        if let (Some(lo), Some(hi)) = (lo, hi) {
            let efuse = Mac::from_efuse_words(lo, hi);
            let same = mac == Some(efuse);
            println!(
                "{:<16}          {efuse} (same as ROM MAC: {same})",
                "mac via eFuse"
            );
            ok &= same;
        } else {
            ok = false;
        }
        let table = chip.caps().identity.value;
        if let ChipIdentity::MagicReg { addr, .. } = table {
            ok &= step("read_reg magic", || probe.read_reg(addr)).is_some();
        }
        println!("{:<16}          table says {table}", "identity");
    }
    ok &= step("package_version", || probe.package_version()).is_some();
    ok &= step("embedded_flash", || probe.embedded_flash()).is_some();
    ok &= step("embedded_psram", || probe.embedded_psram()).is_some();
    if let Some(id) = step("flash_id", || probe.flash_id()) {
        println!(
            "{:<16}          {id} = {:?} bytes (blank bus: {})",
            "flash size",
            id.size_bytes(),
            id.is_blank()
        );
    } else {
        ok = false;
    }
    ok
}
