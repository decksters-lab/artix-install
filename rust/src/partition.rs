use crate::config::{
    BtrfsCompression, BtrfsLayout, EfiPolicy, Filesystem, FirmwareMode, InstallConfig,
    Partitioning, SwapMode,
};

#[derive(Debug, Clone, Default)]
pub struct PartitionPlan {
    pub operations: Vec<String>,
    pub warnings: Vec<String>,
    pub blocked: Vec<String>,
}

impl PartitionPlan {
    pub fn is_reviewable(&self) -> bool {
        self.blocked.is_empty() && !self.operations.is_empty()
    }

    pub fn render(&self) -> String {
        let mut out = String::from("PARTITION LAYOUT PLAN — PREVIEW ONLY\n====================================\n\n");
        out.push_str("This is a description of proposed operations. Nothing is executed.\n");
        out.push_str("There is no partitioner, formatter, cryptsetup, mount or swap activation code in this build.\n\n");

        if !self.blocked.is_empty() {
            out.push_str("BLOCKED — CONFIGURATION INCOMPLETE\n----------------------------------\n");
            for item in &self.blocked {
                out.push_str(&format!("- {item}\n"));
            }
            out.push('\n');
        }

        if !self.operations.is_empty() {
            out.push_str("PROPOSED OPERATIONS\n-------------------\n");
            for (index, operation) in self.operations.iter().enumerate() {
                out.push_str(&format!("{:>2}. {operation}\n", index + 1));
            }
            out.push('\n');
        }

        if !self.warnings.is_empty() {
            out.push_str("REVIEW NOTES\n------------\n");
            for warning in &self.warnings {
                out.push_str(&format!("- {warning}\n"));
            }
            out.push('\n');
        }

        if self.is_reviewable() {
            out.push_str("Status: layout is complete enough for human review only.\n");
        } else {
            out.push_str("Status: no destructive action is available; resolve the blocked items first.\n");
        }
        out.push_str("Execution status: DISABLED — this build cannot change disks.\n");
        out
    }
}

pub fn build_partition_plan(config: &InstallConfig) -> PartitionPlan {
    let mut plan = PartitionPlan::default();
    let Some(target) = config.disk.target.as_ref() else {
        plan.blocked.push("Choose an explicit target disk in the read-only disk inventory.".into());
        return plan;
    };

    plan.operations.push(format!(
        "Recheck target identity, serial/WWN, size, partition ancestry, mountpoints, active swap and holders for {} immediately before any future write operation.",
        target.display()
    ));
    plan.warnings.push(
        "This planner has no live device-topology binding yet. Manual/dual-boot partition paths must be checked against the selected disk before an executor is ever implemented.".into()
    );

    match config.disk.partitioning {
        Partitioning::Automatic => plan_automatic(config, &mut plan, &target.display().to_string()),
        Partitioning::Manual => plan_manual(config, &mut plan),
        Partitioning::DualBoot => plan_dual_boot(config, &mut plan),
    }

    if plan.blocked.is_empty() {
        let root_device = if config.disk.encryption {
            "/dev/mapper/cryptroot".to_owned()
        } else {
            match config.disk.partitioning {
                Partitioning::Automatic => "<planned root partition>".to_owned(),
                Partitioning::Manual | Partitioning::DualBoot => config.disk.root_partition
                    .as_ref().map(|path| path.display().to_string())
                    .unwrap_or_else(|| "<selected root partition>".into()),
            }
        };
        plan_root_and_boot(config, &mut plan, &root_device);
        if config.disk.partitioning == Partitioning::Manual {
            plan_additional_mounts(config, &mut plan);
        }
    }

    plan
}

fn plan_automatic(config: &InstallConfig, plan: &mut PartitionPlan, target: &str) {
    let uefi = config.system.firmware == FirmwareMode::Uefi;
    let table = if uefi { "GPT" } else { "DOS/MBR" };
    plan.operations.push(format!(
        "Create a new {table} partition table on {target}. This discards the existing partition table and all data on that disk."
    ));

    let mut next_number = 1_u8;
    if uefi {
        plan.operations.push(format!(
            "Create partition {next_number}: 1 GiB EFI System Partition (FAT32), then format it as FAT32."
        ));
        next_number += 1;
    }

    if config.disk.swap.mode == SwapMode::Partition {
        let number = next_number;
        plan.operations.push(format!(
            "Create partition {number}: {} GiB Linux swap partition; initialize it with mkswap.",
            config.disk.swap.size_gib
        ));
        next_number += 1;
    }

    let root_number = next_number;
    plan.operations.push(format!(
        "Create partition {root_number}: use the remaining space as the Linux root partition ({}){}.",
        config.disk.root_filesystem,
        if uefi { "." } else { " (bootable MBR partition)." }
    ));
    if config.disk.encryption {
        plan.operations.push(format!(
            "Apply LUKS2 to root partition {root_number}, open it as /dev/mapper/cryptroot, and format the mapped device as {}. The EFI partition remains outside encryption.",
            config.disk.root_filesystem
        ));
    } else {
        plan.operations.push(format!(
            "Format root partition {root_number} as {}.",
            config.disk.root_filesystem
        ));
    }
}

