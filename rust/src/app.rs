use std::fs;

use anyhow::{Context, Result};

use crate::discovery::HardwareSnapshot;
use crate::config::{
    Bootloader, CpuVendor, Desktop, EfiPolicy, Filesystem, FirmwareMode, GpuChoice,
    InitSystem, InstallConfig, Kernel, NetworkStack, Partitioning, PrivilegeEscalation,
    SwapMode, UserServices,
};

pub const SETTING_COUNT: usize = 18;
pub const DESKTOP_CHOICES: [Desktop; 20] = [
    Desktop::Plasma, Desktop::Xfce, Desktop::Cinnamon, Desktop::Mate, Desktop::Lxqt,
    Desktop::Moksha, Desktop::Cosmic, Desktop::Hyprland, Desktop::Niri, Desktop::Wayfire,
    Desktop::Labwc, Desktop::Sway, Desktop::SwayFx, Desktop::Umbriel, Desktop::MangoWm,
    Desktop::Cosmalia, Desktop::I3, Desktop::Openbox, Desktop::Fluxbox, Desktop::Qtile,
];
pub const KERNEL_CHOICES: [Kernel; 5] = [
    Kernel::Linux, Kernel::LinuxLts, Kernel::LinuxZen, Kernel::LinuxLqx, Kernel::LinuxCachyos,
];

pub struct App {
    pub config: InstallConfig,
    pub selected: usize,
    pub multi_cursor: usize,
    pub show_plan: bool,
    pub show_help: bool,
    pub status: String,
    pub hardware: HardwareSnapshot,
    pub show_disks: bool,
    pub selected_disk: usize,
}

impl App {
    pub fn new(config: InstallConfig, hardware: HardwareSnapshot) -> Self {
        Self {
            config, selected: 0, multi_cursor: 0, show_plan: false, show_help: false,
            status: "Review the defaults. No target disk is selected; disk operations are blocked.".into(),
            hardware, show_disks: false, selected_disk: 0,
        }
    }

    pub fn toggle_disks(&mut self) {
        self.show_disks = !self.show_disks;
        if self.show_disks {
            self.status = format!("Read-only inventory: {} physical disk(s) detected.", self.hardware.disks.len());
        } else {
            self.status = "Returned to configuration.".into();
        }
    }

    pub fn move_disk_selection(&mut self, delta: isize) {
        if self.hardware.disks.is_empty() {
            self.selected_disk = 0;
            return;
        }
        self.selected_disk = (self.selected_disk as isize + delta)
            .rem_euclid(self.hardware.disks.len() as isize) as usize;
    }

    pub fn select_disk(&mut self) {
        let Some(disk) = self.hardware.disks.get(self.selected_disk) else {
            self.status = "No physical disk is available to select.".into();
            return;
        };
        if let Some(reason) = disk.selection_block_reason() {
            self.status = format!("Cannot select {}: {reason}.", disk.path.display());
            return;
        }
        self.config.disk.target = Some(disk.path.clone());
        self.status = format!(
            "Selected {} for the plan only. No disk operation is implemented.",
            disk.path.display()
        );
    }

    pub fn move_selection(&mut self, delta: isize) {
        self.selected = (self.selected as isize + delta).clamp(0, SETTING_COUNT as isize - 1) as usize;
        self.multi_cursor = 0;
    }

