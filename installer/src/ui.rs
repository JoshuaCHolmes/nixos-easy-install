//! UI module - the graphical installer interface

use eframe::egui;
use std::sync::{Arc, Mutex};
use std::thread;
use crate::system::{self, SystemInfo, ValidationResult};
use crate::config::{self as config_mod, InstallConfig as RealInstallConfig};
use crate::install;

/// Shared state for progress updates from install thread
#[derive(Default, Clone)]
struct InstallProgress {
    progress: f32,
    status: String,
    error: Option<String>,
    complete: bool,
}

/// The main installer application state
pub struct InstallerApp {
    /// Current step in the installation wizard
    step: InstallStep,
    
    /// User's configuration choices
    config: UiInstallConfig,
    
    /// Detected system information (populated on startup)
    system_info: Option<SystemInfo>,
    
    /// Validation results
    validation: Option<ValidationResult>,
    
    /// Installation progress (0.0 - 1.0)
    progress: f32,
    
    /// Status message during installation
    status: String,
    
    /// Any error that occurred
    error: Option<String>,
    
    /// Whether system detection is in progress (reserved for async detection)
    #[allow(dead_code)]
    detecting: bool,
    
    /// Shared progress state for install thread
    install_progress: Arc<Mutex<InstallProgress>>,
    
    /// Whether installation has been started
    install_started: bool,
    
    /// Validation errors for current form
    form_errors: Vec<String>,
}

#[derive(Default, PartialEq)]
enum InstallStep {
    #[default]
    Welcome,
    InstallType,
    Configuration,
    Partitioning,
    UserSetup,
    Summary,
    Installing,
    Complete,
}

#[derive(Default)]
struct UiInstallConfig {
    install_type: InstallType,
    config_source: ConfigSource,
    custom_flake_url: String,
    custom_flake_hostname: String,  // Which nixosConfigurations.<name> to build
    hostname: String,
    username: String,
    password: String,
    password_confirm: String,
    partition_size_gb: u32,
    encrypt: bool,
}

#[derive(Default, PartialEq, Clone)]
enum InstallType {
    #[default]
    Quick,  // Loopback
    Full,   // Partition
}

#[derive(Default, PartialEq, Clone)]
enum ConfigSource {
    #[default]
    Starter,
    Minimal,
    CustomUrl,
    #[allow(dead_code)]
    LocalPath,  // Reserved for future use
}

impl InstallerApp {
    pub fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        // Start system detection immediately
        let (system_info, validation, error) = match system::detect_system() {
            Ok(info) => {
                let validation = system::validate_requirements(&info);
                (Some(info), Some(validation), None)
            }
            Err(e) => (None, None, Some(format!("System detection failed: {}", e))),
        };
        
