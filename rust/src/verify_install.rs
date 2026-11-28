//! Read-only verification of the supported completed Artix target.
use std::{
    fs,
    os::unix::fs::{FileTypeExt, PermissionsExt},
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result, bail};

use crate::config::{
    Bootloader, Desktop, Filesystem, FirmwareMode, InitSystem, InstallConfig, NetworkStack,
    Partitioning, PrivilegeEscalation, SwapMode, UserServices,
};

const TARGET: &str = "/mnt";
const GRUB_MARKER: &str = "agsi-grub-uefi-managed";

pub fn verify_installation(config: &InstallConfig) -> Result<String> {
    config.validate().map_err(|issues| {
        anyhow::anyhow!(
            "configuration validation failed:\n- {}",
            issues.join("\n- ")
        )
    })?;
    if effective_uid()? != 0 {
        bail!("installation verification requires effective UID 0");
    }
    if config.disk.partitioning != Partitioning::Automatic
        || config.disk.root_filesystem != Filesystem::Ext4
        || config.disk.encryption
        || config.disk.swap.mode != SwapMode::None
        || !config.disk.additional_mounts.is_empty()
    {
        bail!(
            "read-only verification currently supports automatic, unencrypted UEFI/ext4 installs with no swap or additional mounts"
        );
    }
    if config.system.firmware != FirmwareMode::Uefi
        || config.system.init_system != InitSystem::Dinit
        || config.software.bootloader != Bootloader::Grub
    {
        bail!("complete verification currently supports UEFI, Dinit, and GRUB only");
    }

    let disk = config
        .disk
        .target
        .as_deref()
        .context("select the installed target disk before verification")?;
    let disk = fs::canonicalize(disk).context("resolve configured target disk")?;
    if !fs::metadata(&disk)?.file_type().is_block_device() {
        bail!("configured target {} is not a block device", disk.display());
    }
    let table = Command::new("sfdisk")
        .arg("--dump")
        .arg(&disk)
        .output()
        .context("sfdisk is required to verify the target partition table")?;
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
        bail!("target disk does not have the expected GPT partition table");
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
            "target disk has {} partitions; expected exactly two",
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
    verify_filesystem(&efi, "vfat")?;
    verify_filesystem(&root, "ext4")?;
    verify_mount("/mnt", &root)?;
    verify_mount("/mnt/boot", &efi)?;
    let root_uuid = filesystem_uuid(&root)?;
    let efi_uuid = filesystem_uuid(&efi)?;
    verify_fstab(&Path::new(TARGET).join("etc/fstab"), &root_uuid, &efi_uuid)?;

    let etc = Path::new(TARGET).join("etc");
    verify_system_identity(config, &etc)?;
    verify_user_and_privileges(config, &etc)?;
    verify_kernel_images(&Path::new(TARGET).join("boot"))?;
    verify_dinit_services(config, &etc)?;
    verify_grub_uefi(&etc, &Path::new(TARGET).join("boot"))?;
    for mountpoint in ["/mnt/dev", "/mnt/proc", "/mnt/sys"] {
        if is_mountpoint(mountpoint)? {
            bail!(
                "{mountpoint} is still mounted from the installer; unmount it before final verification"
            );
        }
    }

    Ok(format!(
        "ARTIX INSTALL VERIFICATION\n===========================\n[PASS] GPT target disk and exact root/EFI mount sources\n[PASS] ext4 root, FAT EFI, and UUID-based fstab entries\n[PASS] hostname, timezone, locale, and keyboard configuration\n[PASS] target user, password state, wheel access, and privilege policy\n[PASS] kernel/initramfs pairs and Dinit service enablement\n[PASS] UEFI GRUB loader and generated menu\n[PASS] no installer bind mounts remain under /mnt\n\nStatic verification passed. This is not a boot test; reboot the VM to verify firmware handoff and a successful Artix startup."
    ))
}

