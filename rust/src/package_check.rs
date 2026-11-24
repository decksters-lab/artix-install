//! Read-only checks against the currently enabled pacman sync databases.
//! This never refreshes databases or installs packages.

use std::{collections::BTreeMap, path::Path, process::Command};

use anyhow::{Context, Result};

use crate::{
    config::{InstallConfig, Kernel},
    packages::{self, PackageSource},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Availability {
    Available,
    Missing,
    Deferred,
    CouldNotCheck,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageCheck {
    pub name: String,
    pub source: PackageSource,
    pub availability: Availability,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageCheckReport {
    pub checks: Vec<PackageCheck>,
}

impl PackageCheckReport {
    pub fn has_problems(&self) -> bool {
        self.checks.iter().any(|check| {
            matches!(
                check.availability,
                Availability::Missing | Availability::CouldNotCheck
            )
        })
    }

    pub fn render(&self) -> String {
        let mut output = String::from(
            "ARTIX SETUP — READ-ONLY PACKAGE AVAILABILITY CHECK\n===================================================\n\n",
        );
        for check in &self.checks {
            let status = match check.availability {
                Availability::Available => "AVAILABLE",
                Availability::Missing => "NOT FOUND",
                Availability::Deferred => "DEFERRED",
                Availability::CouldNotCheck => "UNVERIFIED",
            };
            output.push_str(&format!(
                "[{status}] {} ({}) — {}\n",
                check.name, check.source, check.detail
            ));
        }
        let available = self
            .checks
            .iter()
            .filter(|check| check.availability == Availability::Available)
            .count();
        let missing = self
            .checks
            .iter()
            .filter(|check| check.availability == Availability::Missing)
            .count();
        let deferred = self
            .checks
            .iter()
            .filter(|check| check.availability == Availability::Deferred)
            .count();
        let unverified = self
            .checks
            .iter()
            .filter(|check| check.availability == Availability::CouldNotCheck)
            .count();
        output.push_str(&format!(
            "\nSummary: {available} available, {missing} not found, {deferred} deferred, {unverified} unverified.\n"
        ));
        output.push_str(
            "Only currently enabled sync databases were queried. No database refresh or package installation was performed.\n",
        );
        output
    }
}

fn repository_target(name: &str, source: PackageSource) -> String {
    match source {
        PackageSource::ArchExtra => format!("extra/{name}"),
        PackageSource::ArtixGalaxy => format!("galaxy/{name}"),
        _ => name.to_owned(),
    }
}

fn is_missing_error(stderr: &str, package: &str) -> bool {
    let message = stderr.to_ascii_lowercase();
    message.contains("target not found")
        || message.contains(&format!("package '{package}' was not found"))
        || message.contains(&format!("group '{package}' was not found"))
}

pub fn check(config: &InstallConfig, pacman: &Path) -> Result<PackageCheckReport> {
    let plan = packages::build_package_plan(config);
    let mut packages = BTreeMap::<String, PackageSource>::new();

    for name in plan
        .base_packages
        .iter()
        .copied()
        .chain(plan.kernel_packages.iter().map(String::as_str))
        .chain(plan.microcode_packages.iter().copied())
        .chain(plan.gpu_packages.iter().copied())
        .chain(plan.init_packages.iter().copied())
        .chain(plan.user_service_packages.iter().copied())
        .chain(plan.storage_packages.iter().copied())
        .chain(plan.bootloader_packages.iter().copied())
        .chain(plan.network_packages.iter().copied())
        .chain(plan.xorg_packages.iter().copied())
        .chain(plan.audio_packages.iter().copied())
        .chain(plan.privilege_packages.iter().copied())
    {
        let external_kernel = (config.software.kernels.contains(&Kernel::LinuxLqx)
            && name.starts_with("linux-lqx"))
            || (config.software.kernels.contains(&Kernel::LinuxCachyos) && name == "linux-cachyos");
        if !external_kernel {
            packages
                .entry(name.to_owned())
                .or_insert(PackageSource::Artix);
        }
    }

    for recipe in &plan.desktop_recipes {
        for name in &recipe.packages {
            packages.entry((*name).to_owned()).or_insert(recipe.source);
        }
    }

    let mut checks = Vec::with_capacity(packages.len());
    for (name, source) in packages {
        if matches!(
            source,
            PackageSource::ExternalBuild | PackageSource::LiveRepositoryChoice
        ) {
            checks.push(PackageCheck {
                name,
                source,
                availability: Availability::Deferred,
                detail: "This recipe requires an external build or an explicit repository choice."
                    .into(),
            });
            continue;
        }

        let is_group = matches!(name.as_str(), "base" | "base-devel");
        let query = if is_group { "-Sg" } else { "-Si" };
        let target = repository_target(&name, source);
        let result = Command::new(pacman)
            .arg(query)
            .arg(&target)
            .output()
            .with_context(|| format!("run {} {query} {target}", pacman.display()))?;
        let found_group = !is_group || !result.stdout.is_empty();
        if result.status.success() && found_group {
            checks.push(PackageCheck {
                name,
                source,
                availability: Availability::Available,
                detail: "Found in the currently enabled sync databases.".into(),
            });
        } else {
            let stderr = String::from_utf8_lossy(&result.stderr);
            let missing =
                (is_group && result.stdout.is_empty()) || is_missing_error(&stderr, &name);
            checks.push(PackageCheck {
                name,
                source,
                availability: if missing {
                    Availability::Missing
                } else {
                    Availability::CouldNotCheck
                },
                detail: if missing {
                    "Not found in enabled sync databases; the repository may need enabling or its sync database may need refreshing.".into()
                } else if stderr.trim().is_empty() {
                    format!("pacman exited with status {}.", result.status)
                } else {
                    stderr.trim().to_owned()
                },
            });
        }
    }

    for (name, source) in [
        ("linux-lqx", PackageSource::LiveRepositoryChoice),
        ("linux-lqx-headers", PackageSource::LiveRepositoryChoice),
        ("linux-cachyos", PackageSource::LiveRepositoryChoice),
    ] {
        let selected = match name {
            "linux-lqx" | "linux-lqx-headers" => {
                config.software.kernels.contains(&Kernel::LinuxLqx)
            }
            _ => config.software.kernels.contains(&Kernel::LinuxCachyos),
        };
        if selected {
            checks.push(PackageCheck {
                name: name.into(),
                source,
                availability: Availability::Deferred,
                detail: "Requires its external repository and signing-key setup before it can be checked.".into(),
            });
        }
    }

    checks.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(PackageCheckReport { checks })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Bootloader, Filesystem, UserServices};
    use std::{fs, os::unix::fs::PermissionsExt};

    fn fake_pacman(name: &str) -> std::path::PathBuf {
        let directory = std::env::temp_dir().join(format!(
            "artix-setup-pacman-check-{}-{name}",
            std::process::id()
        ));
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("pacman");
        fs::write(
            &path,
            "#!/bin/sh\nif [ \"$1\" = \"-Sg\" ] && [ \"$2\" = \"base\" ]; then echo \"base filesystem\"; exit 0; fi\necho \"error: package '$2' was not found\" >&2\nexit 1\n",
        )
        .unwrap();
        let mut permissions = fs::metadata(&path).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&path, permissions).unwrap();
        path
    }

    #[test]
    fn repository_specific_recipes_query_the_declared_repository() {
        assert_eq!(
            repository_target("qtile", PackageSource::ArchExtra),
            "extra/qtile"
        );
        assert_eq!(
            repository_target("cosmic-session", PackageSource::ArtixGalaxy),
            "galaxy/cosmic-session"
        );
        assert_eq!(repository_target("grub", PackageSource::Artix), "grub");
    }

    #[test]
    fn distinguishes_missing_packages_from_missing_sync_databases() {
        assert!(is_missing_error("error: target not found: foo", "foo"));
        assert!(is_missing_error(
            "error: package 'foo' was not found",
            "foo"
        ));
        assert!(!is_missing_error(
            "error: database 'extra' was not found",
            "foo"
        ));
    }

    #[test]
    fn checks_current_repository_packages_without_installing_them() {
        let mut config = InstallConfig::default();
        config.software.desktop_environments.clear();
        config.software.kernels = vec![Kernel::Linux];
        let pacman = fake_pacman("base-check");
        let report = check(&config, &pacman).unwrap();
        assert!(
            report.checks.iter().any(|item| {
                item.name == "base" && item.availability == Availability::Available
            })
        );
        assert!(report.checks.iter().any(|item| {
            item.name == "base-devel" && item.availability == Availability::Missing
        }));
        assert!(report.has_problems());
        fs::remove_dir_all(pacman.parent().unwrap()).unwrap();
    }

    #[test]
    fn selected_user_service_packages_are_included_in_availability_checks() {
        let mut config = InstallConfig::default();
        config.system.user_services = UserServices::Turnstile;
        let pacman = fake_pacman("user-services");
        let report = check(&config, &pacman).unwrap();
        assert!(report.checks.iter().any(|item| item.name == "turnstile"));
        assert!(
            report
                .checks
                .iter()
                .any(|item| item.name == "turnstile-dinit")
        );
        fs::remove_dir_all(pacman.parent().unwrap()).unwrap();
    }

    #[test]
    fn storage_and_bootloader_prerequisites_are_checked() {
        let mut config = InstallConfig::default();
        config.disk.root_filesystem = Filesystem::Btrfs;
        config.disk.encryption = true;
        config.software.bootloader = Bootloader::Limine;
        let pacman = fake_pacman("storage-bootloader");
        let report = check(&config, &pacman).unwrap();
        for expected in [
            "btrfs-progs",
            "cryptsetup",
            "dosfstools",
            "limine",
            "efibootmgr",
            "limine-mkinitcpio-hook",
        ] {
            assert!(
                report.checks.iter().any(|item| item.name == expected),
                "missing check for {expected}"
            );
        }
        fs::remove_dir_all(pacman.parent().unwrap()).unwrap();
    }

    #[test]
    fn external_kernel_repositories_are_deferred_not_misreported_missing() {
        let mut config = InstallConfig::default();
        config.software.kernels = vec![Kernel::LinuxLqx];
        let pacman = fake_pacman("external-kernel");
        let report = check(&config, &pacman).unwrap();
        assert!(report.checks.iter().any(|item| {
            item.name == "linux-lqx" && item.availability == Availability::Deferred
        }));
        assert!(report.checks.iter().any(|item| {
            item.name == "linux-lqx-headers" && item.availability == Availability::Deferred
        }));
        fs::remove_dir_all(pacman.parent().unwrap()).unwrap();
    }
}
