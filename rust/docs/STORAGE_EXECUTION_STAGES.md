# Storage execution stages

This document defines the staged storage workflow for the Rust installer. The command-line operations are developer/test interfaces; the eventual guided installer should orchestrate them behind one reviewed installation plan.

## Supported test path today

1. Read-only hardware discovery and preflight.
2. Automatic GPT/MBR partition-table creation.
3. Read-back verification with sfdisk, partprobe, udevadm, and fresh discovery.
4. Automatic ext4 root formatting with exact partition confirmation and blkid verification.

These stages have been exercised on a disposable Artix live-ISO VM. They do not yet constitute a complete installation.

## Next implementation stage: prepare target filesystems

Initial scope is intentionally limited to the tested, unencrypted, automatic UEFI layout:

- EFI System Partition: newly created by this installer's automatic partition-table writer; format as FAT32 only after proving that the current partition table matches the configured layout and the partition is unused.
- Root: existing ext4 filesystem created by the tested root formatter.
- Mount root at /mnt and EFI at /mnt/boot.
- Verify both mounts by querying the kernel's mount table and comparing canonical source devices.
- On any failure after mounting root, attempt rollback by unmounting in reverse order. If rollback fails, report the exact mount still present and block subsequent installation stages.
- Do not activate swap, open encryption mappings, format additional mounts, or install packages in this stage.

The first implementation must refuse manual and dual-boot layouts, encrypted configurations, unsupported root filesystems, active/mounted/read-only/held devices, unexpected partition counts or types, and a non-empty or already-mounted target mountpoint.

## fstab stage

After mount verification is reliable:

- Resolve filesystem UUIDs with blkid; never write unstable /dev/sdX names to fstab.
- Generate fstab from the verified target mounts, not from configuration alone.
- Validate the generated file with findmnt --verify --tab-file against the target root.
- Preserve existing content only when explicitly operating in a non-destructive/manual workflow; automatic installs generate a new target fstab.
- Do not enable swap until its configured mode has a separately tested executor.

## User experience requirements

The finished guided installer should present one storage summary and one explicit destructive confirmation identifying the disk by model, capacity, and device path. It should then orchestrate the reviewed storage stages with progress reporting. Users should not have to run individual partition, format, mount, or fstab commands.

A failed stage must stop the workflow, preserve logs, report what completed, and never continue into package installation with an unverified mount layout.
