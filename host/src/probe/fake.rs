//! `FakeProbe`: replays the facts recorded in `host/tests/fixtures/<chip>.json` (ADR-011).
//!
//! Test builds only. It behaves like a board seen through the ROM: reads before
//! [`Probe::connect`] fail with [`ProbeError::NotConnected`], [`Probe::hard_reset`] ends the
//! session, and any step can be made to fail on demand ([`FakeProbe::fail_at`]). Every call is
//! recorded, in order, so runner tests can prove the order of steps and that the hard reset
//! always happens ([`FakeProbe::calls`]).
//!
//! Fixture facts (format version 1):
//! - `chip.family`, `chip.revision`, `chip.crystal_mhz`, `chip.package_version`, `chip.mac`;
//! - `identity`: `{"magic_reg": "0x…", "value": "0x…"}` or `{"security_chip_id": n}`;
//! - `registers`: `[{"addr": "0x…", "value": "0x…", "source": "…"}]`, served by `read_reg`; an
//!   address not listed is [`ProbeError::Unsupported`];
//! - `flash.jedec_manufacturer` + `flash.jedec_device`: the JEDEC ID;
//! - `chip.embedded_flash` with `flash.efuse_capacity`/`flash.efuse_vendor`, and
//!   `chip.embedded_psram` with `psram.efuse_capacity`/`psram.efuse_vendor`. When the flag is
//!   true but capacity or vendor is missing, that part is `None` (only what the fixture asserts).

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use serde::Deserialize;

use super::{EmbeddedMem, FlashId, Probe, ProbeError, Revision};
use crate::chips::{Chip, ChipIdentity};
use crate::transport::{Mac, PortDesc};

/// The only fixture format this fake understands.
const FORMAT_VERSION: u32 = 1;

/// One step of the [`Probe`] trait: one value per method.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Step {
    /// [`Probe::connect`].
    Connect,
    /// [`Probe::chip`].
    Chip,
    /// [`Probe::identity`].
    Identity,
    /// [`Probe::revision`].
    Revision,
    /// [`Probe::crystal_mhz`].
    CrystalMhz,
    /// [`Probe::mac`].
    Mac,
    /// [`Probe::read_reg`].
    ReadReg,
    /// [`Probe::package_version`].
    PackageVersion,
    /// [`Probe::embedded_flash`].
    EmbeddedFlash,
    /// [`Probe::embedded_psram`].
    EmbeddedPsram,
    /// [`Probe::flash_id`].
    FlashId,
    /// [`Probe::hard_reset`].
    HardReset,
}

/// A [`Probe`] that replays one recorded board.
#[derive(Debug, Clone)]
pub struct FakeProbe {
    chip: Chip,
    identity: ChipIdentity,
    revision: Revision,
    crystal_mhz: u32,
    mac: Mac,
    package_version: u32,
    embedded_flash: Option<EmbeddedMem>,
    embedded_psram: Option<EmbeddedMem>,
    flash_id: FlashId,
    registers: HashMap<u32, u32>,
    failures: HashMap<Step, ProbeError>,
    connected: bool,
    calls: Vec<Step>,
}

impl FakeProbe {
    /// Loads `host/tests/fixtures/<name>.json` (`name` is the target, e.g. `"esp32c6"`).
    ///
    /// Refuses a `format_version` other than 1 and any fact it cannot decode.
    pub fn from_fixture(name: &str) -> Result<FakeProbe, String> {
        let path: PathBuf = [env!("CARGO_MANIFEST_DIR"), "tests", "fixtures"]
            .iter()
            .collect::<PathBuf>()
            .join(format!("{name}.json"));
        let text =
            fs::read_to_string(&path).map_err(|error| format!("{}: {error}", path.display()))?;
        FakeProbe::from_json(&text).map_err(|error| format!("{}: {error}", path.display()))
    }