fn verify_system_identity(config: &InstallConfig, etc: &Path) -> Result<()> {
    ensure_real_directory(etc)?;
    let hostname = read_regular_file(&etc.join("hostname"))?;
    if hostname.trim() != config.system.hostname {
        bail!(
            "installed hostname '{}' does not match configured hostname '{}'",
            hostname.trim(),
            config.system.hostname
        );
    }

    let locale_conf = read_regular_file(&etc.join("locale.conf"))?;
    if !locale_conf
        .lines()
        .any(|line| line.trim() == format!("LANG={}", config.system.locale))
    {
        bail!("installed /etc/locale.conf does not set the configured LANG value");
    }

    let special_locale = matches!(
        config.system.locale.as_str(),
        "C" | "POSIX" | "C.UTF-8" | "C.utf8"
    );
    if !special_locale {
        let locale_gen = read_regular_file(&etc.join("locale.gen"))?;
        if !locale_gen.lines().any(|line| {
            let fields: Vec<_> = line.split_whitespace().collect();
            !line.trim_start().starts_with('#')
                && fields.first().copied() == Some(config.system.locale.as_str())
                && fields.get(1).copied() == Some("UTF-8")
        }) {
            bail!("configured locale is not enabled in /etc/locale.gen");
        }
    }
    let locales = run_chroot_output(&["locale", "-a"])?;
    let locale_output = String::from_utf8_lossy(&locales.stdout);
    let expected_locale = normalize_locale(&config.system.locale);
    if !locale_output
        .lines()
        .any(|locale| normalize_locale(locale) == expected_locale)
    {
        bail!(
            "configured locale '{}' has not been generated in the target",
            config.system.locale
        );
    }

    let localtime = etc.join("localtime");
    let expected_timezone =
        PathBuf::from(format!("../usr/share/zoneinfo/{}", config.system.timezone));
    let metadata =
        fs::symlink_metadata(&localtime).context("installed /etc/localtime is missing")?;
    if !metadata.file_type().is_symlink()
        || fs::read_link(&localtime).context("read installed timezone symlink")?
            != expected_timezone
    {
        bail!("/etc/localtime does not point to the configured timezone");
    }

    let timezone_path = format!("/usr/share/zoneinfo/{}", config.system.timezone);
    run_chroot_output(&["test", "-f", &timezone_path])?;

    let keymap = console_keymap(&config.system.keyboard_layout);
    let keymap_pattern = format!("{keymap}.map*");
    let keymaps = run_chroot_output(&[
        "find",
        "/usr/share/kbd/keymaps",
        "-type",
        "f",
        "-name",
        &keymap_pattern,
    ])?;
    if keymaps.stdout.is_empty() {
        bail!("configured console keymap '{keymap}' is not installed in the target");
    }
    let vconsole = read_regular_file(&etc.join("vconsole.conf"))?;
    if !vconsole
        .lines()
        .any(|line| line.trim() == format!("KEYMAP={keymap}"))
    {
        bail!("/etc/vconsole.conf does not set the configured console keymap");
    }
    let hosts = read_regular_file(&etc.join("hosts"))?;
    if !hosts.contains(&config.system.hostname) {
        bail!("/etc/hosts does not contain the configured hostname");
    }
    let xorg = read_regular_file(&etc.join("X11/xorg.conf.d/00-keyboard.conf"))?;
    if !xorg.contains(&format!(
        "Option \"XkbLayout\" \"{}\"",
        config.system.keyboard_layout
    )) {
        bail!("X11 keyboard configuration does not match the selected layout");
    }

    let os_release = run_chroot_output(&["cat", "/etc/os-release"])?;
    let os_release = String::from_utf8_lossy(&os_release.stdout);
    if !os_release.lines().any(|line| line == "ID=artix") {
        bail!("installed target does not identify itself as Artix Linux");
    }
    Ok(())
}

