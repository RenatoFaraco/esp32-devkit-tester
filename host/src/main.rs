//! `devkit-tester` binary: a thin entry point over the library.
//!
//! Everything, the exit code included, is decided by `ui::cli::main`, which flushes stdout
//! before it returns; returning an `ExitCode` (rather than calling `std::process::exit`) lets
//! the standard library run its normal shutdown.

use std::process::ExitCode;

fn main() -> ExitCode {
    ExitCode::from(esp32_devkit_tester::ui::cli::main(
        std::env::args_os().skip(1),
    ))
}
