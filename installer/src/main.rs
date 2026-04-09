//! NixOS Easy Install - Windows Installer
//! 
//! A graphical installer that sets up NixOS alongside Windows.

mod ui;
mod install;
mod config;
mod system;
mod loopback;
mod bootloader;
mod assets;
mod switching;

use anyhow::Result;
use tracing::info;

fn main() -> Result<()> {
    // Initialize logging
    tracing_subscriber::fmt::init();
    
    info!("NixOS Easy Install starting...");
    
    // Check if running as administrator
    if !system::is_admin() {
        // Re-launch with admin privileges
        system::elevate()?;
        return Ok(());
    }
    
    // Run the GUI
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([800.0, 600.0])
            .with_min_inner_size([600.0, 400.0])
            .with_icon(load_icon()),
        ..Default::default()
    };
    
    eframe::run_native(
        "NixOS Easy Install",
        options,
        Box::new(|cc| Ok(Box::new(ui::InstallerApp::new(cc)))),
    ).map_err(|e| anyhow::anyhow!("GUI error: {}", e))?;
    
    Ok(())
}

fn load_icon() -> egui::IconData {
    // TODO: Load actual NixOS icon
    egui::IconData::default()
}
