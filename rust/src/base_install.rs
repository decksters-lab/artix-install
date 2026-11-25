//! Guarded Artix base-system installation through basestrap.
//! This stage is deliberately limited to the tested automatic UEFI/ext4 layout.
use std::{
    env, fs,
    os::unix::fs::FileTypeExt,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result, bail};

use crate::{
    config::{Filesystem, FirmwareMode, InstallConfig, Partitioning, SwapMode},
    discovery::{self, BlockDevice},
    package_check::{self, Availability},
    packages::{self, PackageSource},
};

const TARGET_ROOT: &str = "/mnt";
const TARGET_BOOT: &str = "/mnt/boot";

/// Install the selected Artix base system after revalidating the exact mounted target.
/// The caller must supply the exact configured whole-disk path as a second confirmation.
pub fn install_base(config: &InstallConfig, typed_confirmation: &str) -> Result<String> {
    config.validate().map_err(|issues| {
        anyhow::anyhow!(
            "configuration validation failed:\n- {}",
            issues.join("\n- ")
        )
    })?;
    if libc_geteuid() != 0 {
        bail!("basestrap requires effective UID 0");
    }
    verify_artix_live_environment()?;

    let selected = config
        .disk
        .target
        .as_deref()
        .context("select a target disk before installing the base system")?;
    if typed_confirmation != selected.to_string_lossy() {
        bail!(
            "confirmation mismatch: pass --confirm-install '{}' exactly",
            selected.display()
        );
    }
    if config.disk.partitioning != Partitioning::Automatic
        || config.system.firmware != FirmwareMode::Uefi
        || config.disk.root_filesystem != Filesystem::Ext4
        || config.disk.encryption
        || config.disk.swap.mode == SwapMode::Partition
    {
        bail!(
            "the first basestrap stage supports automatic, unencrypted UEFI/ext4 installs without a swap partition"
        );
    }

    let disk = fs::canonicalize(selected)
        .with_context(|| format!("resolve configured target {}", selected.display()))?;
    let metadata = fs::metadata(&disk)
        .with_context(|| format!("inspect configured target {}", disk.display()))?;
    if !metadata.file_type().is_block_device() {
        bail!("configured target {} is not a block device", disk.display());
    }

    let table = Command::new("sfdisk")
        .arg("--dump")
        .arg(&disk)
        .output()
        .context("sfdisk is required to re-read the target partition table")?;
    if !table.status.success() {
        bail!("could not read the target partition table; basestrap was not started");
    }
    let table_text = String::from_utf8(table.stdout)
        .context("sfdisk returned non-UTF-8 partition table data")?;
    let label = table_text
        .lines()
        .find_map(|line| line.trim().strip_prefix("label:"))
        .map(str::trim);
    if label != Some("gpt") {
        bail!("expected a GPT target partition table; basestrap was not started");
    }
    let partitions: Vec<PathBuf> = table_text
        .lines()
        .filter(|line| line.trim_start().starts_with("/dev/"))
        .filter_map(|line| {
            line.split_once(':')
                .map(|(path, _)| PathBuf::from(path.trim()))
        })
        .collect();
    let expected_count = if config.disk.swap.mode == SwapMode::Partition {
        3
    } else {
        2
    };
    if partitions.len() != expected_count || partitions.is_empty() {
        bail!(
            "target partition table has {} partitions; expected {}; basestrap was not started",
            partitions.len(),
            expected_count
        );
    }
    let efi =
        fs::canonicalize(&partitions[0]).context("resolve EFI partition from target table")?;
    let root = fs::canonicalize(
        partitions
            .last()
            .context("target table has no root partition")?,
    )
    .context("resolve root partition from target table")?;
    for (device, description) in [(&efi, "EFI"), (&root, "root")] {
        let metadata = fs::metadata(device)
            .with_context(|| format!("inspect {description} partition {}", device.display()))?;
        if !metadata.file_type().is_block_device() {
            bail!(
                "{description} partition {} is not a block device",
                device.display()
            );
        }
    }
    verify_blkid_type(&efi, "vfat")?;
    verify_blkid_type(&root, "ext4")?;
    let hardware = discovery::discover();
    let target_info = hardware
        .disks
        .iter()
        .find(|candidate| same_device_path(&candidate.path, &disk))
        .context("configured target disk is missing from fresh block-device discovery")?;
    if !target_info.safety_checks_complete
        || target_info.size_bytes == 0
        || target_info.read_only
        || target_info.has_holders
        || target_info.active_swap
        || !target_info.mounted_at.is_empty()
        || target_info.partitions.len() != expected_count
    {
        bail!("configured target disk failed fresh safety discovery; basestrap was not started");
    }
    verify_target_partition(target_info, &efi, TARGET_BOOT)?;
    verify_target_partition(target_info, &root, TARGET_ROOT)?;
    verify_mount_source(Path::new(TARGET_ROOT), &root)?;
    verify_mount_source(Path::new(TARGET_BOOT), &efi)?;

    // Refuse any pre-existing target installation, then repeat this check immediately
    // before basestrap after package queries have completed.
    ensure_pristine_target()?;

    let plan = packages::build_package_plan(config);
    if plan
        .desktop_recipes
        .iter()
        .any(|recipe| recipe.source != PackageSource::Artix)
    {
        bail!(
            "the selected desktop recipe needs a non-default repository or external build; resolve that source before basestrap"
        );
    }
    if !plan.kernel_notes.is_empty() || plan.deferred.len() > 2 {
        bail!(
            "the selected package plan has unresolved external repository requirements; basestrap was not started"
        );
    }
    let package_report = package_check::check(config, Path::new("pacman"))
        .context("check the complete selected package plan before basestrap")?;
    if package_report.checks.iter().any(|check| {
        matches!(
            check.availability,
            Availability::Missing | Availability::CouldNotCheck
        )
    }) {
        bail!(
            "one or more required packages are missing or could not be verified; run --check-packages and resolve the report before basestrap"
        );
    }

    let mut package_names = Vec::<String>::new();
    let mut add = |name: &str| {
        if !package_names.iter().any(|existing| existing == name) {
            package_names.push(name.to_owned());
        }
    };
    for name in &plan.base_packages {
        add(name);
    }
    for name in &plan.kernel_packages {
        add(name);
    }
    for name in &plan.microcode_packages {
        add(name);
    }
    for name in &plan.gpu_packages {
        add(name);
    }
    for name in &plan.init_packages {
        add(name);
    }
    for name in &plan.user_service_packages {
        add(name);
    }
    for name in &plan.storage_packages {
        add(name);
    }
    for name in &plan.bootloader_packages {
        add(name);
    }
    for name in &plan.network_packages {
        add(name);
    }
    for name in &plan.xorg_packages {
        add(name);
    }
    for name in &plan.audio_packages {
        add(name);
    }
    for name in &plan.privilege_packages {
        add(name);
    }
    for recipe in &plan.desktop_recipes {
        for name in &recipe.packages {
            add(name);
        }
    }
    for name in &plan.optional_service_packages {
        if package_report
            .checks
            .iter()
            .any(|check| check.name == *name && check.availability == Availability::Available)
        {
            add(name);
        }
    }
    if package_names.is_empty() {
        bail!("package plan is empty; basestrap was not started");
    }

    if !executable_on_path("basestrap") {
        bail!("basestrap is not installed or not on PATH; no packages were installed");
    }
    if !executable_on_path("pacman") {
        bail!("pacman is not installed or not on PATH; no packages were installed");
    }

    // Recheck mounts, filesystem identities and the pristine target immediately
    // before the package manager is allowed to write to it.
    verify_mount_source(Path::new(TARGET_ROOT), &root)?;
    verify_mount_source(Path::new(TARGET_BOOT), &efi)?;
    verify_no_unexpected_target_mounts()?;
    verify_blkid_type(&efi, "vfat")?;
    verify_blkid_type(&root, "ext4")?;
    ensure_pristine_target()?;

    let mut command = Command::new("basestrap");
    command.arg(TARGET_ROOT).args(&package_names);
    let status = command.status().context("start Artix basestrap")?;
    if !status.success() {
        bail!(
            "basestrap exited with {status}; inspect /mnt before retrying. No later configuration stage was run"
        );
    }

    Ok(format!(
        "basestrap installed {} packages to {} on {}. The target mounts were verified before installation. No fstab, users, passwords, services, bootloader or chroot configuration has been changed.",
        package_names.len(),
        TARGET_ROOT,
        selected.display()
    ))
}

