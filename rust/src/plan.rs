use crate::{
    config::{InstallConfig, Partitioning},
    partition,
};

#[derive(Debug, Clone, Copy)]
enum Risk {
    Info,
    Review,
    Destructive,
    Blocked,
}

struct PlanItem {
    title: String,
    detail: String,
    risk: Risk,
}

pub fn format_plan(config: &InstallConfig) -> String {
    let mut output = String::from(
        "ARTIX GRANULAR SYSTEM INSTALLER — INSTALLATION PLAN\n================================\n\n",
    );
    output.push_str("PLANNING-ONLY PROTOTYPE\nThis preview does not execute commands, partition disks, format filesystems,\nopen LUKS mappings, mount filesystems, install packages, or change the host.\n\n");
    if let Err(issues) = config.validate() {
        output.push_str("CONFIGURATION INVALID — INSTALL STEPS BLOCKED\n---------------------------------------------\n");
        for issue in issues {
            output.push_str(&format!("- {issue}\n"));
        }
        output.push('\n');
    }
    for (index, item) in build_items(config).iter().enumerate() {
        let label = match item.risk {
            Risk::Info => "INFO",
            Risk::Review => "REVIEW",
            Risk::Destructive => "DESTRUCTIVE — NOT EXECUTABLE",
            Risk::Blocked => "BLOCKED",
        };
        output.push_str(&format!(
            "{:>2}. [{}] {}\n    {}\n",
            index + 1,
            label,
            item.title,
            item.detail
        ));
    }
    output.push_str("\nSELECTED CONFIGURATION\n----------------------\n");
    output.push_str(&format!(
        "Init system:       {}\n",
        config.system.init_system
    ));
    output.push_str(&format!("Firmware:          {}\n", config.system.firmware));
    output.push_str(&format!(
        "Target disk:       {}\n",
        config
            .disk
            .target
            .as_ref()
            .map_or_else(|| "NOT SELECTED".into(), |p| p.display().to_string())
    ));
    output.push_str(&format!(
        "Partitioning:      {}\n",
        config.disk.partitioning
    ));
    output.push_str(&format!(
        "Root filesystem:   {}\n",
        config.disk.root_filesystem
    ));
    output.push_str(&format!(
        "Encryption:        {}\n",
        if config.disk.encryption {
            "LUKS2 requested"
        } else {
            "off"
        }
    ));
    output.push_str(&format!(
        "Swap:              {} ({} GiB, zswap: {})\n",
        config.disk.swap.mode, config.disk.swap.size_gib, config.disk.swap.zswap
    ));
    let efi_policy = if config.disk.partitioning == Partitioning::Automatic
        && config.system.firmware == crate::config::FirmwareMode::Uefi
    {
        "automatic: newly created EFI partition is formatted"
    } else if config.system.firmware == crate::config::FirmwareMode::Bios {
        "not applicable (BIOS)"
    } else {
        match config.disk.efi_policy {
            crate::config::EfiPolicy::Preserve => "preserve existing EFI partition",
            crate::config::EfiPolicy::Format => "format selected EFI partition",
        }
    };
    output.push_str(&format!("EFI policy:        {efi_policy}\n"));
    output.push_str(&format!(
        "Bootloader:        {}\n",
        config.software.bootloader
    ));
    output.push_str(&format!("Desktops / WMs:    {}\n", list_desktops(config)));
    output.push_str(&format!(
        "Kernels:            {}\n",
        config
            .software
            .kernels
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ")
    ));
    output.push_str(&format!(
        "CPU / GPU:         {} / {}\n",
        config.software.cpu_vendor, config.software.gpu
    ));
    output.push_str(&format!("Network:           {}\n", config.software.network));
    output.push_str(&format!(
        "Privilege:         {}\n",
        config.system.privilege_escalation
    ));
    output.push_str(&format!(
        "User services:     {}\n",
        config.system.user_services
    ));
    output.push_str(&format!(
        "Multilib:          {}\n",
        config.software.multilib
    ));
    output.push_str("\nPARTITION LAYOUT PREVIEW\n------------------------\n");
    output.push_str(&partition::build_partition_plan(config).render());
    output.push_str("\nPACKAGE PLAN PREVIEW\n--------------------\n");
    let package_plan = crate::packages::build_package_plan(config);
    output.push_str(&format!(
        "Base packages: {}\nKernel packages: {}\nInit packages: {}\nUser service packages: {}\nStorage packages: {}\nBootloader packages: {}\nNetwork: {}\nPrivilege: {}\n",
        package_plan.base_packages.join(" "),
        package_plan.kernel_packages.join(" "),
        package_plan.init_packages.join(" "),
        if package_plan.user_service_packages.is_empty() {
            "none".to_owned()
        } else {
            package_plan.user_service_packages.join(" ")
        },
        if package_plan.storage_packages.is_empty() {
            "base group (filesystem tools)".to_owned()
        } else {
            package_plan.storage_packages.join(" ")
        },
        package_plan.bootloader_packages.join(" "),
        package_plan.network_packages.join(" "),
        package_plan.privilege_packages.join(" ")
    ));
    for note in &package_plan.kernel_notes {
        output.push_str(&format!("Kernel note: {note}\n"));
    }
    output.push_str(&format!(
        "CPU microcode: {}\nGPU packages: {}\nXorg packages: {}\nAudio packages: {}\nOptional audio service integrations: {}\n",
        if package_plan.microcode_packages.is_empty() {
            "none".to_owned()
        } else {
            package_plan.microcode_packages.join(" ")
        },
        package_plan.gpu_packages.join(" "),
        if package_plan.xorg_packages.is_empty() {
            "not required".to_owned()
        } else {
            package_plan.xorg_packages.join(" ")
        },
        if package_plan.audio_packages.is_empty() {
            "not selected".to_owned()
        } else {
            package_plan.audio_packages.join(" ")
        },
        if package_plan.optional_service_packages.is_empty() {
            "none".to_owned()
        } else {
            package_plan.optional_service_packages.join(" ")
        }
    ));
    for recipe in &package_plan.desktop_recipes {
        output.push_str(&format!(
            "{} [{}]: {}\n",
            recipe.desktop,
            recipe.source,
            recipe.packages.join(" ")
        ));
        for note in &recipe.notes {
            output.push_str(&format!("    Note: {note}\n"));
        }
    }
    for note in &package_plan.deferred {
        output.push_str(&format!("Package check: {note}\n"));
    }
    output.push_str("\nThis plan command is read-only. Separate guarded commands can execute only the currently supported automatic, unencrypted UEFI/ext4 storage and base-install path.\n");
    output
}

