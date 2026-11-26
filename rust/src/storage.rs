use std::{
    fs,
    io::Write,
    os::unix::fs::FileTypeExt,
    path::Path,
    process::{Command, Stdio},
};

use anyhow::{Context, Result, bail};

use crate::{
    config::{Filesystem, FirmwareMode, InstallConfig, Partitioning, SwapMode},
    discovery::{self, BlockDevice, HardwareSnapshot},
};

const GIB: u64 = 1024 * 1024 * 1024;
const MIN_ROOT_GIB: u64 = 8;
const PARTITION_ALIGNMENT_MARGIN_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
struct TargetIdentity {
    size_bytes: u64,
    serial: Option<String>,
    wwid: Option<String>,
    model: Option<String>,
    vendor: Option<String>,
}

impl TargetIdentity {
    fn from_disk(disk: &BlockDevice) -> Self {
        Self {
            size_bytes: disk.size_bytes,
            serial: disk.serial.clone(),
            wwid: disk.wwid.clone(),
            model: disk.model.clone(),
            vendor: disk.vendor.clone(),
        }
    }
}

pub fn apply_automatic_partition_table(
    config: &InstallConfig,
    typed_confirmation: &str,
) -> Result<String> {
    config.validate().map_err(|issues| {
        anyhow::anyhow!(
            "configuration validation failed:\n- {}",
            issues.join("\n- ")
        )
    })?;
    if libc_effective_uid() != 0 {
        bail!("partition-table writing requires effective UID 0");
    }
    if config.disk.partitioning != Partitioning::Automatic {
        bail!("the first storage executor supports automatic partitioning only");
    }
    let selected = config
        .disk
        .target
        .as_deref()
        .context("select a target disk before applying a partition table")?;
    if !selected.starts_with("/dev/") {
        bail!("target must be an absolute path below /dev");
    }
    validate_confirmation(selected, typed_confirmation)?;

    let hardware = discovery::discover();
    let disk = find_safe_target(&hardware, selected)?;
    let identity = TargetIdentity::from_disk(disk);
    validate_capacity(config, disk.size_bytes)?;

    let metadata =
        fs::metadata(selected).with_context(|| format!("inspect target {}", selected.display()))?;
    if !metadata.file_type().is_block_device() {
        bail!("{} is not a block device", selected.display());
    }

    ensure_partition_tools()?;
    let script = build_sfdisk_script(config)?;

    // Re-discover after all other preparation and immediately before the write.
    let latest_hardware = discovery::discover();
    let latest_disk = find_safe_target(&latest_hardware, selected)?;
    if TargetIdentity::from_disk(latest_disk) != identity {
        bail!("target identity changed during preflight; refusing to write a partition table");
    }

    let canonical_target = fs::canonicalize(selected)
        .with_context(|| format!("resolve target {}", selected.display()))?;
    let final_metadata = fs::metadata(&canonical_target)
        .with_context(|| format!("reinspect target {}", canonical_target.display()))?;
    if !final_metadata.file_type().is_block_device() {
        bail!("target no longer resolves to a block device; refusing to write");
    }
    let mut child = Command::new("sfdisk")
        .args(["--wipe", "always", "--wipe-partitions", "always", "--"])
        .arg(&canonical_target)
        .stdin(Stdio::piped())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .context("start sfdisk; ensure util-linux is installed")?;
    let mut stdin = child.stdin.take().context("open sfdisk standard input")?;
    if let Err(error) = stdin.write_all(script.as_bytes()) {
        let _ = child.kill();
        drop(stdin);
        let _ = child.wait();
        return Err(error).context("send partition layout to sfdisk; process was stopped");
    }
    drop(stdin);
    let status = child.wait().context("wait for sfdisk")?;
    if !status.success() {
        bail!("sfdisk failed with {status}; inspect the disk before retrying");
    }

    let verify = Command::new("sfdisk")
        .arg("--verify")
        .arg(&canonical_target)
        .status()
        .context("verify the newly written partition table with sfdisk")?;
    if !verify.success() {
        bail!(
            "partition table was written, but sfdisk verification failed; do not continue to formatting"
        );
    }

    let partprobe = Command::new("partprobe")
        .arg(&canonical_target)
        .status()
        .context("refresh the kernel partition table with partprobe")?;
    if !partprobe.success() {
        bail!(
            "partition table was written and verified, but partprobe failed; reboot or resolve the kernel refresh before continuing"
        );
    }
    let settle = Command::new("udevadm")
        .args(["settle", "--timeout=10"])
        .status()
        .context("wait for udev partition device nodes")?;
    if !settle.success() {
        bail!(
            "partition table was written and kernel refreshed, but udev did not settle; inspect device nodes before continuing"
        );
    }

    let dump = Command::new("sfdisk")
        .arg("--dump")
        .arg(&canonical_target)
        .output()
        .context("read back the written partition table")?;
    if !dump.status.success() {
        bail!(
            "partition table was written, but could not be read back; do not continue to formatting"
        );
    }
    let dump_text = String::from_utf8_lossy(&dump.stdout);
    verify_partition_table_dump(config, &dump_text)
        .context("written partition table does not match the requested layout; do not continue")?;

    // Confirm that the kernel and sysfs now expose the same number of child partitions.
    let refreshed_hardware = discovery::discover();
    let refreshed_disk = find_safe_target(&refreshed_hardware, selected).context(
        "partition table was written, but fresh device discovery could not verify the target",
    )?;
    let expected_count = expected_partition_types(config).len();
    if refreshed_disk.partitions.len() != expected_count {
        bail!(
            "partition table was written, but fresh discovery found {} partitions instead of {}; do not continue to formatting",
            refreshed_disk.partitions.len(),
            expected_count
        );
    }
    for (index, partition) in refreshed_disk.partitions.iter().enumerate() {
        if !partition.safety_checks_complete || partition.size_bytes == 0 {
            bail!(
                "partition table was written, but partition {} failed post-write safety/size discovery; do not continue",
                index + 1
            );
        }
        let partition_metadata = fs::metadata(&partition.path).with_context(|| {
            format!(
                "inspect newly discovered partition {}",
                partition.path.display()
            )
        })?;
        if !partition_metadata.file_type().is_block_device() {
            bail!(
                "partition table was written, but {} is not a block device; do not continue",
                partition.path.display()
            );
        }
    }

    Ok(format!(
        "Partition table written, read back, and matched the requested layout on {}. Fresh device discovery confirmed {} partitions. No filesystem was formatted, no encryption was initialized, and no filesystem was mounted.",
        selected.display(),
        expected_count
    ))
}