fn verify_artix_live_environment() -> Result<()> {
    let release = fs::read_to_string("/etc/os-release")
        .context("read /etc/os-release to verify the live operating system")?;
    let is_artix = release.lines().any(|line| {
        line.strip_prefix("ID=")
            .map(|value| value.trim_matches('"').trim_matches('\'') == "artix")
            .unwrap_or(false)
    });
    if !is_artix {
        bail!("this stage must run from an Artix Linux live environment; basestrap was not started");
    }
    Ok(())
}

fn same_device_path(left: &Path, right: &Path) -> bool {
    match (fs::canonicalize(left), fs::canonicalize(right)) {
        (Ok(left), Ok(right)) => left == right,
        _ => left == right,
    }
}

fn verify_target_partition(
    disk: &BlockDevice,
    expected_device: &Path,
    expected_mount: &str,
) -> Result<()> {
    let partition = disk
        .partitions
        .iter()
        .find(|partition| same_device_path(&partition.path, expected_device))
        .with_context(|| {
            format!(
                "partition {} is not a child of the configured target disk",
                expected_device.display()
            )
        })?;
    if !partition.safety_checks_complete
        || partition.size_bytes == 0
        || partition.read_only
        || partition.has_holders
        || partition.active_swap
        || partition.mounted_at.len() != 1
        || partition.mounted_at[0] != expected_mount
    {
        bail!(
            "partition {} failed fresh safety discovery or is mounted somewhere other than {}; refusing basestrap",
            expected_device.display(),
            expected_mount
        );
    }
    Ok(())
}

