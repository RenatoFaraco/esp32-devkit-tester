//! `EspflashProbe`: the real [`Probe`], talking to the ROM bootloader through the espflash
//! library API (ADR-002, ADR-011). Together with `probe/rdid.rs`, the only place of the host
//! that names the `espflash` crate (guarded by `tests/layering.rs`).
//!
//! # What a session does to the board (ADR-009)
//! [`Probe::connect`] opens the port at 115 200 baud and calls `Flasher::try_connect` with
//! `use_stub = false` and `baud = None`, so no code is loaded into RAM and the UART divisor is
//! never changed. espflash 4.5.0 then:
//! 1. drives DTR/RTS into download mode: `UsbJtagSerialReset` when the PID is `0x1001`,
//!    otherwise `ClassicReset` (plus `UnixTightReset` first on Unix), and sends `SYNC`;
//! 2. reads `GET_SECURITY_INFO` (falls back to `READ_REG 0x40001000` on the ESP32) to detect
//!    the chip and Secure Download Mode, then the revision from eFuse;
//! 3. on the ESP32-P4 rev 3.1/3.2 only, powers the flash on through PMU/LP_SYS registers;
//! 4. `SPI_ATTACH`, an RDID through SPI1 (`USR`, `USR2`, `MISO_DLEN`, `W0`, `CMD`; `USR`/`USR2`
//!    restored) to size the flash, and `SPI_SET_PARAMS` (both default and ESP32-PICO-D4 pin
//!    sets are tried, in that order, until the size read works).
//!
//! Steps 3–4 are skipped in Secure Download Mode. After that, every read here is `READ_REG` or
//! `GET_SECURITY_INFO`, except [`Probe::flash_id`] (`probe/rdid.rs`, same SPI1 writes as step
//! 4). [`Probe::hard_reset`] pulses RTS (bridges) or runs the native port's reset sequence, so
//! the chip re-reads the strapping pins and boots the application again.
//!
//! # Errors
//! espflash errors never leave this module: [`map_err`] turns them (and `serialport`/`io`
//! errors) into a [`ProbeError`]. espflash also indexes board-supplied lengths without checks in
//! a few places; every call into it goes through [`shield`], so a malformed reply becomes a
//! [`ProbeError::Protocol`] instead of taking the host down.

use std::cell::Cell;
use std::error::Error as StdError;
use std::io;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::time::Duration;

use espflash::connection::{Connection, ResetAfterOperation, ResetBeforeOperation};
use espflash::flasher::Flasher;
use espflash::target::Chip as EspChip;
use espflash::target::efuse::{self, EfuseField};

use super::rdid;
use super::{EmbeddedMem, FlashId, Probe, ProbeError, Revision};
use crate::chips::{Chip, ChipIdentity};
use crate::transport::{self, Mac, PortDesc};

/// Baud rate of the whole session: the ROM's default; never raised (ADR-009 §2).
const BAUD: u32 = 115_200;

/// Read timeout of the serial port; espflash sets the same value after the sync and every ROM
/// command runs under its own (shorter or equal) timeout.
const SERIAL_TIMEOUT: Duration = Duration::from_secs(3);

/// Never load the flasher stub into RAM (ADR-009 §1).
const USE_STUB: bool = false;

/// Never change the baud rate after the sync (ADR-009 §2).
const BAUD_AFTER_SYNC: Option<u32> = None;

/// Register read by the ROM-based chip detection when `GET_SECURITY_INFO` has no chip id
/// (ESP32). Same address as esptool's `CHIP_DETECT_MAGIC_REG_ADDR`.
const CHIP_DETECT_MAGIC_REG_ADDR: u32 = 0x4000_1000;

/// Longest error text kept from a library message.
const ERROR_TEXT_MAX: usize = 200;

/// [`Probe`] over a real serial port, through espflash.
///
/// One session at a time: [`Probe::connect`] replaces any previous session (closing its port
/// without a reset) and [`Probe::hard_reset`] ends it.
#[derive(Debug, Default)]
pub struct EspflashProbe {
    session: Option<Session>,
}

#[derive(Debug)]
struct Session {
    conn: Connection,
    chip: EspChip,
    /// Only to tell "port gone" from other errors (is it still enumerated?).
    port_name: String,
}

impl EspflashProbe {
    /// A probe with no session.
    pub fn new() -> EspflashProbe {
        EspflashProbe::default()
    }

    /// Runs `f` on the open session, shielded from panics, mapping espflash errors.
    fn run<T>(
        &mut self,
        f: impl FnOnce(&mut Connection, EspChip) -> Result<T, espflash::Error>,
    ) -> Result<T, ProbeError> {
        let session = self.session.as_mut().ok_or(ProbeError::NotConnected)?;
        let chip = session.chip;
        let conn = &mut session.conn;
        match shield(|| f(conn, chip))? {
            Ok(value) => Ok(value),
            Err(error) => Err(map_err(
                &error,
                Stage::Session,
                port_listed(&session.port_name),
            )),
        }
    }