/// Format only the root partition from a freshly verified automatic layout.
pub fn format_automatic_root(config: &InstallConfig, typed_confirmation: &str) -> Result<String> {
    config.validate().map_err(|issues| {
        anyhow::anyhow!(
            "configuration validation failed:\n- {}",
            issues.join("\n- ")
        )
    })?;
    if libc_effective_uid() != 0 {
        bail!("root filesystem formatting requires effective UID 0");
    }
    validate_root_format_config(config)?;

    let selected = config
        .disk
        .target
        .as_deref()
        .context("select a target disk before formatting root")?;
    let hardware = discovery::discover();
    let disk = find_safe_target(&hardware, selected)?;
    let identity = TargetIdentity::from_disk(disk);
    let table = read_partition_table(selected)?;
    verify_partition_table_dump(config, &table)?;
    let root_path = root_partition_path(&table)?;
    validate_confirmation(&root_path, typed_confirmation)?;

    let root_partition = disk
        .partitions
        .iter()
        .find(|partition| same_device_path(&partition.path, &root_path))
        .context(
            "root partition from the partition table is not present in fresh device discovery",
        )?;
    validate_format_target(root_partition, &root_path)?;

    let mkfs = Command::new("mkfs.ext4")
        .arg("-V")
        .output()
        .context("mkfs.ext4 is required to format the configured ext4 root")?;
    if !mkfs.status.success() {
        bail!("mkfs.ext4 is installed but its version check failed");
    }

    // Repeat all device and table checks immediately before invoking the formatter.
    let latest_hardware = discovery::discover();
    let latest_disk = find_safe_target(&latest_hardware, selected)?;
    if TargetIdentity::from_disk(latest_disk) != identity {
        bail!("target disk identity changed before formatting; refusing to continue");
    }
    let latest_table = read_partition_table(selected)?;
    verify_partition_table_dump(config, &latest_table)?;
    let latest_root = root_partition_path(&latest_table)?;
    if !same_device_path(&latest_root, &root_path) {
        bail!("root partition path changed before formatting; refusing to continue");
    }
    let latest_partition = latest_disk
        .partitions
        .iter()
        .find(|partition| same_device_path(&partition.path, &root_path))
        .context("root partition disappeared from fresh discovery; refusing to format")?;
    validate_format_target(latest_partition, &root_path)?;

    let canonical_root = fs::canonicalize(&root_path)
        .with_context(|| format!("resolve root partition {}", root_path.display()))?;
    let metadata = fs::metadata(&canonical_root)
        .with_context(|| format!("inspect root partition {}", canonical_root.display()))?;
    if !metadata.file_type().is_block_device() {
        bail!("root target is no longer a block device; refusing to format");
    }

    let status = Command::new("mkfs.ext4")
        .args(["-F", "--"])
        .arg(&canonical_root)
        .status()
        .context("start ext4 filesystem formatter")?;
    if !status.success() {
        bail!("mkfs.ext4 failed; inspect the partition before retrying");
    }

    let probe = Command::new("blkid")
        .args(["-p", "-s", "TYPE", "-o", "value"])
        .arg(&canonical_root)
        .output()
        .context("verify the newly formatted root filesystem with blkid")?;
    if !probe.status.success() || String::from_utf8_lossy(&probe.stdout).trim() != "ext4" {
        bail!(
            "mkfs.ext4 returned success, but blkid could not confirm an ext4 filesystem; do not continue"
        );
    }

    Ok(format!(
        "Formatted {} as ext4 and verified the filesystem type. No encryption, swap activation, mounting, or installation was performed.",
        root_path.display()
    ))
}