fn plan_manual(config: &InstallConfig, plan: &mut PartitionPlan) {
    let root = config.disk.root_partition.as_ref().map(|p| p.display().to_string());
    let uefi = config.system.firmware == FirmwareMode::Uefi;
    let efi = config.disk.efi_partition.as_ref().map(|p| p.display().to_string());

    if root.is_none() {
        plan.blocked.push("Manual mode requires disk.root_partition.".into());
    }
    if uefi && efi.is_none() {
        plan.blocked.push("Manual UEFI mode requires disk.efi_partition.".into());
    }
    if config.disk.swap.mode == SwapMode::Partition && config.disk.swap_partition.is_none() {
        plan.blocked.push("Manual swap-partition mode requires disk.swap_partition.".into());
    }
    if !plan.blocked.is_empty() {
        return;
    }
    let root = root.expect("root presence checked above");

    if uefi {
        let efi = efi.expect("EFI presence checked above");
        if config.disk.efi_policy == EfiPolicy::Preserve {
            plan.operations.push(format!(
                "Preserve existing EFI partition {efi} without formatting; verify it is a valid FAT32 EFI System Partition."
            ));
        } else {
            plan.operations.push(format!(
                "Format selected EFI partition {efi} as FAT32. This erases existing boot files on that partition."
            ));
        }
    }

    plan.operations.push(format!("Use manually-created root partition {root}."));
    if config.disk.encryption {
        plan.operations.push(format!(
            "Apply LUKS2 to {root}, open it as /dev/mapper/cryptroot, and format the mapped device as {}.",
            config.disk.root_filesystem
        ));
    } else {
        plan.operations.push(format!("Format {root} as {}.", config.disk.root_filesystem));
    }

    if config.disk.swap.mode == SwapMode::Partition {
        let swap = config.disk.swap_partition.as_ref().expect("swap presence checked above");
        plan.operations.push(format!(
            "Initialize selected swap partition {} with mkswap; it will be enabled after the root and mountpoints are ready.",
            swap.display()
        ));
        plan.warnings.push(format!(
            "The selected swap partition {} is an explicit manual choice; its identity and size must be rechecked against live block-device metadata.",
            swap.display()
        ));
    }

}

fn plan_dual_boot(config: &InstallConfig, plan: &mut PartitionPlan) {
    let Some(efi) = config.disk.efi_partition.as_ref() else {
        plan.blocked.push("Dual-boot requires an explicitly selected existing EFI partition.".into());
        return;
    };
    let Some(root) = config.disk.root_partition.as_ref() else {
        plan.blocked.push("Dual-boot requires an explicitly selected root partition.".into());
        return;
    };
    if efi == root {
        plan.blocked.push("EFI and root cannot be the same partition.".into());
        return;
    }

    plan.operations.push("Preserve the existing partition table. Do not run wipefs, repartitioning or mklabel.".into());
    plan.operations.push(format!(
        "Preserve EFI partition {} byte-for-byte: do not format it. Verify it is FAT32.",
        efi.display()
    ));
    if config.disk.encryption {
        plan.operations.push(format!(
            "Apply LUKS2 to selected Artix root partition {}, open it as /dev/mapper/cryptroot, and format the mapped device as {}. Do not touch the EFI partition.",
            root.display(), config.disk.root_filesystem
        ));
    } else {
        plan.operations.push(format!(
            "Format only the selected Artix root partition {} as {}. Do not format any other existing partition.",
            root.display(), config.disk.root_filesystem
        ));
    }
    plan.warnings.push(
        "Dual-boot mode intentionally does not create or reformat a swap partition. Use zram or a swapfile if swap is wanted.".into()
    );
}

fn plan_additional_mounts(config: &InstallConfig, plan: &mut PartitionPlan) {
    for mount in &config.disk.additional_mounts {
        if mount.format {
            plan.operations.push(format!(
                "Format additional partition {} as {} and mount it at {}.",
                mount.device.display(), mount.filesystem, mount.mountpoint
            ));
        } else {
            plan.operations.push(format!(
                "Preserve the existing filesystem on additional partition {} and mount it at {} (verify filesystem and contents first).",
                mount.device.display(), mount.mountpoint
            ));
        }
    }
}

