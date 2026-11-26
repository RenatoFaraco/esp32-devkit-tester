//! Serial ports and raw bytes.
//!
//! Finds ports by USB VID:PID, serial number and description (never by name such as `COMx` or
//! `ttyACMx`), groups the ports of one board, and survives ports that disappear and come back
//! under another name. It may use `platform`, but must not import `probe`, `runner`, `report`,
//! `ui`, `ratatui` or `crossterm`.
//!
//! # Enumeration and classification (H2)
//!
//! Listing ports is the only thing that happens without a user action (ADR-008): it asks the OS
//! for the device list and never opens a port. [`enumerate`] is the only function here with I/O;
//! [`classify`] is pure and decides only from VID:PID and the USB interface number, using the
//! [`KNOWN_USB`] table (ADR-004: a new bridge is a new row, not an `if`).
//!
//! What `serialport` 4.10.1 fills in `UsbPortInfo` (read in its source):
//! - **Windows:** VID, PID, interface and serial are parsed from the device instance ID. For a
//!   composite device (`...&MI_xx`, e.g. `303A:1001` on `MI_00`) the serial comes from the
//!   **parent** composite device, so the native port does carry the MAC (`:` is accepted).
//!   Interface is only known when the ID has `MI_xx`: with FTDI's own driver (`FTDIBUS\...`)
//!   it is `None`. A device without a USB serial (CH340K) gets the first token of the
//!   Windows-generated instance ID (e.g. `"5"`, serialport issue #203), not `None`.
//!   `manufacturer` is the driver's manufacturer and `product` the friendly name (`"... (COM5)"`).
//! - **Linux:** udev properties (`ID_USB_INTERFACE_NUM` for the interface).
//! - **macOS:** IOKit `bInterfaceNumber`; on CDC-ACM it may be the data interface, not the
//!   communication one.
//!
//! The descriptor strings are untrusted input: they are stored as received and sanitized by
//! `report::SafeText` at the model's edge. They never decide a class.

use std::fmt;

/// USB identity of a serial port, as reported by the OS.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsbIds {
    /// USB Vendor ID.
    pub vid: u16,
    /// USB Product ID.
    pub pid: u16,
    /// USB serial number as the OS reports it (untrusted; MAC on the native port).
    pub serial: Option<String>,
    /// Manufacturer string as the OS reports it (untrusted).
    pub manufacturer: Option<String>,
    /// Product string as the OS reports it (untrusted).
    pub product: Option<String>,
    /// USB interface number of this port, when the OS exposes it.
    pub interface: Option<u8>,
}

/// One serial port found by [`enumerate`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortDesc {
    /// OS name used only to open the port and in stderr messages; never used to classify.
    pub name: String,
    /// USB identity; `None` for a port that is not USB (motherboard UART, Bluetooth, PCI).
    pub usb: Option<UsbIds>,
}

/// What kind of USB device a known VID:PID is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortKind {
    /// The chip's own USB-Serial/JTAG peripheral.
    Native,
    /// A USB-UART bridge wired to UART0.
    Bridge,
}

/// How the serial ports of a known device map to USB interfaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Interfaces {
    /// Any serial port of this device is the UART (or the native port).
    Single,
    /// Multi-channel chip: only `uart` is the UART; `jtag` is the JTAG channel (FT2232 on an
    /// ESP-Prog).
    Split {
        /// Interface number of the UART channel.
        uart: u8,
        /// Interface number of the JTAG channel.
        jtag: u8,
    },
}

/// One row of the VID:PID table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UsbEntry {
    /// USB Vendor ID.
    pub vid: u16,
    /// USB Product ID.
    pub pid: u16,
    /// Native port or bridge.
    pub kind: PortKind,
    /// Bridge model, with the exact names of `report-json.md` §3.2; `None` on the native port.
    pub bridge_model: Option<&'static str>,
    /// Which interfaces are the UART.
    pub interfaces: Interfaces,
}

