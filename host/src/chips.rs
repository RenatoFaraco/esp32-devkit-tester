//! Per-chip capabilities as data (ADR-004): one table, no scattered `if chip == ...`.
//!
//! Pure data with no I/O. It must not import `transport`, `probe`, `runner`, `report`, `ui`,
//! `ratatui` or `crossterm`.
//!
//! Every capability is a [`Spec`]: the value the datasheet promises plus a [`FieldKind`] mark
//! (ADR-010). An `Invariant` field is a property of the silicon, so a board that disagrees with
//! the table fails the test; a `Variable` field depends on the module, package or eFuse rating,
//! so a disagreement passes with a finding. The mark lives on each row, not only on the column,
//! because real exceptions exist (single-core ESP32 parts).
//!
//! Adding a chip means adding a [`Chip`] variant and one `ChipCaps { ... }` row to [`CHIPS`];
//! no other module may name a specific chip (guarded by `tests/layering.rs`).

use std::fmt;

/// An ESP32 family supported by the tester.
///
/// The declaration order is also the row order of [`CHIPS`] (checked at compile time).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Chip {
    /// ESP32 (original, Xtensa LX6).
    Esp32,
    /// ESP32-C3.
    Esp32C3,
    /// ESP32-C5.
    Esp32C5,
    /// ESP32-C6.
    Esp32C6,
    /// ESP32-S3.
    Esp32S3,
    /// ESP32-P4.
    Esp32P4,
    /// ESP32-H2.
    Esp32H2,
}

impl Chip {
    /// Capability row of this chip.
    pub fn caps(self) -> &'static ChipCaps {
        // Invariant: `CHIPS[i].chip as usize == i` for every row and the table has one row per
        // variant; both are checked by a `const` assertion below `CHIPS` at compile time and by
        // `every_chip_has_exactly_one_row`, so this index is always in bounds.
        &CHIPS[self as usize]
    }

    /// Chip from its ESP-IDF / esptool target name (`"esp32c6"`) or its marketing name
    /// (`"ESP32-C6"`), ignoring ASCII case and surrounding whitespace. `None` if unknown.
    pub fn from_target(target: &str) -> Option<Chip> {
        let target = target.trim();
        CHIPS
            .iter()
            .find(|row| {
                row.target.eq_ignore_ascii_case(target) || row.name.eq_ignore_ascii_case(target)
            })
            .map(|row| row.chip)
    }
}

impl fmt::Display for Chip {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.caps().name)
    }
}

/// How a board that disagrees with the table is judged (ADR-010).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FieldKind {
    /// Property of the silicon: a mismatch with what the board reports is a `fail`.
    Invariant,
    /// Depends on module, package or eFuse rating: a mismatch is a `pass` with a finding.
    Variable,
}

impl fmt::Display for FieldKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            FieldKind::Invariant => "invariant",
            FieldKind::Variable => "variable",
        })
    }
}

/// A capability value together with its [`FieldKind`] mark.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Spec<T> {
    /// Value promised by the datasheet / TRM.
    pub value: T,
    /// Whether a mismatch with the board fails the test or only annotates it.
    pub kind: FieldKind,
}

impl<T> Spec<T> {
    /// A silicon property (mismatch = `fail`).
    pub const fn invariant(value: T) -> Self {
        Spec {
            value,
            kind: FieldKind::Invariant,
        }
    }

    /// A module/package/eFuse-dependent property (mismatch = `pass` with a finding).
    pub const fn variable(value: T) -> Self {
        Spec {
            value,
            kind: FieldKind::Variable,
        }
    }
}

/// Where a row's values come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Source {
    /// Document name, e.g. `"ESP32-C6 Series Datasheet"`.
    pub doc: &'static str,
    /// Section inside the document, or what the source is used for.
    pub section: &'static str,
}

/// Instruction set / core family of the high-performance cores.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CoreArch {
    /// Xtensa LX6 (ESP32).
    XtensaLx6,
    /// Xtensa LX7 (ESP32-S3).
    XtensaLx7,
    /// 32-bit RISC-V.
    RiscV,
}

impl fmt::Display for CoreArch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            CoreArch::XtensaLx6 => "Xtensa LX6",
            CoreArch::XtensaLx7 => "Xtensa LX7",
            CoreArch::RiscV => "RISC-V",
        })
    }
}