/// Prepare the EFI System Partition from an automatic UEFI layout.
///
/// Existing FAT filesystems are preserved. A blank partition created by our
/// automatic partitioner is formatted as FAT32. Unknown signatures fail closed.
pub fn prepare_automatic_efi(config: &InstallConfig, typed_confirmation: &str) -> Result<String> {
    config.validate().map_err(|issues| {
        anyhow::anyhow!(
            "configuration validation failed:\n- {}",
            issues.join("\n- ")
        )
    })?;
    if libc_effective_uid() != 0 {
        bail!("EFI preparation requires effective UID 0");
    }
    if config.disk.partitioning != Partitioning::Automatic
        || config.system.firmware != FirmwareMode::Uefi
        || config.disk.encryption
    {
        bail!("EFI preparation currently supports unencrypted automatic UEFI layouts only");
    }
    let selected = config
        .disk
        .target
        .as_deref()
        .context("select a target disk before preparing EFI")?;
    let hardware = discovery::discover();
    let disk = find_safe_target(&hardware, selected)?;
    let identity = TargetIdentity::from_disk(disk);
    let table = read_partition_table(selected)?;
    verify_partition_table_dump(config, &table)?;
    let efi_path = table
        .lines()
        .find(|line| line.trim_start().starts_with("/dev/"))
        .and_then(|line| {
            line.split_once(':')
                .map(|(path, _)| Path::new(path.trim()).to_path_buf())
        })
        .context("partition table does not identify an EFI partition")?;
    validate_confirmation(&efi_path, typed_confirmation)?;
    let partition = disk
        .partitions
        .iter()
        .find(|p| same_device_path(&p.path, &efi_path))
        .context("EFI partition is not present in fresh device discovery")?;
    validate_format_target(partition, &efi_path)?;

    let canonical = fs::canonicalize(&efi_path)
        .with_context(|| format!("resolve EFI partition {}", efi_path.display()))?;
    let probe = Command::new("blkid")
        .args(["-p", "-s", "TYPE", "-o", "value"])
        .arg(&canonical)
        .output()
        .context("blkid is required to inspect the EFI partition safely")?;
    let existing_type = String::from_utf8_lossy(&probe.stdout)
        .trim()
        .to_ascii_lowercase();
    let should_format = if probe.status.success() && existing_type == "vfat" {
        false
    } else if probe.status.success() && !existing_type.is_empty() {
        bail!(
            "EFI partition contains an existing '{}' filesystem; refusing to overwrite it",
            existing_type
        );
    } else if (probe.status.success() || probe.status.code() == Some(2)) && existing_type.is_empty()
    {
        // blkid can succeed after finding partition metadata without finding a
        // filesystem TYPE. Confirm that no on-disk signatures exist before
        // treating this as a blank EFI partition.
        let signatures = Command::new("wipefs")
            .args(["--no-act", "--noheadings", "--output", "TYPE", "--"])
            .arg(&canonical)
            .output()
            .context("wipefs is required to confirm that the EFI partition has no signatures")?;
        if !signatures.status.success() {
            bail!("could not inspect EFI partition signatures; refusing to format");
        }
        let signatures = String::from_utf8_lossy(&signatures.stdout);
        if !signatures.trim().is_empty() {
            bail!(
                "EFI partition contains existing on-disk signatures ({}); refusing to overwrite them",
                signatures.trim().replace('\n', ", ")
            );
        }
        true
    } else {
        bail!("could not determine whether EFI partition is blank; refusing to format");
    };

    if should_format {
        let mkfs = Command::new("mkfs.fat")
            .arg("--help")
            .output()
            .context("mkfs.fat is required to prepare a blank EFI partition")?;
        if !mkfs.status.success() && mkfs.status.code() != Some(1) {
            bail!("mkfs.fat is installed but its capability check failed");
        }
        let latest = discovery::discover();
        let latest_disk = find_safe_target(&latest, selected)?;
        if TargetIdentity::from_disk(latest_disk) != identity {
            bail!("target disk identity changed before EFI preparation; refusing to continue");
        }
        let latest_table = read_partition_table(selected)?;
        verify_partition_table_dump(config, &latest_table)?;
        let latest_efi = latest_disk
            .partitions
            .iter()
            .find(|p| same_device_path(&p.path, &efi_path))
            .context("EFI partition disappeared before formatting")?;
        validate_format_target(latest_efi, &efi_path)?;
        let status = Command::new("mkfs.fat")
            .args(["-F", "32", "-n", "ARTIX_EFI", "--"])
            .arg(&canonical)
            .status()
            .context("start FAT32 EFI formatter")?;
        if !status.success() {
            bail!("mkfs.fat failed; inspect the EFI partition before retrying");
        }
    }

    let verify = Command::new("blkid")
        .args(["-p", "-s", "TYPE", "-o", "value"])
        .arg(&canonical)
        .output()
        .context("verify EFI filesystem type")?;
    if !verify.status.success() || String::from_utf8_lossy(&verify.stdout).trim() != "vfat" {
        bail!("EFI preparation did not result in a verified FAT filesystem; do not mount it");
    }
    Ok(if should_format {
        format!(
            "Formatted blank EFI partition {} as FAT32 and verified it. Existing EFI filesystems are never overwritten.",
            efi_path.display()
        )
    } else {
        format!(
            "EFI partition {} already contains FAT and was preserved; filesystem type verified.",
            efi_path.display()
        )
    })
}
/// Mount the verified automatic UEFI/ext4 layout at /mnt for the next install stage.
/// This operation never formats devices and rolls back mounts it created on failure.
pub fn mount_automatic_target(config: &InstallConfig, typed_confirmation: &str) -> Result<String> {
    config.validate().map_err(|issues| {
        anyhow::anyhow!(
            "configuration validation failed:\n- {}",
            issues.join("\n- ")
        )
    })?;
    if libc_effective_uid() != 0 {
        bail!("mounting the installation target requires effective UID 0");
    }
    if config.disk.partitioning != Partitioning::Automatic
        || config.system.firmware != FirmwareMode::Uefi
        || config.disk.root_filesystem != Filesystem::Ext4
        || config.disk.encryption
    {
        bail!(
            "target mounting currently supports unencrypted automatic UEFI layouts with ext4 root only"
        );
    }
    let selected = config
        .disk
        .target
        .as_deref()
        .context("select a target disk before mounting")?;
    validate_confirmation(selected, typed_confirmation)?;

    let hardware = discovery::discover();
    let disk = find_safe_target(&hardware, selected)?;
    let identity = TargetIdentity::from_disk(disk);
    let table = read_partition_table(selected)?;
    verify_partition_table_dump(config, &table)?;
    let efi_path = first_partition_path(&table)?;
    let root_path = root_partition_path(&table)?;
    let efi = disk
        .partitions
        .iter()
        .find(|p| same_device_path(&p.path, &efi_path))
        .context("EFI partition is missing from fresh discovery")?;
    let root = disk
        .partitions
        .iter()
        .find(|p| same_device_path(&p.path, &root_path))
        .context("root partition is missing from fresh discovery")?;
    validate_format_target(efi, &efi_path)?;
    validate_format_target(root, &root_path)?;

    verify_filesystem_type(&efi_path, "vfat")?;
    verify_filesystem_type(&root_path, "ext4")?;
    ensure_mount_tools()?;
    ensure_empty_mount_root(Path::new("/mnt"))?;

    // Recheck the disk, partition table, mount state and filesystem signatures immediately
    // before making the first mount.
    let latest = discovery::discover();
    let latest_disk = find_safe_target(&latest, selected)?;
    if TargetIdentity::from_disk(latest_disk) != identity {
        bail!("target disk identity changed before mounting; refusing to continue");
    }
    let latest_table = read_partition_table(selected)?;
    verify_partition_table_dump(config, &latest_table)?;
    if !same_device_path(&first_partition_path(&latest_table)?, &efi_path)
        || !same_device_path(&root_partition_path(&latest_table)?, &root_path)
    {
        bail!("partition paths changed before mounting; refusing to continue");
    }
    for path in [&efi_path, &root_path] {
        let part = latest_disk
            .partitions
            .iter()
            .find(|p| same_device_path(&p.path, path))
            .context("a target partition disappeared from fresh discovery")?;
        validate_format_target(part, path)?;
    }
    verify_filesystem_type(&efi_path, "vfat")?;
    verify_filesystem_type(&root_path, "ext4")?;
    ensure_empty_mount_root(Path::new("/mnt"))?;

    let root_device = fs::canonicalize(&root_path)?;
    let efi_device = fs::canonicalize(&efi_path)?;
    let root_mount = Path::new("/mnt");
    let efi_mount = Path::new("/mnt/boot");

    run_mount(&root_device, root_mount).context("mount the ext4 root filesystem at /mnt")?;
    let result = (|| -> Result<()> {
        match fs::symlink_metadata(efi_mount) {
            Ok(metadata) if metadata.file_type().is_dir() => {}
            Ok(_) => {
                bail!("/mnt/boot exists but is not a real directory; refusing to mount over it")
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir(efi_mount)
                    .context("create /mnt/boot inside the mounted root filesystem")?;
            }
            Err(error) => return Err(error).context("inspect /mnt/boot before mounting EFI"),
        }
        if mountpoint_is_used(efi_mount)? {
            bail!("/mnt/boot is already a mountpoint; refusing to cover it");
        }
        run_mount(&efi_device, efi_mount).context("mount the EFI filesystem at /mnt/boot")?;
        verify_mount_source(root_mount, &root_device)?;
        verify_mount_source(efi_mount, &efi_device)?;
        Ok(())
    })();

    if let Err(error) = result {
        let mut rollback_errors = Vec::new();
        rollback_mount(
            efi_mount,
            &efi_device,
            "unmount /mnt/boot",
            &mut rollback_errors,
        );
        rollback_mount(
            root_mount,
            &root_device,
            "unmount /mnt",
            &mut rollback_errors,
        );
        if rollback_errors.is_empty() {
            return Err(error).context(
                "mount operation failed; mounts created by this operation were rolled back",
            );
        }
        bail!(
            "{error:#}; rollback was incomplete: {}",
            rollback_errors.join("; ")
        );
    }

    Ok(format!(
        "Mounted verified ext4 root {} at /mnt and FAT EFI {} at /mnt/boot. No filesystems were formatted and no installation has started.",
        root_path.display(),
        efi_path.display()
    ))
}

