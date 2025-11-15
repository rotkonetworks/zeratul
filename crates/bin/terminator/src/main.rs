//! Terminator - High-performance native trading terminal
//!
//! Mouse-controlled GUI for professional trading
//! Built with egui for native performance

use anyhow::Result;
use terminator::shell::egui::EguiShell;

#[tokio::main]
async fn main() -> Result<()> {
    eprintln!("🚀 Terminator - Penumbra DEX Trading Terminal");
    eprintln!("Loading wallet from pcli...");

    // Create egui shell
    let shell = EguiShell::new().await?;

    eprintln!("✓ Initialized");
    eprintln!("Opening GUI window...");

    // Run the GUI (blocks until window closes)
    shell.run()?;

    Ok(())
}
