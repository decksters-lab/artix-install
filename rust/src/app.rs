use std::fs;

use anyhow::{Context, Result};

use crate::config::{
    Bootloader, CpuVendor, Desktop, EfiPolicy, Filesystem, FirmwareMode, GpuChoice, InitSystem,
    InstallConfig, Kernel, NetworkStack, Partitioning, PrivilegeEscalation, SwapMode, UserServices,
};
use crate::discovery::{BlockPartition, HardwareSnapshot};

pub const SETTING_COUNT: usize = 18;
pub const DESKTOP_CHOICES: [Desktop; 20] = [
    Desktop::Plasma,
    Desktop::Xfce,
    Desktop::Cinnamon,
    Desktop::Mate,
    Desktop::Lxqt,
    Desktop::Moksha,
    Desktop::Cosmic,
    Desktop::Hyprland,
    Desktop::Niri,
    Desktop::Wayfire,
    Desktop::Labwc,
    Desktop::Sway,
    Desktop::SwayFx,
    Desktop::Umbriel,
    Desktop::MangoWm,
    Desktop::Cosmalia,
    Desktop::I3,
    Desktop::Openbox,
    Desktop::Fluxbox,
    Desktop::Qtile,
];
pub const KERNEL_CHOICES: [Kernel; 5] = [
    Kernel::Linux,
    Kernel::LinuxLts,
    Kernel::LinuxZen,
    Kernel::LinuxLqx,
    Kernel::LinuxCachyos,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartitionRole {
    Root,
    Efi,
    Swap,
}

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
    pub show_storage: bool,
    pub selected_partition: usize,
}

impl App {
    pub fn new(config: InstallConfig, hardware: HardwareSnapshot) -> Self {
        Self {
            config,
            selected: 0,
            multi_cursor: 0,
            show_plan: false,
            show_help: false,
            status: "Review the defaults. No target disk is selected; disk operations are blocked."
                .into(),
            hardware,
            show_disks: false,
            selected_disk: 0,
            show_storage: false,
            selected_partition: 0,
        }
    }

