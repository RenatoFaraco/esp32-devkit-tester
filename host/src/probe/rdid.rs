//! JEDEC ID of the SPI flash (RDID, `0x9F`) through the ROM's `READ_REG`/`WRITE_REG`.
//!
//! # Why this file exists
//! espflash 4.5.0 reads the flash ID with `Flasher::spi_command`, which is private; its public
//! `flash_detect` only returns the size. This module repeats the same register sequence so the
//! host gets the three JEDEC bytes (manufacturer, memory type, capacity). ADR-009 allows it; it
//! is the **only** file of the host that calls `write_reg` (guard planned in #45).
//!
//! # What the hardware does
//! SPI1 is the controller wired to the flash. A "user command" is configured in its registers
//! and started by setting `SPI_USR` (bit 18) in `CMD`; the controller clears the bit when the
//! transaction is over and leaves the bits read from MISO in `W0`:
//! - `USR`: phase enables, `USR_COMMAND` (bit 31) and `USR_MISO` (bit 28);
//! - `USR2`: command length minus one (bits 31:28 = 7, an 8-bit command) and the command value;
//! - `MISO_DLEN` (or `USR1` on chips without the length registers): bits to read minus one;
//! - `W0`: data buffer, cleared before the command.
//!
//! All of these are flip-flops (volatile). `USR` and `USR2` are saved and restored, as espflash
//! does; `MISO_DLEN`/`USR1`, `W0` and `CMD` keep the RDID values until the final hard reset
//! (ADR-009 §4). RDID itself is a read command of the flash chip: it does not set the write
//! enable latch nor touch the array.
//!
//! # Differences from espflash's `spi_command`
//! - Completion is polled on `CMD` bit 18 (`SPI_USR`), as esptool's `wait_done` does. espflash
//!   4.5.0 polls bit 18 of `USR`, which the sequence itself wrote as 0, so its loop is in practice
//!   a fixed 1 ms delay.
//! - `USR`/`USR2` are restored even when a step in the middle fails (best effort: if the port is
//!   gone the restore fails too, and the hard reset clears them anyway).

use std::thread::sleep;
use std::time::Duration;

use espflash::connection::Connection;

use super::espflash::{Stage, map_err, port_listed, shield};
use super::{FlashId, ProbeError};

/// RDID: Read Identification, answered by every JEDEC SPI NOR flash.
pub(crate) const RDID: u32 = 0x9F;

/// Bits answered to RDID: manufacturer, memory type and capacity.
const RDID_READ_BITS: u32 = 24;

/// `USR_COMMAND`: the transaction has a command phase.
const USR_COMMAND: u32 = 1 << 31;
/// `USR_MISO`: the transaction has a read (MISO) phase.
const USR_MISO: u32 = 1 << 28;
/// `USR2` command length field: 7 means an 8-bit command.
const USR2_COMMAND_LEN_8BIT: u32 = 7 << 28;
/// `SPI_USR` in `CMD`: start the user transaction; cleared by hardware when done.
const CMD_USR: u32 = 1 << 18;
/// `USR1` layout on chips without `MISO_DLEN`: MISO bit length minus one at bit 8.
const USR1_MISO_BITLEN_SHIFT: u32 = 8;
/// Polls of `CMD` before giving up (1 ms apart), as esptool and espflash.
const MAX_POLLS: u32 = 10;

/// Minimal register access through the ROM (`READ_REG`/`WRITE_REG`), so the sequence can be
/// tested against a fake that records the order of operations.
pub(crate) trait RegAccess {
    /// `READ_REG` of one 32-bit word.
    fn read_reg(&mut self, addr: u32) -> Result<u32, ProbeError>;
    /// `WRITE_REG` of one 32-bit word (no mask).
    fn write_reg(&mut self, addr: u32, value: u32) -> Result<(), ProbeError>;
}

/// Addresses of the SPI1 registers of one chip (from espflash's `Chip::spi_registers`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SpiRegs {
    /// `CMD` (base of the controller).
    pub(crate) cmd: u32,
    /// `USR`.
    pub(crate) usr: u32,
    /// `USR1`.
    pub(crate) usr1: u32,
    /// `USR2`.
    pub(crate) usr2: u32,
    /// `W0`, first word of the data buffer.
    pub(crate) w0: u32,
    /// `MOSI_DLEN`, when the chip has it.
    pub(crate) mosi_dlen: Option<u32>,
    /// `MISO_DLEN`, when the chip has it.
    pub(crate) miso_dlen: Option<u32>,
}

impl From<espflash::target::SpiRegisters> for SpiRegs {
    fn from(regs: espflash::target::SpiRegisters) -> SpiRegs {
        SpiRegs {
            cmd: regs.cmd(),
            usr: regs.usr(),
            usr1: regs.usr1(),
            usr2: regs.usr2(),
            w0: regs.w0(),
            mosi_dlen: regs.mosi_length(),
            miso_dlen: regs.miso_length(),
        }
    }
}