fn plan_root_and_boot(config: &InstallConfig, plan: &mut PartitionPlan, root_device: &str) {
    if config.disk.root_filesystem == Filesystem::Btrfs {
        plan_btrfs_layout(config, plan, root_device);
    } else {
        plan.operations.push(format!("Mount {root_device} at / as the root filesystem."));
    }

    if config.system.firmware == FirmwareMode::Uefi {
        let efi_device = match config.disk.partitioning {
            Partitioning::Automatic => "<planned EFI partition 1>".to_owned(),
            Partitioning::Manual | Partitioning::DualBoot => config.disk.efi_partition
                .as_ref().map(|path| path.display().to_string())
                .unwrap_or_else(|| "<selected EFI partition>".into()),
        };
        plan.operations.push(format!("Mount EFI partition {efi_device} at /boot after the root filesystem is mounted."));
    }

    match config.disk.swap.mode {
        SwapMode::None => {}
        SwapMode::Zram | SwapMode::Both => {
            plan.operations.push("Enable zram-backed compressed swap at boot.".into());
        }
        SwapMode::Partition => {
            if let Some(partition) = config.disk.swap_partition.as_ref() {
                plan.operations.push(format!("Enable swap partition {}.", partition.display()));
            } else if config.disk.partitioning == Partitioning::Automatic {
                plan.operations.push(format!(
                    "Enable the planned {} GiB swap partition.",
                    config.disk.swap.size_gib
                ));
            }
        }
        SwapMode::Swapfile => {}
    }

    if matches!(config.disk.swap.mode, SwapMode::Swapfile | SwapMode::Both) {
        if config.disk.root_filesystem == Filesystem::Btrfs {
            let swap_path = if config.disk.btrfs.layout == BtrfsLayout::Flat {
                "/swapfile"
            } else {
                "/swap/swapfile"
            };
            plan.operations.push(format!(
                "Create a {} GiB Btrfs swap file at {swap_path}; set NOCOW (+C) before allocation, set mode 0600, then mkswap and enable it.",
                config.disk.swap.size_gib
            ));
        } else if matches!(config.disk.root_filesystem, Filesystem::F2fs | Filesystem::Nilfs2) {
            plan.warnings.push(format!(
                "Swapfile support on {} must be verified by a live capability check; the existing shell installer skips the file if swapon fails. Prefer zram or a swap partition if unsupported.",
                config.disk.root_filesystem
            ));
            plan.operations.push(format!(
                "Attempt a {} GiB swap file on the root filesystem only after a capability check; mode 0600, mkswap, then swapon.",
                config.disk.swap.size_gib
            ));
        } else {
            plan.operations.push(format!(
                "Create a {} GiB swap file at /swapfile, set mode 0600, initialize with mkswap and enable it.",
                config.disk.swap.size_gib
            ));
        }
    }

    if config.disk.swap.zswap {
        plan.operations.push("Enable the zswap kernel parameter in the selected bootloader configuration.".into());
    }
}

fn plan_btrfs_layout(config: &InstallConfig, plan: &mut PartitionPlan, root_device: &str) {
    if config.disk.btrfs.layout == BtrfsLayout::Flat {
        plan.operations.push(format!(
            "Use a flat Btrfs root on {root_device}; do not create subvolumes."
        ));
        let options = btrfs_mount_options(config, None);
        plan.operations.push(format!("Mount {root_device} at / with options: {options}."));
        return;
    }

    let mut subvolumes = vec![
        ("@", "/"),
        ("@home", "/home"),
        ("@log", "/var/log"),
        ("@cache", "/var/cache"),
    ];
    if config.disk.btrfs.layout == BtrfsLayout::Snapshots {
        subvolumes.push(("@snapshots", "/.snapshots"));
    }
    if matches!(config.disk.swap.mode, SwapMode::Swapfile | SwapMode::Both) {
        subvolumes.push(("@swap", "/swap"));
    }

    plan.operations.push(format!(
        "Mount Btrfs filesystem {root_device} temporarily as the top-level volume; create the following subvolumes:"
    ));
    for (name, mountpoint) in &subvolumes {
        plan.operations.push(format!("Create Btrfs subvolume {name} for {mountpoint}."));
    }
    plan.operations.push("Unmount the temporary top-level mount, then mount subvolume @ as the root filesystem at /.".into());
    for (name, mountpoint) in subvolumes.iter().skip(1) {
        let options = btrfs_mount_options(config, Some(name));
        plan.operations.push(format!(
            "Mount subvolume {name} at {mountpoint} with options: {options}."
        ));
    }
    let root_options = btrfs_mount_options(config, Some("@"));
    plan.operations.push(format!("Root mount options: {root_options}."));
}