fn build_items(config: &InstallConfig) -> Vec<PlanItem> {
    let storage_plan = partition::build_partition_plan(config);
    let validation_issues = config.validate().err().unwrap_or_default();
    let storage_ready = storage_plan.is_reviewable() && validation_issues.is_empty();
    let mut items = vec![
        PlanItem { title: "Inspect installer environment".into(),
            detail: "Read-only hardware discovery is available via --hardware and the TUI. This plan does not embed a fresh hardware snapshot; Artix live-media and required-tool checks remain future work.".into(), risk: Risk::Info },
        PlanItem { title: "Discover block devices".into(),
            detail: "Read-only sysfs/procfs/udev discovery is available in the hardware report and TUI. Partition-table type, filesystem UUID binding, live-media ancestry and stable identity revalidation are still pending. Never choose the first disk automatically.".into(), risk: Risk::Info },
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
                detail: "The supported automatic-install path re-discovers and rechecks target identity, capacity, partition layout, mount state and device sources before writes, and requires exact typed confirmation. Manual/dual-boot device-to-target binding remains pending.".into(), risk: Risk::Review });
            if !storage_ready {
                let mut reasons = storage_plan.blocked.clone();
                for issue in &validation_issues {
                    if !reasons.contains(issue) {
                        reasons.push(issue.clone());
                    }
                }
                let reason = if reasons.is_empty() {
                    "The selected storage configuration is incomplete.".to_owned()
                } else {
                    reasons.join(" ")
                };
                items.push(PlanItem {
                    title: "Partition and prepare storage".into(),
                    detail: format!("Blocked: {reason}"),
                    risk: Risk::Blocked,
                });
            } else {
                let detail = match config.disk.partitioning {
                    Partitioning::Automatic => format!(
                        "Would create a fresh partition layout on {} using {} root. This overwrites the target device and requires typed confirmation. EFI policy: {}.",
                        target.display(),
                        config.disk.root_filesystem,
                        config.disk.efi_policy
                    ),
                    Partitioning::Manual => format!(
                        "Would assign existing root, EFI and optional extra mountpoint partitions on {}. The selected root partition is formatted as {}; EFI follows the explicit preserve/format policy, and each additional mount has its own format flag.",
                        target.display(),
                        config.disk.root_filesystem
                    ),
                    Partitioning::DualBoot => format!(
                        "Would preserve the partition table on {} and use root {} with existing EFI {}. EFI must never be formatted.",
                        target.display(),
                        config
                            .disk
                            .root_partition
                            .as_ref()
                            .map_or("unset".into(), |p| p.display().to_string()),
                        config
                            .disk
                            .efi_partition
                            .as_ref()
                            .map_or("unset".into(), |p| p.display().to_string())
                    ),
                };
                items.push(PlanItem {
                    title: "Partition and prepare storage".into(),
                    detail,
                    risk: Risk::Destructive,
                });
            }
        }
    }
    let risk = if storage_ready {
        Risk::Review
    } else {
        Risk::Blocked
    };
    items.push(PlanItem { title: "Set up encryption, filesystems and swap".into(),
        detail: format!("LUKS2: {}; root: {}; swap: {} ({} GiB); zswap: {}. EFI policy: {}. No passphrase is stored in the profile.", config.disk.encryption, config.disk.root_filesystem, config.disk.swap.mode, config.disk.swap.size_gib, config.disk.swap.zswap, config.disk.efi_policy), risk });
    items.push(PlanItem { title: "Install Artix base system".into(),
        detail: format!("A guarded basestrap step is implemented for the supported automatic, unencrypted UEFI/ext4 path after storage preparation. Full init-specific system configuration and service handling remain pending for {}.", config.system.init_system), risk });
    items.push(PlanItem { title: "Configure users and system defaults".into(),
        detail: format!("Hostname {}, user {}, locale {}, timezone {}, keyboard {}; configure {}. User-service handling: {}.", config.system.hostname, config.system.username, config.system.locale, config.system.timezone, config.system.keyboard_layout, config.system.privilege_escalation, user_services_detail(config)), risk });
    items.push(PlanItem { title: "Install kernels, graphics and desktop packages".into(),
        detail: format!("Kernels: {}; CPU: {}; GPU: {}; desktops: {}. {} Optional package availability must be checked against live repositories.", config.software.kernels.iter().map(ToString::to_string).collect::<Vec<_>>().join(", "), config.software.cpu_vendor, config.software.gpu, list_desktops(config), desktop_details(config)), risk });
    items.push(PlanItem { title: "Configure networking, multilib and bootloader".into(),
        detail: format!("Network: {}; multilib: {}; bootloader: {}. Firmware compatibility and init-specific service activation must be validated.", config.software.network, config.software.multilib, config.software.bootloader), risk });
    items.push(PlanItem { title: "Verify boot and installed services".into(),
        detail: "Future verification module: validate fstab, initramfs, kernel command line, bootloader entries, display manager and enabled services before offering reboot.".into(), risk });
    items
}

