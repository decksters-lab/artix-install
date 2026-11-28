//! Guarded offline Dinit service activation for the installed target.
use std::{
    fs,
    os::unix::fs::FileTypeExt,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result, bail};

use crate::config::{
    Desktop, Filesystem, FirmwareMode, InitSystem, InstallConfig, NetworkStack, Partitioning,
    SwapMode, UserServices,
};

const TARGET: &str = "/mnt";

pub fn configure_services(config: &InstallConfig) -> Result<String> {
    config.validate().map_err(|issues| {
        anyhow::anyhow!(
            "configuration validation failed:\n- {}",
            issues.join("\n- ")
        )
    })?;
    if effective_uid()? != 0 {
        bail!("service configuration requires effective UID 0");
    }
    if config.system.init_system != InitSystem::Dinit {
        bail!("offline service activation currently supports Dinit only; no service files were changed");
    }
    if config.disk.partitioning != Partitioning::Automatic
        || config.system.firmware != FirmwareMode::Uefi
        || config.disk.root_filesystem != Filesystem::Ext4
        || config.disk.encryption
        || config.disk.swap.mode == SwapMode::Partition
    {
        bail!("service configuration currently supports automatic, unencrypted UEFI/ext4 installs without a swap partition");
    }

    let disk = config
        .disk
        .target
        .as_deref()
        .context("select a target disk before configuring services")?;
    let table = Command::new("sfdisk")
        .arg("--dump")
        .arg(disk)
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
    verify_mount("/mnt", &root)?;
    verify_mount("/mnt/boot", &efi)?;

    let etc = Path::new(TARGET).join("etc");
    verify_real_directory(&etc)?;
    if !etc.join("fstab").is_file() || !etc.join("hostname").is_file() {
        bail!("run --generate-fstab and --configure-system before --configure-services");
    }
    let dinit_dir = etc.join("dinit.d");
    verify_real_directory(&dinit_dir)?;

    let mut services = selected_services(config)?;
    if services.iter().any(|name| name == "userspawn") {
        services.retain(|name| name != "userspawn");
        let selected = ["userspawn", "dinit-user-spawn"]
            .into_iter()
            .find(|name| valid_service_file(&dinit_dir.join(name)).is_ok())
            .context("neither userspawn nor dinit-user-spawn exists in the target Dinit service directory")?;
        services.push(selected.to_owned());
    }
    if needs_audio(config) {
        for optional in ["rtkit-daemon", "rtkit"] {
            if valid_service_file(&dinit_dir.join(optional)).is_ok() {
                services.push(optional.to_owned());
                break;
            }
        }
    }

    let mut user_services = Vec::new();
    if config.system.user_services == UserServices::Userspawn && needs_audio(config) {
        for name in ["pipewire-pulse", "wireplumber"] {
            let path = dinit_dir.join("user").join(name);
            verify_service_file(&path)?;
            user_services.push(name.to_owned());
        }
    }

    let boot_dir = dinit_dir.join("boot.d");
    ensure_real_directory(&boot_dir)?;

    let mut links = Vec::new();
    for name in &services {
        let service_file = dinit_dir.join(name);
        verify_service_file(&service_file)?;
        links.push((boot_dir.join(name), PathBuf::from(format!("/etc/dinit.d/{name}"))));
    }

    let mut user_boot_dir = None;
    if !user_services.is_empty() {
        let dir = etc.join("xdg/userspawn/dinit.d/boot.d");
        ensure_directory_chain(&etc, &dir)?;
        user_boot_dir = Some(dir.clone());
        for name in &user_services {
            links.push((
                dir.join(name),
                PathBuf::from(format!("/etc/dinit.d/user/{name}")),
            ));
        }
    }

    for (link, target) in &links {
        verify_link_destination(link, target)?;
    }

    let mut created = Vec::new();
    let mut already_enabled = Vec::new();
    for (link, target) in &links {
        match fs::symlink_metadata(link) {
            Ok(_) => already_enabled.push(link.clone()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if let Err(error) = std::os::unix::fs::symlink(target, link) {
                    for path in &created {
                        let _ = fs::remove_file(path);
                    }
                    return Err(error).with_context(|| format!("enable Dinit service {}", link.display()));
                }
                created.push(link.clone());
            }
            Err(error) => return Err(error).with_context(|| format!("inspect {}", link.display())),
        }
    }

    let zram_note = if matches!(config.disk.swap.mode, SwapMode::Zram | SwapMode::Both) {
        " Zram activation is not included in this stage and remains pending."
    } else {
        ""
    };
    let user_note = if user_boot_dir.is_some() {
        " Per-user PipeWire and WirePlumber services were linked for userspawn."
    } else {
        ""
    };

    Ok(format!(
        "Dinit service activation verified the configured target and enabled: {}. Already enabled: {}.{}{}",
        if created.is_empty() {
            "none".to_owned()
        } else {
            created.iter().map(|path| path.file_name().unwrap_or_default().to_string_lossy()).collect::<Vec<_>>().join(", ")
        },
        if already_enabled.is_empty() {
            "none".to_owned()
        } else {
            already_enabled.iter().map(|path| path.file_name().unwrap_or_default().to_string_lossy()).collect::<Vec<_>>().join(", ")
        },
        user_note,
        zram_note
    ))
}

