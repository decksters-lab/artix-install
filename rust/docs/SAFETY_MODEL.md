# Safety model

The Rust installer is being built in guarded stages alongside the existing Bash installer. The Bash installer remains the supported path until the Rust implementation reaches feature parity and passes disposable-VM testing.

## Implemented executor stages

- `--apply-partition-table` writes only the automatic partition layout after an exact `--confirm-target` path confirmation. It rechecks device identity, reads the partition table back, and verifies the resulting partitions.
- `--format-root` formats only the planned ext4 root partition after an exact `--confirm-partition` confirmation. It rechecks the disk, partition table, and partition safety immediately before formatting.
- `--prepare-efi` preserves an existing FAT EFI filesystem. It formats only a blank EFI partition, after checking both `blkid` and `wipefs`, and requires exact `--confirm-efi` confirmation.
- `--mount-target` mounts the verified ext4 root at `/mnt` and FAT EFI at `/mnt/boot`, after exact `--confirm-mount` confirmation. It verifies the actual kernel mount table and rolls back mounts created by the operation if verification fails.
- `--generate-fstab` requires those exact filesystems to be mounted at the expected paths. It asks Artix `fstabgen -U` for UUID-based entries, validates root and EFI entries, refuses to replace an existing fstab, writes atomically, and verifies the result with `findmnt --verify`.

The storage and basestrap executors currently support only automatic, unencrypted UEFI with an ext4 root. Manual partitioning, dual-boot, encryption, Btrfs, and BIOS executor paths are not implemented.

## Not implemented yet

- Base-system installation with `basestrap`.
- Package installation beyond the base system, chroot configuration, user/password setup, locale and timezone application.
- Init-specific service activation, bootloader installation/configuration, initramfs generation, and first-boot validation.
- A complete guided one-action installation workflow.

Do not treat the current Rust prototype as a replacement for the Bash installer.

## Safety guarantees and constraints

- An unset `disk.target` is the default. Storage operations require an explicit configuration.
- Destructive storage operations require the exact target path confirmation for that operation.
- Disk discovery rejects mounted devices, active swap, read-only devices, active holders, and incomplete safety inspection. The live medium and ambiguous targets must not be selected.
- Disk identity and partition layout are rechecked before writes. Post-write partition tables and filesystem types are read back and verified.
- Existing EFI filesystems are never formatted by `--prepare-efi`; only a demonstrably blank EFI partition can be formatted.
- The mount stage does not format filesystems. It checks filesystem types, refuses occupied mountpoints, verifies actual mount sources, and rolls back its own mounts on failure.
- The fstab stage uses UUIDs, requires exactly one ext4 root entry and one vfat `/boot` entry, and will not overwrite an existing file.
- User-supplied data is passed as process arguments, not interpolated into shell commands.
- Passwords and LUKS passphrases are never serialized into TOML or logs.
- `--plan`, `--validate`, `--hardware`, and `--check-packages` are read-only. Package checks query only currently enabled sync databases and do not refresh them.

## Required before replacing the Bash installer

1. Implement and test base-system installation, package installation, chroot configuration, bootloader setup, and init-specific service activation.
2. Port every supported Bash feature or clearly remove unsupported options from the Rust UI.
3. Add transaction checkpoints and recovery behavior for every stage that can leave a partially installed target.
4. Test success and failure paths on disposable VMs for every supported firmware, init, filesystem, and desktop combination.
5. Verify that the installed system boots and that networking, user login, desktop session, and services work.
6. Keep the Bash installer untouched until parity and release criteria are met.

Any unattended installation must require a fully specified device identity and an explicit destructive-operation opt-in. It must never bypass the same safety checks used by the interactive workflow.