/// Low-power coprocessor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LpCore {
    /// ULP finite-state-machine coprocessor (ESP32).
    UlpFsm,
    /// ULP RISC-V plus ULP FSM coprocessors (ESP32-S3).
    UlpRiscvAndFsm,
    /// LP RISC-V core in the low-power domain (C5, C6, P4).
    LpRiscV,
}

impl fmt::Display for LpCore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            LpCore::UlpFsm => "ULP-FSM",
            LpCore::UlpRiscvAndFsm => "ULP-RISC-V + ULP-FSM",
            LpCore::LpRiscV => "LP RISC-V",
        })
    }
}

/// Wi-Fi generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WifiGen {
    /// Wi-Fi 4 (802.11 b/g/n).
    Wifi4,
    /// Wi-Fi 6 (802.11ax).
    Wifi6,
}

/// Wi-Fi frequency bands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WifiBands {
    /// 2.4 GHz only.
    Band2G4,
    /// Dual band: 2.4 GHz and 5 GHz.
    Band2G4And5G,
}

/// Wi-Fi radio of a chip.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Wifi {
    /// Generation (`gen` is a reserved word in edition 2024).
    pub generation: WifiGen,
    /// Supported bands.
    pub bands: WifiBands,
}

impl fmt::Display for Wifi {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let generation = match self.generation {
            WifiGen::Wifi4 => "Wi-Fi 4",
            WifiGen::Wifi6 => "Wi-Fi 6",
        };
        let bands = match self.bands {
            WifiBands::Band2G4 => "2.4 GHz",
            WifiBands::Band2G4And5G => "2.4 + 5 GHz",
        };
        write!(f, "{generation} ({bands})")
    }
}

/// Radios built into the chip.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Radios {
    /// Wi-Fi, if any.
    pub wifi: Option<Wifi>,
    /// Bluetooth Classic (BR/EDR).
    pub bt_classic: bool,
    /// Bluetooth Low Energy.
    pub ble: bool,
    /// IEEE 802.15.4 (Thread / Zigbee).
    pub ieee802154: bool,
}

impl Radios {
    /// No radio at all (ESP32-P4).
    pub const NONE: Radios = Radios {
        wifi: None,
        bt_classic: false,
        ble: false,
        ieee802154: false,
    };
}

impl fmt::Display for Radios {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut parts: Vec<String> = Vec::new();
        if let Some(wifi) = self.wifi {
            parts.push(wifi.to_string());
        }
        if self.bt_classic {
            parts.push("BT Classic".to_owned());
        }
        if self.ble {
            parts.push("BLE".to_owned());
        }
        if self.ieee802154 {
            parts.push("IEEE 802.15.4".to_owned());
        }
        if parts.is_empty() {
            f.write_str("none")
        } else {
            f.write_str(&parts.join(", "))
        }
    }
}

/// Whether the chip can use PSRAM (the size actually present is read from the board).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PsramSupport {
    /// The chip has no PSRAM interface.
    None,
    /// The chip supports PSRAM (in-package or external, depending on the module).
    Supported,
}

impl fmt::Display for PsramSupport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            PsramSupport::None => "not supported",
            PsramSupport::Supported => "supported",
        })
    }
}

/// How the ROM bootloader tells which chip it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ChipIdentity {
    /// `READ_REG addr` returns `value` (ESP32, which has no `GET_SECURITY_INFO`).
    MagicReg {
        /// ROM address of the magic register.
        addr: u32,
        /// Expected value.
        value: u32,
    },
    /// `chip_id` field of the `GET_SECURITY_INFO` response.
    SecurityChipId(u32),
}

impl fmt::Display for ChipIdentity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ChipIdentity::MagicReg { addr, value } => {
                write!(f, "magic register {addr:#010X} = {value:#010X}")
            }
            ChipIdentity::SecurityChipId(id) => write!(f, "security info chip id {id}"),
        }
    }
}

/// Textual view of one capability, for the Board panel and for findings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldView {
    /// Stable `snake_case` field name (same as the [`ChipCaps`] field).
    pub name: &'static str,
    /// Invariant or variable (ADR-010).
    pub kind: FieldKind,
    /// Human-readable value (plain ASCII, no markup).
    pub value: String,
}