fn user_services_detail(config: &InstallConfig) -> &'static str {
    use crate::config::{InitSystem, UserServices};
    match (config.system.init_system, config.system.user_services) {
        (_, UserServices::None) => "not requested",
        (InitSystem::Dinit, UserServices::Userspawn) => {
            "install userspawn-dinit (which depends on userspawn); verify the live repository before installation"
        }
        (InitSystem::Dinit, UserServices::Turnstile) => {
            "install turnstile with turnstile-dinit; verify the live repository and manually enable turnstiled"
        }
        (InitSystem::OpenRc, UserServices::Native) => {
            "use OpenRC native per-user services (OpenRC 0.62+); ensure XDG_RUNTIME_DIR and PAM session auto-start are configured"
        }
        (InitSystem::OpenRc, UserServices::Turnstile) => {
            "install turnstile with turnstile-openrc; verify the live repository and configure PAM activation"
        }
        (InitSystem::Runit, UserServices::Userspawn) => {
            "install userspawn-runit; verify the live repository before installation"
        }
        (InitSystem::S6, UserServices::Manual) => {
            "manual user-managed setup; do not assume Turnstile or configure it automatically"
        }
        _ => "invalid init/user-service combination; configuration validation must block this plan",
    }
}

fn desktop_details(config: &InstallConfig) -> &'static str {
    use crate::config::Desktop;
    if config
        .software
        .desktop_environments
        .contains(&Desktop::Cosmalia)
    {
        "Cosmalia follows grigio/cosmalia-session-dinit: Dinit session, cosmic-comp launched with --no-xwayland, and Noctalia as the shell. The upstream package manifest does not include a separate cosmic-osd component."
    } else {
        ""
    }
}