    pub fn toggle_disks(&mut self) {
        self.show_disks = !self.show_disks;
        if self.show_disks {
            self.status = format!(
                "Read-only inventory: {} physical disk(s) detected.",
                self.hardware.disks.len()
            );
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
        let target_changed = self.config.disk.target.as_ref() != Some(&disk.path);
        if target_changed {
            self.config.disk.root_partition = None;
            self.config.disk.efi_partition = None;
            self.config.disk.swap_partition = None;
            self.config.disk.additional_mounts.clear();
            self.selected_partition = 0;
        }
        self.config.disk.target = Some(disk.path.clone());
        self.status = if target_changed {
            format!(
                "Selected {}. Cleared previous partition assignments because the target changed. Selection only updates the plan.",
                disk.path.display()
            )
        } else {
            format!(
                "Selected {} for the plan only. No disk operation is implemented.",
                disk.path.display()
            )
        };
    }

    pub fn toggle_storage(&mut self) {
        self.show_storage = !self.show_storage;
        self.selected_partition = 0;
        self.status = if self.show_storage {
            "Partition manager is planning-only. No partitioning or formatting is available.".into()
        } else {
            "Returned to configuration.".into()
        };
    }

    pub fn target_partitions(&self) -> &[BlockPartition] {
        let Some(target) = self.config.disk.target.as_ref() else {
            return &[];
        };
        self.hardware
            .disks
            .iter()
            .find(|disk| disk.path == *target)
            .map(|disk| disk.partitions.as_slice())
            .unwrap_or(&[])
    }

    pub fn move_partition_selection(&mut self, delta: isize) {
        let count = self.target_partitions().len();
        if count == 0 {
            self.selected_partition = 0;
        } else {
            self.selected_partition =
                (self.selected_partition as isize + delta).rem_euclid(count as isize) as usize;
        }
    }

    pub fn assign_partition(&mut self, role: PartitionRole) {
        if !matches!(
            self.config.disk.partitioning,
            Partitioning::Manual | Partitioning::DualBoot
        ) {
            self.status = "Partition role assignment is for Manual or Dual-boot mode. Automatic mode generates its own layout.".into();
            return;
        }
        if role == PartitionRole::Efi && self.config.system.firmware != FirmwareMode::Uefi {
            self.status = "EFI partition assignment is unavailable in BIOS mode.".into();
            return;
        }
        if role == PartitionRole::Swap && self.config.disk.swap.mode != SwapMode::Partition {
            self.status =
                "Select Swap = partition in the configuration before assigning a swap partition."
                    .into();
            return;
        }
        if role == PartitionRole::Swap && self.config.disk.partitioning == Partitioning::DualBoot {
            self.status =
                "Dual-boot mode does not assign or reformat an existing swap partition.".into();
            return;
        }

        let Some(target_disk) = self
            .hardware
            .disks
            .iter()
            .find(|disk| Some(&disk.path) == self.config.disk.target.as_ref())
        else {
            self.status =
                "The configured target disk is not present in the current hardware inventory."
                    .into();
            return;
        };
        if let Some(reason) = target_disk.selection_block_reason() {
            self.status = format!(
                "Cannot assign partitions on {}: {reason}.",
                target_disk.path.display()
            );
            return;
        }

        let Some(partition) = self
            .target_partitions()
            .get(self.selected_partition)
            .cloned()
        else {
            self.status =
                "No partition is selected. Choose a target disk with partitions first.".into();
            return;
        };

        if !partition.safety_checks_complete {
            self.status = format!(
                "Cannot assign {}: partition safety metadata is incomplete.",
                partition.path.display()
            );
            return;
        }
        if role == PartitionRole::Efi
            && (self.config.disk.partitioning == Partitioning::DualBoot
                || self.config.disk.efi_policy == EfiPolicy::Preserve)
            && partition.filesystem.as_deref() != Some("vfat")
        {
            self.status = format!(
                "Cannot preserve {} as EFI: its filesystem is not verified as FAT32/vfat. Use a verified EFI System Partition.",
                partition.path.display()
            );
            return;
        }
        if !partition.mounted_at.is_empty() {
            self.status = format!(
                "Cannot assign {}: it is mounted at {}.",
                partition.path.display(),
                partition.mounted_at.join(", ")
            );
            return;
        }
        if partition.active_swap {
            self.status = format!(
                "Cannot assign {}: it is active swap.",
                partition.path.display()
            );
            return;
        }
        if partition.read_only || partition.has_holders {
            self.status = format!(
                "Cannot assign {}: it is read-only or has active holders.",
                partition.path.display()
            );
            return;
        }

        let path = partition.path.clone();
        let already_used = match role {
            PartitionRole::Root => {
                self.config.disk.efi_partition.as_ref() == Some(&path)
                    || self.config.disk.swap_partition.as_ref() == Some(&path)
            }
            PartitionRole::Efi => {
                self.config.disk.root_partition.as_ref() == Some(&path)
                    || self.config.disk.swap_partition.as_ref() == Some(&path)
            }
            PartitionRole::Swap => {
                self.config.disk.root_partition.as_ref() == Some(&path)
                    || self.config.disk.efi_partition.as_ref() == Some(&path)
            }
        };
        if already_used {
            self.status = format!(
                "{} is already assigned to another partition role.",
                path.display()
            );
            return;
        }

        match role {
            PartitionRole::Root => self.config.disk.root_partition = Some(path.clone()),
            PartitionRole::Efi => {
                self.config.disk.efi_partition = Some(path.clone());
                if self.config.disk.partitioning == Partitioning::DualBoot {
                    self.config.disk.efi_policy = EfiPolicy::Preserve;
                }
            }
            PartitionRole::Swap => self.config.disk.swap_partition = Some(path.clone()),
        }
        let role_name = match role {
            PartitionRole::Root => "root",
            PartitionRole::Efi => "EFI",
            PartitionRole::Swap => "swap",
        };
        self.status = format!(
            "Assigned {} as {role_name} for the plan only. No disk operation was performed.",
            path.display()
        );
    }

    pub fn partition_role(&self, path: &std::path::Path) -> Option<&'static str> {
        if self.config.disk.root_partition.as_deref() == Some(path) {
            Some("ROOT")
        } else if self.config.disk.efi_partition.as_deref() == Some(path) {
            Some("EFI")
        } else if self.config.disk.swap_partition.as_deref() == Some(path) {
            Some("SWAP")
        } else {
            None
        }
    }