/// Capabilities of one chip family, as promised by its datasheet and TRM.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChipCaps {
    /// Which chip this row describes.
    pub chip: Chip,
    /// ESP-IDF / esptool target name, e.g. `"esp32c6"`.
    pub target: &'static str,
    /// Marketing name, e.g. `"ESP32-C6"`.
    pub name: &'static str,
    /// Core architecture of the high-performance cores.
    pub arch: Spec<CoreArch>,
    /// Number of high-performance cores.
    pub cores: Spec<u8>,
    /// Low-power coprocessor, if any.
    pub lp_core: Spec<Option<LpCore>>,
    /// Maximum CPU clock in MHz (the eFuse rating may lower it).
    pub max_clock_mhz: Spec<u16>,
    /// Nominal crystal frequency in MHz.
    pub crystal_mhz: Spec<u8>,
    /// Built-in radios.
    pub radios: Spec<Radios>,
    /// Native USB-Serial/JTAG controller (`303A:1001`).
    pub usb_serial_jtag: Spec<bool>,
    /// USB OTG controller.
    pub usb_otg: Spec<bool>,
    /// PSRAM support.
    pub psram: Spec<PsramSupport>,
    /// How the ROM identifies the chip.
    pub identity: Spec<ChipIdentity>,
    /// Address of the first eFuse word holding the factory MAC (always safe to read).
    pub mac_efuse_reg: Spec<u32>,
    /// Datasheet, TRM and esptool target the row was taken from.
    pub sources: &'static [Source],
}

impl ChipCaps {
    /// Every capability as text: stable name, kind and readable value, in declaration order.
    pub fn fields(&self) -> Vec<FieldView> {
        // Exhaustive destructuring (no `..`): a new field does not compile until it is listed.
        let ChipCaps {
            chip: _,
            target: _,
            name: _,
            arch,
            cores,
            lp_core,
            max_clock_mhz,
            crystal_mhz,
            radios,
            usb_serial_jtag,
            usb_otg,
            psram,
            identity,
            mac_efuse_reg,
            sources: _,
        } = self;

        fn view<T>(name: &'static str, spec: &Spec<T>, value: String) -> FieldView {
            FieldView {
                name,
                kind: spec.kind,
                value,
            }
        }
        fn yes_no(value: bool) -> String {
            if value { "yes" } else { "no" }.to_owned()
        }

        vec![
            view("arch", arch, arch.value.to_string()),
            view("cores", cores, cores.value.to_string()),
            view(
                "lp_core",
                lp_core,
                lp_core
                    .value
                    .map_or_else(|| "none".to_owned(), |core| core.to_string()),
            ),
            view(
                "max_clock_mhz",
                max_clock_mhz,
                format!("{} MHz", max_clock_mhz.value),
            ),
            view(
                "crystal_mhz",
                crystal_mhz,
                format!("{} MHz", crystal_mhz.value),
            ),
            view("radios", radios, radios.value.to_string()),
            view(
                "usb_serial_jtag",
                usb_serial_jtag,
                yes_no(usb_serial_jtag.value),
            ),
            view("usb_otg", usb_otg, yes_no(usb_otg.value)),
            view("psram", psram, psram.value.to_string()),
            view("identity", identity, identity.value.to_string()),
            view(
                "mac_efuse_reg",
                mac_efuse_reg,
                format!("{:#010X}", mac_efuse_reg.value),
            ),
        ]
    }
}

const WIFI4_2G4: Wifi = Wifi {
    generation: WifiGen::Wifi4,
    bands: WifiBands::Band2G4,
};
const WIFI6_2G4: Wifi = Wifi {
    generation: WifiGen::Wifi6,
    bands: WifiBands::Band2G4,
};
const WIFI6_DUAL: Wifi = Wifi {
    generation: WifiGen::Wifi6,
    bands: WifiBands::Band2G4And5G,
};

