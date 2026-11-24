//! Deterministic package recipe previews ported from the existing Bash installer.
//! This module deliberately performs no repository queries or package operations.

use crate::config::{Desktop, InitSystem, InstallConfig, Kernel, NetworkStack, PrivilegeEscalation};

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
            Self::LiveRepositoryChoice => "live repository choice required (Arch [extra] or CachyOS)",
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
    pub init_packages: Vec<&'static str>,
    pub network_packages: Vec<&'static str>,
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
    let mut deferred = vec![
        "Package names and availability must be checked against the selected live Artix repositories before installation.".to_owned(),
        "Init-specific service packages and activation scripts require per-package repository checks; a base package name alone does not guarantee its service integration package exists.".to_owned(),
    ];
    let mut kernel_packages = Vec::new();
    let mut kernel_notes = Vec::new();
    for kernel in &config.software.kernels {
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
            Kernel::Linux => {},
            _ => kernel_packages.push(format!("{package}-headers")),
        }
    }
    if !config.software.kernels.contains(&Kernel::Linux) {
        deferred.push("The selected kernel list has no linux fallback; the current shell installer keeps linux as its guaranteed first-install fallback.".to_owned());
    }
    if config.software.kernels.contains(&Kernel::LinuxLqx) {
        kernel_notes.push("linux-lqx requires the Liquorix repository and signing-key setup.".to_owned());
    }
    let network_packages = match config.software.network {
        NetworkStack::Dhcpcd => vec!["dhcpcd"],
        NetworkStack::Iwd => vec!["iwd"],
        NetworkStack::NetworkManager => vec!["networkmanager"],
    };
    let privilege_packages = match config.system.privilege_escalation {
        PrivilegeEscalation::Doas => vec!["doas"],
        PrivilegeEscalation::Sudo => vec!["sudo"],
    };
    let desktop_recipes = config
        .software
        .desktop_environments
        .iter()
        .copied()
        .map(|desktop| recipe(desktop, config, &mut deferred))
        .collect();

    PackagePlan {
        base_packages: vec![
            "base", "base-devel", "linux-firmware", "ttf-dejavu",
            "ttf-liberation", "noto-fonts",
        ],
        kernel_packages,
        kernel_notes,
        init_packages,
        network_packages,
        privilege_packages,
        desktop_recipes,
        deferred,
    }
}