fn ensure_pristine_target() -> Result<()> {
    let root_entries = fs::read_dir(TARGET_ROOT).context("read mounted target root")?;
    for entry in root_entries {
        let entry = entry.context("inspect mounted target root entry")?;
        let name = entry.file_name();
        let metadata = fs::symlink_metadata(entry.path())
            .with_context(|| format!("inspect target root entry {}", name.to_string_lossy()))?;
        if !metadata.file_type().is_dir() {
            bail!(
                "target root entry '{}' is not a real directory; refusing basestrap",
                name.to_string_lossy()
            );
        }
        match name.to_str() {
            Some("lost+found") => {
                if fs::read_dir(entry.path())
                    .context("inspect lost+found contents")?
                    .next()
                    .is_some()
                {
                    bail!("/mnt/lost+found is not empty; refusing to continue");
                }
            }
            Some("boot") => {
                if fs::read_dir(entry.path())
                    .context("inspect mounted EFI contents")?
                    .next()
                    .is_some()
                {
                    bail!(
                        "/mnt/boot (the EFI partition) is not empty; refusing to overwrite existing boot files"
                    );
                }
            }
            _ => bail!(
                "{} already contains '{}'; refusing to install over existing data",
                TARGET_ROOT,
                name.to_string_lossy()
            ),
        }
    }
    for path in ["/mnt/etc", "/mnt/usr", "/mnt/var"] {
        match fs::symlink_metadata(path) {
            Ok(_) => bail!(
                "{path} already exists; refusing to overwrite a possible existing installation"
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error).with_context(|| format!("inspect {path}")),
        }
    }
    Ok(())
}