    /// Builds the fake from the text of a fixture.
    pub fn from_json(text: &str) -> Result<FakeProbe, String> {
        let fixture: Fixture = serde_json::from_str(text).map_err(|error| error.to_string())?;
        if fixture.format_version != FORMAT_VERSION {
            return Err(format!(
                "unsupported fixture format_version {} (expected {FORMAT_VERSION})",
                fixture.format_version
            ));
        }
        let chip = Chip::from_target(&fixture.chip.family)
            .ok_or_else(|| format!("unknown chip family {:?}", fixture.chip.family))?;
        let identity = match fixture.identity {
            IdentityFx::MagicReg { magic_reg, value } => ChipIdentity::MagicReg {
                addr: parse_hex_u32(&magic_reg)?,
                value: parse_hex_u32(&value)?,
            },
            IdentityFx::SecurityChipId { security_chip_id } => {
                ChipIdentity::SecurityChipId(security_chip_id)
            }
        };
        let mut registers = HashMap::new();
        for register in &fixture.registers {
            let addr = parse_hex_u32(&register.addr)?;
            if registers
                .insert(addr, parse_hex_u32(&register.value)?)
                .is_some()
            {
                return Err(format!("register {addr:#010X} listed twice"));
            }
        }
        let mac = Mac::parse(&fixture.chip.mac).map_err(|error| error.to_string())?;
        let [memory_type, capacity] = parse_hex_bytes(&fixture.flash.jedec_device)?;
        let [manufacturer] = parse_hex_bytes(&fixture.flash.jedec_manufacturer)?;
        let embedded_flash = embedded(
            fixture.chip.embedded_flash,
            fixture.flash.efuse_capacity.as_deref(),
            fixture.flash.efuse_vendor.as_deref(),
        )?;
        let psram = fixture.psram.unwrap_or_default();
        let embedded_psram = embedded(
            fixture.chip.embedded_psram,
            psram.efuse_capacity.as_deref(),
            psram.efuse_vendor.as_deref(),
        )?;
        Ok(FakeProbe {
            chip,
            identity,
            revision: Revision {
                major: fixture.chip.revision.major,
                minor: fixture.chip.revision.minor,
            },
            crystal_mhz: fixture.chip.crystal_mhz,
            mac,
            package_version: fixture.chip.package_version,
            embedded_flash,
            embedded_psram,
            flash_id: FlashId {
                manufacturer,
                memory_type,
                capacity,
            },
            registers,
            failures: HashMap::new(),
            connected: false,
            calls: Vec::new(),
        })
    }

    /// Makes every call of `step` fail with `error` (timeout, disconnection, busy port...).
    /// Other steps keep working; call it again to fail more steps.
    pub fn fail_at(mut self, step: Step, error: ProbeError) -> FakeProbe {
        self.failures.insert(step, error);
        self
    }

    /// Reports `mac` from [`Probe::mac`] instead of the recorded one. The eFuse registers keep
    /// the recorded words, so the H5 cross-check sees the mismatch.
    pub fn with_mac(mut self, mac: Mac) -> FakeProbe {
        self.mac = mac;
        self
    }

    /// Every step called so far, in order (failed calls included).
    pub fn calls(&self) -> &[Step] {
        &self.calls
    }

    /// Records the call, then applies the injected failure and the connection check.
    fn enter(&mut self, step: Step) -> Result<(), ProbeError> {
        self.calls.push(step);
        if let Some(error) = self.failures.get(&step) {
            return Err(error.clone());
        }
        if step != Step::Connect && !self.connected {
            return Err(ProbeError::NotConnected);
        }
        Ok(())
    }
}

impl Probe for FakeProbe {
    fn connect(&mut self, _port: &PortDesc) -> Result<(), ProbeError> {
        self.enter(Step::Connect)?;
        self.connected = true;
        Ok(())
    }

    fn chip(&mut self) -> Result<Chip, ProbeError> {
        self.enter(Step::Chip)?;
        Ok(self.chip)
    }

    fn identity(&mut self) -> Result<ChipIdentity, ProbeError> {
        self.enter(Step::Identity)?;
        Ok(self.identity)
    }

    fn revision(&mut self) -> Result<Revision, ProbeError> {
        self.enter(Step::Revision)?;
        Ok(self.revision)
    }

    fn crystal_mhz(&mut self) -> Result<u32, ProbeError> {
        self.enter(Step::CrystalMhz)?;
        Ok(self.crystal_mhz)
    }

    fn mac(&mut self) -> Result<Mac, ProbeError> {
        self.enter(Step::Mac)?;
        Ok(self.mac)
    }

    fn read_reg(&mut self, addr: u32) -> Result<u32, ProbeError> {
        self.enter(Step::ReadReg)?;
        self.registers.get(&addr).copied().ok_or_else(|| {
            ProbeError::Unsupported(format!("register {addr:#010X} not in the fixture"))
        })
    }