/// The capability table: one row per [`Chip`], in variant order.
pub static CHIPS: [ChipCaps; 7] = [
    ChipCaps {
        chip: Chip::Esp32,
        target: "esp32",
        name: "ESP32",
        arch: Spec::invariant(CoreArch::XtensaLx6),
        // Variable, not Invariant: single-core parts exist (ESP32-S0WD, ESP32-U4WDH) and the
        // eFuse declares them (ESP32 Series Datasheet, "ESP32 Series Comparison").
        cores: Spec::variable(2),
        lp_core: Spec::invariant(Some(LpCore::UlpFsm)),
        // 160 MHz on ESP32-S0WD (eFuse CHIP_CPU_FREQ_RATED/LOW); 2-60 MHz crystals work, but
        // Wi-Fi/Bluetooth need 40 MHz (ESP32 Series Datasheet v5.3, "CPU and Memory", "Clocks").
        max_clock_mhz: Spec::variable(240),
        crystal_mhz: Spec::variable(40),
        radios: Spec::invariant(Radios {
            wifi: Some(WIFI4_2G4),
            bt_classic: true,
            ble: true,
            ieee802154: false,
        }),
        usb_serial_jtag: Spec::invariant(false),
        usb_otg: Spec::invariant(false),
        psram: Spec::variable(PsramSupport::Supported),
        identity: Spec::invariant(ChipIdentity::MagicReg {
            addr: 0x4000_1000,
            value: 0x00F0_1D83,
        }),
        // eFuse BLOCK0 words 1-2.
        mac_efuse_reg: Spec::invariant(0x3FF5_A004),
        sources: &[
            Source {
                doc: "ESP32 Series Datasheet v5.3",
                section: "Features; ESP32 Series Comparison",
            },
            Source {
                doc: "ESP32 Technical Reference Manual",
                section: "eFuse Controller",
            },
            Source {
                doc: "esptool targets/esp32.py (v4.12.dev1)",
                section: "identity, mac_efuse_reg",
            },
        ],
    },
    ChipCaps {
        chip: Chip::Esp32C3,
        target: "esp32c3",
        name: "ESP32-C3",
        arch: Spec::invariant(CoreArch::RiscV),
        cores: Spec::invariant(1),
        lp_core: Spec::invariant(None),
        max_clock_mhz: Spec::variable(160),
        crystal_mhz: Spec::variable(40),
        radios: Spec::invariant(Radios {
            wifi: Some(WIFI4_2G4),
            bt_classic: false,
            ble: true,
            ieee802154: false,
        }),
        usb_serial_jtag: Spec::invariant(true),
        usb_otg: Spec::invariant(false),
        psram: Spec::variable(PsramSupport::None),
        identity: Spec::invariant(ChipIdentity::SecurityChipId(5)),
        mac_efuse_reg: Spec::invariant(0x6000_8844),
        sources: &[
            Source {
                doc: "ESP32-C3 Series Datasheet v2.5",
                section: "Features",
            },
            Source {
                doc: "ESP32-C3 Technical Reference Manual",
                section: "eFuse Controller",
            },
            Source {
                doc: "esptool targets/esp32c3.py (v4.12.dev1)",
                section: "identity, mac_efuse_reg",
            },
        ],
    },
    ChipCaps {
        chip: Chip::Esp32C5,
        target: "esp32c5",
        name: "ESP32-C5",
        arch: Spec::invariant(CoreArch::RiscV),
        cores: Spec::invariant(1),
        lp_core: Spec::invariant(Some(LpCore::LpRiscV)),
        max_clock_mhz: Spec::variable(240),
        crystal_mhz: Spec::variable(48),
        radios: Spec::invariant(Radios {
            wifi: Some(WIFI6_DUAL),
            bt_classic: false,
            ble: true,
            ieee802154: true,
        }),
        usb_serial_jtag: Spec::invariant(true),
        usb_otg: Spec::invariant(false),
        psram: Spec::variable(PsramSupport::Supported),
        identity: Spec::invariant(ChipIdentity::SecurityChipId(23)),
        mac_efuse_reg: Spec::invariant(0x600B_4844),
        sources: &[
            Source {
                doc: "ESP32-C5 Series Datasheet v1.5",
                section: "Features",
            },
            Source {
                doc: "ESP32-C5 Technical Reference Manual",
                section: "eFuse Controller",
            },
            Source {
                doc: "esptool targets/esp32c5.py (v4.12.dev1)",
                section: "identity, mac_efuse_reg",
            },
        ],
    },
    ChipCaps {
        chip: Chip::Esp32C6,
        target: "esp32c6",
        name: "ESP32-C6",
        arch: Spec::invariant(CoreArch::RiscV),
        cores: Spec::invariant(1),
        lp_core: Spec::invariant(Some(LpCore::LpRiscV)),
        max_clock_mhz: Spec::variable(160),
        crystal_mhz: Spec::variable(40),
        radios: Spec::invariant(Radios {
            wifi: Some(WIFI6_2G4),
            bt_classic: false,
            ble: true,
            ieee802154: true,
        }),
        usb_serial_jtag: Spec::invariant(true),
        usb_otg: Spec::invariant(false),
        psram: Spec::variable(PsramSupport::None),
        identity: Spec::invariant(ChipIdentity::SecurityChipId(13)),
        mac_efuse_reg: Spec::invariant(0x600B_0844),
        sources: &[
            Source {
                doc: "ESP32-C6 Series Datasheet v1.6",
                section: "Features",
            },
            Source {
                doc: "ESP32-C6 Technical Reference Manual",
                section: "eFuse Controller",
            },
            Source {
                doc: "esptool targets/esp32c6.py (v4.12.dev1)",
                section: "identity, mac_efuse_reg",
            },
        ],
    },
    ChipCaps {
        chip: Chip::Esp32S3,
        target: "esp32s3",
        name: "ESP32-S3",
        arch: Spec::invariant(CoreArch::XtensaLx7),
        cores: Spec::invariant(2),
        lp_core: Spec::invariant(Some(LpCore::UlpRiscvAndFsm)),
        max_clock_mhz: Spec::variable(240),
        crystal_mhz: Spec::variable(40),
        radios: Spec::invariant(Radios {
            wifi: Some(WIFI4_2G4),
            bt_classic: false,
            ble: true,
            ieee802154: false,
        }),
        usb_serial_jtag: Spec::invariant(true),
        usb_otg: Spec::invariant(true),
        psram: Spec::variable(PsramSupport::Supported),
        identity: Spec::invariant(ChipIdentity::SecurityChipId(9)),
        mac_efuse_reg: Spec::invariant(0x6000_7044),
        sources: &[
            Source {
                doc: "ESP32-S3 Series Datasheet v2.2",
                section: "Features",
            },
            Source {
                doc: "ESP32-S3 Technical Reference Manual",
                section: "eFuse Controller",
            },
            Source {
                doc: "esptool targets/esp32s3.py (v4.12.dev1)",
                section: "identity, mac_efuse_reg",
            },
        ],
    },
    ChipCaps {
        chip: Chip::Esp32P4,
        target: "esp32p4",
        name: "ESP32-P4",
        arch: Spec::invariant(CoreArch::RiscV),
        cores: Spec::invariant(2),
        lp_core: Spec::invariant(Some(LpCore::LpRiscV)),
        // TODO(verify): v1.x silicon is said to top out at 360 MHz; the pre-release v0.7
        // datasheet only states "up to 400 MHz".
        max_clock_mhz: Spec::variable(400),
        crystal_mhz: Spec::variable(40),
        radios: Spec::invariant(Radios::NONE),
        usb_serial_jtag: Spec::invariant(true),
        // High-speed (USB 2.0 HS) OTG.
        usb_otg: Spec::invariant(true),
        psram: Spec::variable(PsramSupport::Supported),
        identity: Spec::invariant(ChipIdentity::SecurityChipId(18)),
        mac_efuse_reg: Spec::invariant(0x5012_D044),
        sources: &[
            Source {
                doc: "ESP32-P4 Series Datasheet pre-release v0.7",
                section: "Features",
            },
            Source {
                doc: "ESP32-P4 Technical Reference Manual",
                section: "eFuse Controller",
            },
            Source {
                doc: "esptool targets/esp32p4.py (v4.12.dev1)",
                section: "identity, mac_efuse_reg",
            },
        ],
    },
    ChipCaps {
        chip: Chip::Esp32H2,
        target: "esp32h2",
        name: "ESP32-H2",
        arch: Spec::invariant(CoreArch::RiscV),
        cores: Spec::invariant(1),
        lp_core: Spec::invariant(None),
        max_clock_mhz: Spec::variable(96),
        crystal_mhz: Spec::variable(32),
        radios: Spec::invariant(Radios {
            wifi: None,
            bt_classic: false,
            ble: true,
            ieee802154: true,
        }),
        usb_serial_jtag: Spec::invariant(true),
        usb_otg: Spec::invariant(false),
        psram: Spec::variable(PsramSupport::None),
        identity: Spec::invariant(ChipIdentity::SecurityChipId(16)),
        // Same eFuse block address as the ESP32-C6 (esptool inherits it).
        mac_efuse_reg: Spec::invariant(0x600B_0844),
        sources: &[
            Source {
                doc: "ESP32-H2 Series Datasheet v1.3",
                section: "Features",
            },
            Source {
                doc: "ESP32-H2 Technical Reference Manual",
                section: "eFuse Controller",
            },
            Source {
                doc: "esptool targets/esp32h2.py (v4.12.dev1)",
                section: "identity, mac_efuse_reg",
            },
        ],
    },
];