/// Known USB devices; must list every VID:PID of `docs/context/hardware.md` (checked by test).
pub const KNOWN_USB: &[UsbEntry] = &[
    UsbEntry {
        vid: 0x303A,
        pid: 0x1001,
        kind: PortKind::Native,
        bridge_model: None,
        interfaces: Interfaces::Single,
    },
    bridge(0x10C4, 0xEA60, "CP210x", Interfaces::Single),
    bridge(0x1A86, 0x7523, "CH340", Interfaces::Single),
    bridge(0x1A86, 0x7522, "CH340K", Interfaces::Single),
    bridge(0x1A86, 0x55D3, "CH343", Interfaces::Single),
    bridge(0x1A86, 0x55D4, "CH9102", Interfaces::Single),
    bridge(
        0x0403,
        0x6010,
        "FTDI",
        Interfaces::Split { uart: 1, jtag: 0 },
    ),
];

const fn bridge(vid: u16, pid: u16, model: &'static str, interfaces: Interfaces) -> UsbEntry {
    UsbEntry {
        vid,
        pid,
        kind: PortKind::Bridge,
        bridge_model: Some(model),
        interfaces,
    }
}

/// Looks up a VID:PID in [`KNOWN_USB`].
pub fn lookup(vid: u16, pid: u16) -> Option<&'static UsbEntry> {
    KNOWN_USB
        .iter()
        .find(|entry| entry.vid == vid && entry.pid == pid)
}

/// What a port is, decided only from VID:PID and the interface number.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortClass {
    /// USB-Serial/JTAG native port (`303A:1001`).
    Native,
    /// Known USB-UART bridge. `ambiguous` means a multi-channel chip whose interface number the
    /// OS did not give: it is listed but not probed.
    Bridge {
        /// Model name from [`KNOWN_USB`].
        model: &'static str,
        /// True when it cannot tell whether this is the UART channel.
        ambiguous: bool,
    },
    /// JTAG channel of a multi-channel bridge (FT2232 interface 0); never opened as a UART.
    JtagInterface {
        /// Model name from [`KNOWN_USB`].
        model: &'static str,
    },
    /// Not a USB port (motherboard UART, Bluetooth, PCI); listed, never probed.
    NotUsb,
    /// USB device not in [`KNOWN_USB`]; listed, never opened.
    Unknown {
        /// USB Vendor ID.
        vid: u16,
        /// USB Product ID.
        pid: u16,
    },
}

impl PortClass {
    /// True if the port may be opened to talk to the ROM (after the user's action, ADR-008).
    pub fn is_probe_candidate(&self) -> bool {
        matches!(
            self,
            PortClass::Native
                | PortClass::Bridge {
                    ambiguous: false,
                    ..
                }
        )
    }

    /// Bridge model for the report (`bridge_model`), if this is a bridge.
    pub fn bridge_model(&self) -> Option<&'static str> {
        match self {
            PortClass::Bridge { model, .. } | PortClass::JtagInterface { model } => Some(model),
            PortClass::Native | PortClass::NotUsb | PortClass::Unknown { .. } => None,
        }
    }
}

/// Classifies a port. Pure: ignores the port name and the descriptor strings.
pub fn classify(port: &PortDesc) -> PortClass {
    let Some(usb) = &port.usb else {
        return PortClass::NotUsb;
    };
    let Some(entry) = lookup(usb.vid, usb.pid) else {
        return PortClass::Unknown {
            vid: usb.vid,
            pid: usb.pid,
        };
    };
    match (entry.kind, entry.bridge_model) {
        (PortKind::Native, _) => PortClass::Native,
        (PortKind::Bridge, model) => {
            // A bridge row always has a model; fall back to a neutral name rather than panic.
            let model = model.unwrap_or("bridge");
            match entry.interfaces {
                Interfaces::Single => PortClass::Bridge {
                    model,
                    ambiguous: false,
                },
                Interfaces::Split { uart, jtag } => match usb.interface {
                    Some(i) if i == jtag => PortClass::JtagInterface { model },
                    Some(i) if i == uart => PortClass::Bridge {
                        model,
                        ambiguous: false,
                    },
                    _ => PortClass::Bridge {
                        model,
                        ambiguous: true,
                    },
                },
            }
        }
    }
}

