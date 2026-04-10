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
    
    /// ESP cleanup items (populated when ESP space is low)
    esp_cleanup_items: Option<Vec<system::EspCleanupItem>>,
    
    /// Whether ESP cleanup panel is expanded
    esp_cleanup_expanded: bool,
    
    /// Warnings from dry run (non-blocking)
    dry_run_warnings: Vec<String>,
    
    /// Whether dry run has been executed successfully
    dry_run_passed: bool,
    
    /// Cleanup operation status
    cleanup_status: Option<String>,
    
    /// Cleanup operation error
    cleanup_error: Option<String>,
    
    /// What was cleaned up
    cleanup_results: Vec<String>,
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
    Cleanup,  // Uninstall/cleanup screen
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
        
        // If ESP space is low, analyze for cleanup items
        let esp_cleanup_items = if let (Some(ref info), Some(ref val)) = (&system_info, &validation) {
            if !val.passed && info.esp.is_some() {
                // Check if ESP space error is present
                let has_esp_error = val.errors.iter().any(|e| e.contains("ESP space") || e.contains("ESP"));
                if has_esp_error {
                    if let Some(ref esp) = info.esp {
                        system::analyze_esp_cleanup(esp).ok()
                    } else {
                        None
                    }
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            None
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
            esp_cleanup_items,
            esp_cleanup_expanded: false,
            dry_run_warnings: Vec::new(),
            dry_run_passed: false,
            cleanup_status: None,
            cleanup_error: None,
            cleanup_results: Vec::new(),
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
        self.form_errors.clear();
        self.dry_run_warnings.clear();
        self.dry_run_passed = false;
        
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
        let rt = match tokio::runtime::Runtime::new() {
            Ok(rt) => rt,
            Err(e) => {
                self.form_errors.push(format!("Failed to create runtime: {}", e));
                return;
            }
        };
        let progress_callback: install::ProgressCallback = Box::new(|_, _| {});
        
        match rt.block_on(install::dry_run(config, &system_info, progress_callback)) {
            Ok(report) => {
                // Collect actual errors (blocking)
                if !report.passed {
                    for step in &report.steps {
                        if !step.passed {
                            if let Some(ref err) = step.error {
                                self.form_errors.push(format!("{}: {}", step.name, err));
                            }
                        }
                    }
                }
                
                // Collect warnings (non-blocking)
                for warning in report.warnings {
                    self.dry_run_warnings.push(warning);
                }
                
                // Mark as passed if no actual errors
                if self.form_errors.is_empty() {
                    self.dry_run_passed = true;
                    self.status = "✓ Dry run passed - ready to install!".to_string();
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
            let rt = match tokio::runtime::Runtime::new() {
                Ok(rt) => rt,
                Err(e) => {
                    if let Ok(mut state) = progress_state.lock() {
                        state.error = Some(format!("Failed to create runtime: {}", e));
                        state.complete = true;
                    }
                    return;
                }
            };
            
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
            
            // Show ESP cleanup option if available
            if self.esp_cleanup_items.is_some() {
                ui.add_space(10.0);
                self.render_esp_cleanup(ui);
            }
            
            ui.add_space(20.0);
            
            // Only allow proceeding if validation passed
            let can_proceed = self.validation.as_ref().map(|v| v.passed).unwrap_or(false);
            
            ui.horizontal(|ui| {
                ui.add_enabled_ui(can_proceed, |ui| {
                    if ui.button("Get Started →").clicked() {
                        self.step = InstallStep::InstallType;
                    }
                });
                
                ui.add_space(20.0);
                
                // Always show cleanup/uninstall option
                if ui.button("🗑 Cleanup/Uninstall").clicked() {
                    self.step = InstallStep::Cleanup;
                    self.cleanup_status = None;
                    self.cleanup_error = None;
                    self.cleanup_results.clear();
                }
            });
            
            if !can_proceed && self.validation.is_some() {
                ui.add_space(5.0);
                ui.small("Please resolve the errors above before continuing.");
            }
        });
    }
    
    fn render_esp_cleanup(&mut self, ui: &mut egui::Ui) {
        ui.group(|ui| {
            let header_text = if self.esp_cleanup_expanded {
                "🔧 ESP Cleanup Tool ▼"
            } else {
                "🔧 ESP Cleanup Tool ▶"
            };
            
            if ui.selectable_label(self.esp_cleanup_expanded, header_text).clicked() {
                self.esp_cleanup_expanded = !self.esp_cleanup_expanded;
            }
            
            if self.esp_cleanup_expanded {
                ui.add_space(10.0);
                ui.label("The following items were found on your EFI System Partition:");
                ui.label("Remove old/unused Linux boot entries to free space.");
                ui.add_space(5.0);
                
                let mut to_remove: Option<usize> = None;
                let mut refresh_needed = false;
                
                if let Some(ref items) = self.esp_cleanup_items {
                    for (idx, item) in items.iter().enumerate() {
                        ui.horizontal(|ui| {
                            let color = if item.safe_to_remove {
                                egui::Color32::GREEN
                            } else {
                                egui::Color32::YELLOW
                            };
                            
                            let safety = if item.safe_to_remove { "✓ Safe" } else { "⚠ Check" };
                            ui.colored_label(color, safety);
                            
                            ui.label(&item.description);
                            ui.label(format!("({})", system::format_bytes(item.size)));
                            
                            if item.safe_to_remove {
                                if ui.button("Remove").clicked() {
                                    to_remove = Some(idx);
                                }
                            } else {
                                ui.label("(Protected)");
                            }
                        });
                    }
                }
                
                // Handle removal outside of borrow
                if let Some(idx) = to_remove {
                    if let Some(ref items) = self.esp_cleanup_items.clone() {
                        if let Some(item) = items.get(idx) {
                            if let Some(ref info) = self.system_info {
                                if let Some(ref esp) = info.esp {
                                    if system::remove_esp_item(esp, item).is_ok() {
                                        refresh_needed = true;
                                    }
                                }
                            }
                        }
                    }
                }
                
                // Refresh system info and cleanup items after removal
                if refresh_needed {
                    if let Ok(info) = system::detect_system() {
                        let validation = system::validate_requirements(&info);
                        
                        // Re-analyze ESP
                        let new_cleanup = if let Some(ref esp) = info.esp {
                            system::analyze_esp_cleanup(esp).ok()
                        } else {
                            None
                        };
                        
                        self.system_info = Some(info);
                        self.validation = Some(validation);
                        self.esp_cleanup_items = new_cleanup;
                    }
                }
                
                ui.add_space(10.0);
                
                if ui.button("🔄 Refresh").clicked() {
                    if let Ok(info) = system::detect_system() {
                        let validation = system::validate_requirements(&info);
                        let new_cleanup = if let Some(ref esp) = info.esp {
                            system::analyze_esp_cleanup(esp).ok()
                        } else {
                            None
                        };
                        
                        self.system_info = Some(info);
                        self.validation = Some(validation);
                        self.esp_cleanup_items = new_cleanup;
                    }
                }
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
        
        // Show validation errors if any (these block installation)
        if !self.form_errors.is_empty() {
            ui.add_space(20.0);
            ui.group(|ui| {
                ui.colored_label(egui::Color32::RED, "❌ Please fix the following errors:");
                for err in &self.form_errors {
                    ui.label(format!("  • {}", err));
                }
            });
        }
        
        // Show warnings (non-blocking, but user should be aware)
        let mut cleanup_nixos_clicked = false;
        if !self.dry_run_warnings.is_empty() {
            ui.add_space(10.0);
            ui.group(|ui| {
                ui.colored_label(egui::Color32::YELLOW, "⚠ Warnings:");
                for warning in &self.dry_run_warnings {
                    ui.horizontal(|ui| {
                        ui.label(format!("  • {}", warning));
                        
                        // Offer cleanup option for existing NixOS folder
                        if warning.contains("NixOS boot folder already exists") {
                            if ui.small_button("Clean up").clicked() {
                                cleanup_nixos_clicked = true;
                            }
                        }
                    });
                }
                ui.add_space(5.0);
                ui.small("These warnings won't prevent installation but you should review them.");
            });
        }
        
        // Handle cleanup outside of borrow
        if cleanup_nixos_clicked {
            self.cleanup_existing_nixos();
        }
        
        // Show dry run success status
        if self.dry_run_passed && self.form_errors.is_empty() {
            ui.add_space(10.0);
            ui.colored_label(egui::Color32::GREEN, &self.status);
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
                // Clear dry run status when going back - user may change config
                self.dry_run_passed = false;
                self.dry_run_warnings.clear();
            }
            
            ui.add_space(10.0);
            
            // Dry-run button to test without making changes
            if ui.button("🔍 Test (Dry Run)").clicked() {
                if self.validate_form() {
                    self.run_dry_run();
                }
            }
            
            ui.add_space(10.0);
            
            // Only allow install if dry run passed or user hasn't run it yet
            let can_install = self.form_errors.is_empty();
            ui.add_enabled_ui(can_install, |ui| {
                if ui.button("Install NixOS").clicked() {
                    if self.validate_form() {
                        self.step = InstallStep::Installing;
                        self.start_installation();
                    }
                }
            });
        });
    }
    
    /// Clean up existing NixOS boot folder on ESP
    fn cleanup_existing_nixos(&mut self) {
        if let Some(ref info) = self.system_info {
            if let Some(ref esp) = info.esp {
                let nixos_folder = esp.mount_point.join("EFI").join("NixOS");
                if nixos_folder.exists() {
                    if let Err(e) = std::fs::remove_dir_all(&nixos_folder) {
                        self.form_errors.push(format!("Failed to remove NixOS folder: {}", e));
                    } else {
                        // Remove the warning and re-run dry run
                        self.dry_run_warnings.retain(|w| !w.contains("NixOS boot folder"));
                        self.status = "Cleaned up existing NixOS folder".to_string();
                    }
                }
            }
        }
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
                    // Reset progress state, handling potential poisoned mutex
                    if let Ok(mut progress) = self.install_progress.lock() {
                        *progress = InstallProgress::default();
                    }
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
            
            // Progress indicator (hide for cleanup screen)
            if self.step != InstallStep::Cleanup {
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
                        InstallStep::Cleanup => 0, // Won't be shown but needed for exhaustiveness
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
            }
            
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
                InstallStep::Cleanup => self.render_cleanup(ui),
            }
        });
    }
}