fn selected_services(config: &InstallConfig) -> Result<Vec<String>> {
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
                services.push("userspawn".to_owned());
            }
            UserServices::Turnstile => {
                services.push("turnstiled".to_owned());
            }
            UserServices::Native => {
                bail!("native user services are not a Dinit option");
            }
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

    Ok(services)
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

fn verify_service_file(path: &Path) -> Result<()> {
    valid_service_file(path).map(|_| ())
}

fn valid_service_file(path: &Path) -> Result<PathBuf> {
    let canonical = fs::canonicalize(path)
        .with_context(|| format!("required Dinit service {} is missing", path.display()))?;
    if !canonical.starts_with(TARGET) || !canonical.is_file() {
        bail!(
            "Dinit service {} does not resolve to a regular file inside the installed target",
            path.display()
        );
    }
    Ok(canonical)
}

fn verify_link_destination(link: &Path, expected: &Path) -> Result<()> {
    match fs::symlink_metadata(link) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            let actual = fs::read_link(link).with_context(|| format!("read {}", link.display()))?;
            if &actual != expected {
                bail!(
                    "{} already points to {}, not {}; refusing to replace it",
                    link.display(),
                    actual.display(),
                    expected.display()
                );
            }
            Ok(())
        }
        Ok(_) => bail!(
            "{} already exists and is not the expected Dinit service symlink",
            link.display()
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("inspect {}", link.display())),
    }
}

fn verify_real_directory(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_dir() => Ok(()),
        Ok(_) => bail!("{} exists but is not a real directory", path.display()),
        Err(error) => Err(error).with_context(|| format!("inspect required directory {}", path.display())),
    }
}

fn ensure_real_directory(path: &Path) -> Result<()> {
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

fn ensure_directory_chain(base: &Path, target: &Path) -> Result<()> {
    let relative = target
        .strip_prefix(base)
        .context("requested directory is outside the target configuration directory")?;
    let mut current = base.to_path_buf();
    for component in relative.components() {
        current.push(component);
        ensure_real_directory(&current)?;
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
            "--target",
            mountpoint,
        ])
        .output()
        .with_context(|| format!("findmnt is required to verify {mountpoint}"))?;
    if !output.status.success() {
        bail!("{mountpoint} is not mounted");
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Desktop, InstallConfig};

    #[test]
    fn dinit_default_services_include_core_and_selected_network() {
        let services = selected_services(&InstallConfig::default()).unwrap();
        assert_eq!(services, ["dbus", "elogind", "NetworkManager"]);
    }

    #[test]
    fn dinit_user_service_selection_is_respected() {
        let mut config = InstallConfig::default();
        config.software.desktop_environments.push(Desktop::Hyprland);
        config.system.user_services = UserServices::Userspawn;
        assert!(
            selected_services(&config)
                .unwrap()
                .contains(&"userspawn".to_owned())
        );

        config.system.user_services = UserServices::Turnstile;
        assert!(
            selected_services(&config)
                .unwrap()
                .contains(&"turnstiled".to_owned())
        );
    }

    #[test]
    fn cosmic_requires_upower_service() {
        let mut config = InstallConfig::default();
        config.software.desktop_environments.push(Desktop::Cosmalia);
        assert!(
            selected_services(&config)
                .unwrap()
                .contains(&"upower".to_owned())
        );
    }
}
