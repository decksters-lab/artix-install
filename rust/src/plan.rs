use crate::{config::{InstallConfig, Partitioning}, partition};

#[derive(Debug, Clone, Copy)]
enum Risk { Info, Review, Destructive, Blocked }

struct PlanItem { title: String, detail: String, risk: Risk }

pub fn format_plan(config: &InstallConfig) -> String {
    let mut output = String::from("ARTIX SETUP — INSTALLATION PLAN\n================================\n\n");
    output.push_str("PLANNING-ONLY PROTOTYPE\nThis preview does not execute commands, partition disks, format filesystems,\nopen LUKS mappings, mount filesystems, install packages, or change the host.\n\n");
    for (index, item) in build_items(config).iter().enumerate() {
        let label = match item.risk {
            Risk::Info => "INFO", Risk::Review => "REVIEW",
            Risk::Destructive => "DESTRUCTIVE — NOT EXECUTABLE", Risk::Blocked => "BLOCKED",
        };
        output.push_str(&format!("{:>2}. [{}] {}\n    {}\n", index + 1, label, item.title, item.detail));
    }
    output.push_str("\nSELECTED CONFIGURATION\n----------------------\n");
    output.push_str(&format!("Init system:       {}\n", config.system.init_system));
    output.push_str(&format!("Firmware:          {}\n", config.system.firmware));
    output.push_str(&format!("Target disk:       {}\n", config.disk.target.as_ref().map_or_else(|| "NOT SELECTED".into(), |p| p.display().to_string())));
    output.push_str(&format!("Partitioning:      {}\n", config.disk.partitioning));
    output.push_str(&format!("Root filesystem:   {}\n", config.disk.root_filesystem));
    output.push_str(&format!("Encryption:        {}\n", if config.disk.encryption { "LUKS2 requested" } else { "off" }));
    output.push_str(&format!("Swap:              {} ({} GiB, zswap: {})\n", config.disk.swap.mode, config.disk.swap.size_gib, config.disk.swap.zswap));
    output.push_str(&format!("EFI policy:        {}\n", config.disk.efi_policy));
    output.push_str(&format!("Bootloader:        {}\n", config.software.bootloader));
    output.push_str(&format!("Desktops / WMs:    {}\n", list_desktops(config)));
    output.push_str(&format!("Kernels:            {}\n", config.software.kernels.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ")));
    output.push_str(&format!("CPU / GPU:         {} / {}\n", config.software.cpu_vendor, config.software.gpu));
    output.push_str(&format!("Network:           {}\n", config.software.network));
    output.push_str(&format!("Privilege:         {}\n", config.system.privilege_escalation));
    output.push_str(&format!("User services:     {}\n", config.system.user_services));
    output.push_str(&format!("Multilib:          {}\n", config.software.multilib));
    output.push_str("\nPARTITION LAYOUT PREVIEW\n------------------------\n");
    output.push_str(&partition::build_partition_plan(config).render());
    output.push_str("\nNo disk operation is permitted by this prototype.\n");
    output
}

fn build_items(config: &InstallConfig) -> Vec<PlanItem> {
    let mut items = vec![
        PlanItem { title: "Inspect installer environment".into(),
            detail: "Future discovery module: verify Artix live media, root permissions, firmware, memory, network, CPU/GPU and required tools.".into(), risk: Risk::Info },
        PlanItem { title: "Discover block devices".into(),
            detail: "Future read-only module: inspect model, size, serial/WWN, partition table, filesystems, UUIDs, mountpoints and holders. Never choose the first disk automatically.".into(), risk: Risk::Info },
    ];
    match &config.disk.target {
        None => {
            items.push(PlanItem { title: "Select and review a target disk".into(),
                detail: "No target is selected. Partitioning, formatting, encryption, mounting and installation are blocked until a user explicitly chooses a device.".into(), risk: Risk::Blocked });
            items.push(PlanItem { title: "Partition and format target".into(),
                detail: "Not generated because disk.target is unset. No default disk will ever be selected automatically.".into(), risk: Risk::Blocked });
        }
        Some(target) => {
            items.push(PlanItem { title: format!("Review target disk {}", target.display()),
                detail: "Future module: verify device identity, capacity, partition table, live-media ancestry and mount state; require typed confirmation of the exact device path.".into(), risk: Risk::Review });
            if config.disk.partitioning == Partitioning::DualBoot
                && (config.disk.efi_partition.is_none() || config.disk.root_partition.is_none()) {
                items.push(PlanItem { title: "Choose existing EFI and root partitions".into(),
                    detail: "Dual-boot mode needs explicit EFI and root partition selections. EFI is locked to preserve; it must not be formatted.".into(), risk: Risk::Blocked });
            } else {
                let detail = match config.disk.partitioning {
                    Partitioning::Automatic => format!("Would create a fresh partition layout on {} using {} root. This overwrites the target device and requires typed confirmation. EFI policy: {}.", target.display(), config.disk.root_filesystem, config.disk.efi_policy),
                    Partitioning::Manual => format!("Would guide partition selection for {} and assign root, EFI and optional extra mountpoints. Formatting is opt-in and separately reviewed.", target.display()),
                    Partitioning::DualBoot => format!("Would preserve the partition table on {} and use root {} with existing EFI {}. EFI must never be formatted.", target.display(), config.disk.root_partition.as_ref().map_or("unset".into(), |p| p.display().to_string()), config.disk.efi_partition.as_ref().map_or("unset".into(), |p| p.display().to_string())),
                };
                items.push(PlanItem { title: "Partition and prepare storage".into(), detail, risk: Risk::Destructive });
            }
        }
    }
    let storage_ready = config.disk.target.is_some()
        && (config.disk.partitioning != Partitioning::DualBoot
            || (config.disk.efi_partition.is_some() && config.disk.root_partition.is_some()));
    let risk = if storage_ready { Risk::Review } else { Risk::Blocked };
    items.push(PlanItem { title: "Set up encryption, filesystems and swap".into(),
        detail: format!("LUKS2: {}; root: {}; swap: {} ({} GiB); zswap: {}. EFI policy: {}. No passphrase is stored in the profile.", config.disk.encryption, config.disk.root_filesystem, config.disk.swap.mode, config.disk.swap.size_gib, config.disk.swap.zswap, config.disk.efi_policy), risk });
    items.push(PlanItem { title: "Install Artix base system".into(),
        detail: format!("Future bootstrap module: install base system and configure {} with init-specific package names and service handling.", config.system.init_system), risk });
    items.push(PlanItem { title: "Configure users and system defaults".into(),
        detail: format!("Hostname {}, user {}, locale {}, timezone {}, keyboard {}; configure {} and {}.", config.system.hostname, config.system.username, config.system.locale, config.system.timezone, config.system.keyboard_layout, config.system.privilege_escalation, config.system.user_services), risk });
    items.push(PlanItem { title: "Install kernels, graphics and desktop packages".into(),
        detail: format!("Kernels: {}; CPU: {}; GPU: {}; desktops: {}. Optional package availability must be checked against live repositories.", config.software.kernels.iter().map(ToString::to_string).collect::<Vec<_>>().join(", "), config.software.cpu_vendor, config.software.gpu, list_desktops(config)), risk });
    items.push(PlanItem { title: "Configure networking, multilib and bootloader".into(),
        detail: format!("Network: {}; multilib: {}; bootloader: {}. Firmware compatibility and init-specific service activation must be validated.", config.software.network, config.software.multilib, config.software.bootloader), risk });
    items.push(PlanItem { title: "Verify boot and installed services".into(),
        detail: "Future verification module: validate fstab, initramfs, kernel command line, bootloader entries, display manager and enabled services before offering reboot.".into(), risk });
    items
}

fn list_desktops(config: &InstallConfig) -> String {
    if config.software.desktop_environments.is_empty() { "CLI only".into() }
    else { config.software.desktop_environments.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ") }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Desktop, InstallConfig};
    use std::path::PathBuf;

    #[test]
    fn default_plan_blocks_disk_operations() {
        let output = format_plan(&InstallConfig::default());
        assert!(output.contains("[BLOCKED] Select and review a target disk"));
        assert!(output.contains("Target disk:       NOT SELECTED"));
        assert!(!output.contains("[DESTRUCTIVE — NOT EXECUTABLE]"));
    }
    #[test]
    fn configured_target_is_non_executable() {
        let mut config = InstallConfig::default();
        config.disk.target = Some(PathBuf::from("/dev/disk/by-id/example"));
        let output = format_plan(&config);
        assert!(output.contains("[DESTRUCTIVE — NOT EXECUTABLE]"));
        assert!(output.contains("No disk operation is permitted"));
    }
    #[test]
    fn desktop_choices_appear_in_the_plan() {
        let mut config = InstallConfig::default();
        config.software.desktop_environments.push(Desktop::Hyprland);
        assert!(format_plan(&config).contains("hyprland"));
    }
}