/// Compile-time proof of the invariant `Chip::caps` relies on: row `i` describes the variant
/// whose discriminant is `i`.
const _: () = {
    let mut i = 0;
    while i < CHIPS.len() {
        assert!(CHIPS[i].chip as usize == i, "CHIPS row order != Chip order");
        i += 1;
    }
};

#[cfg(test)]
mod tests {
    use super::*;

    /// Every variant, listed through an exhaustive `match` so a new variant fails to compile
    /// here until the test knows about it.
    fn every_variant() -> Vec<Chip> {
        let listed = |chip: Chip| match chip {
            Chip::Esp32
            | Chip::Esp32C3
            | Chip::Esp32C5
            | Chip::Esp32C6
            | Chip::Esp32S3
            | Chip::Esp32P4
            | Chip::Esp32H2 => chip,
        };
        [
            Chip::Esp32,
            Chip::Esp32C3,
            Chip::Esp32C5,
            Chip::Esp32C6,
            Chip::Esp32S3,
            Chip::Esp32P4,
            Chip::Esp32H2,
        ]
        .into_iter()
        .map(listed)
        .collect()
    }

    const FIELD_NAMES: [&str; 11] = [
        "arch",
        "cores",
        "lp_core",
        "max_clock_mhz",
        "crystal_mhz",
        "radios",
        "usb_serial_jtag",
        "usb_otg",
        "psram",
        "identity",
        "mac_efuse_reg",
    ];