    pub fn move_selection(&mut self, delta: isize) {
        self.selected =
            (self.selected as isize + delta).clamp(0, SETTING_COUNT as isize - 1) as usize;
        self.multi_cursor = 0;
    }

    pub fn adjust(&mut self, delta: isize) {
        match self.selected {
            0 => {
                self.config.system.init_system = cycle(
                    self.config.system.init_system,
                    &[
                        InitSystem::Dinit,
                        InitSystem::OpenRc,
                        InitSystem::Runit,
                        InitSystem::S6,
                    ],
                    delta,
                );
                if !user_service_choices(self.config.system.init_system)
                    .contains(&self.config.system.user_services)
                {
                    self.config.system.user_services = UserServices::None;
                }
            }
            1 => {
                self.status = "Disk selection is read-only in this milestone. No device is chosen implicitly.".into();
                return;
            }
            2 => {
                self.config.disk.partitioning = cycle(
                    self.config.disk.partitioning,
                    &[
                        Partitioning::Automatic,
                        Partitioning::Manual,
                        Partitioning::DualBoot,
                    ],
                    delta,
                );
                match self.config.disk.partitioning {
                    Partitioning::Automatic => {
                        self.config.disk.efi_partition = None;
                        self.config.disk.root_partition = None;
                        self.config.disk.swap_partition = None;
                        self.config.disk.additional_mounts.clear();
                    }
                    Partitioning::DualBoot => {
                        self.config.disk.efi_policy = EfiPolicy::Preserve;
                        self.config.disk.swap_partition = None;
                        if self.config.disk.swap.mode == SwapMode::Partition {
                            self.config.disk.swap.mode = SwapMode::Zram;
                        }
                        self.config.disk.additional_mounts.clear();
                    }
                    Partitioning::Manual => {}
                }
            }
            3 => {
                self.config.disk.root_filesystem = cycle(
                    self.config.disk.root_filesystem,
                    &[
                        Filesystem::Ext4,
                        Filesystem::Btrfs,
                        Filesystem::Xfs,
                        Filesystem::F2fs,
                        Filesystem::Jfs,
                        Filesystem::Nilfs2,
                    ],
                    delta,
                )
            }
            4 => {
                self.config.disk.swap.mode = cycle(
                    self.config.disk.swap.mode,
                    &[
                        SwapMode::None,
                        SwapMode::Zram,
                        SwapMode::Partition,
                        SwapMode::Swapfile,
                        SwapMode::Both,
                    ],
                    delta,
                );
                if self.config.disk.swap.mode != SwapMode::Partition {
                    self.config.disk.swap_partition = None;
                }
            }
            5 => self.config.disk.encryption = !self.config.disk.encryption,
            6 | 7 => {
                let len = if self.selected == 6 {
                    DESKTOP_CHOICES.len()
                } else {
                    KERNEL_CHOICES.len()
                };
                self.multi_cursor =
                    (self.multi_cursor as isize + delta).rem_euclid(len as isize) as usize;
                let cursor_label = self.multi_options()[self.multi_cursor].0.clone();
                self.status = format!("Cursor: {cursor_label}. Press Space to toggle.");
                return;
            }
            8 => {
                self.config.software.cpu_vendor = cycle(
                    self.config.software.cpu_vendor,
                    &[CpuVendor::Intel, CpuVendor::Amd, CpuVendor::Other],
                    delta,
                )
            }
            9 => {
                self.config.software.gpu = cycle(
                    self.config.software.gpu,
                    &[
                        GpuChoice::Intel,
                        GpuChoice::Amd,
                        GpuChoice::Nvidia,
                        GpuChoice::Hybrid,
                        GpuChoice::Vm,
                    ],
                    delta,
                )
            }
            10 => {
                self.config.software.bootloader = cycle(
                    self.config.software.bootloader,
                    &[Bootloader::Grub, Bootloader::Limine, Bootloader::Refind],
                    delta,
                )
            }
            11 => {
                self.config.system.privilege_escalation = cycle(
                    self.config.system.privilege_escalation,
                    &[PrivilegeEscalation::Doas, PrivilegeEscalation::Sudo],
                    delta,
                )
            }
            12 => {
                self.config.system.user_services = cycle(
                    self.config.system.user_services,
                    user_service_choices(self.config.system.init_system),
                    delta,
                )
            }
            13 => {
                self.config.software.network = cycle(
                    self.config.software.network,
                    &[
                        NetworkStack::Dhcpcd,
                        NetworkStack::Iwd,
                        NetworkStack::NetworkManager,
                    ],
                    delta,
                )
            }
            14 => self.config.software.multilib = !self.config.software.multilib,
            15 => {
                self.config.system.firmware = cycle(
                    self.config.system.firmware,
                    &[FirmwareMode::Uefi, FirmwareMode::Bios],
                    delta,
                );
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
                self.config.disk.efi_policy = cycle(
                    self.config.disk.efi_policy,
                    &[EfiPolicy::Preserve, EfiPolicy::Format],
                    delta,
                );
            }
            17 => self.config.software.noctalia_greeter = !self.config.software.noctalia_greeter,
            _ => {}
        }
        if self.config.system.firmware == FirmwareMode::Bios {
            self.config.software.bootloader = Bootloader::Grub;
        }
        self.status = format!(
            "{}: {}",
            self.setting_label(self.selected),
            self.setting_value(self.selected)
        );
    }