fn recipe(desktop: Desktop, config: &InstallConfig, deferred: &mut Vec<String>) -> DesktopRecipe {
    use Desktop::*;
    let (packages, source, notes): (Vec<&'static str>, PackageSource, Vec<&'static str>) =
        match desktop {
            Plasma => (vec![
                "plasma-desktop", "kwin", "plasma-pa", "plasma-nm", "powerdevil",
                "kscreen", "kde-gtk-config", "breeze", "breeze-gtk", "knotifications",
                "polkit-kde-agent", "xdg-desktop-portal-kde", "dolphin", "konsole",
                "spectacle", "ark", "gwenview", "plasma-systemmonitor",
                "ksystemstats", "bluedevil",
            ], PackageSource::Artix, vec![]),
            Xfce => (vec!["xfce4", "xfce4-goodies", "xdg-desktop-portal-gtk",
                "pavucontrol", "thunar-archive-plugin"], PackageSource::Artix, vec![]),
            Cinnamon => (vec!["cinnamon", "gnome-terminal", "xdg-desktop-portal-gtk"],
                PackageSource::Artix, vec![]),
            Mate => (vec!["mate", "xdg-desktop-portal-gtk"], PackageSource::Artix,
                vec!["Uses the core MATE group; does not install mate-extra."]),
            Lxqt => (vec!["lxqt"], PackageSource::Artix, vec![]),
            Moksha => (vec!["moksha-artix"], PackageSource::Artix, vec![]),
            I3 => (vec!["i3-wm", "dmenu", "xterm"], PackageSource::Artix, vec![]),
            Openbox => (vec!["openbox", "xterm", "dmenu"], PackageSource::Artix, vec![]),
            Fluxbox => (vec!["fluxbox", "xterm", "dmenu"], PackageSource::Artix, vec![]),
            Hyprland => (vec!["hyprland", "xdg-desktop-portal-hyprland"],
                PackageSource::Artix, vec!["The Bash installer also writes a user Hyprland autostart line and session entry."]),
            Niri => (vec!["niri", "xdg-desktop-portal-gnome"], PackageSource::Artix,
                vec!["The Bash installer copies the package default config when available and adds PipeWire autostart."]),
            Wayfire => (vec!["wayfire", "wf-shell", "xdg-desktop-portal-wlr"],
                PackageSource::Artix, vec!["wcm is optional/best-effort; the Bash installer seeds a Wayfire config and audio autostart."]),
            Sway => (vec!["sway", "foot", "xdg-desktop-portal-wlr"], PackageSource::Artix,
                vec!["wmenu is optional/best-effort and may require Arch [extra]."]),
            Labwc => (vec!["labwc", "foot", "xdg-desktop-portal-wlr"], PackageSource::Artix,
                vec!["The Bash installer copies the system autostart and adds PipeWire startup."]),
            Qtile => (vec!["qtile", "xterm"], PackageSource::ArchExtra,
                vec!["Qtile is sourced from Arch [extra], not the Artix repositories."]),
            SwayFx => (vec!["swayfx", "foot", "xdg-desktop-portal-wlr"],
                PackageSource::LiveRepositoryChoice, vec!["Replaces Sway; repository must be selected and verified at runtime.", "wmenu is optional/best-effort."]),
            Umbriel => (vec!["umbriel", "xdg-desktop-portal-umbriel"],
                PackageSource::LiveRepositoryChoice, vec!["Umbriel package name and repository are selected by live availability checks; portal is best-effort."]),
            MangoWm => (vec!["mangowm", "foot", "wmenu"],
                PackageSource::LiveRepositoryChoice, vec!["MangoWM package and repository are selected by live availability checks; terminal/launcher are best-effort."]),
            Cosmalia => (vec![
                "git", "base-devel", "cosmic-comp", "noctalia", "cosmic-settings",
                "cosmic-settings-daemon", "xdg-desktop-portal-cosmic", "cosmic-terminal",
                "polkit", "gnome-keyring", "wireplumber", "upower",
            ], PackageSource::ExternalBuild, vec![
                "Dinit-only session; the launcher starts cosmic-comp with --no-xwayland and Noctalia as the shell.",
                "cosmalia-session-dinit is built from its upstream PKGBUILD as the target user; this is not a repository package.",
                "The upstream manifest does not list a standalone cosmic-osd package.",
            ]),
            Cosmic => (vec![
                "cosmic-session", "cosmic-comp", "cosmic-greeter", "greetd",
                "xdg-desktop-portal-cosmic", "cosmic-terminal", "cosmic-files",
                "cosmic-text-editor", "cosmic-settings", "cosmic-screenshot",
                "cosmic-store", "upower", "pavucontrol",
            ], PackageSource::ArtixGalaxy, vec![
                "The Bash installer enables Artix [galaxy] and performs additional greeter-user, PAM and D-Bus setup.",
            ]),
        };

    if desktop == Cosmalia && config.system.init_system != InitSystem::Dinit {
        deferred.push("Cosmalia was selected with a non-Dinit init system; configuration validation should block this combination.".to_owned());
    }
    if desktop == SwayFx && config.software.desktop_environments.contains(&Sway) {
        deferred.push("SwayFX and Sway are both selected; they provide conflicting implementations of the same compositor.".to_owned());
    }
    DesktopRecipe { desktop, packages, source, notes }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Desktop, InstallConfig, InitSystem, NetworkStack, PrivilegeEscalation};

    #[test]
    fn init_specific_base_packages_are_selected() {
        let mut config = InstallConfig::default();
        config.system.init_system = InitSystem::S6;
        let plan = build_package_plan(&config);
        assert_eq!(plan.init_packages, ["s6-base", "elogind-s6", "dbus-s6"]);
        assert_eq!(plan.kernel_packages, ["linux".to_owned()]);
    }

    #[test]
    fn selected_kernel_packages_include_headers_and_repo_notes() {
        let mut config = InstallConfig::default();
        config.software.kernels = vec![Kernel::LinuxLts, Kernel::LinuxLqx, Kernel::LinuxCachyos];
        let plan = build_package_plan(&config);
        assert!(plan.kernel_packages.contains(&"linux-lts-headers".to_owned()));
        assert!(plan.kernel_packages.contains(&"linux-lqx-headers".to_owned()));
        assert!(plan.kernel_packages.contains(&"linux-cachyos".to_owned()));
        assert!(plan.kernel_notes.iter().any(|note| note.contains("Liquorix")));
        assert!(plan.kernel_notes.iter().any(|note| note.contains("CachyOS")));
    }

    #[test]
    fn network_and_privilege_packages_follow_configuration() {
        let mut config = InstallConfig::default();
        config.software.network = NetworkStack::Iwd;
        config.system.privilege_escalation = PrivilegeEscalation::Sudo;
        let plan = build_package_plan(&config);
        assert_eq!(plan.network_packages, ["iwd"]);
        assert_eq!(plan.privilege_packages, ["sudo"]);
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
        assert!(plan.desktop_recipes[0].notes.iter().any(|n| n.contains("Dinit-only")));
        config.system.init_system = InitSystem::OpenRc;
        let plan = build_package_plan(&config);
        assert!(plan.deferred.iter().any(|n| n.contains("non-Dinit")));
    }

    #[test]
    fn gated_desktops_are_not_misrepresented_as_artix_packages() {
        let mut config = InstallConfig::default();
        config.software.desktop_environments.extend([Desktop::Qtile, Desktop::SwayFx, Desktop::Umbriel, Desktop::MangoWm, Desktop::Cosmic]);
        let plan = build_package_plan(&config);
        assert_eq!(plan.desktop_recipes[0].source, PackageSource::ArchExtra);
        assert_eq!(plan.desktop_recipes[1].source, PackageSource::LiveRepositoryChoice);
        assert_eq!(plan.desktop_recipes[2].source, PackageSource::LiveRepositoryChoice);
        assert_eq!(plan.desktop_recipes[3].source, PackageSource::LiveRepositoryChoice);
        assert_eq!(plan.desktop_recipes[4].source, PackageSource::ArtixGalaxy);
    }
}
