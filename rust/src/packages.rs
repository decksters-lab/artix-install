//! Deterministic package recipe previews ported from the existing Bash installer.
//! This module deliberately performs no repository queries or package operations.

use crate::config::{
    Bootloader, CpuVendor, Desktop, Filesystem, FirmwareMode, GpuChoice, InitSystem, InstallConfig,
    Kernel, NetworkStack, Partitioning, PrivilegeEscalation, UserServices,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageSource {
    Artix,
    ArchExtra,
    ArtixGalaxy,
    LiveRepositoryChoice,
    ExternalBuild,
}

impl std::fmt::Display for PackageSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Artix => "Artix repositories",
            Self::ArchExtra => "Arch [extra] (must be enabled and checked)",
            Self::ArtixGalaxy => "Artix [galaxy] (must be enabled and checked)",
            Self::LiveRepositoryChoice => {
                "live repository choice required (Arch [extra] or CachyOS)"
            }
            Self::ExternalBuild => "external PKGBUILD build required",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesktopRecipe {
    pub desktop: Desktop,
    pub packages: Vec<&'static str>,
    pub source: PackageSource,
    pub notes: Vec<&'static str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackagePlan {
    pub base_packages: Vec<&'static str>,
    pub kernel_packages: Vec<String>,
    pub kernel_notes: Vec<String>,
    pub microcode_packages: Vec<&'static str>,
    pub gpu_packages: Vec<&'static str>,
    pub init_packages: Vec<&'static str>,
    pub user_service_packages: Vec<&'static str>,
    pub storage_packages: Vec<&'static str>,
    pub bootloader_packages: Vec<&'static str>,
    pub network_packages: Vec<&'static str>,
    pub xorg_packages: Vec<&'static str>,
    pub audio_packages: Vec<&'static str>,
    pub optional_service_packages: Vec<&'static str>,
    pub privilege_packages: Vec<&'static str>,
    pub desktop_recipes: Vec<DesktopRecipe>,
    pub deferred: Vec<String>,
}

pub fn build_package_plan(config: &InstallConfig) -> PackagePlan {
    let init_packages = match config.system.init_system {
        InitSystem::Dinit => vec!["dinit", "elogind-dinit", "dbus-dinit"],
        InitSystem::OpenRc => vec!["openrc", "elogind-openrc", "dbus-openrc"],
        InitSystem::Runit => vec!["runit", "elogind-runit", "dbus-runit"],
        InitSystem::S6 => vec!["s6-base", "elogind-s6", "dbus-s6"],
    };
    let user_service_packages = match (config.system.init_system, config.system.user_services) {
        (InitSystem::Dinit, UserServices::Userspawn) => vec!["userspawn", "userspawn-dinit"],
        (InitSystem::Dinit, UserServices::Turnstile) => vec!["turnstile", "turnstile-dinit"],
        (InitSystem::OpenRc, UserServices::Turnstile) => vec!["turnstile", "turnstile-openrc"],
        (InitSystem::Runit, UserServices::Userspawn) => vec!["userspawn", "userspawn-runit"],
        _ => Vec::new(),
    };
    let mut deferred = vec![
        "Package names and availability must be checked against the selected live Artix repositories before installation.".to_owned(),
        "Init-specific service packages and activation scripts require per-package repository checks; a base package name alone does not guarantee its service integration package exists.".to_owned(),
    ];
    let mut kernel_packages = Vec::new();
    let mut kernel_notes = Vec::new();
    for (index, kernel) in config.software.kernels.iter().enumerate() {
        let package = match kernel {
            Kernel::Linux => "linux",
            Kernel::LinuxLts => "linux-lts",
            Kernel::LinuxZen => "linux-zen",
            Kernel::LinuxLqx => "linux-lqx",
            Kernel::LinuxCachyos => "linux-cachyos",
        };
        kernel_packages.push(package.to_owned());
        match kernel {
            Kernel::LinuxLqx => kernel_packages.push("linux-lqx-headers".to_owned()),
            Kernel::LinuxCachyos => kernel_notes.push("linux-cachyos requires CachyOS keyring/repository setup; its package bundles headers.".to_owned()),
            _ if index > 0 => kernel_packages.push(format!("{package}-headers")),
            _ => {}
        }
    }
    if !config.software.kernels.contains(&Kernel::Linux) {
        deferred.push("The selected kernel list has no linux fallback; the current shell installer keeps linux as its guaranteed first-install fallback.".to_owned());
    }
    if config.software.kernels.contains(&Kernel::LinuxLqx) {
        kernel_notes
            .push("linux-lqx requires the Liquorix repository and signing-key setup.".to_owned());
    }
    let microcode_packages = match config.software.cpu_vendor {
        CpuVendor::Intel => vec!["intel-ucode"],
        CpuVendor::Amd => vec!["amd-ucode"],
        CpuVendor::Other => Vec::new(),
    };
    let gpu_packages = match config.software.gpu {
        GpuChoice::Intel => vec!["mesa", "vulkan-intel"],
        GpuChoice::Amd => vec!["mesa", "vulkan-radeon"],
        GpuChoice::Nvidia => vec!["mesa", "nvidia-dkms", "nvidia-utils"],
        GpuChoice::Hybrid => vec!["mesa", "vulkan-intel", "nvidia-dkms", "nvidia-utils"],
        GpuChoice::Vm => vec!["mesa"],
    };
    if matches!(config.software.gpu, GpuChoice::Nvidia | GpuChoice::Hybrid) {
        deferred.push("NVIDIA DKMS needs matching headers for every kernel that will use the module; verify kernel/header availability and supported hardware before installation.".to_owned());
    }
    let mut storage_packages = Vec::new();
    let mut filesystems = vec![config.disk.root_filesystem];
    filesystems.extend(
        config
            .disk
            .additional_mounts
            .iter()
            .map(|mount| mount.filesystem),
    );
    if config.system.firmware == FirmwareMode::Uefi {
        filesystems.push(Filesystem::Vfat);
    }
    if config.disk.encryption {
        storage_packages.push("cryptsetup");
    }
    for filesystem in filesystems {
        let package = match filesystem {
            Filesystem::Btrfs => Some("btrfs-progs"),
            Filesystem::Ext4 => None,
            Filesystem::Xfs => Some("xfsprogs"),
            Filesystem::F2fs => Some("f2fs-tools"),
            Filesystem::Jfs => Some("jfsutils"),
            Filesystem::Nilfs2 => Some("nilfs-utils"),
            Filesystem::Vfat => Some("dosfstools"),
            Filesystem::Exfat => Some("exfatprogs"),
            Filesystem::Ntfs => Some("ntfs-3g"),
        };
        if let Some(package) = package
            && !storage_packages.contains(&package)
        {
            storage_packages.push(package);
        }
    }
    let bootloader_packages = match config.software.bootloader {
        Bootloader::Grub => {
            let mut packages = vec!["grub"];
            if config.system.firmware == FirmwareMode::Uefi {
                packages.push("efibootmgr");
            }
            if config.disk.partitioning == Partitioning::DualBoot {
                packages.push("os-prober");
            }
            packages
        }
        Bootloader::Limine => vec!["limine", "efibootmgr", "limine-mkinitcpio-hook"],
        Bootloader::Refind => vec!["refind", "efibootmgr"],
    };
    let network_packages = match config.software.network {
        NetworkStack::Dhcpcd => vec!["dhcpcd"],
        NetworkStack::Iwd => vec!["iwd"],
        NetworkStack::NetworkManager => vec![
            "networkmanager",
            match config.system.init_system {
                InitSystem::Dinit => "networkmanager-dinit",
                InitSystem::OpenRc => "networkmanager-openrc",
                InitSystem::Runit => "networkmanager-runit",
                InitSystem::S6 => "networkmanager-s6",
            },
        ],
    };
    let needs_xorg = config.software.desktop_environments.iter().any(|desktop| {
        !matches!(
            desktop,
            Desktop::Cosmic
                | Desktop::Cosmalia
                | Desktop::Hyprland
                | Desktop::MangoWm
                | Desktop::Umbriel
                | Desktop::Niri
                | Desktop::Wayfire
                | Desktop::Labwc
                | Desktop::Sway
                | Desktop::SwayFx
        )
    });
    let xorg_packages = if needs_xorg {
        vec![
            "xorg-server",
            "xorg-server-common",
            "xf86-input-libinput",
            "xorg-xinit",
        ]
    } else {
        Vec::new()
    };
    let needs_audio = config.software.desktop_environments.iter().any(|desktop| {
        matches!(
            desktop,
            Desktop::Plasma
                | Desktop::Xfce
                | Desktop::Cinnamon
                | Desktop::Mate
                | Desktop::Lxqt
                | Desktop::Moksha
                | Desktop::Cosmic
                | Desktop::Cosmalia
                | Desktop::Hyprland
                | Desktop::MangoWm
                | Desktop::Umbriel
                | Desktop::Niri
                | Desktop::Wayfire
                | Desktop::Labwc
                | Desktop::Sway
                | Desktop::SwayFx
        )
    });
    let audio_packages = if needs_audio {
        vec![
            "pipewire",
            "pipewire-pulse",
            "pipewire-alsa",
            "wireplumber",
            "rtkit",
        ]
    } else {
        Vec::new()
    };
    let optional_service_packages = if needs_audio {
        match config.system.init_system {
            InitSystem::Dinit => {
                vec![
                    "pipewire-dinit",
                    "pipewire-pulse-dinit",
                    "wireplumber-dinit",
                ]
            }
            InitSystem::OpenRc => {
                vec![
                    "pipewire-openrc",
                    "pipewire-pulse-openrc",
                    "wireplumber-openrc",
                ]
            }
            InitSystem::Runit => {
                vec![
                    "pipewire-runit",
                    "pipewire-pulse-runit",
                    "wireplumber-runit",
                ]
            }
            InitSystem::S6 => {
                vec!["pipewire-s6", "pipewire-pulse-s6", "wireplumber-s6"]
            }
        }
    } else {
        Vec::new()
    };
    let privilege_packages = match config.system.privilege_escalation {
        PrivilegeEscalation::Doas => vec!["opendoas"],
        PrivilegeEscalation::Sudo => vec!["sudo"],
        PrivilegeEscalation::Both => vec!["opendoas", "sudo"],
    };
    let desktop_recipes = config
        .software
        .desktop_environments
        .iter()
        .copied()
        .map(|desktop| recipe(desktop, config, &mut deferred))
        .collect();

    PackagePlan {
        // Install the Artix base-devel toolchain explicitly instead of the
        // base-devel group: that group depends on sudo, which would silently
        // defeat a user's doas-only selection.
        base_packages: vec![
            "base",
            "autoconf",
            "automake",
            "binutils",
            "bison",
            "esysusers",
            "etmpfiles",
            "fakeroot",
            "file",
            "findutils",
            "flex",
            "gawk",
            "gcc",
            "gettext",
            "grep",
            "groff",
            "gzip",
            "libtool",
            "m4",
            "make",
            "pacman",
            "patch",
            "pkgconf",
            "sed",
            "texinfo",
            "which",
            "linux-firmware",
            "ttf-dejavu",
            "ttf-liberation",
            "noto-fonts",
        ],
        kernel_packages,
        kernel_notes,
        microcode_packages,
        gpu_packages,
        init_packages,
        user_service_packages,
        storage_packages,
        bootloader_packages,
        network_packages,
        xorg_packages,
        audio_packages,
        optional_service_packages,
        privilege_packages,
        desktop_recipes,
        deferred,
    }
}

fn recipe(desktop: Desktop, config: &InstallConfig, deferred: &mut Vec<String>) -> DesktopRecipe {
    use Desktop::*;
    let (packages, source, notes): (Vec<&'static str>, PackageSource, Vec<&'static str>) =
        match desktop {
            Plasma => (
                vec![
                    "plasma-desktop",
                    "kwin",
                    "plasma-pa",
                    "plasma-nm",
                    "powerdevil",
                    "kscreen",
                    "kde-gtk-config",
                    "breeze",
                    "breeze-gtk",
                    "knotifications",
                    "polkit-kde-agent",
                    "xdg-desktop-portal-kde",
                    "dolphin",
                    "konsole",
                    "spectacle",
                    "ark",
                    "gwenview",
                    "plasma-systemmonitor",
                    "ksystemstats",
                    "bluedevil",
                ],
                PackageSource::Artix,
                vec![],
            ),
            Xfce => (
                vec![
                    "xfce4",
                    "xfce4-goodies",
                    "xdg-desktop-portal-gtk",
                    "pavucontrol",
                    "thunar-archive-plugin",
                ],
                PackageSource::Artix,
                vec![],
            ),
            Cinnamon => (
                vec!["cinnamon", "gnome-terminal", "xdg-desktop-portal-gtk"],
                PackageSource::Artix,
                vec![],
            ),
            Mate => (
                vec!["mate", "xdg-desktop-portal-gtk"],
                PackageSource::Artix,
                vec!["Uses the core MATE group; does not install mate-extra."],
            ),
            Lxqt => (vec!["lxqt"], PackageSource::Artix, vec![]),
            Moksha => (vec!["moksha-artix"], PackageSource::Artix, vec![]),
            I3 => (
                vec!["i3-wm", "dmenu", "xterm"],
                PackageSource::Artix,
                vec![],
            ),
            Openbox => (
                vec!["openbox", "xterm", "dmenu"],
                PackageSource::Artix,
                vec![],
            ),
            Fluxbox => (
                vec!["fluxbox", "xterm", "dmenu"],
                PackageSource::Artix,
                vec![],
            ),
            Hyprland => (
                vec!["hyprland", "xdg-desktop-portal-hyprland"],
                PackageSource::Artix,
                vec![
                    "The Bash installer also writes a user Hyprland autostart line and session entry.",
                ],
            ),
            Niri => (
                vec!["niri", "xdg-desktop-portal-gnome"],
                PackageSource::Artix,
                vec![
                    "The Bash installer copies the package default config when available and adds PipeWire autostart.",
                ],
            ),
            Wayfire => (
                vec!["wayfire", "wf-shell", "xdg-desktop-portal-wlr"],
                PackageSource::Artix,
                vec![
                    "wcm is optional/best-effort; the Bash installer seeds a Wayfire config and audio autostart.",
                ],
            ),
            Sway => (
                vec!["sway", "foot", "xdg-desktop-portal-wlr"],
                PackageSource::Artix,
                vec!["wmenu is optional/best-effort and may require Arch [extra]."],
            ),
            Labwc => (
                vec!["labwc", "foot", "xdg-desktop-portal-wlr"],
                PackageSource::Artix,
                vec!["The Bash installer copies the system autostart and adds PipeWire startup."],
            ),
            Qtile => (
                vec!["qtile", "xterm"],
                PackageSource::ArchExtra,
                vec!["Qtile is sourced from Arch [extra], not the Artix repositories."],
            ),
            SwayFx => (
                vec!["swayfx", "foot", "xdg-desktop-portal-wlr"],
                PackageSource::LiveRepositoryChoice,
                vec![
                    "Replaces Sway; repository must be selected and verified at runtime.",
                    "wmenu is optional/best-effort.",
                ],
            ),
            Umbriel => (
                vec!["umbriel", "xdg-desktop-portal-umbriel"],
                PackageSource::LiveRepositoryChoice,
                vec![
                    "Umbriel package name and repository are selected by live availability checks; portal is best-effort.",
                ],
            ),
            MangoWm => (
                vec!["mangowm", "foot", "wmenu"],
                PackageSource::LiveRepositoryChoice,
                vec![
                    "MangoWM package and repository are selected by live availability checks; terminal/launcher are best-effort.",
                ],
            ),
            Cosmalia => (
                vec![
                    "git",
                    "base-devel",
                    "cosmic-comp",
                    "noctalia",
                    "cosmic-settings",
                    "cosmic-settings-daemon",
                    "xdg-desktop-portal-cosmic",
                    "cosmic-terminal",
                    "polkit",
                    "gnome-keyring",
                    "wireplumber",
                    "upower",
                ],
                PackageSource::ExternalBuild,
                vec![
                    "Dinit-only session; the launcher starts cosmic-comp with --no-xwayland and Noctalia as the shell.",
                    "cosmalia-session-dinit is built from its upstream PKGBUILD as the target user; this is not a repository package.",
                    "The upstream manifest does not list a standalone cosmic-osd package.",
                ],
            ),
            Cosmic => (
                vec![
                    "cosmic-session",
                    "cosmic-comp",
                    "cosmic-greeter",
                    "greetd",
                    "xdg-desktop-portal-cosmic",
                    "cosmic-terminal",
                    "cosmic-files",
                    "cosmic-text-editor",
                    "cosmic-settings",
                    "cosmic-screenshot",
                    "cosmic-store",
                    "upower",
                    "pavucontrol",
                ],
                PackageSource::ArtixGalaxy,
                vec![
                    "The Bash installer enables Artix [galaxy] and performs additional greeter-user, PAM and D-Bus setup.",
                ],
            ),
        };

    if desktop == Cosmalia && config.system.init_system != InitSystem::Dinit {
        deferred.push("Cosmalia was selected with a non-Dinit init system; configuration validation should block this combination.".to_owned());
    }
    if desktop == SwayFx && config.software.desktop_environments.contains(&Sway) {
        deferred.push("SwayFX and Sway are both selected; they provide conflicting implementations of the same compositor.".to_owned());
    }
    DesktopRecipe {
        desktop,
        packages,
        source,
        notes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Desktop, InitSystem, InstallConfig, NetworkStack, PrivilegeEscalation};

    #[test]
    fn init_specific_base_packages_are_selected() {
        let mut config = InstallConfig::default();
        config.system.init_system = InitSystem::S6;
        let plan = build_package_plan(&config);
        assert_eq!(plan.init_packages, ["s6-base", "elogind-s6", "dbus-s6"]);
        assert_eq!(plan.kernel_packages, ["linux".to_owned()]);
    }

    #[test]
    fn user_service_packages_follow_each_supported_init_system() {
        use crate::config::UserServices;

        let mut config = InstallConfig::default();
        config.system.user_services = UserServices::Userspawn;
        assert_eq!(
            build_package_plan(&config).user_service_packages,
            ["userspawn", "userspawn-dinit"]
        );

        config.system.user_services = UserServices::Turnstile;
        assert_eq!(
            build_package_plan(&config).user_service_packages,
            ["turnstile", "turnstile-dinit"]
        );

        config.system.init_system = InitSystem::OpenRc;
        assert_eq!(
            build_package_plan(&config).user_service_packages,
            ["turnstile", "turnstile-openrc"]
        );
        config.system.user_services = UserServices::Native;
        assert!(build_package_plan(&config).user_service_packages.is_empty());

        config.system.init_system = InitSystem::Runit;
        config.system.user_services = UserServices::Userspawn;
        assert_eq!(
            build_package_plan(&config).user_service_packages,
            ["userspawn", "userspawn-runit"]
        );

        config.system.init_system = InitSystem::S6;
        config.system.user_services = UserServices::Manual;
        assert!(build_package_plan(&config).user_service_packages.is_empty());
    }

    #[test]
    fn storage_and_bootloader_packages_follow_the_install_profile() {
        use crate::config::{EfiPolicy, Filesystem, FirmwareMode, MountConfig, Partitioning};

        let mut config = InstallConfig::default();
        let plan = build_package_plan(&config);
        assert_eq!(plan.storage_packages, ["dosfstools"]);
        assert_eq!(plan.bootloader_packages, ["grub", "efibootmgr"]);

        config.disk.root_filesystem = Filesystem::Btrfs;
        config.disk.encryption = true;
        config.disk.additional_mounts.push(MountConfig {
            device: "/dev/sda3".into(),
            mountpoint: "/home".into(),
            filesystem: Filesystem::Xfs,
            format: true,
        });
        config.disk.partitioning = Partitioning::Manual;
        config.disk.efi_policy = EfiPolicy::Preserve;
        let plan = build_package_plan(&config);
        assert!(plan.storage_packages.contains(&"btrfs-progs"));
        assert!(plan.storage_packages.contains(&"xfsprogs"));
        assert!(plan.storage_packages.contains(&"cryptsetup"));
        assert!(plan.storage_packages.contains(&"dosfstools"));

        config.disk.partitioning = Partitioning::DualBoot;
        assert!(
            build_package_plan(&config)
                .bootloader_packages
                .contains(&"os-prober")
        );

        config.software.bootloader = Bootloader::Limine;
        assert_eq!(
            build_package_plan(&config).bootloader_packages,
            ["limine", "efibootmgr", "limine-mkinitcpio-hook"]
        );
        config.software.bootloader = Bootloader::Refind;
        assert_eq!(
            build_package_plan(&config).bootloader_packages,
            ["refind", "efibootmgr"]
        );

        config.system.firmware = FirmwareMode::Bios;
        config.disk.partitioning = Partitioning::Automatic;
        config.disk.additional_mounts.clear();
        config.disk.root_filesystem = Filesystem::Ext4;
        config.disk.encryption = false;
        config.software.bootloader = Bootloader::Grub;
        let plan = build_package_plan(&config);
        assert!(plan.storage_packages.is_empty());
        assert_eq!(plan.bootloader_packages, ["grub"]);
    }

    #[test]
    fn selected_kernel_packages_include_headers_and_repo_notes() {
        let mut config = InstallConfig::default();
        config.software.kernels = vec![Kernel::LinuxLts, Kernel::LinuxLqx, Kernel::LinuxCachyos];
        let plan = build_package_plan(&config);
        assert!(
            !plan
                .kernel_packages
                .contains(&"linux-lts-headers".to_owned())
        );
        assert!(
            plan.kernel_packages
                .contains(&"linux-lqx-headers".to_owned())
        );
        assert!(plan.kernel_packages.contains(&"linux-cachyos".to_owned()));
        assert!(
            plan.kernel_notes
                .iter()
                .any(|note| note.contains("Liquorix"))
        );
        assert!(
            plan.kernel_notes
                .iter()
                .any(|note| note.contains("CachyOS"))
        );
    }

    #[test]
    fn network_and_privilege_packages_follow_configuration() {
        let mut config = InstallConfig::default();
        config.software.network = NetworkStack::Iwd;
        config.system.privilege_escalation = PrivilegeEscalation::Sudo;
        let plan = build_package_plan(&config);
        assert_eq!(plan.network_packages, ["iwd"]);
        assert_eq!(plan.privilege_packages, ["sudo"]);

        config.system.privilege_escalation = PrivilegeEscalation::Both;
        let plan = build_package_plan(&config);
        assert_eq!(plan.privilege_packages, ["opendoas", "sudo"]);
    }

    #[test]
    fn base_development_tools_do_not_override_privilege_choice() {
        let mut config = InstallConfig::default();
        config.system.privilege_escalation = PrivilegeEscalation::Doas;
        let plan = build_package_plan(&config);
        assert!(!plan.base_packages.contains(&"base-devel"));
        assert!(!plan.base_packages.contains(&"sudo"));
        assert_eq!(plan.privilege_packages, ["opendoas"]);

        config.system.privilege_escalation = PrivilegeEscalation::Sudo;
        assert_eq!(build_package_plan(&config).privilege_packages, ["sudo"]);
    }

    #[test]
    fn network_manager_uses_the_selected_init_service_package() {
        let mut config = InstallConfig::default();
        assert_eq!(
            build_package_plan(&config).network_packages,
            ["networkmanager", "networkmanager-dinit"]
        );
        config.system.init_system = InitSystem::OpenRc;
        assert_eq!(
            build_package_plan(&config).network_packages,
            ["networkmanager", "networkmanager-openrc"]
        );
        config.system.init_system = InitSystem::Runit;
        assert_eq!(
            build_package_plan(&config).network_packages,
            ["networkmanager", "networkmanager-runit"]
        );
        config.system.init_system = InitSystem::S6;
        assert_eq!(
            build_package_plan(&config).network_packages,
            ["networkmanager", "networkmanager-s6"]
        );
    }

    #[test]
    fn cpu_microcode_and_gpu_packages_follow_hardware_choices() {
        let mut config = InstallConfig::default();
        config.software.cpu_vendor = CpuVendor::Intel;
        config.software.gpu = GpuChoice::Hybrid;
        let plan = build_package_plan(&config);
        assert_eq!(plan.microcode_packages, ["intel-ucode"]);
        assert!(plan.gpu_packages.contains(&"vulkan-intel"));
        assert!(plan.gpu_packages.contains(&"nvidia-dkms"));
        assert!(
            plan.deferred
                .iter()
                .any(|note| note.contains("matching headers"))
        );
    }

    #[test]
    fn wayland_only_cosmalia_skips_xorg_but_gets_audio_stack() {
        let mut config = InstallConfig::default();
        config.software.desktop_environments.push(Desktop::Cosmalia);
        let plan = build_package_plan(&config);
        assert!(plan.xorg_packages.is_empty());
        assert!(plan.audio_packages.contains(&"pipewire"));
        assert!(plan.audio_packages.contains(&"rtkit"));
        assert!(plan.optional_service_packages.contains(&"pipewire-dinit"));
        assert!(
            plan.optional_service_packages
                .contains(&"wireplumber-dinit")
        );
    }

    #[test]
    fn cli_only_plan_does_not_add_desktop_audio_or_xorg() {
        let plan = build_package_plan(&InstallConfig::default());
        assert!(plan.xorg_packages.is_empty());
        assert!(plan.audio_packages.is_empty());
    }

    #[test]
    fn plasma_recipe_matches_shell_installer_package_set() {
        let mut config = InstallConfig::default();
        config.software.desktop_environments.push(Desktop::Plasma);
        let plan = build_package_plan(&config);
        assert_eq!(plan.desktop_recipes[0].packages.len(), 20);
        assert!(plan.desktop_recipes[0].packages.contains(&"dolphin"));
        assert_eq!(plan.desktop_recipes[0].source, PackageSource::Artix);
    }

    #[test]
    fn cosmalia_recipe_is_explicitly_external_and_dinit_only() {
        let mut config = InstallConfig::default();
        config.software.desktop_environments.push(Desktop::Cosmalia);
        let plan = build_package_plan(&config);
        assert_eq!(plan.desktop_recipes[0].source, PackageSource::ExternalBuild);
        assert!(
            plan.desktop_recipes[0]
                .notes
                .iter()
                .any(|n| n.contains("Dinit-only"))
        );
        config.system.init_system = InitSystem::OpenRc;
        let plan = build_package_plan(&config);
        assert!(plan.deferred.iter().any(|n| n.contains("non-Dinit")));
    }

    #[test]
    fn gated_desktops_are_not_misrepresented_as_artix_packages() {
        let mut config = InstallConfig::default();
        config.software.desktop_environments.extend([
            Desktop::Qtile,
            Desktop::SwayFx,
            Desktop::Umbriel,
            Desktop::MangoWm,
            Desktop::Cosmic,
        ]);
        let plan = build_package_plan(&config);
        assert_eq!(plan.desktop_recipes[0].source, PackageSource::ArchExtra);
        assert_eq!(
            plan.desktop_recipes[1].source,
            PackageSource::LiveRepositoryChoice
        );
        assert_eq!(
            plan.desktop_recipes[2].source,
            PackageSource::LiveRepositoryChoice
        );
        assert_eq!(
            plan.desktop_recipes[3].source,
            PackageSource::LiveRepositoryChoice
        );
        assert_eq!(plan.desktop_recipes[4].source, PackageSource::ArtixGalaxy);
    }
}