        Self {
            step: InstallStep::Welcome,
            config: UiInstallConfig {
                partition_size_gb: 64,
                hostname: "nixos".to_string(),
                ..Default::default()
            },
            system_info,
            validation,
            progress: 0.0,
            status: String::new(),
            error,
            detecting: false,
            install_progress: Arc::new(Mutex::new(InstallProgress::default())),
            install_started: false,
            form_errors: Vec::new(),
        }
    }
    
    /// Validate the current form inputs
    fn validate_form(&mut self) -> bool {
        self.form_errors.clear();
        
        // Validate hostname
        if let Err(e) = config_mod::validate_hostname(&self.config.hostname) {
            self.form_errors.push(format!("Hostname: {}", e));
        }
        
        // Validate username
        if let Err(e) = config_mod::validate_username(&self.config.username) {
            self.form_errors.push(format!("Username: {}", e));
        }
        
        // Validate password
        if self.config.password.is_empty() {
            self.form_errors.push("Password cannot be empty".to_string());
        } else if let Err(e) = config_mod::validate_password(&self.config.password) {
            self.form_errors.push(format!("Password: {}", e));
        }
        
        // Password confirmation
        if self.config.password != self.config.password_confirm {
            self.form_errors.push("Passwords do not match".to_string());
        }
        
        // Validate custom URL if selected
        if self.config.config_source == ConfigSource::CustomUrl {
            if let Err(e) = config_mod::validate_git_url(&self.config.custom_flake_url) {
                self.form_errors.push(format!("Git URL: {}", e));
            }
        }
        
        self.form_errors.is_empty()
    }
    
    /// Build the actual InstallConfig from UI state
    fn build_install_config(&self) -> Result<RealInstallConfig, String> {
        let flake_type = match self.config.config_source {
            ConfigSource::Starter => "starter",
            ConfigSource::Minimal => "minimal",
            ConfigSource::CustomUrl => "url",
            ConfigSource::LocalPath => "local",
        };
        
        let flake_url = if self.config.config_source == ConfigSource::CustomUrl {
            Some(self.config.custom_flake_url.clone())
        } else {
            None
        };
        
        let password_hash = config_mod::hash_password(&self.config.password);
        
        match self.config.install_type {
            InstallType::Quick => {
                RealInstallConfig::new_loopback(
                    self.config.hostname.clone(),
                    self.config.username.clone(),
                    password_hash,
                    flake_type,
                    flake_url,
                    self.config.partition_size_gb,
                ).map_err(|e| e.to_string())
            }
            InstallType::Full => {
                // Full partition install - not yet supported
                Err("Full partition installation is not yet supported. Please use Quick Install.".to_string())
            }
        }
    }
    
    /// Run a dry-run to test without making changes
    fn run_dry_run(&mut self) {
        let config = match self.build_install_config() {
            Ok(c) => c,
            Err(e) => {
                self.form_errors.push(format!("Config error: {}", e));
                return;
            }
        };
        
        let system_info = match &self.system_info {
            Some(info) => info.clone(),
            None => {
                self.form_errors.push("System info not available".to_string());
                return;
            }
        };
        
        // Run dry-run synchronously (it's quick)
        let rt = tokio::runtime::Runtime::new().unwrap();
        let progress_callback: install::ProgressCallback = Box::new(|_, _| {});
        
        match rt.block_on(install::dry_run(config, &system_info, progress_callback)) {
            Ok(report) => {
                self.form_errors.clear();
                if report.passed {
                    self.status = "✓ Dry run passed - ready to install!".to_string();
                } else {
                    for step in &report.steps {
                        if !step.passed {
                            if let Some(ref err) = step.error {
                                self.form_errors.push(format!("{}: {}", step.name, err));
                            }
                        }
                    }
                }
                for warning in report.warnings {
                    self.form_errors.push(format!("Warning: {}", warning));
                }
            }
            Err(e) => {
                self.form_errors.push(format!("Dry run failed: {}", e));
            }
        }
    }
    
    /// Start the installation in a background thread
    fn start_installation(&mut self) {
        if self.install_started {
            return;
        }
        
        // Build the config
        let config = match self.build_install_config() {
            Ok(c) => c,
            Err(e) => {
                self.error = Some(e);
                return;
            }
        };
        
        let system_info = match &self.system_info {
            Some(info) => info.clone(),
            None => {
                self.error = Some("System info not available".to_string());
                return;
            }
        };
        
        let progress_state = self.install_progress.clone();
        
        self.install_started = true;
        
        // Spawn installation thread
        thread::spawn(move || {
            // Create a runtime for async code
            let rt = tokio::runtime::Runtime::new().unwrap();
            
            rt.block_on(async {
                let progress_callback: install::ProgressCallback = Box::new({
                    let progress_state = progress_state.clone();
                    move |progress, status| {
                        if let Ok(mut state) = progress_state.lock() {
                            state.progress = progress;
                            state.status = status.to_string();
                        }
                    }
                });
                
                match install::install(config, &system_info, progress_callback).await {
                    Ok(()) => {
                        if let Ok(mut state) = progress_state.lock() {
                            state.complete = true;
                            state.progress = 1.0;
                            state.status = "Installation complete!".to_string();
                        }
                    }
                    Err(e) => {
                        if let Ok(mut state) = progress_state.lock() {
                            state.error = Some(e.to_string());
                        }
                    }
                }
            });
        });
    }
    
    fn render_welcome(&mut self, ui: &mut egui::Ui) {
        ui.vertical_centered(|ui| {
            ui.add_space(20.0);
            
            ui.heading("Welcome to NixOS");
            
            ui.add_space(20.0);
            
            ui.label("This installer will set up NixOS alongside your existing Windows installation.");
            
            ui.add_space(20.0);
            
            // Show system detection results
            if let Some(ref info) = self.system_info {
                ui.group(|ui| {
                    ui.heading("System Detected");
                    ui.add_space(10.0);
                    
                    egui::Grid::new("system_info")
                        .num_columns(2)
                        .spacing([20.0, 5.0])
                        .show(ui, |ui| {
                            ui.label("Windows:");
                            ui.label(&info.windows_version);
                            ui.end_row();
                            
                            ui.label("Boot Mode:");
                            ui.label(if info.is_uefi { "UEFI ✓" } else { "Legacy BIOS" });
                            ui.end_row();
                            
                            ui.label("Secure Boot:");
                            ui.label(if info.secure_boot_enabled { "Enabled" } else { "Disabled" });
                            ui.end_row();
                            
                            ui.label("Memory:");
                            ui.label(system::format_bytes(info.total_memory));
                            ui.end_row();
                            
                            if let Some(ref esp) = info.esp {
                                ui.label("EFI Partition:");
                                ui.label(format!("{} free", system::format_bytes(esp.free_space)));
                                ui.end_row();
                            }
                        });
                });
                
                ui.add_space(10.0);
                
                // Show validation results
                if let Some(ref validation) = self.validation {
                    if !validation.errors.is_empty() {
                        ui.group(|ui| {
                            ui.colored_label(egui::Color32::RED, "❌ Requirements not met:");
                            for err in &validation.errors {
                                ui.label(format!("  • {}", err));
                            }
                        });
                    }
                    
                    if !validation.warnings.is_empty() {
                        ui.group(|ui| {
                            ui.colored_label(egui::Color32::YELLOW, "⚠ Warnings:");
                            for warn in &validation.warnings {
                                ui.label(format!("  • {}", warn));
                            }
                        });
                    }
                    
                    if validation.passed {
                        ui.colored_label(egui::Color32::GREEN, "✓ System meets requirements");
                    }
                }
            } else if let Some(ref err) = self.error {
                ui.colored_label(egui::Color32::RED, err);
            } else {
                ui.spinner();
                ui.label("Detecting system...");
            }
            
            ui.add_space(20.0);
            
            // Only allow proceeding if validation passed
            let can_proceed = self.validation.as_ref().map(|v| v.passed).unwrap_or(false);
            
            ui.add_enabled_ui(can_proceed, |ui| {
                if ui.button("Get Started →").clicked() {
                    self.step = InstallStep::InstallType;
                }
            });
            
            if !can_proceed && self.validation.is_some() {
                ui.add_space(5.0);
                ui.small("Please resolve the errors above before continuing.");
            }
        });
    }
    
    fn render_install_type(&mut self, ui: &mut egui::Ui) {
        ui.heading("Choose Installation Type");
        
        ui.add_space(20.0);
        
        ui.group(|ui| {
            ui.horizontal(|ui| {
                ui.radio_value(&mut self.config.install_type, InstallType::Quick, "");
                ui.vertical(|ui| {
                    ui.strong("Quick Install (Recommended for trying NixOS)");
                    ui.label("• No partition changes - installs inside a file on Windows");
                    ui.label("• Easy to remove - just delete the folder");
                    ui.label("• Slight performance overhead");
                });
            });
        });
        
        ui.add_space(10.0);
        
        ui.group(|ui| {
            ui.horizontal(|ui| {
                ui.radio_value(&mut self.config.install_type, InstallType::Full, "");
                ui.vertical(|ui| {
                    ui.strong("Full Install (Recommended for daily use)");
                    ui.label("• Creates a dedicated partition for NixOS");
                    ui.label("• Full native performance");
                    ui.label("• Requires shrinking your Windows partition");
                });
            });
        });
        
        ui.add_space(30.0);
        
        self.render_nav_buttons(ui, Some(InstallStep::Welcome), Some(InstallStep::Configuration));
    }
    
    fn render_configuration(&mut self, ui: &mut egui::Ui) {
        ui.heading("Choose Your Configuration");
        
        ui.add_space(20.0);
        
        ui.label("NixOS uses a declarative configuration. Choose how you want to start:");
        
        ui.add_space(10.0);
        
        ui.group(|ui| {
            ui.radio_value(&mut self.config.config_source, ConfigSource::Starter, "");
            ui.label("Starter Config - A well-documented setup with sensible defaults");
            ui.label("  → Will be forked to your GitHub for customization");
        });
        
        ui.add_space(5.0);
        
        ui.group(|ui| {
            ui.radio_value(&mut self.config.config_source, ConfigSource::Minimal, "");
            ui.label("Minimal Config - Bare NixOS, just enough to boot");
            ui.label("  → For experienced users who want a blank slate");
        });
        
        ui.add_space(5.0);
        
        ui.group(|ui| {
            ui.radio_value(&mut self.config.config_source, ConfigSource::CustomUrl, "");
            ui.label("Custom Flake URL - Bring your own configuration");
            if self.config.config_source == ConfigSource::CustomUrl {
                ui.add_space(5.0);
                ui.horizontal(|ui| {
                    ui.label("Git URL:");
                    ui.text_edit_singleline(&mut self.config.custom_flake_url);
                });
                ui.horizontal(|ui| {
                    ui.label("Config name:");
                    ui.text_edit_singleline(&mut self.config.custom_flake_hostname);
                });
                ui.small("The nixosConfigurations.<name> to build (e.g., 'jch-wsl', 'laptop')");
            }
        });
        
        ui.add_space(30.0);
        
        let next_step = if self.config.install_type == InstallType::Full {
            InstallStep::Partitioning
        } else {
            InstallStep::UserSetup
        };
        
        self.render_nav_buttons(ui, Some(InstallStep::InstallType), Some(next_step));
    }
    
    fn render_partitioning(&mut self, ui: &mut egui::Ui) {
        ui.heading("Partition Setup");
        
        ui.add_space(20.0);
        
        ui.label("How much space would you like to allocate to NixOS?");
        
        ui.add_space(10.0);
        
        ui.horizontal(|ui| {
            ui.label("Size (GB):");
            ui.add(egui::Slider::new(&mut self.config.partition_size_gb, 20..=500));
        });
        
        ui.add_space(10.0);
        
        // TODO: Show disk visualization
        ui.label("TODO: Disk space visualization");
        
        ui.add_space(10.0);
        
        ui.checkbox(&mut self.config.encrypt, "Encrypt the NixOS partition (LUKS)");
        
        ui.add_space(30.0);
        
        self.render_nav_buttons(ui, Some(InstallStep::Configuration), Some(InstallStep::UserSetup));
    }
    
    fn render_user_setup(&mut self, ui: &mut egui::Ui) {
        ui.heading("User Setup");
        
        ui.add_space(20.0);
        
        egui::Grid::new("user_setup_grid")
            .num_columns(2)
            .spacing([10.0, 10.0])
            .show(ui, |ui| {
                ui.label("Hostname:");
                ui.text_edit_singleline(&mut self.config.hostname);
                ui.end_row();
                
                ui.label("Username:");
                ui.text_edit_singleline(&mut self.config.username);
                ui.end_row();
                
                ui.label("Password:");
                ui.add(egui::TextEdit::singleline(&mut self.config.password).password(true));
                ui.end_row();
                
                ui.label("Confirm Password:");
                ui.add(egui::TextEdit::singleline(&mut self.config.password_confirm).password(true));
                ui.end_row();
            });
        
        if !self.config.password.is_empty() 
            && self.config.password != self.config.password_confirm 
        {
            ui.add_space(10.0);
            ui.colored_label(egui::Color32::RED, "Passwords do not match");
        }
        
        // Disk size for Quick Install
        if self.config.install_type == InstallType::Quick {
            ui.add_space(20.0);
            ui.separator();
            ui.add_space(10.0);
            
            ui.label("How much space for NixOS?");
            ui.add_space(5.0);
            
            ui.horizontal(|ui| {
                ui.label("Size:");
                ui.add(egui::Slider::new(&mut self.config.partition_size_gb, 20..=500).suffix(" GB"));
            });
            
            ui.small("This creates a disk image file on your Windows drive. You can resize it later.");
        }
        
        ui.add_space(30.0);
        
        let prev_step = if self.config.install_type == InstallType::Full {
            InstallStep::Partitioning
        } else {
            InstallStep::Configuration
        };
        
        self.render_nav_buttons(ui, Some(prev_step), Some(InstallStep::Summary));
    }
    
    fn render_summary(&mut self, ui: &mut egui::Ui) {
        ui.heading("Summary");
        
        ui.add_space(20.0);
        
        ui.label("Please review your choices:");
        
        ui.add_space(10.0);
        
        egui::Grid::new("summary_grid")
            .num_columns(2)
            .spacing([20.0, 5.0])
            .show(ui, |ui| {
                ui.strong("Installation Type:");
                ui.label(match self.config.install_type {
                    InstallType::Quick => "Quick (Loopback)",
                    InstallType::Full => "Full (Partition)",
                });
                ui.end_row();
                
                ui.strong("Configuration:");
                ui.label(match self.config.config_source {
                    ConfigSource::Starter => "Starter Config",
                    ConfigSource::Minimal => "Minimal Config",
                    ConfigSource::CustomUrl => &self.config.custom_flake_url,
                    ConfigSource::LocalPath => "Local Path",
                });
                ui.end_row();
                
                ui.strong("Hostname:");
                ui.label(&self.config.hostname);
                ui.end_row();
                
                ui.strong("Username:");
                ui.label(&self.config.username);
                ui.end_row();
                
                if self.config.install_type == InstallType::Quick {
                    ui.strong("Disk Size:");
                    ui.label(format!("{} GB", self.config.partition_size_gb));
                    ui.end_row();
                }
                
                if self.config.install_type == InstallType::Full {
                    ui.strong("Partition Size:");
                    ui.label(format!("{} GB", self.config.partition_size_gb));
                    ui.end_row();
                    
                    ui.strong("Encryption:");
                    ui.label(if self.config.encrypt { "Yes" } else { "No" });
                    ui.end_row();
                }
            });
        
        // Show validation errors if any
        if !self.form_errors.is_empty() {
            ui.add_space(20.0);
            ui.group(|ui| {
                ui.colored_label(egui::Color32::RED, "❌ Please fix the following errors:");
                for err in &self.form_errors {
                    ui.label(format!("  • {}", err));
                }
            });
        }
        
        ui.add_space(20.0);
        
        ui.colored_label(
            egui::Color32::YELLOW, 
            "⚠ The installation will modify your system. Make sure you have backups!"
        );
        
        ui.add_space(20.0);
        
        ui.horizontal(|ui| {
            if ui.button("← Back").clicked() {
                self.step = InstallStep::UserSetup;
            }
            
            ui.add_space(10.0);
            
            // Dry-run button to test without making changes
            if ui.button("🔍 Test (Dry Run)").clicked() {
                if self.validate_form() {
                    self.run_dry_run();
                }
            }
            
            ui.add_space(10.0);
            
            // Validate before allowing install
            let install_button = ui.button("Install NixOS");
            if install_button.clicked() {
                if self.validate_form() {
                    self.step = InstallStep::Installing;
                    self.start_installation();
                }
            }
        });
    }
    
    fn render_installing(&mut self, ui: &mut egui::Ui) {
        // Update from shared progress state
        if let Ok(state) = self.install_progress.lock() {
            self.progress = state.progress;
            self.status = state.status.clone();
            if let Some(ref err) = state.error {
                self.error = Some(err.clone());
            }
            if state.complete && self.error.is_none() {
                self.step = InstallStep::Complete;
            }
        }
        
        ui.vertical_centered(|ui| {
            ui.add_space(40.0);
            
            ui.heading("Installing NixOS...");
            
            ui.add_space(20.0);
            
            ui.add(egui::ProgressBar::new(self.progress).show_percentage());
            
            ui.add_space(10.0);
            
            ui.label(&self.status);
            
            if let Some(ref error) = self.error {
                ui.add_space(20.0);
                ui.colored_label(egui::Color32::RED, format!("Error: {}", error));
                
                ui.add_space(20.0);
                if ui.button("← Go Back").clicked() {
                    self.step = InstallStep::Summary;
                    self.install_started = false;
                    self.error = None;
                    *self.install_progress.lock().unwrap() = InstallProgress::default();
                }
            }
        });
        
        // Request repaint while installing to update progress
        ui.ctx().request_repaint();
    }
    
    fn render_complete(&mut self, ui: &mut egui::Ui) {
        ui.vertical_centered(|ui| {
            ui.add_space(40.0);
            
            ui.colored_label(egui::Color32::GREEN, "✓");
            ui.heading("Installation Complete!");
            
            ui.add_space(20.0);
            
            ui.label("NixOS has been prepared on your system.");
            ui.label("Click 'Restart Now' to boot into the NixOS installer,");
            ui.label("which will complete the setup automatically.");
            
            ui.add_space(10.0);
            
            ui.group(|ui| {
                ui.label("After restart:");
                ui.label("  1. Your computer will boot into the NixOS installer");
                ui.label("  2. Installation will complete automatically (5-15 minutes)");
                ui.label("  3. System will reboot into your new NixOS");
            });
            
            ui.add_space(30.0);
            
            ui.horizontal(|ui| {
                if ui.button("Restart Later").clicked() {
                    std::process::exit(0);
                }
                
                ui.add_space(20.0);
                
                if ui.button("Restart Now").clicked() {
                    if let Err(e) = install::reboot() {
                        self.error = Some(format!("Failed to restart: {}", e));
                    }
                }
            });
        });
    }
    
    fn render_nav_buttons(&mut self, ui: &mut egui::Ui, prev: Option<InstallStep>, next: Option<InstallStep>) {
        ui.horizontal(|ui| {
            if let Some(prev_step) = prev {
                if ui.button("← Back").clicked() {
                    self.step = prev_step;
                }
            }
            
            if let Some(next_step) = next {
                ui.add_space(20.0);
                if ui.button("Next →").clicked() {
                    self.step = next_step;
                }
            }
        });
    }
}