    /// eFuse field map of the connected chip.
    fn efuse_row(&self) -> Result<&'static EfuseRow, ProbeError> {
        let session = self.session.as_ref().ok_or(ProbeError::NotConnected)?;
        let target = session.chip.to_string();
        efuse_row(&target).ok_or_else(|| {
            ProbeError::Unsupported(format!("no eFuse map for {}", host_text(&target)))
        })
    }

    /// Reads a value made of one or more eFuse fields.
    fn read_parts(&mut self, parts: &'static [Part]) -> Result<u32, ProbeError> {
        self.run(|conn, chip| {
            let mut value = 0u32;
            for part in parts {
                let bits = chip.read_efuse_le::<u32>(conn, part.field.clone())?;
                value |= bits.checked_shl(part.shift).unwrap_or(0);
            }
            Ok(value)
        })
    }

    fn embedded(
        &mut self,
        pick: fn(&EfuseRow) -> &MemSource,
    ) -> Result<Option<EmbeddedMem>, ProbeError> {
        let row = self.efuse_row()?;
        read_embedded(pick(row), row.package, &mut |parts| self.read_parts(parts))
    }
}

impl Probe for EspflashProbe {
    fn connect(&mut self, port: &PortDesc) -> Result<(), ProbeError> {
        self.session = None;
        let usb = port
            .usb
            .as_ref()
            .ok_or_else(|| ProbeError::Unsupported("not a USB serial port".to_owned()))?;

        let serial = shield(|| {
            serialport::new(&port.name, BAUD)
                .timeout(SERIAL_TIMEOUT)
                .open_native()
        })?
        .map_err(|error| map_err(&error, Stage::Open, port_listed(&port.name)))?;

        // espflash only uses the PID (reset strategy) and keeps the rest for its own messages.
        let info = serialport::UsbPortInfo {
            vid: usb.vid,
            pid: usb.pid,
            serial_number: usb.serial.clone(),
            manufacturer: usb.manufacturer.clone(),
            product: usb.product.clone(),
            interface: usb.interface,
        };
        let conn = Connection::new(
            serial,
            info,
            ResetAfterOperation::HardReset,
            ResetBeforeOperation::DefaultReset,
            BAUD,
        );

        let attempt =
            shield(|| Flasher::try_connect(conn, USE_STUB, false, false, None, BAUD_AFTER_SYNC))?;
        let (conn, chip) = match attempt {
            Ok(flasher) => {
                let chip = flasher.chip();
                (flasher.into_connection(), chip)
            }
            Err(failed) => {
                let (error, mut conn) = *failed;
                if !matches!(error, espflash::Error::FlashConnect) {
                    let mapped = map_err(&error, Stage::Session, port_listed(&port.name));
                    abandon(&mut conn);
                    return Err(mapped);
                }
                // Synced and identified, but no flash answered the size probe (blank bus). Keep
                // the session: the chip facts are still readable and flash/jedec-id reports the
                // blank bus. espflash dropped the detected chip with the Flasher; ask again.
                match shield(|| conn.detect_chip(USE_STUB)) {
                    Ok(Ok(chip)) => (conn, chip),
                    Ok(Err(error)) => {
                        let mapped = map_err(&error, Stage::Session, port_listed(&port.name));
                        abandon(&mut conn);
                        return Err(mapped);
                    }
                    Err(panicked) => {
                        abandon(&mut conn);
                        return Err(panicked);
                    }
                }
            }
        };
        self.session = Some(Session {
            conn,
            chip,
            port_name: port.name.clone(),
        });
        Ok(())
    }

    fn chip(&mut self) -> Result<Chip, ProbeError> {
        let session = self.session.as_ref().ok_or(ProbeError::NotConnected)?;
        let target = session.chip.to_string();
        Chip::from_target(&target).ok_or_else(|| {
            ProbeError::Unsupported(format!(
                "{} is not in the capability table",
                host_text(&target)
            ))
        })
    }

    fn identity(&mut self) -> Result<ChipIdentity, ProbeError> {
        self.run(|conn, _| {
            // Same order as espflash's detect_chip: GET_SECURITY_INFO, then the magic register
            // when the ROM has no such command (ESP32) or gives no chip id.
            if let Ok(info) = conn.security_info(USE_STUB)
                && let Some(id) = info.chip_id
            {
                return Ok(ChipIdentity::SecurityChipId(id));
            }
            let value = conn.read_reg(CHIP_DETECT_MAGIC_REG_ADDR)?;
            Ok(ChipIdentity::MagicReg {
                addr: CHIP_DETECT_MAGIC_REG_ADDR,
                value,
            })
        })
    }

    fn revision(&mut self) -> Result<Revision, ProbeError> {
        let (major, minor) = self.run(|conn, chip| chip.revision(conn))?;
        Ok(Revision { major, minor })
    }

    fn crystal_mhz(&mut self) -> Result<u32, ProbeError> {
        let xtal = self
            .run(|conn, chip| chip.xtal_frequency(conn))?
            .to_string();
        parse_mhz(&xtal).ok_or_else(|| {
            ProbeError::Protocol(format!("unexpected crystal frequency {}", host_text(&xtal)))
        })
    }

