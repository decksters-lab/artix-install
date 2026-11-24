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
        anyhow::anyhow!("configuration validation failed:\n- {}", issues.join("\n- "))
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

    let metadata = fs::metadata(selected)
        .with_context(|| format!("inspect target {}", selected.display()))?;
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
        .args([
            "--wipe",
            "always",
            "--wipe-partitions",
            "always",
            "--",
        ])
        .arg(&canonical_target)
        .stdin(Stdio::piped())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .context("start sfdisk; ensure util-linux is installed")?;
    child
        .stdin
        .take()
        .context("open sfdisk standard input")?
        .write_all(script.as_bytes())
        .context("send partition layout to sfdisk")?;
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
        bail!("partition table was written, but sfdisk verification failed; do not continue to formatting");
    }

    let partprobe = Command::new("partprobe")
        .arg(&canonical_target)
        .status()
        .context("refresh the kernel partition table with partprobe")?;
    if !partprobe.success() {
        bail!("partition table was written and verified, but partprobe failed; reboot or resolve the kernel refresh before continuing");
    }
    let settle = Command::new("udevadm")
        .args(["settle", "--timeout=10"])
        .status()
        .context("wait for udev partition device nodes")?;
    if !settle.success() {
        bail!("partition table was written and kernel refreshed, but udev did not settle; inspect device nodes before continuing");
    }

    Ok(format!(
        "Partition table written and verified on {}. No filesystem was formatted, no encryption was initialized, and no filesystem was mounted. Review the new partition inventory before proceeding.",
        selected.display()
    ))
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
        bail!("installed sfdisk lacks the required explicit signature-wiping options; refusing to write");
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
            disk.selection_block_reason().unwrap_or("safety checks incomplete")
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
    let minimum = efi_bytes + swap_bytes + MIN_ROOT_GIB * GIB;
    if size_bytes < minimum {
        bail!(
            "target is too small: needs at least {} GiB for the planned EFI, swap and minimum root partitions",
            minimum / GIB
        );
    }
    Ok(())
}

pub fn build_sfdisk_script(config: &InstallConfig) -> Result<String> {
    config.validate().map_err(|issues| {
        anyhow::anyhow!("configuration validation failed:\n- {}", issues.join("\n- "))
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
        bail!("uuidgen returned an invalid GPT disk GUID");
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
    fn capacity_check_reserves_a_minimum_root() {
        let config = InstallConfig::default();
        assert!(validate_capacity(&config, 8 * GIB).is_err());
        assert!(validate_capacity(&config, 9 * GIB).is_ok());
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
        assert_eq!(
            build_sfdisk_script(&config).unwrap().lines().count(),
            3
        );
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