    pub fn adjust(&mut self, delta: isize) {
        match self.selected {
            0 => self.config.system.init_system = cycle(self.config.system.init_system,
                &[InitSystem::Dinit, InitSystem::OpenRc, InitSystem::Runit, InitSystem::S6], delta),
            1 => {
                self.status = "Disk selection is read-only in this milestone. No device is chosen implicitly.".into();
                return;
            }
            2 => {
                self.config.disk.partitioning = cycle(self.config.disk.partitioning,
                    &[Partitioning::Automatic, Partitioning::Manual, Partitioning::DualBoot], delta);
                if self.config.disk.partitioning == Partitioning::DualBoot {
                    self.config.disk.efi_policy = EfiPolicy::Preserve;
                }
            }
            3 => self.config.disk.root_filesystem = cycle(self.config.disk.root_filesystem,
                &[Filesystem::Ext4, Filesystem::Btrfs, Filesystem::Xfs, Filesystem::F2fs, Filesystem::Jfs, Filesystem::Nilfs2], delta),
            4 => self.config.disk.swap.mode = cycle(self.config.disk.swap.mode,
                &[SwapMode::None, SwapMode::Zram, SwapMode::Partition, SwapMode::Swapfile, SwapMode::Both], delta),
            5 => self.config.disk.encryption = !self.config.disk.encryption,
            6 | 7 => {
                let len = if self.selected == 6 { DESKTOP_CHOICES.len() } else { KERNEL_CHOICES.len() };
                self.multi_cursor = (self.multi_cursor as isize + delta).rem_euclid(len as isize) as usize;
                let cursor_label = self.multi_options()[self.multi_cursor].0.clone();
                self.status = format!("Cursor: {cursor_label}. Press Space to toggle.");
                return;
            }
            8 => self.config.software.cpu_vendor = cycle(self.config.software.cpu_vendor,
                &[CpuVendor::Intel, CpuVendor::Amd, CpuVendor::Other], delta),
            9 => self.config.software.gpu = cycle(self.config.software.gpu,
                &[GpuChoice::Intel, GpuChoice::Amd, GpuChoice::Nvidia, GpuChoice::Hybrid, GpuChoice::Vm], delta),
            10 => self.config.software.bootloader = cycle(self.config.software.bootloader,
                &[Bootloader::Grub, Bootloader::Limine, Bootloader::Refind], delta),
            11 => self.config.system.privilege_escalation = cycle(self.config.system.privilege_escalation,
                &[PrivilegeEscalation::Doas, PrivilegeEscalation::Sudo], delta),
            12 => self.config.system.user_services = cycle(self.config.system.user_services,
                &[UserServices::None, UserServices::Userspawn, UserServices::Turnstile], delta),
            13 => self.config.software.network = cycle(self.config.software.network,
                &[NetworkStack::Dhcpcd, NetworkStack::Iwd, NetworkStack::NetworkManager], delta),
            14 => self.config.software.multilib = !self.config.software.multilib,
            15 => {
                self.config.system.firmware = cycle(self.config.system.firmware,
                    &[FirmwareMode::Uefi, FirmwareMode::Bios], delta);
                if self.config.system.firmware == FirmwareMode::Bios {
                    self.config.software.bootloader = Bootloader::Grub;
                }
            }
            16 => {
                if self.config.disk.partitioning == Partitioning::DualBoot {
                    self.config.disk.efi_policy = EfiPolicy::Preserve;
                    self.status = "EFI formatting is disabled in dual-boot mode.".into();
                    return;
                }
                self.config.disk.efi_policy = cycle(self.config.disk.efi_policy,
                    &[EfiPolicy::Preserve, EfiPolicy::Format], delta);
            }
            17 => self.config.software.noctalia_greeter = !self.config.software.noctalia_greeter,
            _ => {}
        }
        if self.config.system.firmware == FirmwareMode::Bios {
            self.config.software.bootloader = Bootloader::Grub;
        }
        self.status = format!("{}: {}", self.setting_label(self.selected), self.setting_value(self.selected));
    }

    pub fn toggle_multi_selection(&mut self) {
        match self.selected {
            6 => {
                let choice = DESKTOP_CHOICES[self.multi_cursor];
                if choice == Desktop::Cosmalia && self.config.system.init_system != InitSystem::Dinit {
                    self.status = "Cosmalia is only available with dinit.".into();
                    return;
                }
                if let Some(index) = self.config.software.desktop_environments.iter().position(|v| *v == choice) {
                    self.config.software.desktop_environments.remove(index);
                } else {
                    match choice {
                        Desktop::SwayFx => self.config.software.desktop_environments.retain(|v| !matches!(v, Desktop::Sway | Desktop::Umbriel)),
                        Desktop::Umbriel | Desktop::Sway => self.config.software.desktop_environments.retain(|v| *v != Desktop::SwayFx),
                        _ => {}
                    }
                    self.config.software.desktop_environments.push(choice);
                }
                if self.config.software.desktop_environments.is_empty() || self.config.software.desktop_environments.contains(&Desktop::Cosmic) {
                    self.config.software.noctalia_greeter = false;
                }
            }
            7 => {
                let choice = KERNEL_CHOICES[self.multi_cursor];
                if choice == Kernel::Linux {
                    self.status = "The standard linux kernel stays selected as a guaranteed fallback.".into();
                    return;
                }
                if let Some(index) = self.config.software.kernels.iter().position(|v| *v == choice) {
                    self.config.software.kernels.remove(index);
                } else {
                    self.config.software.kernels.push(choice);
                }
            }
            _ => return,
        }
        self.status = format!("{}: {}", self.setting_label(self.selected), self.setting_value(self.selected));
    }