fn verify_user_and_privileges(config: &InstallConfig, etc: &Path) -> Result<()> {
    let username = config.system.username.as_str();
    let passwd = run_chroot_output(&["getent", "passwd", username])?;
    let passwd = String::from_utf8(passwd.stdout).context("target passwd entry is not UTF-8")?;
    let fields: Vec<_> = passwd.trim().split(':').collect();
    if fields.len() != 7 || fields[0] != username {
        bail!("configured user '{username}' is missing or has an invalid passwd entry");
    }
    if fields[5] != format!("/home/{username}") || fields[6] != "/bin/bash" {
        bail!(
            "configured user '{username}' does not have the expected home directory and Bash login shell"
        );
    }
    let home = Path::new(TARGET).join(format!("home/{username}"));
    ensure_real_directory(&home)?;

    let groups = run_chroot_output(&["id", "-nG", username])?;
    let groups = String::from_utf8_lossy(&groups.stdout);
    if !groups.split_whitespace().any(|group| group == "wheel") {
        bail!("configured user '{username}' is not a member of wheel");
    }

    let password = run_chroot_output(&["passwd", "-S", username])?;
    let password = String::from_utf8_lossy(&password.stdout);
    let status = password.split_whitespace().nth(1).unwrap_or("");
    if status != "P" {
        bail!(
            "configured user '{username}' does not have an unlocked password (passwd status: {status})"
        );
    }

    match config.system.privilege_escalation {
        PrivilegeEscalation::Doas => verify_doas(etc)?,
        PrivilegeEscalation::Sudo => verify_sudo(etc)?,
        PrivilegeEscalation::Both => {
            verify_doas(etc)?;
            verify_sudo(etc)?;
        }
    }
    Ok(())
}

fn verify_doas(etc: &Path) -> Result<()> {
    let path = etc.join("doas.conf");
    let contents = read_regular_file(&path)?;
    if contents != "permit :wheel\n" {
        bail!("/etc/doas.conf does not contain the expected wheel policy");
    }
    verify_mode(&path, 0o400)
}

fn verify_sudo(etc: &Path) -> Result<()> {
    let path = etc.join("sudoers.d/10-agsi-wheel");
    let contents = read_regular_file(&path)?;
    if contents != "%wheel ALL=(ALL:ALL) ALL\n" {
        bail!("AGSI sudoers drop-in does not contain the expected wheel policy");
    }
    verify_mode(&path, 0o440)?;
    let output = run_chroot_output(&["visudo", "-cf", "/etc/sudoers"])?;
    if !output.status.success() {
        bail!("visudo rejected the installed sudoers configuration");
    }
    Ok(())
}

fn verify_dinit_services(config: &InstallConfig, etc: &Path) -> Result<()> {
    let dinit = etc.join("dinit.d");
    ensure_real_directory(&dinit)?;
    let boot = dinit.join("boot.d");
    ensure_real_directory(&boot)?;

    let mut services = vec!["dbus".to_owned(), "elogind".to_owned()];
    services.push(
        match config.software.network {
            NetworkStack::Dhcpcd => "dhcpcd",
            NetworkStack::Iwd => "iwd",
            NetworkStack::NetworkManager => "NetworkManager",
        }
        .to_owned(),
    );

    if !config.software.desktop_environments.is_empty() {
        match config.system.user_services {
            UserServices::None | UserServices::Manual => {}
            UserServices::Userspawn => {
                let service = if verify_service_file(&dinit.join("userspawn")).is_ok() {
                    "userspawn"
                } else if verify_service_file(&dinit.join("dinit-user-spawn")).is_ok() {
                    "dinit-user-spawn"
                } else {
                    bail!("neither userspawn nor dinit-user-spawn exists in the target");
                };
                services.push(service.to_owned());
            }
            UserServices::Turnstile => services.push("turnstiled".to_owned()),
            UserServices::Native => bail!("native user services cannot be verified for Dinit"),
        }
    }

    if config
        .software
        .desktop_environments
        .iter()
        .any(|desktop| matches!(desktop, Desktop::Cosmic | Desktop::Cosmalia))
    {
        services.push("upower".to_owned());
    }

    for name in &services {
        verify_service_file(&dinit.join(name))?;
        verify_service_link(&boot.join(name), &format!("/etc/dinit.d/{name}"))?;
    }

    if needs_audio(config) {
        if verify_service_file(&dinit.join("rtkit-daemon")).is_ok() {
            verify_service_link(&boot.join("rtkit-daemon"), "/etc/dinit.d/rtkit-daemon")?;
        } else if verify_service_file(&dinit.join("rtkit")).is_ok() {
            verify_service_link(&boot.join("rtkit"), "/etc/dinit.d/rtkit")?;
        }
    }

    if config.system.user_services == UserServices::Userspawn
        && !config.software.desktop_environments.is_empty()
        && needs_audio(config)
    {
        let user_dir = dinit.join("user");
        ensure_real_directory(&user_dir)?;
        let user_boot = etc.join("xdg/userspawn/dinit.d/boot.d");
        ensure_real_directory(&user_boot)?;
        for name in ["pipewire-pulse", "wireplumber"] {
            verify_service_file(&user_dir.join(name))?;
            verify_service_link(&user_boot.join(name), &format!("/etc/dinit.d/user/{name}"))?;
        }
    }
    Ok(())
}

