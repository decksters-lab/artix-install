//! Guarded creation of the configured target user and privilege policy.
use std::{
    fs::{self, OpenOptions},
    os::unix::fs::{FileTypeExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result, bail};

use crate::config::{
    Filesystem, FirmwareMode, InstallConfig, Partitioning, PrivilegeEscalation, SwapMode,
};

const TARGET: &str = "/mnt";

pub fn configure_user(config: &InstallConfig) -> Result<String> {
    config.validate().map_err(|issues| {
        anyhow::anyhow!(
            "configuration validation failed:\n- {}",
            issues.join("\n- ")
        )
    })?;
    if effective_uid()? != 0 {
        bail!("user configuration requires effective UID 0");
    }
    if config.disk.partitioning != Partitioning::Automatic
        || config.system.firmware != FirmwareMode::Uefi
        || config.disk.root_filesystem != Filesystem::Ext4
        || config.disk.encryption
        || config.disk.swap.mode == SwapMode::Partition
    {
        bail!(
            "user configuration currently supports automatic, unencrypted UEFI/ext4 installs without a swap partition"
        );
    }

    let disk = config
        .disk
        .target
        .as_deref()
        .context("select a target disk before configuring the installed user")?;
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
    if !fs::symlink_metadata(&etc)
        .context("installed /etc directory is missing")?
        .file_type()
        .is_dir()
    {
        bail!("/mnt/etc is not a real directory");
    }
    if !etc.join("fstab").is_file() || !etc.join("hostname").is_file() {
        bail!("run --generate-fstab and --configure-system before --configure-user");
    }
    if !etc.join("passwd").is_file() || !etc.join("group").is_file() {
        bail!("target account databases are missing; install the base system first");
    }

    let username = &config.system.username;
    let existing = Command::new("chroot")
        .arg(TARGET)
        .args(["id", "-u", username])
        .output()
        .context("check whether the configured user already exists")?;
    if existing.status.success() {
        bail!("user '{username}' already exists in the target; refusing to modify its account");
    }
    if !Command::new("chroot")
        .arg(TARGET)
        .args(["getent", "group", "wheel"])
        .status()
        .context("check target wheel group")?
        .success()
    {
        bail!(
            "the target does not have a wheel group; refusing to create a user without privilege access"
        );
    }

    let mut groups = vec!["wheel".to_owned()];
    for group in ["video", "audio", "input", "storage", "optical", "lp"] {
        let found = Command::new("chroot")
            .arg(TARGET)
            .args(["getent", "group", group])
            .output()
            .with_context(|| format!("check target group {group}"))?;
        if found.status.success() {
            groups.push(group.to_owned());
        }
    }

    let sudo_dropin = etc.join("sudoers.d/10-agsi-wheel");
    let doas_conf = etc.join("doas.conf");
    let policy_result = (|| -> Result<()> {
        match config.system.privilege_escalation {
            PrivilegeEscalation::Doas => {
                ensure_absent(&doas_conf)?;
                write_file(&doas_conf, b"permit :wheel\n", 0o400)?;
            }
            PrivilegeEscalation::Sudo => {
                ensure_sudoers_dropin(&etc, &sudo_dropin)?;
            }
            PrivilegeEscalation::Both => {
                ensure_absent(&doas_conf)?;
                ensure_sudoers_dropin(&etc, &sudo_dropin)?;
                write_file(&doas_conf, b"permit :wheel\n", 0o400)?;
            }
        }
        Ok(())
    })();
    if let Err(error) = policy_result {
        rollback_privilege_files(config.system.privilege_escalation, &doas_conf, &sudo_dropin);
        return Err(error);
    }

    let useradd = Command::new("chroot")
        .arg(TARGET)
        .args([
            "useradd",
            "--create-home",
            "--groups",
            &groups.join(","),
            "--shell",
            "/bin/bash",
            username,
        ])
        .status();
    if !matches!(useradd, Ok(status) if status.success()) {
        rollback_privilege_files(config.system.privilege_escalation, &doas_conf, &sudo_dropin);
        bail!("useradd failed inside /mnt; privilege configuration was rolled back and no password was set");
    }

    eprintln!("Set the password for '{username}' in the target system:");
    let passwd = Command::new("chroot")
        .arg(TARGET)
        .args(["passwd", username])
        .status();
    if !matches!(passwd, Ok(status) if status.success()) {
        let user_removed = rollback_created_user(username);
        rollback_privilege_files(config.system.privilege_escalation, &doas_conf, &sudo_dropin);
        if !user_removed {
            bail!(
                "password setup failed; AGSI privilege files were removed, but userdel could not remove the new account '{username}'"
            );
        }
        bail!("password setup failed; the newly created user and AGSI privilege files were rolled back");
    }

    if matches!(
        config.system.privilege_escalation,
        PrivilegeEscalation::Sudo | PrivilegeEscalation::Both
    ) {
        let check = Command::new("chroot")
            .arg(TARGET)
            .args(["visudo", "-cf", "/etc/sudoers"])
            .status();
        if !matches!(check, Ok(status) if status.success()) {
            rollback_privilege_files(config.system.privilege_escalation, &doas_conf, &sudo_dropin);
            if !rollback_created_user(username) {
                bail!(
                    "visudo rejected the new privilege configuration; AGSI privilege files were removed, but userdel could not remove the new account '{username}'"
                );
            }
            bail!(
                "visudo rejected the new privilege configuration; AGSI privilege files and the newly created user were rolled back"
            );
        }
    }

    Ok(format!(
        "Created target user '{username}' with groups {} and configured {:?} wheel privilege access. Password was entered interactively and was not stored in the installer configuration.",
        groups.join(", "),
        config.system.privilege_escalation
    ))
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

fn ensure_absent(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(_) => bail!(
            "{} already exists; refusing to overwrite existing privilege policy",
            path.display()
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("inspect {}", path.display())),
    }
}

fn ensure_sudoers_dropin(etc: &Path, dropin: &Path) -> Result<()> {
    let directory = etc.join("sudoers.d");
    match fs::symlink_metadata(&directory) {
        Ok(metadata) if metadata.file_type().is_dir() => {}
        Ok(_) => bail!("/mnt/etc/sudoers.d exists but is not a real directory"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir(&directory).context("create /mnt/etc/sudoers.d")?;
            fs::set_permissions(&directory, fs::Permissions::from_mode(0o750))?;
        }
        Err(error) => return Err(error).context("inspect /mnt/etc/sudoers.d"),
    }
    let main =
        fs::read_to_string(etc.join("sudoers")).context("installed /etc/sudoers is missing")?;
    if !main.lines().any(|line| {
        let line = line.trim();
        (line.starts_with("#includedir") || line.starts_with("@includedir"))
            && line.split_whitespace().nth(1) == Some("/etc/sudoers.d")
    }) {
        bail!(
            "installed /etc/sudoers does not include /etc/sudoers.d; refusing to write an inactive policy"
        );
    }
    ensure_absent(dropin)?;
    write_file(dropin, b"%wheel ALL=(ALL:ALL) ALL\n", 0o440)
}

fn write_file(path: &Path, contents: &[u8], mode: u32) -> Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
        .open(path)
        .with_context(|| format!("create {}", path.display()))?;
    use std::io::Write;
    let result = (|| -> Result<()> {
        use std::io::Write;
        file.write_all(contents)
            .with_context(|| format!("write {}", path.display()))?;
        file.sync_all()
            .with_context(|| format!("sync {}", path.display()))?;
        Ok(())
    })();
    if let Err(error) = result {
        drop(file);
        let _ = fs::remove_file(path);
        return Err(error);
    }
    Ok(())
}

fn rollback_created_user(username: &str) -> bool {
    Command::new("chroot")
        .arg(TARGET)
        .args(["userdel", "--remove", username])
        .status()
        .is_ok_and(|status| status.success())
}

fn rollback_privilege_files(mode: PrivilegeEscalation, doas: &Path, sudo: &Path) {
    if matches!(mode, PrivilegeEscalation::Doas | PrivilegeEscalation::Both) {
        let _ = fs::remove_file(doas);
    }
    if matches!(mode, PrivilegeEscalation::Sudo | PrivilegeEscalation::Both) {
        let _ = fs::remove_file(sudo);
    }
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