impl eframe::App for InstallerApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ctx, |ui| {
            // Header
            ui.horizontal(|ui| {
                ui.heading("🐧 NixOS Easy Install");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(format!("v{}", env!("CARGO_PKG_VERSION")));
                });
            });
            
            ui.separator();
            
            // Progress indicator
            ui.horizontal(|ui| {
                let steps = ["Welcome", "Type", "Config", "Partition", "User", "Summary", "Install"];
                let current = match self.step {
                    InstallStep::Welcome => 0,
                    InstallStep::InstallType => 1,
                    InstallStep::Configuration => 2,
                    InstallStep::Partitioning => 3,
                    InstallStep::UserSetup => 4,
                    InstallStep::Summary => 5,
                    InstallStep::Installing | InstallStep::Complete => 6,
                };
                
                for (i, step) in steps.iter().enumerate() {
                    if i > 0 {
                        ui.label("→");
                    }
                    if i == current {
                        ui.strong(*step);
                    } else if i < current {
                        ui.label(egui::RichText::new(*step).color(egui::Color32::GREEN));
                    } else {
                        ui.label(egui::RichText::new(*step).color(egui::Color32::GRAY));
                    }
                }
            });
            
            ui.separator();
            ui.add_space(10.0);
            
            // Main content
            match self.step {
                InstallStep::Welcome => self.render_welcome(ui),
                InstallStep::InstallType => self.render_install_type(ui),
                InstallStep::Configuration => self.render_configuration(ui),
                InstallStep::Partitioning => self.render_partitioning(ui),
                InstallStep::UserSetup => self.render_user_setup(ui),
                InstallStep::Summary => self.render_summary(ui),
                InstallStep::Installing => self.render_installing(ui),
                InstallStep::Complete => self.render_complete(ui),
            }
        });
    }
}