    #[test]
    fn every_chip_has_exactly_one_row() {
        let variants = every_variant();
        assert_eq!(CHIPS.len(), variants.len());
        for chip in variants {
            let rows = CHIPS.iter().filter(|row| row.chip == chip).count();
            assert_eq!(rows, 1, "{chip:?} has {rows} rows");
            assert_eq!(chip.caps().chip, chip);
        }
    }

    #[test]
    fn every_row_cites_a_datasheet_and_a_trm() {
        for row in &CHIPS {
            let cites = |kind: &str| {
                row.sources
                    .iter()
                    .any(|source| source.doc.starts_with(row.name) && source.doc.contains(kind))
            };
            assert!(cites("Datasheet"), "{} cites no datasheet", row.name);
            assert!(
                cites("Technical Reference Manual"),
                "{} cites no TRM",
                row.name
            );
            let esptool = format!("esptool targets/{}.py", row.target);
            assert!(
                row.sources
                    .iter()
                    .any(|source| source.doc.starts_with(&esptool)),
                "{} does not cite {esptool}",
                row.name
            );
        }
    }

    #[test]
    fn field_kinds_match_adr_010() {
        use FieldKind::{Invariant, Variable};
        for row in &CHIPS {
            let name = row.name;
            assert_eq!(row.arch.kind, Invariant, "{name} arch");
            assert_eq!(row.radios.kind, Invariant, "{name} radios");
            assert_eq!(
                row.usb_serial_jtag.kind, Invariant,
                "{name} usb_serial_jtag"
            );
            assert_eq!(row.identity.kind, Invariant, "{name} identity");
            assert_eq!(row.mac_efuse_reg.kind, Invariant, "{name} mac_efuse_reg");
            assert_eq!(row.crystal_mhz.kind, Variable, "{name} crystal_mhz");
            assert_eq!(row.max_clock_mhz.kind, Variable, "{name} max_clock_mhz");
            let expected_cores = if row.chip == Chip::Esp32 {
                Variable
            } else {
                Invariant
            };
            assert_eq!(row.cores.kind, expected_cores, "{name} cores");
        }
    }

