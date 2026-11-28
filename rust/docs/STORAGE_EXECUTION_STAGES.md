# Storage execution stages

This document defines the staged storage workflow for the Rust installer. The command-line operations are developer/test interfaces; the eventual guided installer should orchestrate them behind one reviewed installation plan.

## Supported test path

1. Read-only hardware discovery and preflight.
2. Automatic GPT/MBR partition-table creation.
3. Read-back verification with sfdisk, partprobe, udevadm, and fresh discovery.
4. Automatic ext4 root formatting with exact partition confirmation and blkid verification.

Stages 1–4 have been exercised on a disposable Artix live-ISO VM. They do not yet constitute a complete installation.

## Implemented target preparation and mounting

The first mount executor is intentionally limited to the unencrypted, automatic UEFI layout with ext4 root:

- Verify that the on-disk partition table exactly matches the configured layout.
- Resolve the EFI and root partitions from the verified partition table and fresh block-device discovery.
- Refuse mounted, active-swap, read-only, held, missing, or incompletely inspected partitions.
- Verify EFI is vfat and root is ext4 with blkid. This stage never formats either partition.
- Create /mnt if it is absent; otherwise require it to be a real, empty, unmounted directory.
- Mount root at /mnt, then mount EFI at /mnt/boot.
- Reject a symlink or non-directory at /mnt/boot.
- Verify both mounts against /proc/self/mountinfo and compare canonical source devices.
- On failure, attempt rollback in reverse order. If mount state cannot be read, still attempt unmount; report incomplete rollback rather than claiming success.

Developer/test command (not the intended end-user workflow):

```sh
sudo ./artix-setup --config vm-test.toml --mount-target --confirm-mount /dev/vda
```

The mount executor has passed formatting, compilation, Clippy, and all 111 unit tests in CI. It has not yet been exercised against the disposable Artix VM; do not treat it as VM-verified until that test is complete.

Manual and dual-boot layouts, encrypted configurations, non-ext4 root filesystems, additional mounts, swap activation, and package installation are outside this stage.

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
