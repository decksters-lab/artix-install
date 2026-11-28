//! Guarded post-installation identity and regional configuration.
use std::{
    fs::{self, OpenOptions},
    os::unix::fs::{FileTypeExt, OpenOptionsExt},
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result, bail};

use crate::config::{Filesystem, FirmwareMode, InstallConfig, Partitioning, SwapMode};

const TARGET: &str = "/mnt";

pub fn configure_system(config: &InstallConfig) -> Result<String> {
    config.validate().map_err(|issues| {
        anyhow::anyhow!("configuration validation failed:\n- {}", issues.join("\n- "))
    })?;
    if effective_uid()? != 0 {
        bail!("system configuration requires effective UID 0");
    }
    if config.disk.partitioning != Partitioning::Automatic
        || config.system.firmware != FirmwareMode::Uefi
        || config.disk.root_filesystem != Filesystem::Ext4
        || config.disk.encryption
        || config.disk.swap.mode == SwapMode::Partition
    {
        bail!("system configuration currently supports automatic, unencrypted UEFI/ext4 installs without a swap partition");
    }

    let disk = config.disk.target.as_deref().context("select a target disk before configuring the installed system")?;
    let table = Command::new("sfdisk")
        .arg("--dump")
        .arg(disk)
        .output()
        .context("sfdisk is required to verify the installed target")?;
    if !table.status.success() {
        bail!("could not read the configured target partition table");
    }
    let table = String::from_utf8(table.stdout).context("sfdisk returned non-UTF-8 partition data")?;
    if table.lines().find_map(|line| line.trim().strip_prefix("label:")).map(str::trim) != Some("gpt") {
        bail!("configured target does not have the expected GPT partition table");
    }
    let partitions: Vec<PathBuf> = table.lines()
        .filter(|line| line.trim_start().starts_with("/dev/"))
        .filter_map(|line| line.split_once(':').map(|(path, _)| PathBuf::from(path.trim())))
        .collect();
    if partitions.len() != 2 {
        bail!("configured target has {} partitions; expected exactly two", partitions.len());
    }
    let efi = fs::canonicalize(&partitions[0]).context("resolve target EFI partition")?;
    let root = fs::canonicalize(&partitions[1]).context("resolve target root partition")?;
    for (device, label) in [(&efi, "EFI"), (&root, "root")] {
        if !fs::metadata(device).with_context(|| format!("inspect {label} partition"))?.file_type().is_block_device() {
            bail!("{label} partition is not a block device");
        }
    }
    verify_mount("/mnt", &root)?;
    verify_mount("/mnt/boot", &efi)?;

    let etc = Path::new(TARGET).join("etc");
    let etc_meta = fs::symlink_metadata(&etc).context("installed /etc directory is missing")?;
    if !etc_meta.file_type().is_dir() {
        bail!("/mnt/etc is not a real directory");
    }
    if !etc.join("fstab").is_file() {
        bail!("installed /etc/fstab is missing; generate fstab before system configuration");
    }

    let zoneinfo_root = Path::new(TARGET).join("usr/share/zoneinfo");
    let zoneinfo_root = fs::canonicalize(&zoneinfo_root).context("installed timezone database is missing")?;
    let zoneinfo = fs::canonicalize(zoneinfo_root.join(&config.system.timezone))
        .with_context(|| format!("timezone '{}' is not installed in the target", config.system.timezone))?;
    if !zoneinfo.starts_with(&zoneinfo_root) || !zoneinfo.is_file() {
        bail!("selected timezone resolves outside the installed zoneinfo database");
    }

    let console_keymap = console_keymap(&config.system.keyboard_layout);
    verify_keymap(console_keymap)?;

    let locale_gen = etc.join("locale.gen");
    let locale_contents = fs::read_to_string(&locale_gen)
        .context("installed /etc/locale.gen is missing or unreadable")?;
    let updated_locale_gen = enable_locale(&locale_contents, &config.system.locale)?;

    let hostname = etc.join("hostname");
    let locale_conf = etc.join("locale.conf");
    let vconsole = etc.join("vconsole.conf");
    let hosts = etc.join("hosts");
    let localtime = etc.join("localtime");
    for path in [&hostname, &locale_conf, &vconsole, &hosts, &locale_gen] {
        ensure_regular_file_or_absent(path)?;
    }
    let timezone_link = PathBuf::from(format!("../usr/share/zoneinfo/{}", config.system.timezone));
    match fs::symlink_metadata(&localtime) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            if fs::read_link(&localtime).ok().as_ref() != Some(&timezone_link) {
                bail!("/mnt/etc/localtime already points elsewhere; refusing to replace it");
            }
        }
        Ok(_) => bail!("/mnt/etc/localtime already exists and is not the expected timezone symlink"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error).context("inspect installed /etc/localtime"),
    }

    atomic_write(&locale_gen, updated_locale_gen.as_bytes())?;
    atomic_write(&hostname, format!("{}\n", config.system.hostname).as_bytes())?;
    atomic_write(&locale_conf, format!("LANG={}\n", config.system.locale).as_bytes())?;
    atomic_write(&vconsole, format!("KEYMAP={}\n", config.system.keyboard_layout).as_bytes())?;
    if !localtime.exists() {
        std::os::unix::fs::symlink(&timezone_link, &localtime)
            .context("create installed /etc/localtime timezone symlink")?;
    }

    if !matches!(config.system.locale.as_str(), "C" | "POSIX" | "C.UTF-8" | "C.utf8") {
        let status = Command::new("chroot")
            .arg(TARGET)
            .arg("locale-gen")
            .status()
            .context("run locale-gen inside the installed system")?;
        if !status.success() {
            bail!("locale-gen failed inside /mnt; inspect /mnt/etc/locale.gen before retrying");
        }
    }

    Ok(format!(
        "Configured hostname '{}', timezone '{}', locale '{}', and console keymap '{}' in /mnt. The target was verified against its configured root and EFI partitions.",
        config.system.hostname, config.system.timezone, config.system.locale, config.system.keyboard_layout
    ))
}