    #[test]
    fn esp32_has_no_usb_serial_jtag() {
        assert!(!Chip::Esp32.caps().usb_serial_jtag.value);
        for row in CHIPS.iter().filter(|row| row.chip != Chip::Esp32) {
            assert!(
                row.usb_serial_jtag.value,
                "{} lacks USB-Serial/JTAG",
                row.name
            );
        }
    }

    #[test]
    fn esp32_is_identified_by_magic_register() {
        assert_eq!(
            Chip::Esp32.caps().identity.value,
            ChipIdentity::MagicReg {
                addr: 0x4000_1000,
                value: 0x00F0_1D83
            }
        );
        for row in CHIPS.iter().filter(|row| row.chip != Chip::Esp32) {
            assert!(
                matches!(row.identity.value, ChipIdentity::SecurityChipId(_)),
                "{} should be identified by GET_SECURITY_INFO",
                row.name
            );
        }
    }

    #[test]
    fn identities_are_unique() {
        for (i, a) in CHIPS.iter().enumerate() {
            for b in &CHIPS[i + 1..] {
                assert_ne!(
                    a.identity.value, b.identity.value,
                    "{} and {} share an identity",
                    a.name, b.name
                );
            }
        }
    }

    #[test]
    fn from_target_round_trips() {
        for target in ["esp32", "esp32c3", "esp32c6", "esp32s3"] {
            let chip = Chip::from_target(target);
            assert_eq!(chip.map(|chip| chip.caps().target), Some(target));
        }
        for row in &CHIPS {
            assert_eq!(Chip::from_target(row.target), Some(row.chip));
        }
    }

    #[test]
    fn from_target_accepts_marketing_name_and_ignores_case() {
        assert_eq!(Chip::from_target("ESP32-C6"), Some(Chip::Esp32C6));
        assert_eq!(Chip::from_target(" ESP32S3 "), Some(Chip::Esp32S3));
    }

    #[test]
    fn from_target_rejects_unknown_names() {
        for target in ["", "esp8266", "esp32c2", "esp32-s3x", "\u{1b}[31mesp32"] {
            assert_eq!(Chip::from_target(target), None, "{target:?}");
        }
    }

    #[test]
    fn fields_list_every_capability_with_stable_names() {
        for row in &CHIPS {
            let fields = row.fields();
            let names: Vec<&str> = fields.iter().map(|field| field.name).collect();
            assert_eq!(names, FIELD_NAMES, "{}", row.name);
            for field in &fields {
                assert!(
                    field
                        .name
                        .chars()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'),
                    "{} is not snake_case",
                    field.name
                );
                assert!(!field.value.is_empty(), "{} {} empty", row.name, field.name);
                assert!(
                    field.value.chars().all(|c| c.is_ascii() && !c.is_control()),
                    "{} {} is not plain ASCII: {:?}",
                    row.name,
                    field.name,
                    field.value
                );
            }
        }
    }

    #[test]
    fn fields_carry_the_row_kind() {
        let esp32 = Chip::Esp32.caps().fields();
        let c6 = Chip::Esp32C6.caps().fields();
        let kind_of = |fields: &[FieldView], name: &str| {
            fields
                .iter()
                .find(|field| field.name == name)
                .map(|field| field.kind)
        };
        assert_eq!(kind_of(&esp32, "cores"), Some(FieldKind::Variable));
        assert_eq!(kind_of(&c6, "cores"), Some(FieldKind::Invariant));
    }

    #[test]
    fn fields_render_readable_values() {
        let fields = Chip::Esp32C6.caps().fields();
        let value_of = |name: &str| {
            fields
                .iter()
                .find(|field| field.name == name)
                .map(|field| field.value.as_str())
        };
        assert_eq!(value_of("arch"), Some("RISC-V"));
        assert_eq!(
            value_of("radios"),
            Some("Wi-Fi 6 (2.4 GHz), BLE, IEEE 802.15.4")
        );
        assert_eq!(value_of("identity"), Some("security info chip id 13"));
        assert_eq!(value_of("mac_efuse_reg"), Some("0x600B0844"));
        assert_eq!(
            Chip::Esp32.caps().fields()[9].value,
            "magic register 0x40001000 = 0x00F01D83"
        );
        assert_eq!(Chip::Esp32P4.caps().radios.value.to_string(), "none");
    }
}