    fn mac(&mut self) -> Result<Mac, ProbeError> {
        let text = self.run(|conn, chip| chip.mac_address(conn))?;
        Mac::parse(&text).map_err(|error| ProbeError::Protocol(error.to_string()))
    }

    fn read_reg(&mut self, addr: u32) -> Result<u32, ProbeError> {
        self.run(|conn, _| conn.read_reg(addr))
    }

    fn package_version(&mut self) -> Result<u32, ProbeError> {
        let row = self.efuse_row()?;
        self.read_parts(row.package)
    }

    fn embedded_flash(&mut self) -> Result<Option<EmbeddedMem>, ProbeError> {
        self.embedded(|row| &row.flash)
    }

    fn embedded_psram(&mut self) -> Result<Option<EmbeddedMem>, ProbeError> {
        self.embedded(|row| &row.psram)
    }

    fn flash_id(&mut self) -> Result<FlashId, ProbeError> {
        let session = self.session.as_mut().ok_or(ProbeError::NotConnected)?;
        rdid::read_jedec_rom(&mut session.conn, session.chip, &session.port_name)
    }

    fn hard_reset(&mut self) -> Result<(), ProbeError> {
        let mut session = self.session.take().ok_or(ProbeError::NotConnected)?;
        let chip = session.chip;
        // The connection was built with ResetAfterOperation::HardReset: RTS pulse on a bridge,
        // the USB-Serial/JTAG sequence on the native port. The port closes when `session` drops.
        match shield(|| session.conn.reset_after(USE_STUB, chip))? {
            Ok(()) => Ok(()),
            Err(error) => Err(map_err(
                &error,
                Stage::Session,
                port_listed(&session.port_name),
            )),
        }
    }
}

/// Best-effort reset back to the application after a failed connect, so the board does not
/// stay in download mode (ADR-009 §4). Errors are ignored: the connect error is what matters.
fn abandon(conn: &mut Connection) {
    let _ = shield(|| conn.reset());
}

/// Runs a call into espflash, turning a panic into [`ProbeError::Protocol`].
///
/// espflash slices some replies by lengths the board sends (e.g. the response header of
/// `GET_SECURITY_INFO`), so a corrupted reply could panic inside it. While `f` runs,
/// [`in_shield`] is true on this thread, so a panic hook can stay quiet about a panic that is
/// about to be caught (see `probe::quiet_shielded_panics`).
pub(crate) fn shield<T>(f: impl FnOnce() -> T) -> Result<T, ProbeError> {
    SHIELD_DEPTH.with(|depth| depth.set(depth.get().saturating_add(1)));
    // Dropped after `catch_unwind` returns, on both paths.
    let _guard = ShieldGuard;
    catch_unwind(AssertUnwindSafe(f)).map_err(|_| {
        ProbeError::Protocol("malformed reply from the ROM (espflash panicked)".to_owned())
    })
}

thread_local! {
    /// How many [`shield`] calls are active on this thread (they may nest).
    static SHIELD_DEPTH: Cell<u32> = const { Cell::new(0) };
}

/// Leaves one level of [`shield`] when dropped.
struct ShieldGuard;

impl Drop for ShieldGuard {
    fn drop(&mut self) {
        SHIELD_DEPTH.with(|depth| depth.set(depth.get().saturating_sub(1)));
    }
}

/// True while a [`shield`] call is running on the current thread: a panic now will be caught
/// and turned into a [`ProbeError::Protocol`].
pub(crate) fn in_shield() -> bool {
    SHIELD_DEPTH.with(|depth| depth.get() > 0)
}

/// Whether `name` is still enumerated by the OS. An enumeration error counts as "listed", so it
/// never turns another error into a disconnection.
pub(crate) fn port_listed(name: &str) -> bool {
    transport::enumerate().map_or(true, |ports| ports.iter().any(|port| port.name == name))
}

/// When the error happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Stage {
    /// Opening the port: "no device" while the port is still listed means another program
    /// holds it (Windows `ERROR_ACCESS_DENIED`, Unix `EBUSY`; serialport maps both to
    /// `NoDevice`).
    Open,
    /// Talking to the ROM through an open port.
    Session,
}