    pub fn toggle_multi_selection(&mut self) {
        match self.selected {
            6 => {
                let choice = DESKTOP_CHOICES[self.multi_cursor];
                if choice == Desktop::Cosmalia
                    && self.config.system.init_system != InitSystem::Dinit
                {
                    self.status = "Cosmalia is only available with dinit.".into();
                    return;
                }
                if let Some(index) = self
                    .config
                    .software
                    .desktop_environments
                    .iter()
                    .position(|v| *v == choice)
                {
                    self.config.software.desktop_environments.remove(index);
                } else {
                    match choice {
                        Desktop::SwayFx => self
                            .config
                            .software
                            .desktop_environments
                            .retain(|v| !matches!(v, Desktop::Sway | Desktop::Umbriel)),
                        Desktop::Umbriel | Desktop::Sway => self
                            .config
                            .software
                            .desktop_environments
                            .retain(|v| *v != Desktop::SwayFx),
                        _ => {}
                    }
                    self.config.software.desktop_environments.push(choice);
                }
                if self.config.software.desktop_environments.is_empty()
                    || self
                        .config
                        .software
                        .desktop_environments
                        .contains(&Desktop::Cosmic)
                {
                    self.config.software.noctalia_greeter = false;
                }
            }
            7 => {
                let choice = KERNEL_CHOICES[self.multi_cursor];
                if choice == Kernel::Linux {
                    self.status =
                        "The standard linux kernel stays selected as a guaranteed fallback.".into();
                    return;
                }
                if let Some(index) = self
                    .config
                    .software
                    .kernels
                    .iter()
                    .position(|v| *v == choice)
                {
                    self.config.software.kernels.remove(index);
                } else {
                    self.config.software.kernels.push(choice);
                }
            }
            _ => return,
        }
        self.status = format!(
            "{}: {}",
            self.setting_label(self.selected),
            self.setting_value(self.selected)
        );
    }

