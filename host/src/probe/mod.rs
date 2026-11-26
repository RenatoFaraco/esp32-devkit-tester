//! Talking to the chip: ROM bootloader (via the espflash library API) and JTAG.
//!
//! Read-only by default (ADR-003). It may use `transport`, `platform` and `chips`, but must not
//! import `runner`, `report`, `ui`, `ratatui` or `crossterm`.
//!
//! # The test seam (ADR-011)
//!
//! espflash takes a concrete serial port type, so fake bytes cannot be injected below it. The
//! seam sits **above** it instead: the [`Probe`] trait returns *facts*, i.e. what the board
//! said, already decoded (chip, revision, MAC, eFuse-derived values, flash ID, register reads).
//! The runner depends only on this trait; the real implementation, [`EspflashProbe`]
//! (`probe/espflash.rs`, plus the RDID sequence in `probe/rdid.rs`), is the only place that
//! imports `espflash`, and its errors never cross the seam: they become a [`ProbeError`].
//!
//! There is one method per fact so the runner can time each one and blame a failure on the
//! right test. In tests, `FakeProbe` (in `probe::fake`, test builds only) replays the facts
//! recorded in `host/tests/fixtures/<chip>.json` and fails on demand.

mod espflash;
#[cfg(test)]
pub mod fake;
mod rdid;

pub use self::espflash::EspflashProbe;

use std::fmt;

use crate::chips::{Chip, ChipIdentity};
use crate::transport::{Mac, PortDesc};

/// Keeps the panic hook quiet about panics that [`EspflashProbe`] catches.
///
/// Every call into espflash runs inside a `catch_unwind` (`shield`), so a malformed reply from
/// the board becomes a [`ProbeError::Protocol`] instead of a crash. The panic hook, however, runs
/// *before* the unwind is caught and by default prints the message to stderr, which would
/// scribble over the TUI. This wraps the hook installed so far (e.g. the one from
/// `ratatui::init`, which restores the terminal): it is skipped while the panicking thread is
/// inside the shield, and called as before for every other panic. Install it after the
/// terminal's hook; it stays installed for the life of the process.
pub fn quiet_shielded_panics() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if !self::espflash::in_shield() {
            previous(info);
        }
    }));
}

/// Source of facts about one board, one session at a time.
///
/// Used as `&mut dyn Probe` (object-safe). Every read before a successful
/// [`connect`](Probe::connect) returns [`ProbeError::NotConnected`]. Every call is bounded by a
/// timeout in the implementation; none blocks forever.
pub trait Probe {
    /// Reset into the ROM bootloader through `port` and sync (H3). One session at a time.
    fn connect(&mut self, port: &PortDesc) -> Result<(), ProbeError>;

    /// Chip family identified by the ROM.
    fn chip(&mut self) -> Result<Chip, ProbeError>;

    /// How the ROM identified the chip: the magic register on the ESP32, the
    /// `GET_SECURITY_INFO` chip id on the others. Compared with `caps().identity`.
    fn identity(&mut self) -> Result<ChipIdentity, ProbeError>;

    /// Silicon revision read from eFuse.
    fn revision(&mut self) -> Result<Revision, ProbeError>;

    /// Crystal frequency detected by the ROM, in MHz.
    fn crystal_mhz(&mut self) -> Result<u32, ProbeError>;

    /// Factory base MAC as reported by the ROM.
    fn mac(&mut self) -> Result<Mac, ProbeError>;

    /// `READ_REG` of one 32-bit word (H5). Only addresses that are safe to read are asked for.
    fn read_reg(&mut self, addr: u32) -> Result<u32, ProbeError>;

    /// Package version field of the eFuse.
    fn package_version(&mut self) -> Result<u32, ProbeError>;

    /// Flash inside the chip package as declared in eFuse; `None` when there is none.
    fn embedded_flash(&mut self) -> Result<Option<EmbeddedMem>, ProbeError>;

    /// PSRAM inside the chip package as declared in eFuse; `None` when there is none.
    fn embedded_psram(&mut self) -> Result<Option<EmbeddedMem>, ProbeError>;

    /// JEDEC ID of the SPI flash (RDID, `0x9F`).
    fn flash_id(&mut self) -> Result<FlashId, ProbeError>;

    /// Hard reset back to the application (ADR-009 §4); always attempted at the end of a
    /// session. Ends the session: reads after it return [`ProbeError::NotConnected`].
    fn hard_reset(&mut self) -> Result<(), ProbeError>;
}

