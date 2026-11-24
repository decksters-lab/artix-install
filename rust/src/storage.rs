use std::{
    fs,
    io::Write,
    os::unix::fs::FileTypeExt,
    path::Path,
    process::{Command, Stdio},
};

use anyhow::{Context, Result, bail};

use crate::{
    config::{FirmwareMode, InstallConfig, Partitioning, SwapMode},
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