    pub fn setting_label(&self, index: usize) -> &'static str {
        match index {
            0 => "Init system",
            1 => "Target disk",
            2 => "Partitioning",
            3 => "Root filesystem",
            4 => "Swap",
            5 => "LUKS2 encryption",
            6 => "Desktops / WMs",
            7 => "Kernels",
            8 => "CPU vendor",
            9 => "GPU",
            10 => "Bootloader",
            11 => "Privilege escalation",
            12 => "User services",
            13 => "Network stack",
            14 => "Multilib",
            15 => "Firmware",
            16 => "EFI policy",
            17 => "Noctalia greeter",
            _ => "Unknown",
        }
    }

    pub fn setting_value(&self, index: usize) -> String {
        match index {
            0 => self.config.system.init_system.to_string(),
            1 => self
                .config
                .disk
                .target
                .as_ref()
                .map_or_else(|| "Not selected".into(), |p| p.display().to_string()),
            2 => self.config.disk.partitioning.to_string(),
            3 => self.config.disk.root_filesystem.to_string(),
            4 => format!(
                "{} ({} GiB{})",
                self.config.disk.swap.mode,
                self.config.disk.swap.size_gib,
                if self.config.disk.swap.zswap {
                    ", zswap"
                } else {
                    ""
                }
            ),
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
            0 => {
                "Choose dinit, OpenRC, runit or s6. Init-specific packages and services are central to Artix."
            }
            1 => {
                "Press D to inspect physical disks. Only unmounted, writable devices without active holders can be selected; selection only edits the plan."
            }
            2 => {
                "Automatic replaces the selected drive layout; manual assigns existing partitions; dual-boot preserves existing partitions and EFI."
            }
            3 => {
                "Root filesystems supported by the shell installer: ext4, Btrfs, XFS, F2FS, JFS and NILFS2."
            }
            4 => {
                "Choose none, zram, swap partition, swapfile or both. zswap is modeled separately for real swap devices."
            }
            5 => "LUKS2 intent only. Passphrases are never saved in profiles or logs.",
            6 => match self.config.system.init_system {
                InitSystem::Dinit => {
                    "Multi-select desktops and window managers. Cosmalia is a Dinit-only session; its launcher runs cosmic-comp --no-xwayland with Noctalia as the shell."
                }
                _ => {
                    "Multi-select desktops and window managers. Cosmalia is Dinit-only and is unavailable with the currently selected init system."
                }
            }
            7 => {
                "Multi-select kernels. Standard linux remains selected as the fallback. Left/Right moves the cursor; Space toggles."
            }
            8 => "Intel, AMD or Other / VM CPU choice for microcode selection.",
            9 => "Intel, AMD, NVIDIA, hybrid Intel+NVIDIA or VM/basic Mesa graphics choice.",
            10 => "UEFI supports GRUB2, Limine or rEFInd. BIOS is locked to GRUB2.",
            11 => "Choose doas or sudo.",
            12 => {
                "Init-specific user services: Dinit userspawn-dinit or Turnstile; OpenRC native (0.62+, PAM/XDG_RUNTIME_DIR) or Turnstile; runit userspawn-runit; s6 manual."
            }
            13 => "Choose dhcpcd, iwd or NetworkManager.",
            14 => "Enable multilib for Steam, Wine and 32-bit applications.",
            15 => "UEFI or BIOS. Future discovery will detect this from the live environment.",
            16 => {
                "Preserve is the safe default. Formatting an EFI partition can destroy another OS boot files."
            }
            17 => {
                "Optional noctalia-greeter; availability must be checked against live package repositories."
            }
            _ => "",
        }
    }

    pub fn multi_options(&self) -> Vec<(String, bool)> {
        match self.selected {
            6 => DESKTOP_CHOICES
                .iter()
                .map(|c| {
                    let name = if *c == Desktop::Cosmalia
                        && self.config.system.init_system != InitSystem::Dinit
                    {
                        format!("{c} (Dinit only — unavailable)")
                    } else {
                        c.to_string()
                    };
                    (
                        name,
                        self.config.software.desktop_environments.contains(c),
                    )
                })
                .collect(),
            7 => KERNEL_CHOICES
                .iter()
                .map(|c| (c.to_string(), self.config.software.kernels.contains(c)))
                .collect(),
            _ => Vec::new(),
        }
    }

    pub fn save_config(&mut self) -> Result<()> {
        self.config
            .validate()
            .map_err(|issues| anyhow::anyhow!(issues.join("; ")))?;
        let contents = toml::to_string_pretty(&self.config).context("serialize configuration")?;
        fs::write("artix-setup.toml", contents).context("write artix-setup.toml")?;
        self.status = "Saved configuration to ./artix-setup.toml".into();
        Ok(())
    }
}