/// Why a [`Probe`] call failed. Our own type: library errors never cross the seam.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeError {
    /// The board did not answer in time.
    Timeout,
    /// The port disappeared (cable pulled, USB-Serial/JTAG reset).
    Disconnected,
    /// The port is open in another program.
    Busy,
    /// The board answered something unexpected. The text is written by the host (raw board
    /// bytes, if any, only as hex), so it is safe to show.
    Protocol(String),
    /// The operation does not apply to this chip or port. Host-written text.
    Unsupported(String),
    /// A read was attempted without a connected session.
    NotConnected,
}

impl fmt::Display for ProbeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProbeError::Timeout => f.write_str("timeout"),
            ProbeError::Disconnected => f.write_str("port disconnected"),
            ProbeError::Busy => {
                f.write_str("port busy: another program (e.g. a serial monitor) has it open")
            }
            ProbeError::Protocol(detail) => write!(f, "protocol error: {detail}"),
            ProbeError::Unsupported(detail) => write!(f, "unsupported: {detail}"),
            ProbeError::NotConnected => f.write_str("not connected"),
        }
    }
}

impl std::error::Error for ProbeError {}

/// Silicon revision read from eFuse (`v0.2` = major 0, minor 2).
///
/// A probe fact; the runner converts it into the report model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Revision {
    /// Major revision.
    pub major: u32,
    /// Minor revision.
    pub minor: u32,
}

impl fmt::Display for Revision {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "v{}.{}", self.major, self.minor)
    }
}

/// Memory inside the chip package (flash or PSRAM), as declared in eFuse.
///
/// Each part is `None` when the eFuse does not say it (a capacity code outside the known table,
/// or a vendor code without a name): the probe returns only what it can assert.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbeddedMem {
    /// Size in MB (2^20 bytes).
    pub size_mb: Option<u32>,
    /// Vendor name decoded from eFuse (`"XMC"`, `"AP_3v3"`).
    pub vendor: Option<String>,
}

/// JEDEC ID of the SPI flash: the three bytes answered to RDID (`0x9F`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FlashId {
    /// Manufacturer ID (`0x68` = Boya, `0x5E` = Zbit, ...).
    pub manufacturer: u8,
    /// Memory type byte.
    pub memory_type: u8,
    /// Capacity byte: the size is `2^capacity` bytes.
    pub capacity: u8,
}

impl FlashId {
    /// True for `0x000000` and `0xFFFFFF`: nothing drove the bus (no flash, spec §4).
    pub fn is_blank(&self) -> bool {
        let bytes = [self.manufacturer, self.memory_type, self.capacity];
        bytes == [0x00; 3] || bytes == [0xFF; 3]
    }

    /// Size in bytes from the capacity byte (`2^capacity`), as esptool does.
    ///
    /// `None` for a blank bus or a capacity byte too large to be a size.
    pub fn size_bytes(&self) -> Option<u64> {
        if self.is_blank() {
            return None;
        }
        1u64.checked_shl(u32::from(self.capacity))
    }
}

impl fmt::Display for FlashId {
    /// `0x` + 6 uppercase hex digits, manufacturer first (`0x684018`, spec §4).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "0x{:02X}{:02X}{:02X}",
            self.manufacturer, self.memory_type, self.capacity
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(manufacturer: u8, memory_type: u8, capacity: u8) -> FlashId {
        FlashId {
            manufacturer,
            memory_type,
            capacity,
        }
    }

    #[test]
    fn flash_size_from_capacity_byte() {
        assert_eq!(id(0x5E, 0x40, 0x16).size_bytes(), Some(4 * 1024 * 1024));
        assert_eq!(id(0x68, 0x40, 0x18).size_bytes(), Some(16 * 1024 * 1024));
        assert_eq!(id(0x68, 0x40, 0x40).size_bytes(), None);
        assert_eq!(id(0x68, 0x40, 0xFE).size_bytes(), None);
    }

    #[test]
    fn blank_bus_has_no_size() {
        for blank in [id(0x00, 0x00, 0x00), id(0xFF, 0xFF, 0xFF)] {
            assert!(blank.is_blank());
            assert_eq!(blank.size_bytes(), None);
        }
        assert!(!id(0x00, 0x00, 0x16).is_blank());
    }

    #[test]
    fn flash_id_prints_as_spec() {
        assert_eq!(id(0x68, 0x40, 0x18).to_string(), "0x684018");
        assert_eq!(id(0x5E, 0x40, 0x16).to_string(), "0x5E4016");
    }

    #[test]
    fn revision_prints_as_spec() {
        assert_eq!(Revision { major: 0, minor: 2 }.to_string(), "v0.2");
    }

    #[test]
    fn probe_is_object_safe() {
        fn takes(_: Option<&mut dyn Probe>) {}
        takes(None);
    }
}