/// Generate a UUID-based fstab only after the target root and EFI mounts are verified.
pub fn generate_automatic_fstab(config: &InstallConfig) -> Result<String> {
    config.validate().map_err(|issues| {
        anyhow::anyhow!(
            "configuration validation failed:\n- {}",
            issues.join("\n- ")
        )
    })?;
    if libc_effective_uid() != 0 {
        bail!("fstab generation requires effective UID 0");
    }
    if config.disk.partitioning != Partitioning::Automatic
        || config.system.firmware != FirmwareMode::Uefi
        || config.disk.root_filesystem != Filesystem::Ext4
        || config.disk.encryption
    {
        bail!("fstab generation currently supports unencrypted automatic UEFI/ext4 layouts only");
    }

    let selected = config
        .disk
        .target
        .as_deref()
        .context("select a target disk before generating fstab")?;
    let table = read_partition_table(selected)?;
    verify_partition_table_dump(config, &table)?;
    let efi_path = first_partition_path(&table)?;
    let root_path = root_partition_path(&table)?;
    let root_device = fs::canonicalize(&root_path)?;
    let efi_device = fs::canonicalize(&efi_path)?;
    verify_filesystem_type(&root_device, "ext4")?;
    verify_filesystem_type(&efi_device, "vfat")?;
    verify_mount_source(Path::new("/mnt"), &root_device)?;
    verify_mount_source(Path::new("/mnt/boot"), &efi_device)?;

    let etc = Path::new("/mnt/etc");
    let etc_metadata = fs::symlink_metadata(etc)
        .context("/mnt/etc does not exist; install the base system with basestrap first")?;
    if !etc_metadata.file_type().is_dir() {
        bail!("/mnt/etc is not a real directory; refusing to write fstab");
    }
    let fstab = etc.join("fstab");
    match fs::symlink_metadata(&fstab) {
        Ok(_) => bail!("/mnt/etc/fstab already exists; refusing to overwrite or append to it"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error).context("inspect /mnt/etc/fstab before writing"),
    }

    let generated = Command::new("fstabgen")
        .args(["-U", "/mnt"])
        .output()
        .context("fstabgen is required to generate UUID-based filesystem entries")?;
    if !generated.status.success() {
        bail!(
            "fstabgen failed: {}",
            String::from_utf8_lossy(&generated.stderr).trim()
        );
    }
    let contents =
        String::from_utf8(generated.stdout).context("fstabgen returned non-UTF-8 output")?;
    validate_generated_fstab(&contents)?;

    // fstabgen runs an external process. Reconfirm that /mnt still refers to
    // the selected partitions before creating any file in the target tree.
    verify_mount_source(Path::new("/mnt"), &root_device)
        .context("root mount changed while generating fstab; refusing to write")?;
    verify_mount_source(Path::new("/mnt/boot"), &efi_device)
        .context("EFI mount changed while generating fstab; refusing to write")?;
    verify_filesystem_type(&root_device, "ext4")?;
    verify_filesystem_type(&efi_device, "vfat")?;

    let temporary = etc.join(format!(".fstab.artix-setup-{}.tmp", std::process::id()));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .with_context(|| format!("create temporary fstab {}", temporary.display()))?;
    if let Err(error) = (|| -> Result<()> {
        file.write_all(contents.as_bytes())
            .context("write generated fstab")?;
        file.sync_all().context("flush generated fstab to disk")?;
        fs::hard_link(&temporary, &fstab)
            .context("install generated /etc/fstab without replacing an existing file")?;
        fs::remove_file(&temporary).context("remove temporary fstab link")?;
        let directory = fs::File::open(etc).context("open /mnt/etc to sync directory metadata")?;
        directory
            .sync_all()
            .context("sync /mnt/etc directory metadata")?;
        Ok(())
    })() {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }

    let verify = Command::new("findmnt")
        .args(["--verify", "--tab-file"])
        .arg(&fstab)
        .output()
        .context("findmnt is required to verify the generated fstab")?;
    if !verify.status.success() {
        let _ = fs::remove_file(&fstab);
        bail!(
            "findmnt rejected the generated fstab; the file was removed: {}",
            String::from_utf8_lossy(&verify.stderr).trim()
        );
    }

    Ok("Generated and verified /mnt/etc/fstab using filesystem UUIDs. Existing fstab files are never overwritten.".to_owned())
}

fn validate_generated_fstab(contents: &str) -> Result<()> {
    let mut root = 0;
    let mut boot = 0;
    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let fields: Vec<_> = line.split_whitespace().collect();
        if fields.len() != 6 {
            bail!("fstabgen produced an entry with an invalid field count: {line}");
        }
        let uuid = fields[0]
            .strip_prefix("UUID=")
            .context("fstabgen did not use UUID sources for every entry")?;
        let standard_uuid = uuid.len() == 36
            && uuid.bytes().enumerate().all(|(index, byte)| match index {
                8 | 13 | 18 | 23 => byte == b'-',
                _ => byte.is_ascii_hexdigit(),
            });
        let fat_uuid = uuid.len() == 9
            && uuid.as_bytes()[4] == b'-'
            && uuid
                .bytes()
                .enumerate()
                .all(|(index, byte)| index == 4 || byte.is_ascii_hexdigit());
        match fields[1] {
            "/" => {
                root += 1;
                if fields[2] != "ext4" || !standard_uuid {
                    bail!("generated root fstab entry must use ext4 and a standard UUID");
                }
            }
            "/boot" => {
                if fields[2] != "vfat" || !(standard_uuid || fat_uuid) {
                    bail!("generated EFI fstab entry must use vfat and a valid FAT UUID");
                }
                boot += 1;
            }
            _ => {}
        }
    }
    if root != 1 || boot != 1 {
        bail!(
            "generated fstab must contain exactly one UUID-based ext4 / entry and one UUID-based vfat /boot entry"
        );
    }
    Ok(())
}

fn first_partition_path(dump: &str) -> Result<std::path::PathBuf> {
    dump.lines()
        .find(|line| line.trim_start().starts_with("/dev/"))
        .and_then(|line| line.split_once(':').map(|(path, _)| Path::new(path.trim())))
        .map(Path::to_path_buf)
        .context("partition table does not identify an EFI partition")
}

fn verify_filesystem_type(device: &Path, expected: &str) -> Result<()> {
    let probe = Command::new("blkid")
        .args(["-p", "-s", "TYPE", "-o", "value"])
        .arg(device)
        .output()
        .with_context(|| format!("inspect filesystem on {}", device.display()))?;
    let actual = String::from_utf8_lossy(&probe.stdout)
        .trim()
        .to_ascii_lowercase();
    if !probe.status.success() || actual != expected {
        bail!(
            "expected {expected} filesystem on {}, found {}; refusing to mount",
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

fn ensure_mount_tools() -> Result<()> {
    for tool in ["mount", "umount"] {
        let output = Command::new(tool)
            .arg("--version")
            .output()
            .with_context(|| format!("{tool} is required to mount the installation target"))?;
        if !output.status.success() {
            bail!("{tool} is installed but its version check failed");
        }
    }
    Ok(())
}

fn ensure_empty_mount_root(path: &Path) -> Result<()> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir(path)
                .with_context(|| format!("create mount root {}", path.display()))?;
            fs::symlink_metadata(path)
                .with_context(|| format!("inspect newly created mount root {}", path.display()))?
        }
        Err(error) => {
            return Err(error).with_context(|| format!("inspect mount root {}", path.display()));
        }
    };
    if !metadata.file_type().is_dir() {
        bail!("{} exists but is not a real directory", path.display());
    }
    if mountpoint_is_used(path)? {
        bail!(
            "{} is already a mountpoint; refusing to mount over it",
            path.display()
        );
    }
    if fs::read_dir(path)
        .with_context(|| format!("inspect contents of {}", path.display()))?
        .next()
        .is_some()
    {
        bail!(
            "{} is not empty; move its contents or choose a different mount target",
            path.display()
        );
    }
    Ok(())
}

fn valid_mount_device_number(value: &str) -> bool {
    let mut parts = value.split(':');
    matches!(
        (parts.next(), parts.next(), parts.next()),
        (Some(major), Some(minor), None)
            if major.parse::<u32>().is_ok() && minor.parse::<u32>().is_ok()
    )
}

