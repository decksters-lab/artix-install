//! Read-only checks against the currently enabled pacman sync databases.
//! This never refreshes databases or installs packages.

use std::{
    collections::BTreeMap,
    path::Path,
    process::Command,
};

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

pub fn check(config: &InstallConfig, pacman: &Path) -> Result<PackageCheckReport> {
    let plan = packages::build_package_plan(config);
    let mut packages = BTreeMap::<String, PackageSource>::new();

    for name in plan
        .base_packages
        .iter()
        .chain(plan.kernel_packages.iter().map(String::as_str))
        .chain(plan.microcode_packages.iter().copied())
        .chain(plan.gpu_packages.iter().copied())
        .chain(plan.init_packages.iter().copied())
        .chain(plan.network_packages.iter().copied())
        .chain(plan.xorg_packages.iter().copied())
        .chain(plan.audio_packages.iter().copied())
        .chain(plan.privilege_packages.iter().copied())
    {
        let external_kernel = (config.software.kernels.contains(&Kernel::LinuxLqx)
            && name.starts_with("linux-lqx"))
            || (config.software.kernels.contains(&Kernel::LinuxCachyos)
                && name == "linux-cachyos");
        if !external_kernel {
            packages
                .entry(name.to_owned())
                .or_insert(PackageSource::Artix);
        }
    }

    for recipe in &plan.desktop_recipes {
        for name in &recipe.packages {
            packages
                .entry((*name).to_owned())
                .or_insert(recipe.source);
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
                detail: "This recipe requires an external build or an explicit repository choice.".into(),
            });
            continue;
        }

        let result = Command::new(pacman)
            .arg("-Si")
            .arg(&name)
            .output()
            .with_context(|| format!("run {} -Si {}", pacman.display(), name))?;
        if result.status.success() {
            checks.push(PackageCheck {
                name,
                source,
                availability: Availability::Available,
                detail: "Found in the currently enabled sync databases.".into(),
            });
        } else {
            let stderr = String::from_utf8_lossy(&result.stderr);
            let missing = stderr.to_ascii_lowercase().contains("not found");
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
        ("linux-cachyos", PackageSource::LiveRepositoryChoice),
    ] {
        let selected = match name {
            "linux-lqx" => config.software.kernels.contains(&Kernel::LinuxLqx),
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
    use std::{
        fs,
        os::unix::fs::PermissionsExt,
    };

    fn fake_pacman() -> std::path::PathBuf {
        let directory = std::env::temp_dir().join(format!(
            "artix-setup-pacman-check-{}",
            std::process::id()
        ));
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("pacman");
        fs::write(
            &path,
            "#!/bin/sh\nif [ \"$2\" = \"base\" ]; then exit 0; fi\necho \"error: package '$2' was not found\" >&2\nexit 1\n",
        )
        .unwrap();
        let mut permissions = fs::metadata(&path).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&path, permissions).unwrap();
        path
    }

    #[test]
    fn checks_current_repository_packages_without_installing_them() {
        let mut config = InstallConfig::default();
        config.software.desktop_environments.clear();
        config.software.kernels = vec![Kernel::Linux];
        let pacman = fake_pacman();
        let report = check(&config, &pacman).unwrap();
        assert!(report.checks.iter().any(|item| {
            item.name == "base" && item.availability == Availability::Available
        }));
        assert!(report.checks.iter().any(|item| {
            item.name == "base-devel" && item.availability == Availability::Missing
        }));
        assert!(report.has_problems());
        fs::remove_dir_all(pacman.parent().unwrap()).unwrap();
    }

    #[test]
    fn external_kernel_repositories_are_deferred_not_misreported_missing() {
        let mut config = InstallConfig::default();
        config.software.kernels = vec![Kernel::LinuxLqx];
        let pacman = fake_pacman();
        let report = check(&config, &pacman).unwrap();
        assert!(report.checks.iter().any(|item| {
            item.name == "linux-lqx" && item.availability == Availability::Deferred
        }));
        fs::remove_dir_all(pacman.parent().unwrap()).unwrap();
    }
}
