# Safety model (prototype)

This milestone is intentionally unable to alter the host system.

## Current guarantees

- There is no process-launching API in the Rust source (`std::process::Command`, shell execution, spawn or output calls are absent).
- The source contains no disk-writing workflow. Partitioning, formatting, LUKS setup, mounting, chroot and package installation are not implemented.
- An unset `disk.target` is the default. The planner blocks storage and downstream installation steps when no explicit target is configured.
- A configured target must be an explicit `/dev/...` path; `/dev/disk/by-id/...` is preferred for a future unattended workflow.
- Dual-boot mode rejects `efi_policy = "format"`; EFI preservation is the default.
- BIOS mode is GRUB-only. BIOS + F2FS is rejected pending a deliberate compatibility override.
- Manual additional mountpoints must be absolute non-root paths and cannot contain `..`.
- Desktop conflicts (SwayFX/Sway and SwayFX/Umbriel) and the dinit-only Cosmalia constraint are validated.
- `linux` is retained as the required fallback kernel.
- Profiles do not contain passwords or LUKS passphrases.
- `--plan`, `--validate` and `--hardware` are read-only operations.
- Hardware discovery reads sysfs, procfs and udev metadata only; it does not spawn commands or open block devices for writing.
- The disk inventory lists whole physical block devices and their partition children. It treats a disk as busy if the disk or any discovered partition is mounted.
- The TUI refuses to select disks that are mounted, have active swap on the disk or any child partition, are read-only, or have active holders. Removable status is shown but is not an automatic exclusion.
- Disk selection fails closed when the current mount table, active swap list, device read-only flag, holder directory or device number cannot be inspected; missing safety data is never treated as proof that a disk is free.
- A selected device path is still only configuration data; no disk operations exist in this milestone.

## Before a real executor is added

1. Extend read-only block-device discovery to verify partition-table type, filesystem UUIDs/labels, holders, ancestry and live-media identity on target Artix ISOs.
2. Reject the live medium, mounted devices, devices containing mounted descendants, and ambiguous/removable targets by default.
3. For automatic mode, show the exact partition table and format operations; require a typed confirmation of the full device identity and target path.
4. For manual mode, represent every selected partition, mountpoint and format policy in the plan before executing any action.
5. For dual-boot, require explicit root and EFI partitions, verify the EFI filesystem and existing contents, and make EFI formatting structurally impossible in the executor.
6. Re-check device identities immediately before each destructive operation to avoid stale device paths.
7. Never interpolate user data into shell strings. Invoke programs with argument arrays and validated paths.
8. Run one operation at a time, capture stdout/stderr, check exit status and stop on failure.
9. Add transaction/checkpoint recovery and test every failure point in disposable virtual machines before enabling disk writes.
10. Keep secrets out of TOML, process arguments and logs; use no-echo prompts or dedicated file descriptors.
11. Verify init-specific package/service availability and optional desktop package availability against current Artix/Arch/CachyOS repositories before offering those choices.

Any unattended mode must require a fully specified device identity and an explicit destructive-operation opt-in. It must never bypass the same safety checks used by interactive installs.