fn verify_no_unexpected_target_mounts() -> Result<()> {
    let contents = fs::read_to_string("/proc/self/mountinfo")
        .context("read mount table to check for unexpected target mounts")?;
    for line in contents.lines() {
        let (left, _) = match line.split_once(" - ") {
            Some(parts) => parts,
            None => continue,
        };
        let fields: Vec<_> = left.split_whitespace().collect();
        if fields.len() < 5 {
            continue;
        }
        let mountpoint = fields[4]
            .replace("\\040", " ")
            .replace("\\011", "\t")
            .replace("\\012", "\n")
            .replace("\\134", "\\\\");
        if mountpoint.starts_with("/mnt/") && mountpoint != TARGET_BOOT {
            bail!("unexpected nested mount {mountpoint} exists under /mnt; refusing basestrap");
        }
    }
    Ok(())
}

fn verify_blkid_type(device: &Path, expected: &str) -> Result<()> {
    let result = Command::new("blkid")
        .args(["-p", "-s", "TYPE", "-o", "value"])
        .arg(device)
        .output()
        .with_context(|| format!("inspect filesystem on {}", device.display()))?;
    let actual = String::from_utf8_lossy(&result.stdout)
        .trim()
        .to_ascii_lowercase();
    if !result.status.success() || actual != expected {
        bail!(
            "expected {expected} filesystem on {}, found {}; refusing basestrap",
            device.display(),
            if actual.is_empty() {
                "unknown"
            } else {
                &actual
            }
        );
    }
    Ok(())
}

fn verify_mount_source(mountpoint: &Path, expected_device: &Path) -> Result<()> {
    let contents = fs::read_to_string("/proc/self/mountinfo")
        .context("read kernel mount table to verify basestrap target")?;
    let expected = fs::canonicalize(expected_device)?;
    let device_name = expected
        .file_name()
        .context("verified partition path has no device name")?
        .to_str()
        .context("verified partition device name is not UTF-8")?;
    let sysfs_device = fs::read_to_string(format!("/sys/class/block/{device_name}/dev"))
        .with_context(|| format!("read sysfs device number for {device_name}"))?;
    let expected_devno = sysfs_device.trim();
    let wanted = mountpoint.to_string_lossy();
    for line in contents.lines() {
        let (left, right) = match line.split_once(" - ") {
            Some(parts) => parts,
            None => continue,
        };
        let before: Vec<_> = left.split_whitespace().collect();
        let after: Vec<_> = right.split_whitespace().collect();
        if before.len() < 5 || after.len() < 2 {
            continue;
        }
        let mounted_at = before[4]
            .replace("\\040", " ")
            .replace("\\011", "\t")
            .replace("\\012", "\n")
            .replace("\\134", "\\");
        if mounted_at == wanted {
            if before[2] != expected_devno {
                bail!(
                    "{} is mounted from device number {}, not expected device number {}",
                    mountpoint.display(),
                    before[2],
                    expected_devno
                );
            }
            let source = after[1]
                .replace("\\040", " ")
                .replace("\\011", "\t")
                .replace("\\012", "\n")
                .replace("\\134", "\\");
            let actual = fs::canonicalize(&source)
                .with_context(|| format!("resolve mounted source {source}"))?;
            if actual == expected {
                return Ok(());
            }
            bail!(
                "{} is mounted from {}, not selected device {}",
                mountpoint.display(),
                source,
                expected_device.display()
            );
        }
    }
    bail!(
        "{} is not a verified mountpoint; refusing basestrap",
        mountpoint.display()
    )
}

fn executable_on_path(program: &str) -> bool {
    env::split_paths(&env::var_os("PATH").unwrap_or_default())
        .map(|directory| directory.join(program))
        .any(|candidate| fs::metadata(candidate).is_ok_and(|metadata| metadata.is_file()))
}

fn libc_geteuid() -> u32 {
    // Read the effective UID from procfs without adding a libc dependency.
    fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|contents| {
            contents
                .lines()
                .find(|line| line.starts_with("Uid:"))
                .and_then(|line| line.split_whitespace().nth(1))
                .and_then(|value| value.parse().ok())
        })
        .unwrap_or(u32::MAX)
}