/// Parse every mountinfo row and fail closed if any row is malformed.
fn parse_mountinfo(contents: &str) -> Result<Vec<(String, String)>> {
    if contents.trim().is_empty() {
        bail!("the kernel mount table is empty; refusing to assume a mountpoint is unused");
    }

    let mut entries = Vec::new();
    for (index, line) in contents.lines().enumerate() {
        let line_number = index + 1;
        let (left, right) = line
            .split_once(" - ")
            .with_context(|| format!("malformed mountinfo row {line_number}: missing separator"))?;
        let before: Vec<_> = left.split_whitespace().collect();
        let after: Vec<_> = right.split_whitespace().collect();
        if before.len() < 6 || after.len() < 3 || !valid_mount_device_number(before[2]) {
            bail!("malformed or incomplete mountinfo row {line_number}; refusing mount operation");
        }
        entries.push((unescape_mountinfo(before[4]), unescape_mountinfo(after[1])));
    }

    if entries.is_empty() {
        bail!("the kernel mount table contains no parseable entries");
    }
    Ok(entries)
}

fn unique_mount_source<'a>(entries: &'a [(String, String)], mountpoint: &str) -> Result<&'a str> {
    let mut matches = entries
        .iter()
        .filter(|(mounted_at, _)| mounted_at == mountpoint);
    let (_, source) = matches
        .next()
        .with_context(|| format!("{mountpoint} does not appear in the kernel mount table"))?;
    if matches.next().is_some() {
        bail!(
            "{mountpoint} appears more than once in the kernel mount table; refusing to trust an ambiguous mount"
        );
    }
    Ok(source)
}

fn unescape_mountinfo(value: &str) -> String {
    value
        .replace("\\040", " ")
        .replace("\\011", "\t")
        .replace("\\012", "\n")
        .replace("\\134", "\\")
}

fn mountpoint_is_used(path: &Path) -> Result<bool> {
    let contents = fs::read_to_string("/proc/self/mountinfo")
        .context("read /proc/self/mountinfo to verify mount state")?;
    let entries = parse_mountinfo(&contents)?;
    let wanted = path.to_string_lossy();
    Ok(entries
        .iter()
        .any(|(mountpoint, _)| mountpoint == wanted.as_ref()))
}

fn verify_mount_source(mountpoint: &Path, expected_device: &Path) -> Result<()> {
    let contents = fs::read_to_string("/proc/self/mountinfo")
        .context("read mount table to verify mounted source")?;
    let entries = parse_mountinfo(&contents)?;
    let wanted_mount = mountpoint.to_string_lossy();
    let source = unique_mount_source(&entries, wanted_mount.as_ref())?;
    let expected = fs::canonicalize(expected_device).with_context(|| {
        format!(
            "resolve expected mounted device {}",
            expected_device.display()
        )
    })?;
    let actual =
        fs::canonicalize(source).with_context(|| format!("resolve mounted source {source}"))?;
    if actual != expected {
        bail!(
            "{} is mounted from {}, not the selected device {}",
            mountpoint.display(),
            source,
            expected_device.display()
        );
    }
    Ok(())
}

fn rollback_mount(
    mountpoint: &Path,
    expected_device: &Path,
    description: &str,
    errors: &mut Vec<String>,
) {
    match mountpoint_is_used(mountpoint) {
        Ok(false) => return,
        Err(error) => {
            errors.push(format!(
                "{description}: could not verify mount state ({error:#}); leaving it untouched"
            ));
            return;
        }
        Ok(true) => {}
    }

    // Never unmount a mount created by another process. During rollback, only
    // remove a mount whose source still resolves to the device this operation used.
    if let Err(error) = verify_mount_source(mountpoint, expected_device) {
        errors.push(format!(
            "{description}: mount source could not be verified as {}; leaving it untouched ({error:#})",
            expected_device.display()
        ));
        return;
    }

    if let Err(error) = run_umount(mountpoint) {
        errors.push(format!("{description}: {error:#}"));
    }
}

fn run_mount(device: &Path, mountpoint: &Path) -> Result<()> {
    let status = Command::new("mount")
        .arg("--")
        .arg(device)
        .arg(mountpoint)
        .status()
        .with_context(|| format!("run mount for {}", mountpoint.display()))?;
    if !status.success() {
        bail!("mount failed for {} with {status}", mountpoint.display());
    }
    Ok(())
}

fn run_umount(mountpoint: &Path) -> Result<()> {
    let status = Command::new("umount")
        .arg("--")
        .arg(mountpoint)
        .status()
        .with_context(|| format!("run umount for {}", mountpoint.display()))?;
    if !status.success() {
        bail!("umount failed for {} with {status}", mountpoint.display());
    }
    Ok(())
}

fn validate_root_format_config(config: &InstallConfig) -> Result<()> {
    if config.disk.partitioning != Partitioning::Automatic {
        bail!("root formatting currently supports automatic partition layouts only");
    }
    if config.disk.encryption {
        bail!(
            "encrypted root formatting is not implemented; refusing to format an unencrypted partition"
        );
    }
    if config.disk.root_filesystem != Filesystem::Ext4 {
        bail!("the first root formatter supports ext4 only; no formatting command was run");
    }
    Ok(())
}

fn read_partition_table(disk: &Path) -> Result<String> {
    let output = Command::new("sfdisk")
        .arg("--dump")
        .arg(disk)
        .output()
        .context("read the selected disk partition table")?;
    if !output.status.success() {
        bail!("could not read the selected disk partition table; refusing to format");
    }
    String::from_utf8(output.stdout).context("sfdisk returned non-UTF-8 partition table data")
}

fn root_partition_path(dump: &str) -> Result<std::path::PathBuf> {
    dump.lines()
        .rfind(|line| line.trim_start().starts_with("/dev/"))
        .and_then(|line| line.split_once(':').map(|(path, _)| Path::new(path.trim())))
        .map(Path::to_path_buf)
        .context("partition table does not identify a root partition")
}

fn same_device_path(left: &Path, right: &Path) -> bool {
    match (fs::canonicalize(left), fs::canonicalize(right)) {
        (Ok(left), Ok(right)) => left == right,
        _ => left == right,
    }
}

fn validate_format_target(
    partition: &crate::discovery::BlockPartition,
    expected_path: &Path,
) -> Result<()> {
    if !partition.safety_checks_complete
        || partition.size_bytes == 0
        || !partition.mounted_at.is_empty()
        || partition.active_swap
        || partition.read_only
        || partition.has_holders
    {
        bail!(
            "root partition {} is mounted, active swap, read-only, held, empty, or failed safety discovery",
            expected_path.display()
        );
    }
    let metadata = fs::metadata(expected_path)
        .with_context(|| format!("inspect root partition {}", expected_path.display()))?;
    if !metadata.file_type().is_block_device() {
        bail!("{} is not a block device", expected_path.display());
    }
    Ok(())
}