fn list_desktops(config: &InstallConfig) -> String {
    if config.software.desktop_environments.is_empty() {
        "CLI only".into()
    } else {
        config
            .software
            .desktop_environments
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ")
    }
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
        assert!(output.contains("This plan command is read-only"));
    }

    #[test]
    fn manual_plan_explicitly_discloses_root_formatting() {
        let mut config = InstallConfig::default();
        config.disk.target = Some(PathBuf::from("/dev/disk/by-id/example"));
        config.disk.partitioning = Partitioning::Manual;
        config.disk.root_partition = Some(PathBuf::from("/dev/sda2"));
        config.disk.efi_partition = Some(PathBuf::from("/dev/sda1"));
        let output = format_plan(&config);
        assert!(output.contains("The selected root partition is formatted as ext4"));
    }

    #[test]
    fn incomplete_manual_layout_blocks_top_level_storage_steps() {
        let mut config = InstallConfig::default();
        config.disk.target = Some(PathBuf::from("/dev/disk/by-id/example"));
        config.disk.partitioning = Partitioning::Manual;
        config.disk.root_partition = Some(PathBuf::from("/dev/sda2"));
        let output = format_plan(&config);
        assert!(output.contains("[BLOCKED] Partition and prepare storage"));
        assert!(output.contains("[BLOCKED] Set up encryption, filesystems and swap"));
        assert!(output.contains("manual UEFI partitioning requires disk.efi_partition"));
    }

    #[test]
    fn invalid_configuration_blocks_all_install_steps_in_plan() {
        let mut config = InstallConfig::default();
        config.disk.target = Some(PathBuf::from("/dev/disk/by-id/example"));
        config.system.username = "INVALID USER".into();
        let output = format_plan(&config);
        assert!(output.contains("[BLOCKED] Partition and prepare storage"));
        assert!(output.contains("[BLOCKED] Set up encryption, filesystems and swap"));
        assert!(output.contains("[BLOCKED] Install Artix base system"));
        assert!(output.contains("username must start with a lowercase letter"));
    }

    #[test]
    fn s6_plan_leaves_user_services_to_the_user() {
        let mut config = InstallConfig::default();
        config.system.init_system = crate::config::InitSystem::S6;
        config.system.user_services = crate::config::UserServices::Manual;
        let output = format_plan(&config);
        assert!(output.contains("manual user-managed setup"));
        assert!(output.contains("do not assume Turnstile"));
    }

    #[test]
    fn desktop_choices_appear_in_the_plan() {
        let mut config = InstallConfig::default();
        config.software.desktop_environments.push(Desktop::Hyprland);
        assert!(format_plan(&config).contains("hyprland"));
    }
}
