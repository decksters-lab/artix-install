//! Guarded GRUB installation for the supported UEFI target.
use std::{
    fs::{self, OpenOptions},
    os::unix::fs::{FileTypeExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    process::{Command, Output},
};

use anyhow::{Context, Result, bail};

use crate::config::{
    Bootloader, Filesystem, FirmwareMode, InstallConfig, Partitioning, SwapMode,
};

const TARGET: &str = "/mnt";
const EFI_VENDOR: &str = "Artix";
const GRUB_MARKER: &str = "agsi-grub-uefi-managed";

pub fn install_grub_uefi(config: &InstallConfig) -> Result<String> {
    config.validate().map_err(|issues| {
        anyhow::anyhow!(
            "configuration validation failed:\n- {}",
            issues.join("\n- ")
        )
    })?;
    if effective_uid()? != 0 {
        bail!("bootloader installation requires effective UID 0");
    }
    if config.software.bootloader != Bootloader::Grub
        || config.system.firmware != FirmwareMode::Uefi
    {
        bail!("this stage currently supports GRUB on UEFI systems only; no boot files were changed");
    }
    if config.disk.partitioning != Partitioning::Automatic
        || config.disk.root_filesystem != Filesystem::Ext4
        || config.disk.encryption
        || config.disk.swap.mode == SwapMode::Partition
    {
        bail!("GRUB installation currently supports automatic, unencrypted UEFI/ext4 installs without a swap partition");
    }

    let selected_disk = config
        .disk
        .target
        .as_deref()
        .context("select a target disk before installing GRUB")?;
    let disk = fs::canonicalize(selected_disk)
        .with_context(|| format!("resolve configured disk {}", selected_disk.display()))?;
    if !fs::metadata(&disk)
        .with_context(|| format!("inspect target disk {}", disk.display()))?
        .file_type()
        .is_block_device()
    {
        bail!("configured target {} is not a block device", disk.display());
    }

    let table = Command::new("sfdisk")
        .arg("--dump")
        .arg(&disk)
        .output()
        .context("sfdisk is required to verify the installed target")?;
    if !table.status.success() {
        bail!("could not read the configured target partition table");
    }
    let table =
        String::from_utf8(table.stdout).context("sfdisk returned non-UTF-8 partition data")?;
    if table
        .lines()
        .find_map(|line| line.trim().strip_prefix("label:"))
        .map(str::trim)
        != Some("gpt")
    {
        bail!("configured target does not have the expected GPT partition table");
    }
    let partitions: Vec<PathBuf> = table
        .lines()
        .filter(|line| line.trim_start().starts_with("/dev/"))
        .filter_map(|line| {
            line.split_once(':')
                .map(|(path, _)| PathBuf::from(path.trim()))
        })
        .collect();
    if partitions.len() != 2 {
        bail!(
            "configured target has {} partitions; expected exactly two",
            partitions.len()
        );
    }
    let efi = fs::canonicalize(&partitions[0]).context("resolve target EFI partition")?;
    let root = fs::canonicalize(&partitions[1]).context("resolve target root partition")?;
    for (device, label) in [(&efi, "EFI"), (&root, "root")] {
        if !fs::metadata(device)
            .with_context(|| format!("inspect {label} partition"))?
            .file_type()
            .is_block_device()
        {
            bail!("{label} partition is not a block device");
        }
    }
    verify_blkid_type(&efi, "vfat")?;
    verify_blkid_type(&root, "ext4")?;
    verify_mount("/mnt", &root)?;
    verify_mount("/mnt/boot", &efi)?;

    let etc = Path::new(TARGET).join("etc");
    verify_real_directory(&etc)?;
    for required in ["fstab", "hostname", "default/grub"] {
        let path = etc.join(required);
        ensure_regular_file(&path)?;
    }
    let marker = etc.join(GRUB_MARKER);
    ensure_regular_or_absent(&marker)?;

    let boot = Path::new(TARGET).join("boot");
    verify_real_directory(&boot)?;
    verify_kernel_initramfs_pairs(&boot)?;

    let efi_root = boot.join("EFI");
    ensure_directory_or_absent(&efi_root)?;
    let vendor_dir = efi_root.join(EFI_VENDOR);
    let vendor_existed = directory_exists(&vendor_dir)?;
    if vendor_existed {
        verify_real_directory(&vendor_dir)?;
        let vendor_marker = vendor_dir.join(GRUB_MARKER);
        if !vendor_marker.is_file() {
            bail!(
                "/boot/EFI/{EFI_VENDOR} already exists without the AGSI marker; refusing to overwrite another bootloader"
            );
        }
        ensure_regular_file(&vendor_marker)?;
    }

    let grub_cfg = boot.join("grub/grub.cfg");
    let grub_cfg_existed = path_exists(&grub_cfg)?;
    if grub_cfg_existed && !marker.is_file() {
        bail!("/boot/grub/grub.cfg already exists without the AGSI marker; refusing to overwrite it");
    }
    if grub_cfg_existed {
        ensure_regular_file(&grub_cfg)?;
    }
    let grub_dir = boot.join("grub");
    ensure_directory_or_absent(&grub_dir)?;

    let fallback_dir = efi_root.join("BOOT");
    let fallback_existed = path_exists(&fallback_dir.join("BOOTX64.EFI"))?;
    ensure_directory_or_absent(&fallback_dir)?;
    if fallback_existed {
        ensure_regular_file(&fallback_dir.join("BOOTX64.EFI"))?;
    }

    for tool in ["grub-install", "grub-mkconfig"] {
        let output = Command::new("chroot")
            .arg(TARGET)
            .args([tool, "--version"])
            .output()
            .with_context(|| format!("check that {tool} is installed in the target"))?;
        if !output.status.success() {
            bail!("{tool} is not installed or failed its version check inside /mnt");
        }
    }

    let mounted = mount_chroot_pseudofilesystems()?;
    let install_result = (|| -> Result<()> {
        run_chroot(&[
            "grub-install",
            "--target=x86_64-efi",
            "--efi-directory=/boot",
            "--bootloader-id=Artix",
            "--no-nvram",
            "--recheck",
        ])?;
        run_chroot(&["grub-mkconfig", "-o", "/boot/grub/grub.cfg"])?;
        Ok(())
    })();
    let unmount_result = unmount_chroot_pseudofilesystems(&mounted);
    if let Err(error) = install_result {
        if !vendor_existed {
            let _ = fs::remove_dir_all(&vendor_dir);
        }
        if !grub_cfg_existed {
            let _ = fs::remove_file(&grub_cfg);
        }
        unmount_result?;
        return Err(error);
    }
    unmount_result?;

    let vendor_loader = vendor_dir.join("grubx64.efi");
    ensure_nonempty_file(&vendor_loader)?;
    ensure_nonempty_file(&grub_cfg)?;
    write_marker(&vendor_dir.join(GRUB_MARKER))?;

    let fallback_loader = fallback_dir.join("BOOTX64.EFI");
    let fallback_status = if fallback_existed {
        "preserved the existing EFI fallback loader"
    } else {
        ensure_real_directory(&fallback_dir)?;
        fs::copy(&vendor_loader, &fallback_loader)
            .context("copy GRUB to the UEFI fallback boot path")?;
        write_marker(&fallback_dir.join(GRUB_MARKER))?;
        "installed the UEFI fallback loader"
    };
    write_marker(&marker)?;

    let efi_part_number = partition_number(&efi)?;
    let nvram_status = register_nvram_entry(&disk, &efi_part_number);

    Ok(format!(
        "Installed GRUB for UEFI on {} (EFI partition {}). Generated /boot/grub/grub.cfg, {}. {}",
        disk.display(),
        efi_part_number,
        fallback_status,
        nvram_status
    ))
}

fn register_nvram_entry(disk: &Path, partition: &str) -> String {
    let efivars = Path::new("/sys/firmware/efi/efivars");
    if !efivars.is_dir() {
        return "UEFI NVRAM is unavailable in the live environment; the fallback loader remains available.".into();
    }
    let existing = Command::new("chroot")
        .arg(TARGET)
        .args(["efibootmgr", "-v"])
        .output();
    if let Ok(output) = existing {
        let text = String::from_utf8_lossy(&output.stdout);
        if text.contains("\\EFI\\Artix\\grubx64.efi") {
            return "An Artix UEFI NVRAM entry already exists.".into();
        }
    }
    let disk = disk.to_string_lossy();
    let status = Command::new("chroot")
        .arg(TARGET)
        .args([
            "efibootmgr",
            "--create",
            "--disk",
            &disk,
            "--part",
            partition,
            "--label",
            "Artix Linux",
            "--loader",
            "\\EFI\\Artix\\grubx64.efi",
        ])
        .status();
    if matches!(status, Ok(status) if status.success()) {
        "Created an Artix UEFI NVRAM boot entry.".into()
    } else {
        "Could not create a UEFI NVRAM entry; the fallback loader remains available.".into()
    }
}

fn partition_number(partition: &Path) -> Result<String> {
    let name = partition
        .file_name()
        .context("EFI partition path has no device name")?
        .to_str()
        .context("EFI partition device name is not UTF-8")?;
    fs::read_to_string(format!("/sys/class/block/{name}/partition"))
        .with_context(|| format!("read partition number for {name}"))
        .map(|number| number.trim().to_owned())
}

fn verify_kernel_initramfs_pairs(boot: &Path) -> Result<()> {
    let mut kernels = Vec::new();
    for entry in fs::read_dir(boot).context("read installed /boot directory")? {
        let entry = entry.context("inspect installed /boot entry")?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if let Some(kernel) = name.strip_prefix("vmlinuz-") {
            if entry
                .file_type()
                .context("inspect kernel image type")?
                .is_file()
            {
                kernels.push(kernel.to_owned());
            }
        }
    }
    if kernels.is_empty() {
        bail!("no vmlinuz-* kernel image exists in /mnt/boot");
    }
    for kernel in &kernels {
        let initramfs = boot.join(format!("initramfs-{kernel}.img"));
        if !initramfs.is_file() || fs::metadata(&initramfs)?.len() == 0 {
            bail!(
                "kernel {kernel} has no non-empty matching initramfs in /mnt/boot"
            );
        }
    }
    Ok(())
}

fn verify_blkid_type(device: &Path, expected: &str) -> Result<()> {
    let output = Command::new("blkid")
        .args(["-p", "-s", "TYPE", "-o", "value"])
        .arg(device)
        .output()
        .with_context(|| format!("inspect filesystem on {}", device.display()))?;
    let actual = String::from_utf8_lossy(&output.stdout)
        .trim()
        .to_ascii_lowercase();
    if !output.status.success() || actual != expected {
        bail!(
            "expected {expected} filesystem on {}, found {}",
            device.display(),
            if actual.is_empty() { "unknown" } else { &actual }
        );
    }
    Ok(())
}

fn verify_mount(mountpoint: &str, expected: &Path) -> Result<()> {
    let output = Command::new("findmnt")
        .args([
            "--noheadings",
            "--raw",
            "--output",
            "SOURCE",
            "--mountpoint",
            mountpoint,
        ])
        .output()
        .with_context(|| format!("findmnt is required to verify {mountpoint}"))?;
    if !output.status.success() {
        bail!("{mountpoint} is not an exact mountpoint");
    }
    let source = String::from_utf8(output.stdout).context("findmnt returned non-UTF-8 source")?;
    let actual = fs::canonicalize(source.trim())
        .with_context(|| format!("resolve mounted source for {mountpoint}"))?;
    if actual != expected {
        bail!(
            "{mountpoint} is mounted from {}, not expected {}",
            actual.display(),
            expected.display()
        );
    }
    Ok(())
}

fn mount_chroot_pseudofilesystems() -> Result<Vec<PathBuf>> {
    let mut mounted = Vec::new();
    for (source, target) in [
        ("/dev", "/mnt/dev"),
        ("/proc", "/mnt/proc"),
        ("/sys", "/mnt/sys"),
    ] {
        let target_path = Path::new(target);
        verify_real_directory(target_path)?;
        if is_mountpoint(target)? {
            unmount_chroot_pseudofilesystems(&mounted)?;
            bail!("{target} is already a mountpoint; refusing to replace it");
        }
        let status = Command::new("mount")
            .args(["--rbind", source, target])
            .status()
            .with_context(|| format!("bind-mount {source} into the target"))?;
        if !status.success() {
            unmount_chroot_pseudofilesystems(&mounted)?;
            bail!("could not bind-mount {source} into {target}");
        }
        mounted.push(target_path.to_path_buf());
        let status = Command::new("mount")
            .args(["--make-rslave", target])
            .status()
            .with_context(|| format!("make {target} a slave mount"))?;
        if !status.success() {
            unmount_chroot_pseudofilesystems(&mounted)?;
            bail!("could not make {target} a slave mount");
        }
    }
    Ok(mounted)
}

fn unmount_chroot_pseudofilesystems(mounted: &[PathBuf]) -> Result<()> {
    for target in mounted.iter().rev() {
        let status = Command::new("umount")
            .arg("--recursive")
            .arg(target)
            .status()
            .with_context(|| format!("unmount {}", target.display()))?;
        if !status.success() {
            bail!("could not unmount {}; inspect the live mount table before continuing", target.display());
        }
    }
    Ok(())
}

fn is_mountpoint(path: &str) -> Result<bool> {
    let output = Command::new("findmnt")
        .args(["--noheadings", "--mountpoint", path])
        .output()
        .with_context(|| format!("check whether {path} is already mounted"))?;
    Ok(output.status.success())
}

fn run_chroot(arguments: &[&str]) -> Result<()> {
    let status = Command::new("chroot")
        .arg(TARGET)
        .args(arguments)
        .status()
        .with_context(|| format!("run {} inside the installed target", arguments[0]))?;
    if !status.success() {
        bail!("{} failed inside /mnt", arguments[0]);
    }
    Ok(())
}

fn ensure_directory_or_absent(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_dir() => Ok(()),
        Ok(_) => bail!("{} exists but is not a real directory", path.display()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("inspect {}", path.display())),
    }
}