/// Maps an espflash, `serialport` or `io` error to a [`ProbeError`]. Pure: the caller says
/// whether the port is still enumerated.
///
/// - port no longer listed → `Disconnected` (whatever the error says);
/// - `io::ErrorKind::TimedOut`, espflash's "Timeout while running …" and "Failed to connect to
///   the device" (no sync after every reset attempt) → `Timeout`;
/// - `NotFound`, `BrokenPipe`, connection reset/aborted, `UnexpectedEof`, espflash's "Serial
///   port not found", and `NoDevice`/`PermissionDenied` during a session → `Disconnected`;
/// - `NoDevice` while opening a listed port → `Busy`;
/// - anything else → `Protocol` with the error chain as sanitized text.
///
/// espflash's `ConnectionError` is private, so it is recognised by its message; the version is
/// pinned (`=4.5.0`) and `map_err_classifies_espflash_errors` builds the real errors.
pub(crate) fn map_err(
    error: &(dyn StdError + 'static),
    stage: Stage,
    port_listed: bool,
) -> ProbeError {
    if !port_listed {
        return ProbeError::Disconnected;
    }
    let mut current = Some(error);
    while let Some(layer) = current {
        if let Some(found) = classify_layer(layer, stage) {
            return found;
        }
        current = layer.source();
    }
    ProbeError::Protocol(describe(error))
}

fn classify_layer(layer: &(dyn StdError + 'static), stage: Stage) -> Option<ProbeError> {
    if let Some(error) = layer.downcast_ref::<io::Error>() {
        return classify_io(error.kind(), stage);
    }
    if let Some(error) = layer.downcast_ref::<serialport::Error>() {
        return match error.kind() {
            serialport::ErrorKind::NoDevice => Some(match stage {
                Stage::Open => ProbeError::Busy,
                Stage::Session => ProbeError::Disconnected,
            }),
            serialport::ErrorKind::Io(kind) => classify_io(kind, stage),
            serialport::ErrorKind::InvalidInput | serialport::ErrorKind::Unknown => None,
        };
    }
    let text = layer.to_string();
    if text.starts_with("Timeout while running") || text == "Failed to connect to the device" {
        Some(ProbeError::Timeout)
    } else if text == "Serial port not found" {
        Some(ProbeError::Disconnected)
    } else {
        None
    }
}

fn classify_io(kind: io::ErrorKind, stage: Stage) -> Option<ProbeError> {
    use io::ErrorKind::{
        BrokenPipe, ConnectionAborted, ConnectionReset, NotConnected, NotFound, PermissionDenied,
        TimedOut, UnexpectedEof,
    };
    match kind {
        TimedOut => Some(ProbeError::Timeout),
        NotFound | BrokenPipe | ConnectionReset | ConnectionAborted | NotConnected
        | UnexpectedEof => Some(ProbeError::Disconnected),
        // Opening: a permission problem (Unix device permissions), reported as text. During a
        // session: the device went away under the open handle (Windows after an unplug).
        PermissionDenied if stage == Stage::Session => Some(ProbeError::Disconnected),
        _ => None,
    }
}

/// The error and its sources, `": "`-separated, as host text.
fn describe(error: &(dyn StdError + 'static)) -> String {
    let mut parts: Vec<String> = Vec::new();
    let mut current = Some(error);
    while let Some(layer) = current {
        let text = layer.to_string();
        if parts.last() != Some(&text) {
            parts.push(text);
        }
        current = layer.source();
    }
    host_text(&parts.join(": "))
}

/// Printable ASCII only (anything else becomes `?`), at most [`ERROR_TEXT_MAX`] characters.
/// Library messages can quote board bytes; the report sanitizes again at its edge.
fn host_text(text: &str) -> String {
    text.chars()
        .map(|c| if (' '..='~').contains(&c) { c } else { '?' })
        .take(ERROR_TEXT_MAX)
        .collect()
}

/// `"40 MHz"` (espflash's `XtalFrequency` display) → 40.
fn parse_mhz(text: &str) -> Option<u32> {
    text.trim().strip_suffix("MHz")?.trim().parse().ok()
}

// ---------------------------------------------------------------------------------------------
// eFuse field map (ADR-004: data, not a `match` per chip)
// ---------------------------------------------------------------------------------------------

/// One eFuse field contributing `field << shift` to a value (S3 PSRAM capacity and ESP32
/// package version are split over two fields).
#[derive(Debug)]
struct Part {
    field: EfuseField,
    shift: u32,
}

const fn part(field: EfuseField) -> Part {
    Part { field, shift: 0 }
}

const fn part_at(field: EfuseField, shift: u32) -> Part {
    Part { field, shift }
}

/// Where the eFuse says whether memory sits inside the package.
#[derive(Debug)]
enum MemSource {
    /// The family has no variant with this memory in the package (and no PSRAM support at all
    /// for PSRAM): always `None`.
    Absent,
    /// Not decodable for this chip: the runner turns it into a SKIP.
    Unsupported(&'static str),
    /// Decided by the package version alone; size and vendor are not in eFuse (ESP32).
    ByPackage(&'static [u32]),
    /// Capacity (and vendor) codes. Code 0 is "none" in every Espressif eFuse table; a nonzero
    /// code outside `sizes_mb`/`vendors` is reported with that part as `None`.
    Fields {
        cap: &'static [Part],
        sizes_mb: &'static [(u32, u32)],
        vendor: Option<&'static [Part]>,
        vendors: &'static [(u32, &'static str)],
    },
}

/// eFuse fields of one chip, keyed by ESP-IDF target name (same key as `ChipCaps::target`).
#[derive(Debug)]
struct EfuseRow {
    target: &'static str,
    package: &'static [Part],
    flash: MemSource,
    psram: MemSource,
}

/// Field positions: `espflash::target::efuse::<chip>` (4.5.0). Codes: `dict` of the same fields
/// in espefuse's `efuse_defs/<chip>.yaml` and esptool's `targets/<chip>.py` (esptool
/// v4.12.dev1, the ESP-IDF v5.5.2 venv). Chips whose yaml has no `dict` get no codes (size and
/// vendor stay `None`, only presence is asserted).
static EFUSE_ROWS: [EfuseRow; 7] = [
    EfuseRow {
        target: "esp32",
        // esptool get_pkg_version: CHIP_PACKAGE | CHIP_PACKAGE_4BIT << 3.
        package: &[
            part(efuse::esp32::CHIP_PACKAGE),
            part_at(efuse::esp32::CHIP_PACKAGE_4BIT, 3),
        ],
        // esptool get_chip_features: "Embedded Flash" for packages 2, 4, 5, 6 (D2WD, PICO-D4,
        // PICO-V3, PICO-V3-02); "Embedded PSRAM" for 6.
        flash: MemSource::ByPackage(&[2, 4, 5, 6]),
        psram: MemSource::ByPackage(&[6]),
    },
    EfuseRow {
        target: "esp32c3",
        package: &[part(efuse::esp32c3::PKG_VERSION)],
        flash: MemSource::Fields {
            cap: &[part(efuse::esp32c3::FLASH_CAP)],
            sizes_mb: &[(1, 4), (2, 2), (3, 1), (4, 8)],
            vendor: Some(&[part(efuse::esp32c3::FLASH_VENDOR)]),
            vendors: &[(1, "XMC"), (2, "GD"), (3, "FM"), (4, "TT"), (5, "ZBIT")],
        },
        psram: MemSource::Absent,
    },
    EfuseRow {
        target: "esp32c5",
        package: &[part(efuse::esp32c5::PKG_VERSION)],
        flash: MemSource::Fields {
            cap: &[part(efuse::esp32c5::FLASH_CAP)],
            sizes_mb: &[],
            vendor: Some(&[part(efuse::esp32c5::FLASH_VENDOR)]),
            vendors: &[],
        },
        psram: MemSource::Fields {
            cap: &[part(efuse::esp32c5::PSRAM_CAP)],
            sizes_mb: &[],
            vendor: Some(&[part(efuse::esp32c5::PSRAM_VENDOR)]),
            vendors: &[],
        },
    },
    EfuseRow {
        target: "esp32c6",
        package: &[part(efuse::esp32c6::PKG_VERSION)],
        flash: MemSource::Fields {
            cap: &[part(efuse::esp32c6::FLASH_CAP)],
            // esptool targets/esp32c6.py get_chip_features (C6FH4 / C6FH8); the yaml has no dict.
            sizes_mb: &[(1, 4), (2, 8)],
            vendor: Some(&[part(efuse::esp32c6::FLASH_VENDOR)]),
            vendors: &[],
        },
        psram: MemSource::Absent,
    },
    EfuseRow {
        target: "esp32s3",
        package: &[part(efuse::esp32s3::PKG_VERSION)],
        flash: MemSource::Fields {
            cap: &[part(efuse::esp32s3::FLASH_CAP)],
            sizes_mb: &[(1, 8), (2, 4)],
            vendor: Some(&[part(efuse::esp32s3::FLASH_VENDOR)]),
            vendors: &[(1, "XMC"), (2, "GD"), (3, "FM"), (4, "TT"), (5, "BY")],
        },
        // esptool get_psram_cap: PSRAM_CAP | PSRAM_CAP_3 << 2.
        psram: MemSource::Fields {
            cap: &[
                part(efuse::esp32s3::PSRAM_CAP),
                part_at(efuse::esp32s3::PSRAM_CAP_3, 2),
            ],
            sizes_mb: &[(1, 8), (2, 2), (3, 16), (4, 4)],
            vendor: Some(&[part(efuse::esp32s3::PSRAM_VENDOR)]),
            vendors: &[(1, "AP_3v3"), (2, "AP_1v8")],
        },
    },
    EfuseRow {
        target: "esp32p4",
        package: &[part(efuse::esp32p4::PKG_VERSION)],
        flash: MemSource::Unsupported("the ESP32-P4 eFuse map has no flash capacity field"),
        psram: MemSource::Fields {
            cap: &[part(efuse::esp32p4::PSRAM_CAP)],
            sizes_mb: &[],
            vendor: Some(&[part(efuse::esp32p4::PSRAM_VENDOR)]),
            vendors: &[],
        },
    },
    EfuseRow {
        target: "esp32h2",
        package: &[part(efuse::esp32h2::PKG_VERSION)],
        flash: MemSource::Fields {
            cap: &[part(efuse::esp32h2::FLASH_CAP)],
            sizes_mb: &[],
            vendor: Some(&[part(efuse::esp32h2::FLASH_VENDOR)]),
            vendors: &[],
        },
        psram: MemSource::Absent,
    },
];

fn efuse_row(target: &str) -> Option<&'static EfuseRow> {
    EFUSE_ROWS.iter().find(|row| row.target == target)
}

/// Decides the embedded memory from `source`, reading eFuse values through `read`.
fn read_embedded(
    source: &MemSource,
    package: &'static [Part],
    read: &mut dyn FnMut(&'static [Part]) -> Result<u32, ProbeError>,
) -> Result<Option<EmbeddedMem>, ProbeError> {
    match source {
        MemSource::Absent => Ok(None),
        MemSource::Unsupported(why) => Err(ProbeError::Unsupported((*why).to_owned())),
        MemSource::ByPackage(packages) => {
            let package = read(package)?;
            Ok(packages.contains(&package).then_some(EmbeddedMem {
                size_mb: None,
                vendor: None,
            }))
        }
        MemSource::Fields {
            cap,
            sizes_mb,
            vendor,
            vendors,
        } => {
            let code = read(cap)?;
            if code == 0 {
                return Ok(None);
            }
            let vendor_code = match vendor {
                Some(parts) => Some(read(parts)?),
                None => None,
            };
            Ok(Some(EmbeddedMem {
                size_mb: lookup(sizes_mb, code).copied(),
                vendor: vendor_code
                    .and_then(|code| lookup(vendors, code))
                    .map(|name| (*name).to_owned()),
            }))
        }
    }
}

fn lookup<T>(table: &[(u32, T)], code: u32) -> Option<&T> {
    table
        .iter()
        .find(|(key, _)| *key == code)
        .map(|(_, value)| value)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::fs;
    use std::path::PathBuf;

    use super::*;
    use crate::chips::CHIPS;

    #[test]
    fn efuse_field_table_covers_every_chip_in_chips_table() {
        for caps in &CHIPS {
            let rows = EFUSE_ROWS
                .iter()
                .filter(|row| row.target == caps.target)
                .count();
            assert_eq!(rows, 1, "{} has {rows} eFuse rows", caps.target);
        }
        for row in &EFUSE_ROWS {
            assert!(
                Chip::from_target(row.target).is_some(),
                "{} is not in CHIPS",
                row.target
            );
            let esp: Result<EspChip, _> = row.target.parse();
            assert!(esp.is_ok(), "espflash does not know {}", row.target);
            assert!(
                !row.package.is_empty(),
                "{} has no package field",
                row.target
            );
        }
    }

    #[test]
    fn efuse_table_agrees_with_chip_psram_support() {
        use crate::chips::PsramSupport;
        for caps in &CHIPS {
            let row = efuse_row(caps.target).unwrap();
            if caps.psram.value == PsramSupport::None {
                assert!(
                    matches!(row.psram, MemSource::Absent),
                    "{} cannot have PSRAM in package",
                    caps.target
                );
            }
        }
    }

    /// Extracts a field from the eFuse words captured in a fixture (`efuse_blocks`), the same
    /// way espflash's `read_efuse_le` does for fields of up to 32 bits.
    fn extract(blocks: &HashMap<u32, Vec<u32>>, field: &EfuseField) -> u32 {
        let words = &blocks[&field.block];
        let word = (field.bit_start / 32) as usize;
        let offset = field.bit_start % 32;
        let lo = u64::from(words[word]);
        let hi = u64::from(words.get(word + 1).copied().unwrap_or(0));
        let wide = ((hi << 32) | lo) >> offset;
        (wide & ((1u64 << field.bit_count) - 1)) as u32
    }

    struct Dump {
        blocks: HashMap<u32, Vec<u32>>,
        json: serde_json::Value,
    }

    fn dump(target: &str) -> Dump {
        let path: PathBuf = [env!("CARGO_MANIFEST_DIR"), "tests", "fixtures"]
            .iter()
            .collect::<PathBuf>()
            .join(format!("{target}.json"));
        let json: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
        let mut blocks = HashMap::new();
        for (name, words) in json["efuse_blocks"].as_object().unwrap() {
            let index: u32 = name.strip_prefix("BLOCK").unwrap().parse().unwrap();
            let words = words
                .as_array()
                .unwrap()
                .iter()
                .map(|w| u32::from_str_radix(w.as_str().unwrap(), 16).unwrap())
                .collect();
            blocks.insert(index, words);
        }
        Dump { blocks, json }
    }

    fn expected_mem(present: bool, section: &serde_json::Value) -> Option<EmbeddedMem> {
        present.then(|| EmbeddedMem {
            size_mb: section["efuse_capacity"]
                .as_str()
                .map(|cap| cap.trim_end_matches('M').parse().unwrap()),
            vendor: section["efuse_vendor"].as_str().map(str::to_owned),
        })
    }

    #[test]
    fn efuse_table_decodes_the_fixture_dumps() {
        for target in ["esp32", "esp32c3", "esp32c6", "esp32s3"] {
            let Dump { blocks, json } = dump(target);
            let row = efuse_row(target).unwrap();
            let mut read = |parts: &'static [Part]| -> Result<u32, ProbeError> {
                Ok(parts
                    .iter()
                    .fold(0, |acc, p| acc | (extract(&blocks, &p.field) << p.shift)))
            };
            let chip = &json["chip"];
            assert_eq!(
                u64::from(read(row.package).unwrap()),
                chip["package_version"].as_u64().unwrap(),
                "{target} package"
            );
            let flash = read_embedded(&row.flash, row.package, &mut read).unwrap();
            assert_eq!(
                flash,
                expected_mem(chip["embedded_flash"].as_bool().unwrap(), &json["flash"]),
                "{target} flash"
            );
            let psram = read_embedded(&row.psram, row.package, &mut read).unwrap();
            assert_eq!(
                psram,
                expected_mem(chip["embedded_psram"].as_bool().unwrap(), &json["psram"]),
                "{target} psram"
            );
        }
    }

    #[test]
    fn embedded_decoding_rules() {
        let fields = MemSource::Fields {
            cap: &[],
            sizes_mb: &[(1, 4)],
            vendor: Some(&[]),
            vendors: &[(1, "XMC")],
        };
        let codes = |cap: u32, vendor: u32| {
            let mut n = 0;
            move |_: &'static [Part]| -> Result<u32, ProbeError> {
                n += 1;
                Ok(if n == 1 { cap } else { vendor })
            }
        };
        assert_eq!(read_embedded(&fields, &[], &mut codes(0, 1)), Ok(None));
        assert_eq!(
            read_embedded(&fields, &[], &mut codes(1, 1)),
            Ok(Some(EmbeddedMem {
                size_mb: Some(4),
                vendor: Some("XMC".to_owned())
            }))
        );
        // Unknown codes: present, but nothing asserted about size or vendor.
        assert_eq!(
            read_embedded(&fields, &[], &mut codes(6, 7)),
            Ok(Some(EmbeddedMem {
                size_mb: None,
                vendor: None
            }))
        );
        let by_package = MemSource::ByPackage(&[6]);
        assert_eq!(read_embedded(&by_package, &[], &mut codes(1, 0)), Ok(None));
        assert!(
            read_embedded(&by_package, &[], &mut codes(6, 0))
                .unwrap()
                .is_some()
        );
        assert!(matches!(
            read_embedded(&MemSource::Unsupported("x"), &[], &mut codes(0, 0)),
            Err(ProbeError::Unsupported(_))
        ));
        assert_eq!(
            read_embedded(&MemSource::Absent, &[], &mut |_| Err(ProbeError::Timeout)),
            Ok(None)
        );
        // A failed read is the read's error.
        assert_eq!(
            read_embedded(&fields, &[], &mut |_| Err(ProbeError::Disconnected)),
            Err(ProbeError::Disconnected)
        );
    }

    #[derive(Debug)]
    struct Message(&'static str);

    impl std::fmt::Display for Message {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str(self.0)
        }
    }

    impl StdError for Message {}

    fn slip(error: slip_codec::SlipError) -> espflash::Error {
        espflash::Error::from(error)
    }

    #[test]
    fn map_err_classifies_espflash_errors() {
        use ProbeError::{Busy, Disconnected, Timeout};
        use Stage::{Open, Session};
        let io_err = |kind| io::Error::new(kind, "x");
        let sp_err = |kind| serialport::Error::new(kind, "x");
        let cases: Vec<(Box<dyn StdError>, Stage, ProbeError)> = vec![
            // espflash's private ConnectionError, built through its public From<SlipError>.
            (
                Box::new(slip(slip_codec::SlipError::ReadError(io_err(
                    io::ErrorKind::TimedOut,
                )))),
                Session,
                Timeout,
            ),
            (
                Box::new(slip(slip_codec::SlipError::ReadError(io_err(
                    io::ErrorKind::NotFound,
                )))),
                Session,
                Disconnected,
            ),
            (
                Box::new(slip(slip_codec::SlipError::ReadError(io_err(
                    io::ErrorKind::BrokenPipe,
                )))),
                Session,
                Disconnected,
            ),
            // ConnectionFailed (every reset + sync attempt failed) has no source.
            (
                Box::new(espflash::Error::Connection(Box::new(Message(
                    "Failed to connect to the device",
                )))),
                Session,
                Timeout,
            ),
            // io / serialport errors wrapped by espflash.
            (
                Box::new(espflash::Error::from(io_err(io::ErrorKind::TimedOut))),
                Session,
                Timeout,
            ),
            (
                Box::new(espflash::Error::from(sp_err(
                    serialport::ErrorKind::NoDevice,
                ))),
                Session,
                Disconnected,
            ),
            (
                Box::new(espflash::Error::from(io_err(
                    io::ErrorKind::PermissionDenied,
                ))),
                Session,
                Disconnected,
            ),
            // Opening the port.
            (
                Box::new(sp_err(serialport::ErrorKind::NoDevice)),
                Open,
                Busy,
            ),
            (
                Box::new(sp_err(serialport::ErrorKind::Io(io::ErrorKind::NotFound))),
                Open,
                Disconnected,
            ),
            (Box::new(io_err(io::ErrorKind::TimedOut)), Open, Timeout),
        ];
        for (error, stage, expected) in cases {
            assert_eq!(map_err(error.as_ref(), stage, true), expected, "{error:?}");
        }
    }

    #[test]
    fn map_err_says_disconnected_when_the_port_is_gone() {
        let error = espflash::Error::from(io::Error::other("device error"));
        assert_eq!(
            map_err(&error, Stage::Session, false),
            ProbeError::Disconnected
        );
        let busy = serialport::Error::new(serialport::ErrorKind::NoDevice, "x");
        assert_eq!(map_err(&busy, Stage::Open, false), ProbeError::Disconnected);
    }

    #[test]
    fn map_err_keeps_the_rest_as_sanitized_protocol_text() {
        let framing = slip(slip_codec::SlipError::FramingError);
        match map_err(&framing, Stage::Session, true) {
            ProbeError::Protocol(text) => assert!(text.contains("SLIP framing"), "{text}"),
            other => panic!("{other:?}"),
        }
        let hostile = espflash::Error::InvalidResponse("\u{1b}[2J\u{7}é".to_owned());
        match map_err(&hostile, Stage::Session, true) {
            ProbeError::Protocol(text) => {
                assert!(text.chars().all(|c| (' '..='~').contains(&c)), "{text:?}");
                assert!(text.contains("?[2J"), "{text:?}");
            }
            other => panic!("{other:?}"),
        }
        let long = espflash::Error::InvalidResponse("x".repeat(1000));
        match map_err(&long, Stage::Session, true) {
            ProbeError::Protocol(text) => assert_eq!(text.len(), ERROR_TEXT_MAX),
            other => panic!("{other:?}"),
        }
        let unknown = serialport::Error::new(serialport::ErrorKind::Unknown, "weird");
        assert!(matches!(
            map_err(&unknown, Stage::Open, true),
            ProbeError::Protocol(_)
        ));
    }

    #[test]
    fn shield_turns_a_panic_into_a_protocol_error() {
        let hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let result: Result<u32, ProbeError> = shield(|| {
            let reply: &[u8] = &[1, 2];
            let length = usize::from(reply[0]) + 8;
            u32::from(reply[length])
        });
        std::panic::set_hook(hook);
        assert!(matches!(result, Err(ProbeError::Protocol(_))));
        assert_eq!(shield(|| 7), Ok(7));
    }

    #[test]
    fn in_shield_is_true_only_inside_shield_even_nested_or_after_a_panic() {
        assert!(!in_shield());
        assert_eq!(shield(in_shield), Ok(true));
        assert_eq!(shield(|| shield(in_shield)), Ok(Ok(true)));
        assert!(!in_shield());
        let hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let caught: Result<(), ProbeError> = shield(|| panic!("malformed"));
        std::panic::set_hook(hook);
        assert!(caught.is_err());
        assert!(!in_shield(), "the depth is restored after a caught panic");
    }

    #[test]
    fn crystal_text_parses() {
        assert_eq!(parse_mhz("40 MHz"), Some(40));
        assert_eq!(parse_mhz("26 MHz"), Some(26));
        assert_eq!(parse_mhz("48MHz"), Some(48));
        assert_eq!(parse_mhz("32 MHz"), Some(32));
        assert_eq!(parse_mhz("fast"), None);
        assert_eq!(parse_mhz(" MHz"), None);
    }

    #[test]
    fn espflash_xtal_display_is_what_parse_mhz_expects() {
        // espflash 4.5.0 displays every XtalFrequency as "<n> MHz" (26, 32, 40, 48).
        let default = espflash::target::XtalFrequency::default().to_string();
        assert_eq!(default, "40 MHz");
        assert_eq!(parse_mhz(&default), Some(40));
    }

    #[test]
    fn espflash_chip_names_map_to_our_table() {
        for caps in &CHIPS {
            let esp: EspChip = caps.target.parse().unwrap();
            assert_eq!(Chip::from_target(&esp.to_string()), Some(caps.chip));
        }
    }

    #[test]
    fn reads_without_session_are_not_connected() {
        let mut probe = EspflashProbe::new();
        assert_eq!(probe.chip(), Err(ProbeError::NotConnected));
        assert_eq!(probe.identity(), Err(ProbeError::NotConnected));
        assert_eq!(probe.mac(), Err(ProbeError::NotConnected));
        assert_eq!(probe.read_reg(0x6000_8844), Err(ProbeError::NotConnected));
        assert_eq!(probe.package_version(), Err(ProbeError::NotConnected));
        assert_eq!(probe.embedded_flash(), Err(ProbeError::NotConnected));
        assert_eq!(probe.flash_id(), Err(ProbeError::NotConnected));
        assert_eq!(probe.hard_reset(), Err(ProbeError::NotConnected));
    }

    #[test]
    fn connect_refuses_a_port_that_is_not_usb() {
        let mut probe = EspflashProbe::new();
        let port = PortDesc {
            name: "not-a-real-port".to_owned(),
            usb: None,
        };
        assert!(matches!(
            probe.connect(&port),
            Err(ProbeError::Unsupported(_))
        ));
    }

    #[test]
    fn probe_can_move_to_the_worker_thread() {
        // ADR-011 §5: the runner owns the probe in its own thread.
        fn send<T: Send>() {}
        send::<EspflashProbe>();
    }
}