fn verify_grub_uefi(etc: &Path, boot: &Path) -> Result<()> {
    let marker = etc.join(GRUB_MARKER);
    if read_regular_file(&marker)? != "Managed by AGSI.\n" {
        bail!("AGSI GRUB installation marker is invalid");
    }
    ensure_nonempty_file(&boot.join("EFI/Artix/grubx64.efi"))?;
    ensure_nonempty_file(&boot.join("grub/grub.cfg"))?;
    let fallback_marker = boot.join("EFI/BOOT").join(GRUB_MARKER);
    if path_exists(&fallback_marker)? {
        ensure_nonempty_file(&boot.join("EFI/BOOT/BOOTX64.EFI"))?;
        if read_regular_file(&fallback_marker)? != "Managed by AGSI.\n" {
            bail!("AGSI fallback EFI marker is invalid");
        }
    }
    Ok(())
}

fn verify_fstab(path: &Path, root_uuid: &str, efi_uuid: &str) -> Result<()> {
    let contents = read_regular_file(path)?;
    let mut root_entries = Vec::new();
    let mut boot_entries = Vec::new();
    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let fields: Vec<_> = line.split_whitespace().collect();
        if fields.len() < 4 {
            bail!("fstab contains a malformed entry: {line}");
        }
        match fields[1] {
            "/" => root_entries.push(fields),
            "/boot" => boot_entries.push(fields),
            _ => {}
        }
    }
    if root_entries.len() != 1 || boot_entries.len() != 1 {
        bail!("fstab must contain exactly one root entry and one /boot entry");
    }
    if root_entries[0][0] != format!("UUID={root_uuid}") || root_entries[0][2] != "ext4" {
        bail!("fstab root entry does not match the installed ext4 root UUID");
    }
    if boot_entries[0][0] != format!("UUID={efi_uuid}") || boot_entries[0][2] != "vfat" {
        bail!("fstab /boot entry does not match the installed EFI filesystem UUID");
    }
    Ok(())
}

fn verify_service_file(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("required Dinit service {} is missing", path.display()))?;
    if metadata.file_type().is_file() {
        return Ok(());
    }
    if !metadata.file_type().is_symlink() {
        bail!(
            "Dinit service {} is not a file or service symlink",
            path.display()
        );
    }
    let relative = path
        .strip_prefix(TARGET)
        .context("Dinit service path is outside the installed target")?;
    let target_path = format!("/{}", relative.display());
    let status = Command::new("chroot")
        .arg(TARGET)
        .args(["test", "-f", &target_path])
        .status()
        .context("chroot is required to verify target Dinit service symlinks")?;
    if !status.success() {
        bail!(
            "Dinit service {} does not resolve inside the installed target",
            path.display()
        );
    }
    Ok(())
}

fn verify_service_link(link: &Path, expected: &str) -> Result<()> {
    let metadata = fs::symlink_metadata(link)
        .with_context(|| format!("Dinit service link {} is missing", link.display()))?;
    if !metadata.file_type().is_symlink() {
        bail!("{} is not a Dinit service symlink", link.display());
    }
    let actual = fs::read_link(link).with_context(|| format!("read {}", link.display()))?;
    if actual != Path::new(expected) {
        bail!(
            "{} points to {}, expected {}",
            link.display(),
            actual.display(),
            expected
        );
    }
    Ok(())
}

fn verify_kernel_images(boot: &Path) -> Result<()> {
    let mut kernels = Vec::new();
    for entry in fs::read_dir(boot).context("read installed /boot directory")? {
        let entry = entry.context("inspect /boot entry")?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if let Some(kernel) = name.strip_prefix("vmlinuz-")
            && entry
                .file_type()
                .context("inspect kernel image type")?
                .is_file()
            && entry.metadata().context("inspect kernel image size")?.len() > 0
        {
            kernels.push(kernel.to_owned());
        }
    }
    if kernels.is_empty() {
        bail!("no non-empty vmlinuz-* kernel image exists in /mnt/boot");
    }
    for kernel in &kernels {
        ensure_nonempty_file(&boot.join(format!("initramfs-{kernel}.img")))?;
    }
    Ok(())
}