fn btrfs_mount_options(config: &InstallConfig, subvolume: Option<&str>) -> String {
    let mut options = vec!["noatime".to_owned()];
    match config.disk.btrfs.compression {
        BtrfsCompression::None => {}
        BtrfsCompression::Zstd => options.push("compress=zstd:3".into()),
        BtrfsCompression::Lzo => options.push("compress=lzo".into()),
    }
    if let Some(subvolume) = subvolume {
        options.push(format!("subvol={subvolume}"));
    }
    options.join(",")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn with_target() -> InstallConfig {
        let mut config = InstallConfig::default();
        config.disk.target = Some(PathBuf::from("/dev/nvme0n1"));
        config
    }

    #[test]
    fn no_target_blocks_all_partition_steps() {
        let plan = build_partition_plan(&InstallConfig::default());
        assert!(!plan.is_reviewable());
        assert!(plan.blocked.iter().any(|item| item.contains("explicit target disk")));
        assert!(plan.operations.is_empty());
    }

    #[test]
    fn automatic_uefi_layout_uses_gpt_efi_and_root() {
        let config = with_target();
        let plan = build_partition_plan(&config);
        let text = plan.render();
        assert!(text.contains("GPT"));
        assert!(text.contains("1 GiB EFI System Partition"));
        assert!(text.contains("FAT32"));
        assert!(text.contains("remaining space as the Linux root"));
        assert!(plan.is_reviewable());
    }

    #[test]
    fn automatic_swap_partition_is_inserted_between_efi_and_root() {
        let mut config = with_target();
        config.disk.swap.mode = SwapMode::Partition;
        let text = build_partition_plan(&config).render();
        assert!(text.contains("partition 2: 4 GiB Linux swap"));
        assert!(text.contains("partition 3: use the remaining space"));
    }

    #[test]
    fn bios_automatic_layout_uses_dos_and_no_efi() {
        let mut config = with_target();
        config.system.firmware = FirmwareMode::Bios;
        config.software.bootloader = crate::config::Bootloader::Grub;
        let text = build_partition_plan(&config).render();
        assert!(text.contains("DOS/MBR"));
        assert!(!text.contains("EFI System Partition"));
    }

    #[test]
    fn manual_mode_requires_explicit_root_and_efi() {
        let mut config = with_target();
        config.disk.partitioning = Partitioning::Manual;
        let plan = build_partition_plan(&config);
        assert!(!plan.is_reviewable());
        assert!(plan.blocked.iter().any(|item| item.contains("root partition")));
        assert!(plan.blocked.iter().any(|item| item.contains("EFI partition")));
    }

    #[test]
    fn manual_mode_can_preserve_efi_and_format_only_root() {
        let mut config = with_target();
        config.disk.partitioning = Partitioning::Manual;
        config.disk.efi_partition = Some(PathBuf::from("/dev/nvme0n1p1"));
        config.disk.root_partition = Some(PathBuf::from("/dev/nvme0n1p2"));
        let text = build_partition_plan(&config).render();
        assert!(text.contains("Preserve existing EFI partition"));
        assert!(text.contains("Format /dev/nvme0n1p2 as ext4"));
        assert!(!text.contains("Format selected EFI partition"));
    }

    #[test]
    fn dual_boot_preserves_efi_and_existing_partition_table() {
        let mut config = with_target();
        config.disk.partitioning = Partitioning::DualBoot;
        config.disk.efi_partition = Some(PathBuf::from("/dev/nvme0n1p1"));
        config.disk.root_partition = Some(PathBuf::from("/dev/nvme0n1p4"));
        let text = build_partition_plan(&config).render();
        assert!(text.contains("Preserve the existing partition table"));
        assert!(text.contains("Preserve EFI partition /dev/nvme0n1p1 byte-for-byte"));
        assert!(text.contains("Format only the selected Artix root partition /dev/nvme0n1p4"));
        assert!(!text.contains("Format selected EFI partition"));
    }

    #[test]
    fn btrfs_standard_layout_creates_expected_subvolumes() {
        let mut config = with_target();
        config.disk.root_filesystem = Filesystem::Btrfs;
        let text = build_partition_plan(&config).render();
        for name in ["@", "@home", "@log", "@cache"] {
            assert!(text.contains(&format!("Create Btrfs subvolume {name}")));
        }
        assert!(text.contains("compress=zstd:3"));
    }

    #[test]
    fn btrfs_swapfile_gets_a_dedicated_nocow_subvolume() {
        let mut config = with_target();
        config.disk.root_filesystem = Filesystem::Btrfs;
        config.disk.swap.mode = SwapMode::Swapfile;
        let text = build_partition_plan(&config).render();
        assert!(text.contains("Create Btrfs subvolume @swap"));
        assert!(text.contains("NOCOW on /swap/swapfile"));
    }

    #[test]
    fn encrypted_root_formats_the_luks_mapping_not_the_raw_partition() {
        let mut config = with_target();
        config.disk.encryption = true;
        let text = build_partition_plan(&config).render();
        assert!(text.contains("Apply LUKS2 to root partition"));
        assert!(text.contains("format the mapped device as ext4"));
    }
}