fn expected_partition_types(config: &InstallConfig) -> Vec<&'static str> {
    let uefi = config.system.firmware == FirmwareMode::Uefi;
    let mut types = Vec::new();
    if uefi {
        types.push("C12A7328-F81F-11D2-BA4B-00A0C93EC93B");
    }
    if config.disk.swap.mode == SwapMode::Partition {
        types.push(if uefi {
            "0657FD6D-A4AB-43C4-84E5-0933C84B4F4F"
        } else {
            "82"
        });
    }
    types.push(if uefi {
        "0FC63DAF-8483-4772-8E79-3D69D8477DE4"
    } else {
        "83"
    });
    types
}

/// Validate the actual sfdisk read-back, including partition order, start sectors,
/// sizes for fixed-size partitions, and type identifiers.
fn verify_partition_table_dump(config: &InstallConfig, dump: &str) -> Result<()> {
    let expected_label = if config.system.firmware == FirmwareMode::Uefi {
        "gpt"
    } else {
        "dos"
    };
    let label = dump
        .lines()
        .find_map(|line| line.trim().strip_prefix("label:"))
        .map(str::trim)
        .context("sfdisk dump has no disklabel")?;
    if label != expected_label {
        bail!("expected {expected_label} partition table, found {label}");
    }

    let mut actual = Vec::<(u64, u64, String)>::new();
    for line in dump
        .lines()
        .filter(|line| line.trim_start().starts_with("/dev/"))
    {
        let (_, fields) = line
            .split_once(':')
            .context("malformed partition row in sfdisk dump")?;
        let start = sfdisk_field(fields, "start")?
            .parse::<u64>()
            .context("partition start is not an integer")?;
        let size = sfdisk_field(fields, "size")?
            .parse::<u64>()
            .context("partition size is not an integer")?;
        let kind = sfdisk_field(fields, "type")?
            .trim_matches('"')
            .to_ascii_uppercase();
        actual.push((start, size, kind));
    }

    let expected_types = expected_partition_types(config);
    if actual.len() != expected_types.len() {
        bail!(
            "expected {} partitions, found {}",
            expected_types.len(),
            actual.len()
        );
    }

    let mut expected_start = 2048_u64;
    let mut index = 0;
    if config.system.firmware == FirmwareMode::Uefi {
        verify_partition_entry(
            &actual[index],
            expected_start,
            1024 * 1024 * 1024 / 512,
            expected_types[index],
        )?;
        expected_start += 1024 * 1024 * 1024 / 512;
        index += 1;
    }
    if config.disk.swap.mode == SwapMode::Partition {
        let swap_sectors = u64::from(config.disk.swap.size_gib) * 1024 * 1024 * 1024 / 512;
        verify_partition_entry(
            &actual[index],
            expected_start,
            swap_sectors,
            expected_types[index],
        )?;
        expected_start += swap_sectors;
        index += 1;
    }
    let root = &actual[index];
    if root.0 != expected_start || root.1 == 0 || root.2 != expected_types[index] {
        bail!("root partition has an unexpected start, size, or type");
    }
    Ok(())
}

fn sfdisk_field<'a>(fields: &'a str, name: &str) -> Result<&'a str> {
    fields
        .split(',')
        .find_map(|field| {
            let (key, value) = field.trim().split_once('=')?;
            (key.trim() == name).then_some(value.trim())
        })
        .with_context(|| format!("partition row is missing {name}"))
}

fn verify_partition_entry(
    actual: &(u64, u64, String),
    expected_start: u64,
    expected_size: u64,
    expected_type: &str,
) -> Result<()> {
    if actual.0 != expected_start || actual.1 != expected_size || actual.2 != expected_type {
        bail!(
            "partition mismatch: expected start={expected_start}, size={expected_size}, type={expected_type}; found start={}, size={}, type={}",
            actual.0,
            actual.1,
            actual.2
        );
    }
    Ok(())
}

fn ensure_partition_tools() -> Result<()> {
    for tool in ["sfdisk", "partprobe", "udevadm"] {
        let output = Command::new(tool)
            .arg("--version")
            .output()
            .with_context(|| format!("{tool} is required for partition-table execution"))?;
        if !output.status.success() {
            bail!("{tool} is installed but its version check failed");
        }
    }
    let help = Command::new("sfdisk")
        .arg("--help")
        .output()
        .context("inspect sfdisk options before writing")?;
    let help_text = String::from_utf8_lossy(&help.stdout);
    if !help.status.success()
        || !help_text.contains("--wipe")
        || !help_text.contains("--wipe-partitions")
    {
        bail!(
            "installed sfdisk lacks the required explicit signature-wiping options; refusing to write"
        );
    }
    Ok(())
}

fn validate_confirmation(selected: &Path, typed_confirmation: &str) -> Result<()> {
    if typed_confirmation != selected.to_string_lossy() {
        bail!(
            "confirmation mismatch: pass --confirm-target '{}' exactly",
            selected.display()
        );
    }
    Ok(())
}

fn find_safe_target<'a>(
    hardware: &'a HardwareSnapshot,
    selected: &Path,
) -> Result<&'a BlockDevice> {
    let canonical = fs::canonicalize(selected)
        .with_context(|| format!("resolve configured target {}", selected.display()))?;
    let disk = hardware
        .disks
        .iter()
        .find(|disk| fs::canonicalize(&disk.path).is_ok_and(|path| path == canonical))
        .context("configured target is not present in fresh whole-disk discovery")?;
    if !disk.can_be_selected() {
        bail!(
            "target {} is blocked by fresh safety discovery: {}",
            selected.display(),
            disk.selection_block_reason()
                .unwrap_or("safety checks incomplete")
        );
    }
    Ok(disk)
}

fn validate_capacity(config: &InstallConfig, size_bytes: u64) -> Result<()> {
    let efi_bytes = if config.system.firmware == FirmwareMode::Uefi {
        GIB
    } else {
        0
    };
    let swap_bytes = if config.disk.swap.mode == SwapMode::Partition {
        u64::from(config.disk.swap.size_gib) * GIB
    } else {
        0
    };
    let minimum = efi_bytes + swap_bytes + MIN_ROOT_GIB * GIB + PARTITION_ALIGNMENT_MARGIN_BYTES;
    if size_bytes < minimum {
        bail!(
            "target is too small: needs at least {} GiB for the planned EFI, swap and minimum root partitions",
            minimum.div_ceil(GIB)
        );
    }
    Ok(())
}

pub fn build_sfdisk_script(config: &InstallConfig) -> Result<String> {
    config.validate().map_err(|issues| {
        anyhow::anyhow!(
            "configuration validation failed:\n- {}",
            issues.join("\n- ")
        )
    })?;
    if config.disk.partitioning != Partitioning::Automatic {
        bail!("only automatic partition layouts can be serialized for sfdisk");
    }

    let uefi = config.system.firmware == FirmwareMode::Uefi;
    let mut script = String::new();
    script.push_str(if uefi { "label: gpt\n" } else { "label: dos\n" });
    if uefi {
        script.push_str("label-id: ");
        script.push_str(&new_uuid()?);
        script.push('\n');
    }

    if uefi {
        script.push_str("size=+1G, type=C12A7328-F81F-11D2-BA4B-00A0C93EC93B\n");
    }
    if config.disk.swap.mode == SwapMode::Partition {
        let swap_type = if uefi {
            "0657FD6D-A4AB-43C4-84E5-0933C84B4F4F"
        } else {
            "82"
        };
        script.push_str(&format!(
            "size=+{}G, type={}\n",
            config.disk.swap.size_gib, swap_type
        ));
    }
    if uefi {
        script.push_str("size=+, type=0FC63DAF-8483-4772-8E79-3D69D8477DE4\n");
    } else {
        script.push_str("size=+, type=83, bootable\n");
    }
    Ok(script)
}