fn verify_filesystem(device: &Path, expected: &str) -> Result<()> {
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
            actual
        );
    }
    Ok(())
}

fn filesystem_uuid(device: &Path) -> Result<String> {
    let output = Command::new("blkid")
        .args(["-p", "-s", "UUID", "-o", "value"])
        .arg(device)
        .output()
        .with_context(|| format!("read filesystem UUID for {}", device.display()))?;
    if !output.status.success() {
        bail!("could not read filesystem UUID for {}", device.display());
    }
    let uuid = String::from_utf8(output.stdout)
        .context("blkid returned a non-UTF-8 filesystem UUID")?
        .trim()
        .to_owned();
    if uuid.is_empty() {
        bail!("filesystem UUID is empty for {}", device.display());
    }
    Ok(uuid)
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

fn path_exists(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error).with_context(|| format!("inspect {}", path.display())),
    }
}

fn verify_mode(path: &Path, expected: u32) -> Result<()> {
    let mode = fs::metadata(path)
        .with_context(|| format!("inspect permissions on {}", path.display()))?
        .permissions()
        .mode()
        & 0o777;
    if mode != expected {
        bail!(
            "{} has mode {:04o}; expected {:04o}",
            path.display(),
            mode,
            expected
        );
    }
    Ok(())
}

fn read_regular_file(path: &Path) -> Result<String> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("required file {} is missing", path.display()))?;
    if !metadata.file_type().is_file() {
        bail!("{} is not a regular file", path.display());
    }
    fs::read_to_string(path).with_context(|| format!("read {}", path.display()))
}

fn ensure_nonempty_file(path: &Path) -> Result<()> {
    read_regular_file(path)?;
    if fs::metadata(path)
        .with_context(|| format!("inspect {}", path.display()))?
        .len()
        == 0
    {
        bail!("{} is empty", path.display());
    }
    Ok(())
}

fn ensure_real_directory(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_dir() => Ok(()),
        Ok(_) => bail!("{} is not a real directory", path.display()),
        Err(error) => Err(error).with_context(|| format!("inspect {}", path.display())),
    }
}

fn run_chroot_output(arguments: &[&str]) -> Result<std::process::Output> {
    let output = Command::new("chroot")
        .arg(TARGET)
        .args(arguments)
        .output()
        .with_context(|| format!("run {} inside the target", arguments[0]))?;
    if !output.status.success() {
        bail!("{} failed inside /mnt", arguments[0]);
    }
    Ok(output)
}

fn is_mountpoint(path: &str) -> Result<bool> {
    let output = Command::new("findmnt")
        .args(["--noheadings", "--mountpoint", path])
        .output()
        .with_context(|| format!("check whether {path} is still mounted"))?;
    Ok(output.status.success())
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

fn normalize_locale(locale: &str) -> String {
    locale.trim().to_ascii_lowercase().replace('-', "")
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

fn needs_audio(config: &InstallConfig) -> bool {
    config.software.desktop_environments.iter().any(|desktop| {
        matches!(
            desktop,
            Desktop::Plasma
                | Desktop::Xfce
                | Desktop::Cinnamon
                | Desktop::Mate
                | Desktop::Lxqt
                | Desktop::Moksha
                | Desktop::Cosmic
                | Desktop::Cosmalia
                | Desktop::Hyprland
                | Desktop::MangoWm
                | Desktop::Umbriel
                | Desktop::Niri
                | Desktop::Wayfire
                | Desktop::Labwc
                | Desktop::Sway
                | Desktop::SwayFx
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locale_normalization_matches_locale_gen_output() {
        assert_eq!(normalize_locale("en_US.UTF-8"), "en_us.utf8");
        assert_eq!(normalize_locale("en_US.utf8"), "en_us.utf8");
    }

    #[test]
    fn console_keymap_aliases_match_system_configuration() {
        assert_eq!(console_keymap("us-intl"), "us");
        assert_eq!(console_keymap("jp106"), "jp106");
    }
}
