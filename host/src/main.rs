//! `devkit-tester` binary: a thin entry point over the library.

fn main() -> anyhow::Result<()> {
    esp32_devkit_tester::ui::run()?;
    Ok(())
}
