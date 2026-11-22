use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstallConfig {
    pub schema_version: u32,
    pub system: SystemConfig,
    pub disk: DiskConfig,
    pub software: SoftwareConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemConfig {
    pub init_system: InitSystem,
    pub hostname: String,
    pub username: String,
    pub locale: String,
    pub timezone: String,
    pub keyboard_layout: String,
    pub firmware: FirmwareMode,
    pub privilege_escalation: PrivilegeEscalation,
    pub user_services: UserServices,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiskConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<PathBuf>,
    pub partitioning: Partitioning,
    pub root_filesystem: Filesystem,
    pub swap: SwapConfig,
    pub encryption: bool,
    pub efi_policy: EfiPolicy,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub efi_partition: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root_partition: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub swap_partition: Option<PathBuf>,
    #[serde(default)]
    pub additional_mounts: Vec<MountConfig>,
    #[serde(default)]
    pub btrfs: BtrfsConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SwapConfig {
    pub mode: SwapMode,
    pub size_gib: u8,
    pub zswap: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MountConfig {
    pub device: PathBuf,
    pub mountpoint: String,
    pub filesystem: Filesystem,
    #[serde(default)]
    pub format: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BtrfsConfig {
    pub layout: BtrfsLayout,
    pub compression: BtrfsCompression,
}

impl Default for BtrfsConfig {
    fn default() -> Self {
        Self { layout: BtrfsLayout::Standard, compression: BtrfsCompression::Zstd }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SoftwareConfig {
    #[serde(default)]
    pub desktop_environments: Vec<Desktop>,
    pub kernels: Vec<Kernel>,
    pub cpu_vendor: CpuVendor,
    pub gpu: GpuChoice,
    pub bootloader: Bootloader,
    pub network: NetworkStack,
    pub multilib: bool,
    pub noctalia_greeter: bool,
}

macro_rules! string_enum {
    ($name:ident { $($variant:ident => $value:literal),+ $(,)? }) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
        pub enum $name {
            $(#[serde(rename = $value)] $variant),+
        }
        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(match self { $(Self::$variant => $value),+ })
            }
        }
    };
}

string_enum!(InitSystem {
    Dinit => "dinit", OpenRc => "openrc", Runit => "runit", S6 => "s6"
});
string_enum!(FirmwareMode { Uefi => "uefi", Bios => "bios" });
string_enum!(Partitioning {
    Automatic => "automatic", Manual => "manual", DualBoot => "dualboot"
});
string_enum!(Filesystem {
    Btrfs => "btrfs", Ext4 => "ext4", Xfs => "xfs", F2fs => "f2fs",
    Jfs => "jfs", Nilfs2 => "nilfs2", Vfat => "vfat", Exfat => "exfat", Ntfs => "ntfs"
});
string_enum!(SwapMode {
    None => "none", Zram => "zram", Partition => "partition", Swapfile => "swapfile", Both => "both"
});
string_enum!(EfiPolicy { Preserve => "preserve", Format => "format" });
string_enum!(BtrfsLayout { Flat => "flat", Standard => "standard", Snapshots => "snapshots" });
string_enum!(BtrfsCompression { None => "none", Zstd => "zstd", Lzo => "lzo" });
string_enum!(Desktop {
    Plasma => "plasma", Xfce => "xfce", Cinnamon => "cinnamon", Mate => "mate", Lxqt => "lxqt",
    Moksha => "moksha", Cosmic => "cosmic", Hyprland => "hyprland", Niri => "niri",
    Wayfire => "wayfire", Sway => "sway", SwayFx => "sway-fx", Labwc => "labwc",
    Cosmalia => "cosmalia", Umbriel => "umbriel", MangoWm => "mango-wm", I3 => "i3",
    Openbox => "openbox", Fluxbox => "fluxbox", Qtile => "qtile"
});
string_enum!(Kernel {
    Linux => "linux", LinuxLts => "linux-lts", LinuxZen => "linux-zen",
    LinuxLqx => "linux-lqx", LinuxCachyos => "linux-cachyos"
});
string_enum!(CpuVendor { Intel => "intel", Amd => "amd", Other => "other" });
string_enum!(GpuChoice {
    Intel => "intel", Amd => "amd", Nvidia => "nvidia", Hybrid => "hybrid", Vm => "vm"
});
string_enum!(Bootloader { Grub => "grub", Limine => "limine", Refind => "refind" });
string_enum!(NetworkStack {
    Dhcpcd => "dhcpcd", Iwd => "iwd", NetworkManager => "network-manager"
});
string_enum!(PrivilegeEscalation { Doas => "doas", Sudo => "sudo" });
string_enum!(UserServices {
    None => "none", Native => "native", Userspawn => "userspawn",
    Turnstile => "turnstile", Manual => "manual"
});

impl Default for InstallConfig {
    fn default() -> Self {
        Self {
            schema_version: 1,
            system: SystemConfig {
                init_system: InitSystem::Dinit,
                hostname: "artix".into(),
                username: "user".into(),
                locale: "en_US.UTF-8".into(),
                timezone: "Europe/London".into(),
                keyboard_layout: "us".into(),
                firmware: FirmwareMode::Uefi,
                privilege_escalation: PrivilegeEscalation::Doas,
                user_services: UserServices::None,
            },
            disk: DiskConfig {
                target: None,
                partitioning: Partitioning::Automatic,
                root_filesystem: Filesystem::Ext4,
                swap: SwapConfig { mode: SwapMode::Zram, size_gib: 4, zswap: false },
                encryption: false,
                efi_policy: EfiPolicy::Preserve,
                efi_partition: None,
                root_partition: None,
                swap_partition: None,
                additional_mounts: Vec::new(),
                btrfs: BtrfsConfig::default(),
            },
            software: SoftwareConfig {
                desktop_environments: Vec::new(),
                kernels: vec![Kernel::Linux],
                cpu_vendor: CpuVendor::Other,
                gpu: GpuChoice::Vm,
                bootloader: Bootloader::Grub,
                network: NetworkStack::NetworkManager,
                multilib: false,
                noctalia_greeter: false,
            },
        }
    }
}

impl InstallConfig {
    /// Shape validation only; no device probing or mutations happen here.
    pub fn validate(&self) -> Result<(), Vec<String>> {
        let mut issues = Vec::new();
        if self.schema_version != 1 {
            issues.push(format!("unsupported schema_version {}; expected 1", self.schema_version));
        }
        if self.system.hostname.is_empty() || self.system.hostname.len() > 63
            || self.system.hostname.starts_with('-') || self.system.hostname.ends_with('-')
            || !self.system.hostname.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
            issues.push("hostname must be 1-63 ASCII letters, numbers, or internal hyphens".into());
        }
        if self.system.username.is_empty()
            || !self.system.username.as_bytes()[0].is_ascii_lowercase()
            || !self.system.username.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-') {
            issues.push("username must start with a lowercase letter and contain only lowercase letters, numbers, _ or -".into());
        }
        for (name, value) in [
            ("locale", self.system.locale.as_str()),
            ("timezone", self.system.timezone.as_str()),
            ("keyboard_layout", self.system.keyboard_layout.as_str()),
        ] {
            if value.trim().is_empty() || value.contains('\n') || value.contains('\r') {
                issues.push(format!("{name} must be a non-empty single-line value"));
            }
        }
        if let Some(target) = &self.disk.target { validate_device_path(target, "disk.target", &mut issues); }
        if let Some(efi) = &self.disk.efi_partition { validate_device_path(efi, "disk.efi_partition", &mut issues); }
        if let Some(root) = &self.disk.root_partition { validate_device_path(root, "disk.root_partition", &mut issues); }
        if let Some(swap) = &self.disk.swap_partition { validate_device_path(swap, "disk.swap_partition", &mut issues); }

        if self.disk.partitioning == Partitioning::DualBoot && self.disk.efi_policy == EfiPolicy::Format {
            issues.push("dual-boot mode requires efi_policy = 'preserve'".into());
        }
        if self.disk.partitioning == Partitioning::DualBoot && self.system.firmware != FirmwareMode::Uefi {
            issues.push("dual-boot partition planning currently requires UEFI firmware".into());
        }
        if self.disk.partitioning == Partitioning::Automatic
            && (self.disk.efi_partition.is_some() || self.disk.root_partition.is_some() || self.disk.swap_partition.is_some()) {
            issues.push("automatic mode calculates its own partition paths; clear manual EFI/root/swap partition assignments".into());
        }
        if self.disk.partitioning != Partitioning::Manual && !self.disk.additional_mounts.is_empty() {
            issues.push("additional_mounts are currently supported only in manual partitioning mode".into());
        }
        if self.system.firmware == FirmwareMode::Bios {
            if self.software.bootloader != Bootloader::Grub {
                issues.push("BIOS installations use GRUB2; Limine and rEFInd are UEFI-only".into());
            }
            if self.disk.root_filesystem == Filesystem::F2fs {
                issues.push("BIOS + F2FS needs a deliberate GRUB compatibility override; use ext4, XFS or Btrfs by default".into());
            }
        }
        if matches!(self.disk.root_filesystem, Filesystem::Vfat | Filesystem::Exfat | Filesystem::Ntfs) {
            issues.push("FAT32, exFAT and NTFS are not supported root filesystems".into());
        }
        if self.disk.swap.size_gib == 0 || self.disk.swap.size_gib > 64 {
            issues.push("swap.size_gib must be between 1 and 64".into());
        }
        if self.disk.swap.zswap && !matches!(self.disk.swap.mode, SwapMode::Partition | SwapMode::Swapfile | SwapMode::Both) {
            issues.push("zswap requires a swap partition or swapfile (Swapfile or Both mode)".into());
        }
        if self.disk.swap.mode != SwapMode::Partition && self.disk.swap_partition.is_some() {
            issues.push("disk.swap_partition is only valid when swap.mode = 'partition'".into());
        }
        if self.disk.partitioning == Partitioning::DualBoot && self.disk.swap.mode == SwapMode::Partition {
            issues.push("dual-boot mode does not create or reformat a swap partition; use zram or a swapfile".into());
        }
        if self.disk.partitioning == Partitioning::DualBoot && self.disk.swap_partition.is_some() {
            issues.push("dual-boot mode cannot assign a new swap partition".into());
        }
        if self.disk.partitioning == Partitioning::Manual && self.disk.swap.mode == SwapMode::Partition
            && self.disk.swap_partition.is_none() {
            issues.push("manual partitioning with swap.mode = 'partition' requires disk.swap_partition".into());
        }
        if self.disk.partitioning == Partitioning::Manual {
            if self.disk.root_partition.is_none() {
                issues.push("manual partitioning requires disk.root_partition".into());
            }
            if self.system.firmware == FirmwareMode::Uefi && self.disk.efi_partition.is_none() {
                issues.push("manual UEFI partitioning requires disk.efi_partition".into());
            }
        }
        if self.disk.partitioning == Partitioning::DualBoot {
            if self.disk.root_partition.is_none() {
                issues.push("dual-boot requires an existing disk.root_partition".into());
            }
            if self.disk.efi_partition.is_none() {
                issues.push("dual-boot requires an existing disk.efi_partition".into());
            }
            if self.disk.efi_policy != EfiPolicy::Preserve {
                issues.push("dual-boot must preserve the existing EFI partition".into());
            }
        }
        if let (Some(efi), Some(root)) = (&self.disk.efi_partition, &self.disk.root_partition)
            && efi == root {
            issues.push("EFI and root partitions must be different devices".into());
        }
        if let (Some(swap), Some(root)) = (&self.disk.swap_partition, &self.disk.root_partition)
            && swap == root {
            issues.push("swap and root partitions must be different devices".into());
        }
        if let (Some(swap), Some(efi)) = (&self.disk.swap_partition, &self.disk.efi_partition)
            && swap == efi {
            issues.push("swap and EFI partitions must be different devices".into());
        }
        if let Some(root) = &self.disk.root_partition
            && self.disk.additional_mounts.iter().any(|mount| &mount.device == root)
        {
            issues.push("root partition cannot also be used as an additional mount".into());
        }
        if let Some(efi) = &self.disk.efi_partition
            && self.disk.additional_mounts.iter().any(|mount| &mount.device == efi)
        {
            issues.push("EFI partition cannot also be used as an additional mount".into());
        }
        if let Some(swap) = &self.disk.swap_partition
            && self.disk.additional_mounts.iter().any(|mount| &mount.device == swap)
        {
            issues.push("swap partition cannot also be used as an additional mount".into());
        }
        let mut devices = Vec::<PathBuf>::new();
        let mut mountpoints = Vec::<String>::new();
        for mount in &self.disk.additional_mounts {
            validate_device_path(&mount.device, "additional_mounts.device", &mut issues);
            if !mount.mountpoint.starts_with('/') || mount.mountpoint == "/" || mount.mountpoint.contains("..") {
                issues.push(format!("mountpoint '{}' must be absolute, non-root and without '..'", mount.mountpoint));
            }
            if devices.contains(&mount.device) {
                issues.push(format!("device {} appears more than once", mount.device.display()));
            }
            if mountpoints.contains(&mount.mountpoint) {
                issues.push(format!("mountpoint '{}' appears more than once", mount.mountpoint));
            }
            devices.push(mount.device.clone());
            mountpoints.push(mount.mountpoint.clone());
        }
        if self.software.kernels.is_empty() || !self.software.kernels.contains(&Kernel::Linux) {
            issues.push("software.kernels must include 'linux' as the guaranteed fallback kernel".into());
        }
        let user_services_supported = match self.system.init_system {
            InitSystem::Dinit => matches!(self.system.user_services, UserServices::None | UserServices::Userspawn | UserServices::Turnstile),
            InitSystem::OpenRc => matches!(self.system.user_services, UserServices::None | UserServices::Native | UserServices::Turnstile),
            InitSystem::Runit => matches!(self.system.user_services, UserServices::None | UserServices::Userspawn),
            InitSystem::S6 => matches!(self.system.user_services, UserServices::None | UserServices::Manual),
        };
        if !user_services_supported {
            issues.push(format!(
                "user_services = '{}' is not a supported setup choice for {}; choose an init-specific option",
                self.system.user_services, self.system.init_system
            ));
        }
        if self.software.desktop_environments.contains(&Desktop::Cosmalia)
            && self.system.init_system != InitSystem::Dinit {
            issues.push("Cosmalia requires dinit".into());
        }
        if self.software.desktop_environments.contains(&Desktop::SwayFx)
            && self.software.desktop_environments.contains(&Desktop::Sway) {
            issues.push("SwayFX replaces Sway; select only one".into());
        }
        if self.software.desktop_environments.contains(&Desktop::SwayFx)
            && self.software.desktop_environments.contains(&Desktop::Umbriel) {
            issues.push("SwayFX and Umbriel require conflicting scenefx versions; select only one".into());
        }
        if self.software.noctalia_greeter
            && (self.software.desktop_environments.is_empty() || self.software.desktop_environments.contains(&Desktop::Cosmic)) {
            issues.push("noctalia-greeter requires a non-COSMIC desktop/WM selection".into());
        }
        if issues.is_empty() { Ok(()) } else { Err(issues) }
    }
}

fn validate_device_path(path: &Path, field: &str, issues: &mut Vec<String>) {
    let text = path.to_string_lossy();
    if !text.starts_with("/dev/") || text.contains('\n') || text.contains('\r')
        || text.split('/').any(|part| part == "..") {
        issues.push(format!("{field} must be an explicit /dev/... path without '..' (prefer /dev/disk/by-id/...)"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_valid_and_leave_disk_unselected() {
        let config = InstallConfig::default();
        assert!(config.validate().is_ok());
        assert!(config.disk.target.is_none());
        assert_eq!(config.software.kernels, vec![Kernel::Linux]);
    }

    #[test]
    fn dual_boot_cannot_format_efi() {
        let mut config = InstallConfig::default();
        config.disk.partitioning = Partitioning::DualBoot;
        config.disk.efi_policy = EfiPolicy::Format;
        assert!(config.validate().is_err());
    }

    #[test]
    fn target_must_be_an_explicit_device_path() {
        let mut config = InstallConfig::default();
        config.disk.target = Some(PathBuf::from("nvme0n1"));
        assert!(config.validate().is_err());
    }

    #[test]
    fn bios_requires_grub_and_rejects_f2fs_by_default() {
        let mut config = InstallConfig::default();
        config.system.firmware = FirmwareMode::Bios;
        config.software.bootloader = Bootloader::Limine;
        assert!(config.validate().is_err());
        config.software.bootloader = Bootloader::Grub;
        config.disk.root_filesystem = Filesystem::F2fs;
        assert!(config.validate().is_err());
    }

    #[test]
    fn desktop_conflicts_are_rejected() {
        let mut config = InstallConfig::default();
        config.software.desktop_environments = vec![Desktop::Sway, Desktop::SwayFx];
        assert!(config.validate().is_err());
        config.software.desktop_environments = vec![Desktop::Umbriel, Desktop::SwayFx];
        assert!(config.validate().is_err());
    }

    #[test]
    fn user_service_options_are_init_specific() {
        let mut config = InstallConfig::default();
        config.system.init_system = InitSystem::S6;
        config.system.user_services = UserServices::Turnstile;
        assert!(config.validate().unwrap_err().iter().any(|issue| issue.contains("user_services")));
        config.system.user_services = UserServices::Manual;
        assert!(config.validate().is_ok());

        config.system.init_system = InitSystem::OpenRc;
        config.system.user_services = UserServices::Userspawn;
        assert!(config.validate().unwrap_err().iter().any(|issue| issue.contains("user_services")));
        config.system.user_services = UserServices::Native;
        assert!(config.validate().is_ok());
        config.system.user_services = UserServices::Turnstile;
        assert!(config.validate().is_ok());

        config.system.init_system = InitSystem::Runit;
        config.system.user_services = UserServices::Userspawn;
        assert!(config.validate().is_ok());
    }

    #[test]
    fn cosmalia_requires_dinit() {
        let mut config = InstallConfig::default();
        config.system.init_system = InitSystem::Runit;
        config.software.desktop_environments = vec![Desktop::Cosmalia];
        assert!(config.validate().is_err());
    }

    #[test]
    fn zswap_only_allowed_with_real_swap() {
        let mut config = InstallConfig::default();
        config.disk.swap.zswap = true;
        assert!(config.validate().is_err());
        config.disk.swap.mode = SwapMode::Swapfile;
        assert!(config.validate().is_ok());
    }

    #[test]
    fn manual_uefi_requires_root_and_efi_partitions() {
        let mut config = InstallConfig::default();
        config.disk.partitioning = Partitioning::Manual;
        let issues = config.validate().unwrap_err();
        assert!(issues.iter().any(|issue| issue.contains("root_partition")));
        assert!(issues.iter().any(|issue| issue.contains("efi_partition")));
        config.disk.root_partition = Some("/dev/nvme0n1p2".into());
        config.disk.efi_partition = Some("/dev/nvme0n1p1".into());
        assert!(config.validate().is_ok());
    }

    #[test]
    fn manual_swap_partition_requires_an_explicit_device() {
        let mut config = InstallConfig::default();
        config.disk.partitioning = Partitioning::Manual;
        config.disk.root_partition = Some("/dev/sda2".into());
        config.disk.efi_partition = Some("/dev/sda1".into());
        config.disk.swap.mode = SwapMode::Partition;
        assert!(config.validate().unwrap_err().iter().any(|issue| issue.contains("swap_partition")));
        config.disk.swap_partition = Some("/dev/sda3".into());
        assert!(config.validate().is_ok());
    }

    #[test]
    fn automatic_mode_rejects_stale_manual_partition_paths() {
        let mut config = InstallConfig::default();
        config.disk.root_partition = Some("/dev/sda2".into());
        assert!(config.validate().unwrap_err().iter().any(|issue| issue.contains("calculates its own partition paths")));
    }

    #[test]
    fn dual_boot_requires_uefi_and_preserved_efi() {
        let mut config = InstallConfig::default();
        config.disk.partitioning = Partitioning::DualBoot;
        config.disk.root_partition = Some("/dev/sda3".into());
        config.disk.efi_partition = Some("/dev/sda1".into());
        config.disk.efi_policy = EfiPolicy::Format;
        assert!(config.validate().unwrap_err().iter().any(|issue| issue.contains("preserve")));
        config.system.firmware = FirmwareMode::Bios;
        config.disk.efi_policy = EfiPolicy::Preserve;
        assert!(config.validate().unwrap_err().iter().any(|issue| issue.contains("UEFI")));
        config.system.firmware = FirmwareMode::Uefi;
        assert!(config.validate().is_ok());
    }

    #[test]
    fn additional_mounts_are_manual_only() {
        let mut config = InstallConfig::default();
        config.disk.additional_mounts.push(MountConfig {
            device: "/dev/sda4".into(),
            mountpoint: "/home".into(),
            filesystem: Filesystem::Ext4,
            format: false,
        });
        assert!(config.validate().unwrap_err().iter().any(|issue| issue.contains("manual partitioning")));
        config.disk.partitioning = Partitioning::Manual;
        config.disk.root_partition = Some("/dev/sda2".into());
        config.disk.efi_partition = Some("/dev/sda1".into());
        assert!(config.validate().is_ok());
    }

    #[test]
    fn shipped_profiles_parse_and_validate() {
        for profile in [
            include_str!("../configs/minimal-dinit.toml"),
            include_str!("../configs/wayland-hyprland.toml"),
            include_str!("../configs/gaming-dinit.toml"),
        ] {
            let config: InstallConfig = toml::from_str(profile).expect("parse profile");
            assert!(config.validate().is_ok());
        }
    }
}
