# Porting plan from `artix-install.sh`

The Rust application must grow beside the current 13-step Bash installer until feature parity and VM testing are demonstrated. Do not replace the existing entrypoint yet.

## Module boundaries

| Module | Responsibility |
|---|---|
| `config` | Typed, versioned TOML schema, import/export and validation |
| `planner` | Pure transformation from profile + detected hardware into a reviewable ordered plan |
| `discovery` | Read-only detection of firmware, disks, CPU, GPU, memory, network and package availability |
| `storage` | Automatic/manual/dual-boot layouts, filesystems, LUKS2, swap, additional mounts and fstab plan |
| `bootstrap` | Artix base installation and mirror/keyring preparation |
| `init` | dinit, OpenRC, runit and s6 package/service mappings |
| `kernel` | linux fallback plus LTS, Zen, LQX and CachyOS kernel handling |
| `desktop` | Multi-select desktops/WMs, package dependency sets and display-manager priority |
| `bootloader` | GRUB2, Limine and rEFInd configuration, firmware and kernel command-line validation |
| `network` | NetworkManager, iwd and dhcpcd setup |
| `user` | Users, doas, sudo (individually or together), and userspawn/turnstile init-specific setup |
| `executor` | Typed command execution, cancellation, logging and checkpoints |
| `ui` | Ratatui screens, progress, dialogs, multi-selects and review |

## Port order

1. Keep `artix-install.sh` untouched while the Rust crate becomes a separate, testable application.
2. Finish schema migration and validation against every current wizard option (this prototype is the first pass).
3. Add read-only hardware and disk discovery. The first pass reads sysfs/procfs/udev and lists physical disks, partitions, mounts, basic filesystem metadata, CPU/GPU, memory, firmware and network interfaces. No disk formatting is offered in this phase.
4. Harden discovery with partition-table type, filesystem UUIDs, live-media ancestry and more robust device identity; add init-specific and package-availability probes, including dynamic availability checks for MangoWM, Umbriel, SwayFX, noctalia-greeter and userspawn/turnstile.
5. Implement a deterministic end-to-end install plan and snapshot tests for each init system, firmware mode, partitioning mode, filesystem, swap and desktop combination.
6. Add storage backend and VM tests using disposable virtual disks, including dual-boot EFI preservation tests and failure recovery.
7. Add the executor only after the plan and safety gates have high test coverage.
8. Test the old and new installers in the same VM matrix; compare resulting packages, services, bootloader configuration and boot success before changing documentation or defaults.
