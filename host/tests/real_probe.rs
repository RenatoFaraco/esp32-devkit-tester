//! Reads every fact through a real board (bench only, never in CI).
//!
//! ```powershell
//! $env:DKT_PORT = 'COM6'
//! cargo test --manifest-path host/Cargo.toml --locked --test real_probe -- --ignored --nocapture
//! ```
//!
//! Read-only (ADR-009); the board is hard-reset back to its application at the end, also when
//! a step fails.

use esp32_devkit_tester::chips::ChipIdentity;
use esp32_devkit_tester::probe::{EspflashProbe, Probe};
use esp32_devkit_tester::transport::{self, Mac};

#[test]
#[ignore = "needs a board; set DKT_PORT and run by hand"]
fn real_probe_reads_every_fact() {
    let name = std::env::var("DKT_PORT").expect("set DKT_PORT to the port of the board");
    let ports = transport::enumerate().expect("list serial ports");
    let port = ports
        .iter()
        .find(|port| port.name.eq_ignore_ascii_case(&name))
        .unwrap_or_else(|| panic!("{name:?} not among {ports:?}"));

    let mut probe = EspflashProbe::new();
    probe.connect(port).expect("connect");
    // Every read happens before any assertion, so the hard reset always runs.
    let chip = probe.chip();
    let identity = probe.identity();
    let revision = probe.revision();
    let crystal = probe.crystal_mhz();
    let mac = probe.mac();
    let efuse_words = chip.as_ref().ok().map(|chip| {
        let reg = chip.caps().mac_efuse_reg.value;
        (probe.read_reg(reg), probe.read_reg(reg + 4))
    });
    let package = probe.package_version();
    let flash = probe.embedded_flash();
    let psram = probe.embedded_psram();
    let flash_id = probe.flash_id();
    let reset = probe.hard_reset();
    println!(
        "{chip:?}\n{identity:?}\n{revision:?}\n{crystal:?} MHz\n{mac:?}\npackage {package:?}\n\
         flash {flash:?}\npsram {psram:?}\n{flash_id:?}\nhard reset {reset:?}"
    );

    let caps = chip.expect("chip").caps();
    let identity = identity.expect("identity");
    assert_eq!(
        identity, caps.identity.value,
        "identity vs capability table"
    );
    if let ChipIdentity::MagicReg { .. } = identity {
        println!("identified by the magic register");
    }
    revision.expect("revision");
    crystal.expect("crystal");
    let mac = mac.expect("mac");
    let (lo, hi) = efuse_words.expect("eFuse words");
    assert_eq!(
        Mac::from_efuse_words(lo.expect("mac lo"), hi.expect("mac hi")),
        mac,
        "ROM MAC vs eFuse words"
    );
    package.expect("package version");
    flash.expect("embedded flash");
    psram.expect("embedded psram");
    let flash_id = flash_id.expect("flash id");
    assert!(!flash_id.is_blank(), "blank flash bus: {flash_id}");
    reset.expect("hard reset");
}