fn list_values<T: std::fmt::Display>(values: &[T]) -> String {
    if values.is_empty() {
        "CLI only".into()
    } else {
        values
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ")
    }
}

fn user_service_choices(init: InitSystem) -> &'static [UserServices] {
    match init {
        InitSystem::Dinit => &[
            UserServices::None,
            UserServices::Userspawn,
            UserServices::Turnstile,
        ],
        InitSystem::OpenRc => &[
            UserServices::None,
            UserServices::Native,
            UserServices::Turnstile,
        ],
        InitSystem::Runit => &[UserServices::None, UserServices::Userspawn],
        InitSystem::S6 => &[UserServices::None, UserServices::Manual],
    }
}

fn bool_label(value: bool) -> String {
    if value {
        "Enabled".into()
    } else {
        "Disabled".into()
    }
}
fn cycle<T: Copy + PartialEq>(current: T, options: &[T], delta: isize) -> T {
    let index = options
        .iter()
        .position(|value| *value == current)
        .unwrap_or(0) as isize;
    options[(index + delta).rem_euclid(options.len() as isize) as usize]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discovery::{BlockDevice, BlockPartition};

    fn sample_hardware() -> HardwareSnapshot {
        HardwareSnapshot {
            firmware: "UEFI".into(),
            memory_bytes: Some(8 * 1024 * 1024 * 1024),
            cpu_vendor: Some("AuthenticAMD".into()),
            cpu_model: Some("Test CPU".into()),
            gpus: vec![],
            network_interfaces: vec![],
            disks: vec![BlockDevice {
                name: "nvme0n1".into(),
                path: "/dev/nvme0n1".into(),
                size_bytes: 500 * 1024 * 1024 * 1024,
                model: Some("Test NVMe".into()),
                vendor: Some("Test".into()),
                serial: Some("SERIAL123".into()),
                wwid: None,
                removable: false,
                read_only: false,
                mounted_at: vec![],
                active_swap: false,
                safety_checks_complete: true,
                has_holders: false,
                partitions: vec![
                    BlockPartition {
                        name: "nvme0n1p1".into(),
                        path: "/dev/nvme0n1p1".into(),
                        size_bytes: 1024 * 1024 * 1024,
                        filesystem: Some("vfat".into()),
                        label: Some("EFI".into()),
                        mounted_at: vec![],
                        active_swap: false,
                        read_only: false,
                        has_holders: false,
                        safety_checks_complete: true,
                    },
                    BlockPartition {
                        name: "nvme0n1p2".into(),
                        path: "/dev/nvme0n1p2".into(),
                        size_bytes: 499 * 1024 * 1024 * 1024,
                        filesystem: Some("ext4".into()),
                        label: None,
                        mounted_at: vec![],
                        active_swap: false,
                        read_only: false,
                        has_holders: false,
                        safety_checks_complete: true,
                    },
                ],
            }],
            warnings: vec![],
        }
    }

    fn manual_app() -> App {
        let mut config = InstallConfig::default();
        config.disk.target = Some("/dev/nvme0n1".into());
        config.disk.partitioning = Partitioning::Manual;
        App::new(config, sample_hardware())
    }

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
        assert!(
            app.config
                .software
                .desktop_environments
                .contains(&Desktop::Hyprland)
        );
    }

    #[test]
    fn changing_swap_away_from_partition_clears_stale_partition_assignment() {
        let mut app = manual_app();
        app.config.disk.root_partition = Some("/dev/nvme0n1p2".into());
        app.config.disk.efi_partition = Some("/dev/nvme0n1p1".into());
        app.config.disk.swap.mode = SwapMode::Partition;
        app.config.disk.swap_partition = Some("/dev/nvme0n1p3".into());
        app.selected = 4;

        app.adjust(1);

        assert_eq!(app.config.disk.swap.mode, SwapMode::Swapfile);
        assert!(app.config.disk.swap_partition.is_none());
        assert!(app.config.validate().is_ok());
    }

    #[test]
    fn manual_partition_manager_assigns_root_and_efi_from_target_disk() {
        let mut app = manual_app();
        app.assign_partition(PartitionRole::Efi);
        assert_eq!(
            app.config.disk.efi_partition.as_deref(),
            Some(std::path::Path::new("/dev/nvme0n1p1"))
        );
        app.move_partition_selection(1);
        app.assign_partition(PartitionRole::Root);
        assert_eq!(
            app.config.disk.root_partition.as_deref(),
            Some(std::path::Path::new("/dev/nvme0n1p2"))
        );
    }

    #[test]
    fn partition_role_assignments_reject_duplicate_partition_use() {
        let mut app = manual_app();
        app.assign_partition(PartitionRole::Efi);
        app.assign_partition(PartitionRole::Root);
        assert!(app.config.disk.root_partition.is_none());
    }

    #[test]
    fn preserving_efi_requires_verified_vfat_filesystem() {
        let mut hardware = sample_hardware();
        hardware.disks[0].partitions[0].filesystem = None;
        let mut app = manual_app();
        app.hardware = hardware;
        app.assign_partition(PartitionRole::Efi);
        assert!(app.config.disk.efi_partition.is_none());
        assert!(app.status.contains("not verified as FAT32/vfat"));
    }

    #[test]
    fn partition_manager_rejects_partitions_when_parent_disk_is_blocked() {
        let mut app = manual_app();
        app.hardware.disks[0].has_holders = true;
        app.assign_partition(PartitionRole::Efi);
        assert!(app.config.disk.efi_partition.is_none());
        assert!(app.status.contains("active holders"));
    }

    #[test]
    fn partition_manager_rejects_mounted_partitions() {
        let mut hardware = sample_hardware();
        hardware.disks[0].partitions[0].mounted_at = vec!["/boot/efi".into()];
        let mut app = manual_app();
        app.hardware = hardware;
        app.assign_partition(PartitionRole::Efi);
        assert!(app.config.disk.efi_partition.is_none());
        assert!(app.status.contains("mounted"));
    }

    #[test]
    fn changing_target_disk_clears_old_manual_partition_assignments() {
        let mut hardware = sample_hardware();
        let mut second = hardware.disks[0].clone();
        second.name = "sdb".into();
        second.path = "/dev/sdb".into();
        second.serial = Some("SERIAL456".into());
        for partition in &mut second.partitions {
            partition.name = partition.name.replace("nvme0n1", "sdb");
            partition.path = std::path::PathBuf::from(
                partition.path.to_string_lossy().replace("nvme0n1", "sdb"),
            );
        }
        hardware.disks.push(second);

        let mut app = manual_app();
        app.hardware = hardware;
        app.config.disk.root_partition = Some("/dev/nvme0n1p2".into());
        app.config.disk.efi_partition = Some("/dev/nvme0n1p1".into());
        app.selected_disk = 1;
        app.select_disk();

        assert_eq!(
            app.config.disk.target.as_deref(),
            Some(std::path::Path::new("/dev/sdb"))
        );
        assert!(app.config.disk.root_partition.is_none());
        assert!(app.config.disk.efi_partition.is_none());
        assert!(app.config.disk.swap_partition.is_none());
        assert!(
            app.status
                .contains("Cleared previous partition assignments")
        );
    }

    #[test]
    fn dual_boot_assignment_forces_efi_preservation() {
        let mut app = manual_app();
        app.config.disk.partitioning = Partitioning::DualBoot;
        app.config.disk.efi_policy = EfiPolicy::Format;
        app.assign_partition(PartitionRole::Efi);
        assert_eq!(app.config.disk.efi_policy, EfiPolicy::Preserve);
        assert_eq!(
            app.config.disk.efi_partition.as_deref(),
            Some(std::path::Path::new("/dev/nvme0n1p1"))
        );
    }
}