fn new_uuid() -> Result<String> {
    let value = fs::read_to_string("/proc/sys/kernel/random/uuid")
        .context("read the kernel UUID generator")?
        .trim()
        .to_owned();
    if value.len() != 36
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() || byte == b'-')
    {
        bail!("kernel UUID generator returned an invalid GPT disk GUID");
    }
    Ok(value)
}

fn libc_effective_uid() -> u32 {
    // Linux exposes the effective UID in /proc/self/status; avoid a libc dependency.
    fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|contents| {
            contents.lines().find_map(|line| {
                let (key, value) = line.split_once(':')?;
                (key == "Uid")
                    .then(|| value.split_whitespace().nth(1)?.parse::<u32>().ok())
                    .flatten()
            })
        })
        .unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Filesystem, SwapConfig};

    #[test]
    fn mount_root_is_created_and_unsafe_paths_are_rejected() {
        let base = std::env::temp_dir().join(format!(
            "artix-setup-mount-root-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir(&base).unwrap();
        let empty = base.join("empty");
        ensure_empty_mount_root(&empty).unwrap();
        assert!(empty.is_dir());

        fs::write(empty.join("existing-file"), "do not cover").unwrap();
        assert!(ensure_empty_mount_root(&empty).is_err());

        let target = base.join("target");
        fs::create_dir(&target).unwrap();
        let link = base.join("link");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(ensure_empty_mount_root(&link).is_err());

        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn generated_fstab_requires_uuid_root_and_efi_entries() {
        let valid = "UUID=11111111-1111-1111-1111-111111111111 / ext4 rw,relatime 0 1\nUUID=22222222-2222-2222-2222-222222222222 /boot vfat rw,umask=0077 0 2\n";
        assert!(validate_generated_fstab(valid).is_ok());
        assert!(validate_generated_fstab(
            "UUID=11111111-1111-1111-1111-111111111111 / ext4 defaults 0 1\nUUID=1234-ABCD /boot vfat defaults 0 2\\n"
        )
        .is_ok());
        assert!(validate_generated_fstab(
            "UUID=1234-ABCD / ext4 defaults 0 1\\nUUID=1234-ABCD /boot vfat defaults 0 2\\n"
        )
        .is_err());
        assert!(
            validate_generated_fstab(
                "UUID=11111111-1111-1111-1111-111111111111 / ext4 defaults 0 1\\n"
            )
            .is_err()
        );
        assert!(validate_generated_fstab(
            "UUID= / ext4 defaults 0 1\nUUID=22222222-2222-2222-2222-222222222222 /boot vfat defaults 0 2\n"
        )
        .is_err());
        assert!(validate_generated_fstab(
            "UUID=not-a-uuid / ext4 defaults 0 1\nUUID=22222222-2222-2222-2222-222222222222 /boot vfat defaults 0 2\n"
        )
        .is_err());
        assert!(validate_generated_fstab(
            "UUID=11111111-1111-1111-1111-111111111111 / ext4 defaults 0 1\nUUID=22222222-2222-2222-2222-222222222222 /boot vfat defaults 0 2 extra\n"
        )
        .is_err());
        assert!(
            validate_generated_fstab(
                "UUID=11111111-1111-1111-1111-111111111111 / ext4 defaults 0 1\nUUID=22222222-2222-2222-2222-222222222222 /boot ext4 defaults 0 2\n"
            )
            .is_err()
        );
        assert!(
            validate_generated_fstab(
                "/dev/vda2 / ext4 defaults 0 1\nUUID=22222222-2222-2222-2222-222222222222 /boot vfat defaults 0 2\n"
            )
            .is_err()
        );
        assert!(validate_generated_fstab("UUID=11111111-1111-1111-1111-111111111111 / ext4 defaults 0 1\nUUID=22222222-2222-2222-2222-222222222222 / ext4 defaults 0 2\nUUID=33333333-3333-3333-3333-333333333333 /boot vfat defaults 0 2\n").is_err());
    }

    #[test]
    fn mountinfo_parser_extracts_mountpoint_and_source_and_unescapes_spaces() {
        let sample = "36 25 8:2 / /mnt/my\\040root rw,relatime shared:1 - ext4 /dev/vda2 rw\n";
        assert_eq!(
            parse_mountinfo(sample).unwrap(),
            vec![("/mnt/my root".to_string(), "/dev/vda2".to_string())]
        );
    }

    #[test]
    fn mountinfo_parser_fails_closed_on_empty_or_malformed_rows() {
        assert!(parse_mountinfo("").is_err());
        assert!(parse_mountinfo("not a mountinfo row\n").is_err());

        let valid = "36 25 8:2 / /mnt rw,relatime - ext4 /dev/vda2 rw\n";
        let malformed_after_valid = format!("{valid}broken row\n");
        assert!(parse_mountinfo(&malformed_after_valid).is_err());

        let invalid_device_number = "36 25 unknown / /mnt rw,relatime - ext4 /dev/vda2 rw\n";
        assert!(parse_mountinfo(invalid_device_number).is_err());

        let incomplete_filesystem = "36 25 8:2 / /mnt rw,relatime - ext4\n";
        assert!(parse_mountinfo(incomplete_filesystem).is_err());
    }

    #[test]
    fn mount_source_lookup_rejects_missing_or_ambiguous_mountpoints() {
        let entries = vec![
            ("/mnt".to_string(), "/dev/vda2".to_string()),
            ("/mnt/boot".to_string(), "/dev/vda1".to_string()),
        ];
        assert_eq!(unique_mount_source(&entries, "/mnt").unwrap(), "/dev/vda2");
        assert!(unique_mount_source(&entries, "/missing").is_err());

        let stacked = vec![
            ("/mnt".to_string(), "/dev/vda2".to_string()),
            ("/mnt".to_string(), "/dev/vdb2".to_string()),
        ];
        assert!(unique_mount_source(&stacked, "/mnt").is_err());
    }

    #[test]
    fn first_partition_path_selects_efi_and_root_path_selects_last_partition() {
        let dump = "label: gpt\n/dev/vda1 : start=2048, size=2097152, type=C12A7328-F81F-11D2-BA4B-00A0C93EC93B\n/dev/vda2 : start=2099200, size=91852800, type=0FC63DAF-8483-4772-8E79-3D69D8477DE4\n";
        assert_eq!(first_partition_path(dump).unwrap(), Path::new("/dev/vda1"));
        assert_eq!(root_partition_path(dump).unwrap(), Path::new("/dev/vda2"));
        assert!(first_partition_path("label: gpt\n").is_err());
    }

    #[test]
    fn root_format_config_allows_only_unencrypted_automatic_ext4() {
        let mut config = InstallConfig::default();
        assert!(validate_root_format_config(&config).is_ok());
        config.disk.encryption = true;
        assert!(validate_root_format_config(&config).is_err());
        config.disk.encryption = false;
        config.disk.root_filesystem = Filesystem::Btrfs;
        assert!(validate_root_format_config(&config).is_err());
        config.disk.root_filesystem = Filesystem::Ext4;
        config.disk.partitioning = Partitioning::Manual;
        assert!(validate_root_format_config(&config).is_err());
    }

    #[test]
    fn root_partition_path_uses_final_partition_and_rejects_empty_dump() {
        let dump = "label: gpt\n/dev/vda1 : start=2048, size=2097152, type=C12A7328-F81F-11D2-BA4B-00A0C93EC93B\n/dev/vda2 : start=2099200, size=91852800, type=0FC63DAF-8483-4772-8E79-3D69D8477DE4\n";
        assert_eq!(root_partition_path(dump).unwrap(), Path::new("/dev/vda2"));
        assert!(root_partition_path("label: gpt\n").is_err());
    }

    #[test]
    fn uefi_layout_writes_gpt_efi_and_remaining_root() {
        let config = InstallConfig::default();
        let script = build_sfdisk_script(&config).unwrap();
        assert!(script.starts_with("label: gpt\nlabel-id: "));
        assert!(script.contains("size=+1G, type=C12A7328-F81F-11D2-BA4B-00A0C93EC93B"));
        assert!(script.contains("size=+, type=0FC63DAF-8483-4772-8E79-3D69D8477DE4"));
        assert!(!script.contains("type=82"));
    }

    #[test]
    fn bios_layout_writes_dos_and_bootable_root() {
        let mut config = InstallConfig::default();
        config.system.firmware = FirmwareMode::Bios;
        let script = build_sfdisk_script(&config).unwrap();
        assert_eq!(script, "label: dos\nsize=+, type=83, bootable\n");
    }

    #[test]
    fn partition_swap_is_inserted_before_root() {
        let mut config = InstallConfig::default();
        config.disk.swap.mode = SwapMode::Partition;
        config.disk.swap.size_gib = 8;
        let script = build_sfdisk_script(&config).unwrap();
        let swap = script.find("size=+8G").unwrap();
        let root = script.find("size=+, type=0FC63").unwrap();
        assert!(swap < root);
    }

    #[test]
    fn unsupported_partitioning_modes_are_refused() {
        let mut config = InstallConfig::default();
        config.disk.partitioning = Partitioning::Manual;
        assert!(build_sfdisk_script(&config).is_err());
    }

    #[test]
    fn bios_swap_partition_uses_mbr_swap_type_before_bootable_root() {
        let mut config = InstallConfig::default();
        config.system.firmware = FirmwareMode::Bios;
        config.disk.swap.mode = SwapMode::Partition;
        config.disk.swap.size_gib = 6;
        let script = build_sfdisk_script(&config).unwrap();
        assert_eq!(
            script,
            "label: dos\nsize=+6G, type=82\nsize=+, type=83, bootable\n"
        );
    }

    #[test]
    fn capacity_check_reserves_a_minimum_root() {
        let config = InstallConfig::default();
        assert!(validate_capacity(&config, 8 * GIB).is_err());
        assert!(validate_capacity(&config, 9 * GIB).is_err());
        assert!(validate_capacity(&config, 9 * GIB + PARTITION_ALIGNMENT_MARGIN_BYTES).is_ok());
    }

    #[test]
    fn path_confirmation_is_exact_and_not_a_prefix() {
        assert!(validate_confirmation(Path::new("/dev/sda"), "/dev/sda").is_ok());
        assert!(validate_confirmation(Path::new("/dev/sda"), "/dev/sda1").is_err());
        assert!(validate_confirmation(Path::new("/dev/sda"), "/dev/sd").is_err());
    }

    #[test]
    fn filesystem_config_does_not_change_partition_table_script() {
        let mut config = InstallConfig::default();
        config.disk.root_filesystem = Filesystem::Btrfs;
        assert_eq!(build_sfdisk_script(&config).unwrap().lines().count(), 4);
    }

    #[test]
    fn readback_verification_accepts_the_expected_uefi_layout() {
        let config = InstallConfig::default();
        let dump = "label: gpt\n/dev/vda1 : start= 2048, size= 2097152, type=C12A7328-F81F-11D2-BA4B-00A0C93EC93B\n/dev/vda2 : start= 2099200, size= 91852800, type=0FC63DAF-8483-4772-8E79-3D69D8477DE4\n";
        assert!(verify_partition_table_dump(&config, dump).is_ok());
    }

    #[test]
    fn readback_verification_rejects_wrong_partition_type_and_extra_partition() {
        let config = InstallConfig::default();
        let wrong_type = "label: gpt\n/dev/vda1 : start= 2048, size= 2097152, type=0FC63DAF-8483-4772-8E79-3D69D8477DE4\n/dev/vda2 : start= 2099200, size= 91852800, type=0FC63DAF-8483-4772-8E79-3D69D8477DE4\n";
        assert!(verify_partition_table_dump(&config, wrong_type).is_err());
        let extra = format!(
            "{wrong_type}/dev/vda3 : start= 93952000, size= 100, type=0FC63DAF-8483-4772-8E79-3D69D8477DE4\n"
        );
        assert!(verify_partition_table_dump(&config, &extra).is_err());
    }

    #[test]
    fn readback_verification_checks_bios_swap_layout() {
        let mut config = InstallConfig::default();
        config.system.firmware = FirmwareMode::Bios;
        config.disk.swap.mode = SwapMode::Partition;
        config.disk.swap.size_gib = 4;
        let dump = "label: dos\n/dev/vda1 : start= 2048, size= 8388608, type=82\n/dev/vda2 : start= 8390656, size= 1000000, type=83\n";
        assert!(verify_partition_table_dump(&config, dump).is_ok());
    }

    #[test]
    fn generated_gpt_layout_is_accepted_by_sfdisk_on_a_sparse_image() {
        if Command::new("sfdisk").arg("--version").output().is_err() {
            return;
        }
        let image =
            std::env::temp_dir().join(format!("artix-setup-gpt-image-{}.img", std::process::id()));
        let file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&image)
            .unwrap();
        file.set_len(20 * GIB).unwrap();
        drop(file);

        let script = build_sfdisk_script(&InstallConfig::default()).unwrap();
        let mut child = Command::new("sfdisk")
            .args(["--wipe", "always", "--wipe-partitions", "always", "--"])
            .arg(&image)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(script.as_bytes())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "sfdisk rejected generated GPT script: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        let dump = Command::new("sfdisk")
            .arg("--dump")
            .arg(&image)
            .output()
            .unwrap();
        assert!(dump.status.success());
        let dump = String::from_utf8_lossy(&dump.stdout);
        assert!(dump.contains("label: gpt"));
        assert!(dump.contains("C12A7328-F81F-11D2-BA4B-00A0C93EC93B"));
        assert!(dump.contains("0FC63DAF-8483-4772-8E79-3D69D8477DE4"));
        let _ = fs::remove_file(image);
    }

    #[test]
    fn explicit_swap_settings_are_serialized_without_shell_interpolation() {
        let mut config = InstallConfig::default();
        config.disk.swap = SwapConfig {
            mode: SwapMode::Partition,
            size_gib: 4,
            zswap: false,
        };
        assert!(build_sfdisk_script(&config).unwrap().contains("size=+4G"));
    }
}