    fn package_version(&mut self) -> Result<u32, ProbeError> {
        self.enter(Step::PackageVersion)?;
        Ok(self.package_version)
    }

    fn embedded_flash(&mut self) -> Result<Option<EmbeddedMem>, ProbeError> {
        self.enter(Step::EmbeddedFlash)?;
        Ok(self.embedded_flash.clone())
    }

    fn embedded_psram(&mut self) -> Result<Option<EmbeddedMem>, ProbeError> {
        self.enter(Step::EmbeddedPsram)?;
        Ok(self.embedded_psram.clone())
    }

    fn flash_id(&mut self) -> Result<FlashId, ProbeError> {
        self.enter(Step::FlashId)?;
        Ok(self.flash_id)
    }

    fn hard_reset(&mut self) -> Result<(), ProbeError> {
        self.enter(Step::HardReset)?;
        self.connected = false;
        Ok(())
    }
}

#[derive(Deserialize)]
struct Fixture {
    format_version: u32,
    chip: ChipFx,
    flash: FlashFx,
    #[serde(default)]
    psram: Option<PsramFx>,
    identity: IdentityFx,
    registers: Vec<RegisterFx>,
}

#[derive(Deserialize)]
struct ChipFx {
    family: String,
    revision: RevisionFx,
    package_version: u32,
    crystal_mhz: u32,
    mac: String,
    embedded_flash: bool,
    embedded_psram: bool,
}

#[derive(Deserialize)]
struct RevisionFx {
    major: u32,
    minor: u32,
}

#[derive(Deserialize)]
struct FlashFx {
    jedec_manufacturer: String,
    jedec_device: String,
    #[serde(default)]
    efuse_capacity: Option<String>,
    #[serde(default)]
    efuse_vendor: Option<String>,
}

#[derive(Deserialize, Default)]
struct PsramFx {
    #[serde(default)]
    efuse_capacity: Option<String>,
    #[serde(default)]
    efuse_vendor: Option<String>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum IdentityFx {
    MagicReg { magic_reg: String, value: String },
    SecurityChipId { security_chip_id: u32 },
}

#[derive(Deserialize)]
struct RegisterFx {
    addr: String,
    value: String,
    // Provenance, required so every register says where it came from; not used by the fake.
    #[allow(dead_code)]
    source: String,
}

/// `"0x600B0844"` → `0x600B0844`; exactly 8 hex digits after `0x`, any case.
fn parse_hex_u32(text: &str) -> Result<u32, String> {
    let digits = text
        .strip_prefix("0x")
        .filter(|digits| digits.len() == 8 && digits.bytes().all(|b| b.is_ascii_hexdigit()))
        .ok_or_else(|| format!("{text:?} is not 0x + 8 hex digits"))?;
    u32::from_str_radix(digits, 16).map_err(|error| format!("{text:?}: {error}"))
}

/// `"4016"` → `[0x40, 0x16]`: exactly `2 * N` hex digits, no prefix.
fn parse_hex_bytes<const N: usize>(text: &str) -> Result<[u8; N], String> {
    if text.len() != 2 * N || !text.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(format!("{text:?} is not {} hex digits", 2 * N));
    }
    let mut bytes = [0u8; N];
    for (i, byte) in bytes.iter_mut().enumerate() {
        let pair = text
            .get(2 * i..2 * i + 2)
            .ok_or_else(|| format!("{text:?} is too short"))?;
        *byte = u8::from_str_radix(pair, 16).map_err(|error| format!("{text:?}: {error}"))?;
    }
    Ok(bytes)
}