/// [`RegAccess`] over an open ROM session (`READ_REG`/`WRITE_REG` commands).
struct RomRegs<'a> {
    conn: &'a mut Connection,
    /// Only to tell "port gone" from other errors.
    port_name: &'a str,
}

impl RomRegs<'_> {
    fn check<T>(
        &self,
        result: Result<Result<T, espflash::Error>, ProbeError>,
    ) -> Result<T, ProbeError> {
        match result? {
            Ok(value) => Ok(value),
            Err(error) => Err(map_err(&error, Stage::Session, port_listed(self.port_name))),
        }
    }
}

impl RegAccess for RomRegs<'_> {
    fn read_reg(&mut self, addr: u32) -> Result<u32, ProbeError> {
        let conn = &mut *self.conn;
        let result = shield(|| conn.read_reg(addr));
        self.check(result)
    }

    fn write_reg(&mut self, addr: u32, value: u32) -> Result<(), ProbeError> {
        let conn = &mut *self.conn;
        let result = shield(|| conn.write_reg(addr, value, None));
        self.check(result)
    }
}

/// RDID on the connected chip through the ROM: SPI1 addresses from espflash's
/// `Chip::spi_registers`, sequence from [`read_jedec`].
pub(crate) fn read_jedec_rom(
    conn: &mut Connection,
    chip: espflash::target::Chip,
    port_name: &str,
) -> Result<FlashId, ProbeError> {
    let regs = SpiRegs::from(chip.spi_registers());
    read_jedec(&regs, &mut RomRegs { conn, port_name })
}

/// Sends RDID through SPI1 and decodes the three JEDEC bytes.
///
/// Order: save `USR`/`USR2` → write `USR`, `USR2`, the MISO length, `W0` → start (`CMD`) →
/// poll `CMD` → read `W0` → restore `USR`/`USR2`.
pub(crate) fn read_jedec(regs: &SpiRegs, bus: &mut dyn RegAccess) -> Result<FlashId, ProbeError> {
    let old_usr = bus.read_reg(regs.usr)?;
    let old_usr2 = bus.read_reg(regs.usr2)?;

    let result = transact(regs, bus);

    // Restore even if the transaction failed; report the first error.
    let restored = bus
        .write_reg(regs.usr, old_usr)
        .and_then(|()| bus.write_reg(regs.usr2, old_usr2));
    let word = result?;
    restored?;

    let [manufacturer, memory_type, capacity, _] = word.to_le_bytes();
    Ok(FlashId {
        manufacturer,
        memory_type,
        capacity,
    })
}