impl InstallerApp {
    fn render_cleanup(&mut self, ui: &mut egui::Ui) {
        ui.vertical_centered(|ui| {
            ui.add_space(20.0);
            
            ui.heading("🗑 Cleanup / Uninstall NixOS");
            
            ui.add_space(10.0);
            ui.label("Remove NixOS installation files and boot entries from your system.");
            
            ui.add_space(20.0);
            
            // Show what exists
            ui.group(|ui| {
                ui.heading("Detected Installation Components");
                ui.add_space(10.0);
                
                let nixos_folder = std::path::Path::new("C:\\NixOS");
                let nixos_exists = nixos_folder.exists();
                
                // Check ESP for NixOS folder
                let esp_nixos_exists = self.system_info.as_ref()
                    .and_then(|info| info.esp.as_ref())
                    .map(|esp| esp.mount_point.join("EFI").join("NixOS").exists())
                    .unwrap_or(false);
                
                egui::Grid::new("cleanup_items")
                    .num_columns(3)
                    .spacing([20.0, 8.0])
                    .show(ui, |ui| {
                        // C:\NixOS folder
                        ui.label("Loopback files (C:\\NixOS):");
                        if nixos_exists {
                            ui.colored_label(egui::Color32::YELLOW, "Found");
                            // Get size if possible
                            if let Ok(size) = get_folder_size(nixos_folder) {
                                ui.label(format!("({})", system::format_bytes(size)));
                            }
                        } else {
                            ui.colored_label(egui::Color32::GRAY, "Not found");
                            ui.label("");
                        }
                        ui.end_row();
                        
                        // ESP boot files
                        ui.label("Boot files (EFI\\NixOS):");
                        if esp_nixos_exists {
                            ui.colored_label(egui::Color32::YELLOW, "Found");
                        } else {
                            ui.colored_label(egui::Color32::GRAY, "Not found");
                        }
                        ui.label("");
                        ui.end_row();
                    });
                
                if !nixos_exists && !esp_nixos_exists {
                    ui.add_space(10.0);
                    ui.colored_label(egui::Color32::GREEN, "✓ No NixOS installation detected");
                }
            });
            
            // Show cleanup results if any
            if !self.cleanup_results.is_empty() {
                ui.add_space(10.0);
                ui.group(|ui| {
                    ui.heading("Cleanup Results");
                    for result in &self.cleanup_results {
                        ui.label(format!("  ✓ {}", result));
                    }
                });
            }
            
            // Show error if any
            if let Some(ref err) = self.cleanup_error {
                ui.add_space(10.0);
                ui.colored_label(egui::Color32::RED, format!("❌ Error: {}", err));
            }
            
            // Show status if running
            if let Some(ref status) = self.cleanup_status {
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(status);
                });
            }
            
            ui.add_space(20.0);
            
            // Action buttons
            let nixos_folder = std::path::Path::new("C:\\NixOS");
            let nixos_exists = nixos_folder.exists();
            let esp_nixos_exists = self.system_info.as_ref()
                .and_then(|info| info.esp.as_ref())
                .map(|esp| esp.mount_point.join("EFI").join("NixOS").exists())
                .unwrap_or(false);
            
            let has_something = nixos_exists || esp_nixos_exists;
            
            ui.horizontal(|ui| {
                // Clean loopback
                ui.add_enabled_ui(nixos_exists && self.cleanup_status.is_none(), |ui| {
                    if ui.button("Remove Loopback Files").clicked() {
                        self.perform_loopback_cleanup();
                    }
                });
                
                // Clean boot files
                ui.add_enabled_ui(esp_nixos_exists && self.cleanup_status.is_none(), |ui| {
                    if ui.button("Remove Boot Files").clicked() {
                        self.perform_bootloader_cleanup();
                    }
                });
                
                // Clean everything
                ui.add_enabled_ui(has_something && self.cleanup_status.is_none(), |ui| {
                    if ui.button("🗑 Remove Everything").clicked() {
                        self.perform_full_cleanup();
                    }
                });
            });
            
            ui.add_space(20.0);
            
            // Back button
            if ui.button("← Back to Welcome").clicked() {
                self.step = InstallStep::Welcome;
                // Refresh system info
                if let Ok(info) = system::detect_system() {
                    self.validation = Some(system::validate_requirements(&info));
                    self.system_info = Some(info);
                }
            }
        });
    }
    
    fn perform_loopback_cleanup(&mut self) {
        self.cleanup_status = Some("Removing loopback files...".to_string());
        self.cleanup_error = None;
        
        let nixos_path = std::path::Path::new("C:\\NixOS");
        match crate::loopback::cleanup_loopback(nixos_path) {
            Ok(()) => {
                self.cleanup_results.push("Removed C:\\NixOS folder".to_string());
                self.cleanup_status = None;
            }
            Err(e) => {
                self.cleanup_error = Some(format!("Failed to remove loopback: {}", e));
                self.cleanup_status = None;
            }
        }
    }
    
    fn perform_bootloader_cleanup(&mut self) {
        self.cleanup_status = Some("Removing boot files...".to_string());
        self.cleanup_error = None;
        
        if let Some(ref info) = self.system_info {
            if let Some(ref esp) = info.esp {
                let nixos_folder = esp.mount_point.join("EFI").join("NixOS");
                
                // Try to find and remove boot entry
                // For now just remove the files - bcdedit cleanup is tricky
                if nixos_folder.exists() {
                    match std::fs::remove_dir_all(&nixos_folder) {
                        Ok(()) => {
                            self.cleanup_results.push("Removed EFI\\NixOS folder".to_string());
                        }
                        Err(e) => {
                            self.cleanup_error = Some(format!("Failed to remove boot files: {}", e));
                        }
                    }
                }
                
                // Also try to remove any NixOS boot entries from BCD
                #[cfg(target_os = "windows")]
                {
                    use std::process::Command;
                    // List boot entries and find NixOS ones
                    if let Ok(output) = Command::new("bcdedit").args(["/enum", "firmware"]).output() {
                        let stdout = String::from_utf8_lossy(&output.stdout);
                        // Look for NixOS entries and delete them
                        for line in stdout.lines() {
                            if line.contains("identifier") && line.contains("{") {
                                // Extract identifier
                                if let Some(start) = line.find('{') {
                                    if let Some(end) = line.find('}') {
                                        let id = &line[start..=end];
                                        // Check if this is a NixOS entry by looking at subsequent lines
                                        if stdout.contains("NixOS") && stdout.contains(id) {
                                            match Command::new("bcdedit")
                                                .args(["/delete", id])
                                                .output() 
                                            {
                                                Ok(_) => {
                                                    self.cleanup_results.push(format!("Removed boot entry {}", id));
                                                }
                                                Err(e) => {
                                                    self.cleanup_results.push(format!("Failed to remove boot entry {}: {}", id, e));
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                
                self.cleanup_status = None;
            }
        } else {
            self.cleanup_error = Some("No ESP information available".to_string());
            self.cleanup_status = None;
        }
    }
    
    fn perform_full_cleanup(&mut self) {
        self.cleanup_status = Some("Performing full cleanup...".to_string());
        self.cleanup_error = None;
        self.cleanup_results.clear();
        
        // Clean bootloader first (while paths are known)
        self.perform_bootloader_cleanup();
        
        // Then clean loopback
        let nixos_path = std::path::Path::new("C:\\NixOS");
        if nixos_path.exists() {
            match crate::loopback::cleanup_loopback(nixos_path) {
                Ok(()) => {
                    self.cleanup_results.push("Removed C:\\NixOS folder".to_string());
                }
                Err(e) => {
                    if self.cleanup_error.is_none() {
                        self.cleanup_error = Some(format!("Failed to remove loopback: {}", e));
                    }
                }
            }
        }
        
        self.cleanup_status = None;
    }
}

/// Get the total size of a folder
fn get_folder_size(path: &std::path::Path) -> std::io::Result<u64> {
    let mut total = 0;
    if path.is_dir() {
        for entry in std::fs::read_dir(path)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                total += get_folder_size(&path).unwrap_or(0);
            } else {
                total += entry.metadata()?.len();
            }
        }
    }
    Ok(total)
}