/// The OS could not list the serial ports.
#[derive(Debug)]
pub struct EnumerateError(serialport::Error);

impl fmt::Display for EnumerateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "could not list serial ports: {}", self.0)
    }
}

impl std::error::Error for EnumerateError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.0)
    }
}

/// Lists the serial ports the OS knows about, without opening any. Never panics.
///
/// On an OS error the caller gets the error (and therefore an empty list) to show in stderr.
pub fn enumerate() -> Result<Vec<PortDesc>, EnumerateError> {
    serialport::available_ports()
        .map(|ports| ports.into_iter().map(port_desc).collect())
        .map_err(EnumerateError)
}

fn port_desc(info: serialport::SerialPortInfo) -> PortDesc {
    let usb = match info.port_type {
        serialport::SerialPortType::UsbPort(usb) => Some(UsbIds {
            vid: usb.vid,
            pid: usb.pid,
            serial: usb.serial_number,
            manufacturer: usb.manufacturer,
            product: usb.product,
            interface: usb.interface,
        }),
        serialport::SerialPortType::PciPort
        | serialport::SerialPortType::BluetoothPort
        | serialport::SerialPortType::Unknown => None,
    };
    PortDesc {
        name: info.port_name,
        usb,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usb_port(name: &str, vid: u16, pid: u16, interface: Option<u8>) -> PortDesc {
        PortDesc {
            name: name.to_owned(),
            usb: Some(UsbIds {
                vid,
                pid,
                serial: None,
                manufacturer: None,
                product: None,
                interface,
            }),
        }
    }

    #[test]
    fn native_port_is_probe_candidate() {
        for interface in [None, Some(0), Some(1)] {
            let class = classify(&usb_port("p", 0x303A, 0x1001, interface));
            assert_eq!(class, PortClass::Native);
            assert!(class.is_probe_candidate());
            assert_eq!(class.bridge_model(), None);
        }
    }

    #[test]
    fn known_bridges_report_spec_model_names() {
        let cases = [
            (0x10C4, 0xEA60, "CP210x"),
            (0x1A86, 0x7523, "CH340"),
            (0x1A86, 0x7522, "CH340K"),
            (0x1A86, 0x55D3, "CH343"),
            (0x1A86, 0x55D4, "CH9102"),
        ];
        for (vid, pid, model) in cases {
            let class = classify(&usb_port("p", vid, pid, None));
            assert_eq!(
                class,
                PortClass::Bridge {
                    model,
                    ambiguous: false
                }
            );
            assert!(class.is_probe_candidate());
            assert_eq!(class.bridge_model(), Some(model));
        }
        let ftdi = classify(&usb_port("p", 0x0403, 0x6010, Some(1)));
        assert_eq!(ftdi.bridge_model(), Some("FTDI"));
    }

    #[test]
    fn every_bridge_row_has_a_model_and_native_has_none() {
        for entry in KNOWN_USB {
            match entry.kind {
                PortKind::Bridge => assert!(entry.bridge_model.is_some(), "{entry:?}"),
                PortKind::Native => assert!(entry.bridge_model.is_none(), "{entry:?}"),
            }
        }
    }

    #[test]
    fn unknown_vid_pid_is_listed_not_dropped() {
        let port = usb_port("p", 0x1234, 0x5678, None);
        let class = classify(&port);
        assert_eq!(
            class,
            PortClass::Unknown {
                vid: 0x1234,
                pid: 0x5678
            }
        );
        assert!(!class.is_probe_candidate());
    }

    #[test]
    fn non_usb_port_is_listed_not_probed() {
        let port = PortDesc {
            name: "COM1".to_owned(),
            usb: None,
        };
        let class = classify(&port);
        assert_eq!(class, PortClass::NotUsb);
        assert!(!class.is_probe_candidate());
    }

    #[test]
    fn ft2232_only_uart_interface_is_candidate() {
        let uart = classify(&usb_port("p", 0x0403, 0x6010, Some(1)));
        assert_eq!(
            uart,
            PortClass::Bridge {
                model: "FTDI",
                ambiguous: false
            }
        );
        assert!(uart.is_probe_candidate());

        let jtag = classify(&usb_port("p", 0x0403, 0x6010, Some(0)));
        assert_eq!(jtag, PortClass::JtagInterface { model: "FTDI" });
        assert!(!jtag.is_probe_candidate());
    }

    #[test]
    fn ft2232_without_interface_number_is_not_probed() {
        for interface in [None, Some(7)] {
            let class = classify(&usb_port("p", 0x0403, 0x6010, interface));
            assert_eq!(
                class,
                PortClass::Bridge {
                    model: "FTDI",
                    ambiguous: true
                }
            );
            assert!(!class.is_probe_candidate());
        }
    }

    #[test]
    fn classification_ignores_port_name() {
        for (vid, pid) in [(0x303A, 0x1001), (0x10C4, 0xEA60), (0xDEAD, 0xBEEF)] {
            let reference = classify(&usb_port("COM3", vid, pid, None));
            for name in ["/dev/ttyACM0", "CP210x", "/dev/cu.usbmodem1101", ""] {
                assert_eq!(classify(&usb_port(name, vid, pid, None)), reference);
            }
        }
    }

    #[test]
    fn classification_ignores_descriptor_strings() {
        let plain = usb_port("p", 0xDEAD, 0xBEEF, None);
        let mut lying = plain.clone();
        if let Some(usb) = lying.usb.as_mut() {
            usb.manufacturer = Some("Silicon Labs".to_owned());
            usb.product = Some("CP2102 USB to UART Bridge \u{1b}[2J".to_owned());
            usb.serial = Some("303A:1001".to_owned());
        }
        assert_eq!(classify(&lying), classify(&plain));
    }

    #[test]
    fn os_info_is_converted_without_touching_strings() {
        let info = serialport::SerialPortInfo {
            port_name: "COM7".to_owned(),
            port_type: serialport::SerialPortType::UsbPort(serialport::UsbPortInfo {
                vid: 0x303A,
                pid: 0x1001,
                serial_number: Some("E8:F6:0A:F7:FF:D4".to_owned()),
                manufacturer: Some("Microsoft".to_owned()),
                product: Some("USB Serial Device (COM7)\u{7}".to_owned()),
                interface: Some(0),
            }),
        };
        let desc = port_desc(info);
        assert_eq!(desc.name, "COM7");
        let usb = desc.usb.as_ref().expect("usb port");
        assert_eq!((usb.vid, usb.pid, usb.interface), (0x303A, 0x1001, Some(0)));
        assert_eq!(usb.serial.as_deref(), Some("E8:F6:0A:F7:FF:D4"));
        assert_eq!(
            usb.product.as_deref(),
            Some("USB Serial Device (COM7)\u{7}")
        );

        for port_type in [
            serialport::SerialPortType::PciPort,
            serialport::SerialPortType::BluetoothPort,
            serialport::SerialPortType::Unknown,
        ] {
            let desc = port_desc(serialport::SerialPortInfo {
                port_name: "/dev/ttyS0".to_owned(),
                port_type,
            });
            assert_eq!(desc.usb, None);
            assert_eq!(classify(&desc), PortClass::NotUsb);
        }
    }

    #[test]
    fn enumerate_does_not_panic() {
        // Runs for real, with or without boards; an OS error is a value, not a panic.
        match enumerate() {
            Ok(ports) => {
                for port in &ports {
                    let _ = classify(port);
                }
            }
            Err(error) => assert!(!error.to_string().is_empty()),
        }
    }

    /// Manual check with boards plugged in (only lists, never opens a port):
    /// `cargo test --manifest-path host/Cargo.toml -- --ignored --nocapture prints_real_ports`.
    /// `{:?}` escapes control characters, so descriptor strings cannot reach the terminal raw.
    #[test]
    #[ignore = "needs boards plugged in; run by hand"]
    fn prints_real_ports() {
        match enumerate() {
            Ok(ports) => {
                println!("{} port(s)", ports.len());
                for port in &ports {
                    let class = classify(port);
                    println!(
                        "{port:?}\n    -> {class:?} (candidate: {})",
                        class.is_probe_candidate()
                    );
                }
            }
            Err(error) => println!("{error}"),
        }
    }
}