fn verify_mount(mountpoint: &str, expected: &Path) -> Result<()> {
    let output = Command::new("findmnt")
        .args(["--noheadings", "--raw", "--output", "SOURCE", "--target", mountpoint])
        .output()
        .with_context(|| format!("findmnt is required to verify {mountpoint}"))?;
    if !output.status.success() {
        bail!("{mountpoint} is not mounted");
    }
    let source = String::from_utf8(output.stdout).context("findmnt returned non-UTF-8 source")?;
    let source = source.trim();
    let actual = fs::canonicalize(source)
        .with_context(|| format!("resolve mounted source '{source}' for {mountpoint}"))?;
    if actual != expected {
        bail!("{mountpoint} is mounted from {}, not the expected {}", actual.display(), expected.display());
    }
    Ok(())
}

fn console_keymap(layout: &str) -> &str {
    match layout {
        "us-intl" => "us",
        "cz-qwerty" => "cz-qwerty",
        "fr-bepo" => "fr-bepo",
        "br-abnt2" => "br-abnt2",
        "de-latin1" => "de-latin1",
        "jp106" => "jp106",
        other => other,
    }
}

fn ensure_directory(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_dir() => Ok(()),
        Ok(_) => bail!("{} exists but is not a real directory", path.display()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir(path).with_context(|| format!("create {}", path.display()))?;
            Ok(())
        }
        Err(error) => Err(error).with_context(|| format!("inspect {}", path.display())),
    }
}

fn verify_keymap(layout: &str) -> Result<()> {
    let root = Path::new(TARGET).join("usr/share/kbd/keymaps");
    if !root.is_dir() {
        bail!("installed console keymap database is missing");
    }
    let pattern = format!("{layout}.map*");
    let output = Command::new("find")
        .arg(&root)
        .args(["-type", "f", "-name"])
        .arg(&pattern)
        .output()
        .context("find is required to verify the selected console keymap")?;
    if !output.status.success() || output.stdout.is_empty() {
        bail!("console keymap '{}' is not present in the installed target", layout);
    }
    Ok(())
}

fn enable_locale(contents: &str, locale: &str) -> Result<String> {
    if matches!(locale, "C" | "POSIX" | "C.UTF-8" | "C.utf8") {
        return Ok(contents.to_owned());
    }
    let mut found = false;
    let mut output = String::with_capacity(contents.len() + 32);
    for line in contents.lines() {
        let uncommented = line.trim_start().trim_start_matches('#').trim_start();
        let fields: Vec<_> = uncommented.split_whitespace().collect();
        if fields.first().copied() == Some(locale) {
            if fields.get(1).copied() != Some("UTF-8") {
                bail!("locale '{}' exists in locale.gen but is not configured with UTF-8", locale);
            }
            if found {
                bail!("locale '{}' appears more than once in locale.gen", locale);
            }
            output.push_str(&format!("{locale} UTF-8\n"));
            found = true;
        } else {
            output.push_str(line);
            output.push('\n');
        }
    }
    if !found {
        bail!("locale '{}' is not listed in the installed /etc/locale.gen; refusing to invent a locale entry", locale);
    }
    Ok(output)
}

fn ensure_regular_file_or_absent(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_file() => Ok(()),
        Ok(_) => bail!("{} exists but is not a regular file; refusing to replace it", path.display()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("inspect {}", path.display())),
    }
}

fn atomic_write(path: &Path, contents: &[u8]) -> Result<()> {
    let parent = path.parent().context("configuration path has no parent directory")?;
    let temporary = parent.join(format!(".agsi-{}-{}.tmp", path.file_name().unwrap_or_default().to_string_lossy(), std::process::id()));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o644)
        .open(&temporary)
        .with_context(|| format!("create temporary file {}", temporary.display()))?;
    if let Err(error) = (|| -> Result<()> {
        use std::io::Write;
        file.write_all(contents).context("write temporary configuration")?;
        file.sync_all().context("sync temporary configuration")?;
        fs::rename(&temporary, path).with_context(|| format!("install {}", path.display()))?;
        fs::File::open(parent)?.sync_all().context("sync configuration directory")?;
        Ok(())
    })() {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }
    Ok(())
}

fn effective_uid() -> Result<u32> {
    let status = fs::read_to_string("/proc/self/status")
        .context("read /proc/self/status to verify effective UID")?;
    let uid_line = status.lines().find(|line| line.starts_with("Uid:"))
        .context("/proc/self/status has no Uid entry")?;
    uid_line.split_whitespace().nth(2)
        .context("effective UID is missing from /proc/self/status")?
        .parse::<u32>()
        .context("effective UID is not numeric")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enables_selected_locale_without_changing_other_entries() {
        let input = "#en_US.UTF-8 UTF-8\n#de_DE.UTF-8 UTF-8\n";
        let output = enable_locale(input, "en_US.UTF-8").unwrap();
        assert_eq!(output, "en_US.UTF-8 UTF-8\n#de_DE.UTF-8 UTF-8\n");
    }

    #[test]
    fn refuses_locale_missing_from_locale_gen() {
        assert!(enable_locale("#en_US.UTF-8 UTF-8\n", "fr_FR.UTF-8").is_err());
    }
}
