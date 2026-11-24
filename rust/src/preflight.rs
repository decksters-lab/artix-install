//! Read-only checks for the live environment before an installation can begin.
//! This module deliberately does not invoke commands or modify the host.

use std::{
    ffi::OsStr,
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

use crate::{
    config::{Filesystem, InstallConfig, Partitioning},
    discovery::HardwareSnapshot,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckStatus {
    Pass,
    Warning,
    Fail,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreflightCheck {
    pub name: &'static str,
    pub status: CheckStatus,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreflightReport {
    pub checks: Vec<PreflightCheck>,
}

impl PreflightReport {
    pub fn is_ready(&self) -> bool {
        self.checks.iter().all(|check| check.status != CheckStatus::Fail)
    }

    pub fn render(&self) -> String {
        let mut output = String::from(
            "ARTIX SETUP — LIVE ENVIRONMENT PREFLIGHT\n=========================================\n\n",
        );
        for check in &self.checks {
            let label = match check.status {
                CheckStatus::Pass => "PASS",
                CheckStatus::Warning => "WARN",
                CheckStatus::Fail => "FAIL",
            };
            output.push_str(&format!("[{label}] {}: {}\n", check.name, check.detail));
        }
        output.push('\n');
        if self.is_ready() {
            output.push_str("No blocking preflight issue was detected. This is not authorization to write to a disk.\n");
        } else {
            output.push_str("Preflight found blocking issues. Resolve them before installation work can proceed.\n");
        }
        output.push_str("This report is read-only; it does not install packages or change disks.\n");
        output
    }
}

/// Evaluate the live environment using supplied snapshots. Inputs are injectable
/// so the safety rules can be tested without relying on the host running CI.
pub fn evaluate(
    config: &InstallConfig,
    hardware: &HardwareSnapshot,
    os_release: Option<&str>,
    effective_uid: Option<u32>,
    search_path: &OsStr,
) -> PreflightReport {
    let mut checks = Vec::new();

    let is_artix = os_release.is_some_and(is_artix_release);
    checks.push(PreflightCheck {
        name: "Artix Linux",
        status: if is_artix {
            CheckStatus::Pass
        } else {
            CheckStatus::Fail
        },
        detail: if is_artix {
            "The operating system identifies as Artix Linux; this check does not independently verify that it was booted from live media.".into()
        } else {
            "This does not identify as Artix Linux; boot the Artix live ISO before continuing.".into()
        },
    });

    let is_root = effective_uid == Some(0);
    checks.push(PreflightCheck {
        name: "Root privileges",
        status: if is_root {
            CheckStatus::Pass
        } else {
            CheckStatus::Fail
        },
        detail: if is_root {
            "The effective user ID is 0.".into()
        } else {
            "Run the installer as root (for example, use su or sudo).".into()
        },
    });

    let pacman = find_executable("pacman", search_path);
    checks.push(PreflightCheck {
        name: "Package manager",
        status: if pacman.is_some() {
            CheckStatus::Pass
        } else {
            CheckStatus::Fail
        },
        detail: pacman.map_or_else(
            || "pacman was not found in PATH.".into(),
            |path| format!("Found {}.", path.display()),
        ),
    });

    match config.validate() {
        Ok(()) => checks.push(PreflightCheck {
            name: "Configuration",
            status: CheckStatus::Pass,
            detail: "The selected configuration passes schema and compatibility validation.".into(),
        }),
        Err(issues) => checks.push(PreflightCheck {
            name: "Configuration",
            status: CheckStatus::Fail,
            detail: issues.join("; "),
        }),
    }

    let (firmware_status, firmware_detail) = match (
        config.system.firmware,
        hardware.firmware.as_str(),
    ) {
        (crate::config::FirmwareMode::Uefi, "UEFI") => (
            CheckStatus::Pass,
            "UEFI boot was detected and matches the selected firmware mode.".into(),
        ),
        (crate::config::FirmwareMode::Uefi, detected) => (
            CheckStatus::Fail,
            format!("UEFI was selected, but detected firmware is {detected}."),
        ),
        (crate::config::FirmwareMode::Bios, "UEFI") => (
            CheckStatus::Fail,
            "BIOS was selected, but the live system was booted in UEFI mode.".into(),
        ),
        (crate::config::FirmwareMode::Bios, detected) => (
            CheckStatus::Warning,
            format!(
                "UEFI was not detected ({detected}); sysfs cannot positively distinguish legacy BIOS from unavailable firmware information."
            ),
        ),
    };
    checks.push(PreflightCheck {
        name: "Firmware mode",
        status: firmware_status,
        detail: firmware_detail,
    });

    let active_network = hardware
        .network_interfaces
        .iter()
        .any(|interface| interface.state.as_deref() == Some("up"));
    checks.push(PreflightCheck {
        name: "Network interface",
        status: if active_network {
            CheckStatus::Pass
        } else {
            CheckStatus::Warning
        },
        detail: if active_network {
            "At least one network interface reports link state up; internet access is not verified.".into()
        } else {
            "No network interface reports link state up. Connect to the network before package installation; internet access is not verified by this check.".into()
        },
    });

    match &config.disk.target {
        None => checks.push(PreflightCheck {
            name: "Target disk",
            status: CheckStatus::Warning,
            detail: "No target disk has been selected yet.".into(),
        }),
        Some(target) => match hardware
            .disks
            .iter()
            .find(|disk| same_device_path(target, &disk.path))
        {
            None => checks.push(PreflightCheck {
                name: "Target disk",
                status: CheckStatus::Fail,
                detail: format!(
                    "{} is not present in the current hardware snapshot; refresh discovery and select a detected disk.",
                    target.display()
                ),
            }),
            Some(disk) if !disk.can_be_selected() => checks.push(PreflightCheck {
                name: "Target disk",
                status: CheckStatus::Fail,
                detail: format!(
                    "{} is blocked: {}.",
                    target.display(),
                    disk.selection_block_reason().unwrap_or("device safety checks failed")
                ),
            }),
            Some(_) => checks.push(PreflightCheck {
                name: "Target disk",
                status: CheckStatus::Pass,
                detail: format!(
                    "{} is present and passed current read-only selection checks.",
                    target.display()
                ),
            }),
        },
    }

    let mut required_tools = vec![
        "lsblk",
        "blkid",
        "wipefs",
        "mount",
        "umount",
        "basestrap",
        "fstabgen",
        "artix-chroot",
    ];
    if config.disk.partitioning == Partitioning::Automatic {
        required_tools.push(match config.system.firmware {
            crate::config::FirmwareMode::Uefi => "sgdisk",
            crate::config::FirmwareMode::Bios => "sfdisk",
        });
    }
    required_tools.push(match config.disk.root_filesystem {
        Filesystem::Btrfs => "mkfs.btrfs",
        Filesystem::Ext4 => "mkfs.ext4",
        Filesystem::Xfs => "mkfs.xfs",
        Filesystem::F2fs => "mkfs.f2fs",
        Filesystem::Jfs => "mkfs.jfs",
        Filesystem::Nilfs2 => "mkfs.nilfs2",
        Filesystem::Vfat | Filesystem::Exfat | Filesystem::Ntfs => "mkfs",
    });
    if matches!(
        config.disk.swap.mode,
        crate::config::SwapMode::Partition
            | crate::config::SwapMode::Swapfile
            | crate::config::SwapMode::Both
    ) {
        required_tools.push("mkswap");
    }
    if config.disk.encryption {
        required_tools.push("cryptsetup");
    }
    required_tools.sort_unstable();
    required_tools.dedup();
    let missing = required_tools
        .iter()
        .filter(|tool| find_executable(tool, search_path).is_none())
        .copied()
        .collect::<Vec<_>>();
    checks.push(PreflightCheck {
        name: "Storage utilities",
        status: if missing.is_empty() {
            CheckStatus::Pass
        } else {
            CheckStatus::Warning
        },
        detail: if missing.is_empty() {
            format!(
                "Found all currently planned utilities: {}.",
                required_tools.join(", ")
            )
        } else {
            format!(
                "Not currently found (may need to be installed in the live session before a future installer executor can use them): {}",
                missing.join(", ")
            )
        },
    });

    PreflightReport { checks }
}

fn is_artix_release(contents: &str) -> bool {
    let values = parse_os_release(contents);
    values.get("ID").is_some_and(|id| id == "artix")
        || values
            .get("ID_LIKE")
            .is_some_and(|ids| ids.split_whitespace().any(|id| id == "artix"))
}

fn parse_os_release(contents: &str) -> std::collections::HashMap<String, String> {
    contents
        .lines()
        .filter_map(|line| {
            let (key, value) = line.split_once('=')?;
            if key.is_empty()
                || !key.bytes().all(|byte| {
                    byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_'
                })
            {
                return None;
            }
            let value = value.trim();
            let value = if value.len() >= 2
                && ((value.starts_with('"') && value.ends_with('"'))
                    || (value.starts_with('\'') && value.ends_with('\'')))
            {
                &value[1..value.len() - 1]
            } else {
                value
            };
            Some((key.to_owned(), value.to_owned()))
        })
        .collect()
}


fn same_device_path(configured: &Path, discovered: &Path) -> bool {
    configured == discovered
        || matches!(
            (fs::canonicalize(configured), fs::canonicalize(discovered)),
            (Ok(configured), Ok(discovered)) if configured == discovered
        )
}

fn find_executable(command: &str, search_path: &OsStr) -> Option<PathBuf> {
    std::env::split_paths(search_path)
        .map(|directory| directory.join(command))
        .find(|candidate| {
            fs::metadata(candidate).is_ok_and(|metadata| {
                metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
            })
        })
}

pub fn run(config: &InstallConfig, hardware: &HardwareSnapshot) -> PreflightReport {
    let os_release = fs::read_to_string(Path::new("/etc/os-release")).ok();
    let effective_uid = fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|status| {
            status.lines().find_map(|line| {
                let values = line
                    .strip_prefix("Uid:")?
                    .split_whitespace()
                    .collect::<Vec<_>>();
                values.get(1)?.parse::<u32>().ok()
            })
        });
    let path = std::env::var_os("PATH").unwrap_or_default();
    evaluate(
        config,
        hardware,
        os_release.as_deref(),
        effective_uid,
        &path,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config::{FirmwareMode, Partitioning},
        discovery::HardwareSnapshot,
    };
    use std::fs;

    fn executable_dir(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "artix-setup-preflight-{}-{name}",
            std::process::id()
        ));
        fs::create_dir_all(&path).unwrap();
        let executable = path.join("pacman");
        fs::write(&executable, "#!/bin/sh\nexit 0\n").unwrap();
        let mut permissions = fs::metadata(&executable).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&executable, permissions).unwrap();
        path
    }

    #[test]
    fn stable_device_symlink_matches_discovered_kernel_path() {
        let directory = std::env::temp_dir().join(format!(
            "artix-setup-device-link-{}",
            std::process::id()
        ));
        fs::create_dir_all(&directory).unwrap();
        let device = directory.join("sda");
        let stable = directory.join("by-id-disk");
        fs::write(&device, "").unwrap();
        std::os::unix::fs::symlink(&device, &stable).unwrap();
        assert!(same_device_path(&stable, &device));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn recognizes_artix_os_release_and_id_like() {
        assert!(is_artix_release("NAME=Artix Linux\nID=artix\n"));
        assert!(is_artix_release("ID=arch\nID_LIKE=\"arch artix\"\n"));
        assert!(!is_artix_release("ID=arch\nID_LIKE=arch\n"));
    }

    #[test]
    fn missing_artix_root_and_pacman_are_blockers() {
        let config = InstallConfig::default();
        let report = evaluate(
            &config,
            &HardwareSnapshot::default(),
            None,
            Some(1000),
            OsStr::new(""),
        );
        assert!(!report.is_ready());
        assert!(report.checks.iter().any(|check| {
            check.name == "Root privileges" && check.status == CheckStatus::Fail
        }));
        assert!(report.checks.iter().any(|check| {
            check.name == "Package manager" && check.status == CheckStatus::Fail
        }));
    }

    #[test]
    fn reports_missing_target_as_warning_not_a_false_disk_selection() {
        let directory = executable_dir("missing-target");
        let config = InstallConfig::default();
        let report = evaluate(
            &config,
            &HardwareSnapshot {
                firmware: "UEFI".into(),
                ..HardwareSnapshot::default()
            },
            Some("ID=artix\n"),
            Some(0),
            directory.as_os_str(),
        );
        let target = report
            .checks
            .iter()
            .find(|check| check.name == "Target disk")
            .unwrap();
        assert_eq!(target.status, CheckStatus::Warning);
        assert!(report.is_ready());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn selected_target_must_exist_in_the_current_hardware_snapshot() {
        let directory = executable_dir("unknown-target");
        let mut config = InstallConfig::default();
        config.disk.target = Some("/dev/sda".into());
        let report = evaluate(
            &config,
            &HardwareSnapshot {
                firmware: "UEFI".into(),
                ..HardwareSnapshot::default()
            },
            Some("ID=artix\n"),
            Some(0),
            directory.as_os_str(),
        );
        assert!(report.checks.iter().any(|check| {
            check.name == "Target disk" && check.status == CheckStatus::Fail
        }));
        assert!(!report.is_ready());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn unavailable_firmware_metadata_is_not_a_bios_pass() {
        let directory = executable_dir("firmware-unknown");
        let mut config = InstallConfig::default();
        config.system.firmware = FirmwareMode::Bios;
        let report = evaluate(
            &config,
            &HardwareSnapshot {
                firmware: "BIOS / legacy or unavailable".into(),
                ..HardwareSnapshot::default()
            },
            Some("ID=artix\n"),
            Some(0),
            directory.as_os_str(),
        );
        assert!(report.checks.iter().any(|check| {
            check.name == "Firmware mode" && check.status == CheckStatus::Warning
        }));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn firmware_mismatch_blocks_preflight() {
        let directory = executable_dir("firmware-mismatch");
        let mut config = InstallConfig::default();
        config.system.firmware = FirmwareMode::Bios;
        config.disk.partitioning = Partitioning::Automatic;
        let report = evaluate(
            &config,
            &HardwareSnapshot {
                firmware: "UEFI".into(),
                ..HardwareSnapshot::default()
            },
            Some("ID=artix\n"),
            Some(0),
            directory.as_os_str(),
        );
        assert!(report.checks.iter().any(|check| {
            check.name == "Firmware mode" && check.status == CheckStatus::Fail
        }));
        fs::remove_dir_all(directory).unwrap();
    }
}