    pub fn setting_label(&self, index: usize) -> &'static str {
        match index {
            0 => "Init system", 1 => "Target disk", 2 => "Partitioning", 3 => "Root filesystem",
            4 => "Swap", 5 => "LUKS2 encryption", 6 => "Desktops / WMs", 7 => "Kernels",
            8 => "CPU vendor", 9 => "GPU", 10 => "Bootloader", 11 => "Privilege escalation",
            12 => "User services", 13 => "Network stack", 14 => "Multilib", 15 => "Firmware",
            16 => "EFI policy", 17 => "Noctalia greeter", _ => "Unknown",
        }
    }

    pub fn setting_value(&self, index: usize) -> String {
        match index {
            0 => self.config.system.init_system.to_string(),
            1 => self.config.disk.target.as_ref().map_or_else(|| "Not selected".into(), |p| p.display().to_string()),
            2 => self.config.disk.partitioning.to_string(),
            3 => self.config.disk.root_filesystem.to_string(),
            4 => format!("{} ({} GiB{})", self.config.disk.swap.mode, self.config.disk.swap.size_gib,
                if self.config.disk.swap.zswap { ", zswap" } else { "" }),
            5 => bool_label(self.config.disk.encryption),
            6 => list_values(&self.config.software.desktop_environments),
            7 => list_values(&self.config.software.kernels),
            8 => self.config.software.cpu_vendor.to_string(),
            9 => self.config.software.gpu.to_string(),
            10 => self.config.software.bootloader.to_string(),
            11 => self.config.system.privilege_escalation.to_string(),
            12 => self.config.system.user_services.to_string(),
            13 => self.config.software.network.to_string(),
            14 => bool_label(self.config.software.multilib),
            15 => self.config.system.firmware.to_string(),
            16 => self.config.disk.efi_policy.to_string(),
            17 => bool_label(self.config.software.noctalia_greeter),
            _ => String::new(),
        }
    }

    pub fn setting_description(&self, index: usize) -> &'static str {
        match index {
            0 => "Choose dinit, OpenRC, runit or s6. Init-specific packages and services are central to Artix.",
            1 => "Press D to inspect physical disks. Only unmounted, writable devices without active holders can be selected; selection only edits the plan.",
            2 => "Automatic replaces the selected drive layout; manual assigns existing partitions; dual-boot preserves existing partitions and EFI.",
            3 => "Root filesystems supported by the shell installer: ext4, Btrfs, XFS, F2FS, JFS and NILFS2.",
            4 => "Choose none, zram, swap partition, swapfile or both. zswap is modeled separately for real swap devices.",
            5 => "LUKS2 intent only. Passphrases are never saved in profiles or logs.",
            6 => "Multi-select desktops and window managers. Left/Right moves the cursor; Space toggles. Optional packages need live repo checks.",
            7 => "Multi-select kernels. Standard linux remains selected as the fallback. Left/Right moves the cursor; Space toggles.",
            8 => "Intel, AMD or Other / VM CPU choice for microcode selection.",
            9 => "Intel, AMD, NVIDIA, hybrid Intel+NVIDIA or VM/basic Mesa graphics choice.",
            10 => "UEFI supports GRUB2, Limine or rEFInd. BIOS is locked to GRUB2.",
            11 => "Choose doas or sudo.",
            12 => "Choose userspawn, turnstile or neither; package availability depends on the selected init.",
            13 => "Choose dhcpcd, iwd or NetworkManager.",
            14 => "Enable multilib for Steam, Wine and 32-bit applications.",
            15 => "UEFI or BIOS. Future discovery will detect this from the live environment.",
            16 => "Preserve is the safe default. Formatting an EFI partition can destroy another OS boot files.",
            17 => "Optional noctalia-greeter; availability must be checked against live package repositories.",
            _ => "",
        }
    }

    pub fn multi_options(&self) -> Vec<(String, bool)> {
        match self.selected {
            6 => DESKTOP_CHOICES.iter().map(|c| (c.to_string(), self.config.software.desktop_environments.contains(c))).collect(),
            7 => KERNEL_CHOICES.iter().map(|c| (c.to_string(), self.config.software.kernels.contains(c))).collect(),
            _ => Vec::new(),
        }
    }

    pub fn save_config(&mut self) -> Result<()> {
        self.config.validate().map_err(|issues| anyhow::anyhow!(issues.join("; ")))?;
        let contents = toml::to_string_pretty(&self.config).context("serialize configuration")?;
        fs::write("artix-setup.toml", contents).context("write artix-setup.toml")?;
        self.status = "Saved configuration to ./artix-setup.toml".into();
        Ok(())
    }
}

fn list_values<T: std::fmt::Display>(values: &[T]) -> String {
    if values.is_empty() { "CLI only".into() }
    else { values.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ") }
}
fn bool_label(value: bool) -> String { if value { "Enabled".into() } else { "Disabled".into() } }
fn cycle<T: Copy + PartialEq>(current: T, options: &[T], delta: isize) -> T {
    let index = options.iter().position(|value| *value == current).unwrap_or(0) as isize;
    options[(index + delta).rem_euclid(options.len() as isize) as usize]
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn defaults_to_safe_unselected_disk() {
        let app = App::new(InstallConfig::default(), HardwareSnapshot::default());
        assert_eq!(app.setting_value(1), "Not selected");
        assert!(app.config.disk.target.is_none());
    }
    #[test]
    fn toggles_desktop_multi_select() {
        let mut app = App::new(InstallConfig::default(), HardwareSnapshot::default());
        app.selected = 6;
        app.multi_cursor = 7; // Hyprland
        app.toggle_multi_selection();
        assert!(app.config.software.desktop_environments.contains(&Desktop::Hyprland));
    }
}