/// The part of the sequence between save and restore; returns `W0`.
fn transact(regs: &SpiRegs, bus: &mut dyn RegAccess) -> Result<u32, ProbeError> {
    // RDID sends no data after the command: no MOSI phase.
    bus.write_reg(regs.usr, USR_COMMAND | USR_MISO)?;
    bus.write_reg(regs.usr2, USR2_COMMAND_LEN_8BIT | RDID)?;
    let miso_bits = RDID_READ_BITS - 1;
    match (regs.mosi_dlen, regs.miso_dlen) {
        // MOSI_DLEN is not written: there is no data to send (as espflash).
        (Some(_), Some(miso_dlen)) => bus.write_reg(miso_dlen, miso_bits)?,
        // Chips without the length registers keep both lengths in USR1; the MOSI length field
        // (bits from 17) stays 0 because there is no data to send.
        _ => bus.write_reg(regs.usr1, miso_bits << USR1_MISO_BITLEN_SHIFT)?,
    }
    bus.write_reg(regs.w0, 0)?;
    bus.write_reg(regs.cmd, CMD_USR)?;

    let mut polls = 0;
    loop {
        sleep(Duration::from_millis(1));
        if bus.read_reg(regs.cmd)? & CMD_USR == 0 {
            break;
        }
        polls += 1;
        if polls >= MAX_POLLS {
            return Err(ProbeError::Timeout);
        }
    }
    bus.read_reg(regs.w0)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Op {
        Read(u32),
        Write(u32, u32),
    }

    /// Register file that records every access. `CMD_USR` clears itself after `busy_polls`
    /// reads of `CMD`; `W0` gets `answer` when the command starts.
    struct FakeBus {
        regs: HashMap<u32, u32>,
        ops: Vec<Op>,
        cmd: u32,
        w0: u32,
        answer: u32,
        busy_polls: u32,
        fail_write_to: Option<u32>,
    }

    impl FakeBus {
        fn new(regs: &SpiRegs, answer: u32) -> FakeBus {
            let mut map = HashMap::new();
            map.insert(regs.usr, 0x8000_00C0);
            map.insert(regs.usr2, 0x7000_0005);
            FakeBus {
                regs: map,
                ops: Vec::new(),
                cmd: regs.cmd,
                w0: regs.w0,
                answer,
                busy_polls: 0,
                fail_write_to: None,
            }
        }
    }

    impl RegAccess for FakeBus {
        fn read_reg(&mut self, addr: u32) -> Result<u32, ProbeError> {
            self.ops.push(Op::Read(addr));
            if addr == self.cmd && self.busy_polls > 0 {
                self.busy_polls -= 1;
                return Ok(CMD_USR);
            }
            Ok(self.regs.get(&addr).copied().unwrap_or(0))
        }

        fn write_reg(&mut self, addr: u32, value: u32) -> Result<(), ProbeError> {
            self.ops.push(Op::Write(addr, value));
            if self.fail_write_to == Some(addr) {
                return Err(ProbeError::Disconnected);
            }
            if addr == self.cmd && value & CMD_USR != 0 {
                self.regs.insert(self.w0, self.answer);
                self.regs.insert(self.cmd, 0);
            } else {
                self.regs.insert(addr, value);
            }
            Ok(())
        }
    }

    /// SPI1 of the ESP32-C3/S3 in espflash 4.5.0 (base 0x6000_2000).
    fn c3_regs() -> SpiRegs {
        SpiRegs {
            cmd: 0x6000_2000,
            usr: 0x6000_2018,
            usr1: 0x6000_201C,
            usr2: 0x6000_2020,
            w0: 0x6000_2058,
            mosi_dlen: Some(0x6000_2024),
            miso_dlen: Some(0x6000_2028),
        }
    }

    #[test]
    fn rdid_restores_spi_user_registers() {
        let regs = c3_regs();
        // W0 after RDID on the ESP32-C6 fixture board: 68 40 18 (LSB first).
        let mut bus = FakeBus::new(&regs, 0x0018_4068);
        bus.busy_polls = 2;
        let id = read_jedec(&regs, &mut bus).unwrap();
        assert_eq!(id.to_string(), "0x684018");

        let expected = [
            Op::Read(regs.usr),
            Op::Read(regs.usr2),
            Op::Write(regs.usr, USR_COMMAND | USR_MISO),
            Op::Write(regs.usr2, (7 << 28) | 0x9F),
            Op::Write(0x6000_2028, 23),
            Op::Write(regs.w0, 0),
            Op::Write(regs.cmd, 1 << 18),
            Op::Read(regs.cmd),
            Op::Read(regs.cmd),
            Op::Read(regs.cmd),
            Op::Read(regs.w0),
            Op::Write(regs.usr, 0x8000_00C0),
            Op::Write(regs.usr2, 0x7000_0005),
        ];
        assert_eq!(bus.ops, expected);
    }

    #[test]
    fn rdid_without_length_registers_uses_usr1() {
        let regs = SpiRegs {
            mosi_dlen: None,
            miso_dlen: None,
            ..c3_regs()
        };
        let mut bus = FakeBus::new(&regs, 0x0016_405E);
        let id = read_jedec(&regs, &mut bus).unwrap();
        assert_eq!(id.to_string(), "0x5E4016");
        assert!(bus.ops.contains(&Op::Write(regs.usr1, 23 << 8)));
        assert!(
            !bus.ops
                .iter()
                .any(|op| matches!(op, Op::Write(addr, _) if *addr == 0x6000_2028))
        );
    }

    #[test]
    fn rdid_restores_even_when_the_transaction_fails() {
        let regs = c3_regs();
        let mut bus = FakeBus::new(&regs, 0);
        bus.fail_write_to = Some(regs.w0);
        assert_eq!(read_jedec(&regs, &mut bus), Err(ProbeError::Disconnected));
        let tail = &bus.ops[bus.ops.len() - 2..];
        assert_eq!(
            tail,
            [
                Op::Write(regs.usr, 0x8000_00C0),
                Op::Write(regs.usr2, 0x7000_0005)
            ]
        );
    }

    #[test]
    fn rdid_times_out_when_the_controller_never_finishes() {
        let regs = c3_regs();
        let mut bus = FakeBus::new(&regs, 0);
        bus.busy_polls = u32::MAX;
        assert_eq!(read_jedec(&regs, &mut bus), Err(ProbeError::Timeout));
        let cmd_polls = bus
            .ops
            .iter()
            .filter(|op| **op == Op::Read(regs.cmd))
            .count();
        assert_eq!(cmd_polls, MAX_POLLS as usize);
        // Restored anyway.
        assert_eq!(bus.ops.last(), Some(&Op::Write(regs.usr2, 0x7000_0005)));
    }

    #[test]
    fn spi_regs_come_from_espflash() {
        let chip: espflash::target::Chip = match "esp32c3".parse() {
            Ok(chip) => chip,
            Err(error) => panic!("{error}"),
        };
        assert_eq!(SpiRegs::from(chip.spi_registers()), c3_regs());
    }
}