fn ensure_real_directory(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_dir() => Ok(()),
        Ok(_) => bail!("{} exists but is not a real directory", path.display()),
        Err(error) => Err(error).with_context(|| format!("inspect {}", path.display())),
    }
}

fn ensure_regular_file(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("required file {} is missing", path.display()))?;
    if !metadata.file_type().is_file() {
        bail!("{} is not a regular file", path.display());
    }
    Ok(())
}

fn ensure_regular_or_absent(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_file() => Ok(()),
        Ok(_) => bail!("{} exists but is not a regular file", path.display()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("inspect {}", path.display())),
    }
}

fn ensure_nonempty_file(path: &Path) -> Result<()> {
    ensure_regular_file(path)?;
    if fs::metadata(path)
        .with_context(|| format!("inspect {}", path.display()))?
        .len()
        == 0
    {
        bail!("{} is empty", path.display());
    }
    Ok(())
}

fn directory_exists(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_dir() => Ok(true),
        Ok(_) => bail!("{} exists but is not a real directory", path.display()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error).with_context(|| format!("inspect {}", path.display())),
    }
}

fn path_exists(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error).with_context(|| format!("inspect {}", path.display())),
    }
}

fn write_marker(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_file() => return Ok(()),
        Ok(_) => bail!("AGSI marker {} is not a regular file", path.display()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error).with_context(|| format!("inspect {}", path.display())),
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o644)
        .open(path)
        .with_context(|| format!("create AGSI marker {}", path.display()))?;
    use std::io::Write;
    file.write_all(b"Managed by AGSI.\n")
        .with_context(|| format!("write AGSI marker {}", path.display()))?;
    file.sync_all()
        .with_context(|| format!("sync AGSI marker {}", path.display()))?;
    Ok(())
}

fn effective_uid() -> Result<u32> {
    let status = fs::read_to_string("/proc/self/status")
        .context("read /proc/self/status to verify effective UID")?;
    status
        .lines()
        .find(|line| line.starts_with("Uid:"))
        .and_then(|line| line.split_whitespace().nth(2))
        .context("effective UID is missing from /proc/self/status")?
        .parse::<u32>()
        .context("effective UID is not numeric")
}