/// Embedded memory from the eFuse flag plus esptool's `"4M"` capacity and vendor name.
fn embedded(
    present: bool,
    capacity: Option<&str>,
    vendor: Option<&str>,
) -> Result<Option<EmbeddedMem>, String> {
    if !present {
        return Ok(None);
    }
    let size_mb = capacity
        .map(|capacity| {
            capacity
                .strip_suffix('M')
                .and_then(|mb| mb.parse::<u32>().ok())
                .ok_or_else(|| format!("capacity {capacity:?} is not <n>M"))
        })
        .transpose()?;
    Ok(Some(EmbeddedMem {
        size_mb,
        vendor: vendor.map(str::to_owned),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURES: [&str; 4] = ["esp32", "esp32c3", "esp32c6", "esp32s3"];

    fn port() -> PortDesc {
        PortDesc {
            name: "fake".to_owned(),
            usb: None,
        }
    }

    fn load(name: &str) -> FakeProbe {
        FakeProbe::from_fixture(name).unwrap()
    }

    fn connected(name: &str) -> FakeProbe {
        let mut probe = load(name);
        probe.connect(&port()).unwrap();
        probe
    }

    /// Facts expected per board, from `docs/context/hardware.md`.
    struct Expected {
        target: &'static str,
        revision: (u32, u32),
        mac: &'static str,
        flash_id: &'static str,
        flash_bytes: u64,
        embedded_flash: Option<EmbeddedMem>,
        embedded_psram: Option<EmbeddedMem>,
    }

    fn mem(size_mb: u32, vendor: &str) -> Option<EmbeddedMem> {
        Some(EmbeddedMem {
            size_mb: Some(size_mb),
            vendor: Some(vendor.to_owned()),
        })
    }

    #[test]
    fn fake_replays_fixture_facts() {
        const MB: u64 = 1024 * 1024;
        let expected = [
            Expected {
                target: "esp32",
                revision: (3, 1),
                mac: "1c:c3:ab:d2:25:b4",
                flash_id: "0x5E4016",
                flash_bytes: 4 * MB,
                embedded_flash: None,
                embedded_psram: None,
            },
            Expected {
                target: "esp32c3",
                revision: (0, 4),
                mac: "80:53:e0:0a:5d:70",
                flash_id: "0x464016",
                flash_bytes: 4 * MB,
                embedded_flash: mem(4, "XMC"),
                embedded_psram: None,
            },
            Expected {
                target: "esp32c6",
                revision: (0, 2),
                mac: "e8:f6:0a:f7:ff:d4",
                flash_id: "0x684018",
                flash_bytes: 16 * MB,
                embedded_flash: None,
                embedded_psram: None,
            },
            Expected {
                target: "esp32s3",
                revision: (0, 2),
                mac: "10:51:db:84:14:0c",
                flash_id: "0x684018",
                flash_bytes: 16 * MB,
                embedded_flash: None,
                embedded_psram: mem(8, "AP_3v3"),
            },
        ];
        for board in expected {
            let name = board.target;
            let mut probe = connected(name);
            let chip = probe.chip().unwrap();
            assert_eq!(Some(chip), Chip::from_target(name), "{name}");
            let revision = probe.revision().unwrap();
            assert_eq!((revision.major, revision.minor), board.revision, "{name}");
            assert_eq!(probe.crystal_mhz().unwrap(), 40, "{name}");
            let mac = probe.mac().unwrap();
            assert_eq!(mac.to_string(), board.mac, "{name}");
            let flash = probe.flash_id().unwrap();
            assert_eq!(flash.to_string(), board.flash_id, "{name}");
            assert_eq!(flash.size_bytes(), Some(board.flash_bytes), "{name}");
            assert_eq!(
                probe.embedded_flash().unwrap(),
                board.embedded_flash,
                "{name}"
            );
            assert_eq!(
                probe.embedded_psram().unwrap(),
                board.embedded_psram,
                "{name}"
            );

            // H5: re-reading the MAC words through READ_REG gives the ROM's MAC.
            let reg = chip.caps().mac_efuse_reg.value;
            let lo = probe.read_reg(reg).unwrap();
            let hi = probe.read_reg(reg + 4).unwrap();
            assert_eq!(Mac::from_efuse_words(lo, hi), mac, "{name}");
            probe.hard_reset().unwrap();
        }
    }

    #[test]
    fn fixture_identity_matches_chip_table() {
        for name in FIXTURES {
            let mut probe = connected(name);
            let caps = probe.chip().unwrap().caps();
            assert_eq!(caps.target, name);
            assert_eq!(probe.identity().unwrap(), caps.identity.value, "{name}");
            if let ChipIdentity::MagicReg { addr, value } = caps.identity.value {
                assert_eq!(probe.read_reg(addr), Ok(value), "{name} magic register");
            }
            let reg = caps.mac_efuse_reg.value;
            assert!(probe.read_reg(reg).is_ok(), "{name} lacks {reg:#010X}");
            assert!(
                probe.read_reg(reg + 4).is_ok(),
                "{name} lacks {reg:#010X}+4"
            );
        }
    }

    #[test]
    fn unknown_register_is_unsupported() {
        let mut probe = connected("esp32c6");
        assert!(matches!(
            probe.read_reg(0x0000_0000),
            Err(ProbeError::Unsupported(_))
        ));
    }

    #[test]
    fn reads_before_connect_are_not_connected() {
        let mut probe = load("esp32c6");
        assert_eq!(probe.chip(), Err(ProbeError::NotConnected));
        assert_eq!(probe.mac(), Err(ProbeError::NotConnected));
        assert_eq!(probe.read_reg(0x600B_0844), Err(ProbeError::NotConnected));
        assert_eq!(probe.flash_id(), Err(ProbeError::NotConnected));
        assert_eq!(probe.hard_reset(), Err(ProbeError::NotConnected));

        // The hard reset ends the session.
        probe.connect(&port()).unwrap();
        assert!(probe.chip().is_ok());
        probe.hard_reset().unwrap();
        assert_eq!(probe.revision(), Err(ProbeError::NotConnected));
    }

    #[test]
    fn fake_fails_on_demand_at_the_given_step() {
        let mut busy = load("esp32").fail_at(Step::Connect, ProbeError::Busy);
        assert_eq!(busy.connect(&port()), Err(ProbeError::Busy));
        assert_eq!(busy.chip(), Err(ProbeError::NotConnected));

        let mut pulled = load("esp32s3")
            .fail_at(Step::FlashId, ProbeError::Disconnected)
            .fail_at(Step::HardReset, ProbeError::Disconnected);
        pulled.connect(&port()).unwrap();
        assert!(pulled.mac().is_ok());
        assert_eq!(pulled.flash_id(), Err(ProbeError::Disconnected));
        assert_eq!(pulled.hard_reset(), Err(ProbeError::Disconnected));

        let mut slow = load("esp32c3").fail_at(Step::ReadReg, ProbeError::Timeout);
        slow.connect(&port()).unwrap();
        assert_eq!(slow.read_reg(0x6000_8844), Err(ProbeError::Timeout));
        assert!(slow.revision().is_ok());
    }

    #[test]
    fn with_mac_changes_only_the_rom_mac() {
        let other = Mac([0x02, 0, 0, 0, 0, 1]);
        let mut probe = load("esp32c6").with_mac(other);
        probe.connect(&port()).unwrap();
        assert_eq!(probe.mac(), Ok(other));
        let caps = probe.chip().unwrap().caps();
        let reg = caps.mac_efuse_reg.value;
        let efuse = Mac::from_efuse_words(
            probe.read_reg(reg).unwrap(),
            probe.read_reg(reg + 4).unwrap(),
        );
        assert_ne!(efuse, other);
    }

    #[test]
    fn fake_records_call_order() {
        let mut probe = load("esp32c6").fail_at(Step::Mac, ProbeError::Timeout);
        let _ = probe.chip();
        probe.connect(&port()).unwrap();
        let _ = probe.identity();
        let _ = probe.mac();
        let _ = probe.read_reg(0x600B_0844);
        let _ = probe.hard_reset();
        assert_eq!(
            probe.calls(),
            [
                Step::Chip,
                Step::Connect,
                Step::Identity,
                Step::Mac,
                Step::ReadReg,
                Step::HardReset
            ]
        );
    }

    #[test]
    fn fixture_loader_rejects_other_versions_and_bad_hex() {
        let text = fs::read_to_string(
            [
                env!("CARGO_MANIFEST_DIR"),
                "tests",
                "fixtures",
                "esp32c6.json",
            ]
            .iter()
            .collect::<PathBuf>(),
        )
        .unwrap();
        assert!(FakeProbe::from_json(&text).is_ok());
        let v2 = text.replacen("\"format_version\": 1", "\"format_version\": 2", 1);
        assert!(
            FakeProbe::from_json(&v2)
                .unwrap_err()
                .contains("format_version")
        );
        let bad = text.replacen("0x600B0844", "0x600B08", 1);
        assert!(FakeProbe::from_json(&bad).is_err());
        assert!(FakeProbe::from_fixture("esp8266").is_err());
    }

    #[test]
    fn probe_runs_as_trait_object() {
        let mut fake = load("esp32c6");
        let probe: &mut dyn Probe = &mut fake;
        probe.connect(&port()).unwrap();
        assert!(probe.chip().is_ok());
        assert!(probe.hard_reset().is_ok());
        assert_eq!(fake.calls(), [Step::Connect, Step::Chip, Step::HardReset]);
    }
}
