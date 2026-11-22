# Artix Setup — Rust prototype

A Rust + Ratatui reimplementation track for [decksters-lab/artix-install](https://github.com/decksters-lab/artix-install).

> **Status: prototype only — not an installer.** This crate provides a keyboard-driven TUI, versioned TOML configuration, validation, and a reviewable plan. It does not execute system commands or modify disks. Continue using the existing `artix-install.sh` for real installs until the Rust port has feature parity and has passed disposable-VM testing.

## Build

Requires Rust 1.88 or newer.

```sh
cargo fmt --all
cargo build --release
cargo test
cargo run
```

## Run modes

```sh
cargo run -- --plan
cargo run -- --validate --config configs/gaming-dinit.toml
cargo run -- --config configs/wayland-hyprland.toml --plan
cargo run -- --write-default artix-setup.toml
cargo run -- --hardware
```

Inside the TUI:

- `↑` / `↓`: select a setting
- `←` / `→`: cycle a setting, or move the cursor in the desktop/kernel multi-selects
- `Space` / `Enter`: toggle the highlighted desktop or kernel
- `p`: preview the generated plan
- `s`: save `./artix-setup.toml`
- `?`: help
- `d`: open the read-only hardware/disk inventory
- `m`: open the partition manager / layout preview
- `q` / `Esc`: quit

## Current configuration coverage

The schema is based on the current shell wizard and covers:

- Init systems: dinit, OpenRC, runit and s6.
- Firmware and boot: UEFI/BIOS, GRUB2, Limine and rEFInd (BIOS is GRUB-only).
- Storage: automatic/manual/dual-boot layout planning; ext4, Btrfs, XFS, F2FS, JFS and NILFS2 root filesystems; optional manual additional mountpoints; LUKS2 intent; zram, swap partition, swapfile, combined zram+swapfile, swap size and zswap intent; configurable Btrfs subvolume/compression plans.
- Locale, timezone, keyboard layout, hostname and username. Passwords/passphrases are deliberately not part of saved profiles.
- Privilege escalation: doas or sudo.
- User services: userspawn, turnstile or neither. Availability must be checked for the selected init system at install time.
- Multi-select desktop/window-manager choices: Plasma, XFCE, Cinnamon, MATE, LXQt, Moksha, COSMIC, Hyprland, Niri, Wayfire, labwc, Sway, SwayFX, Umbriel, MangoWM, Cosmalia, i3, Openbox, Fluxbox and Qtile.
- Multi-select kernels: linux, linux-lts, linux-zen, linux-lqx and linux-cachyos. `linux` is retained as a guaranteed fallback.
- CPU/GPU selection: Intel, AMD, other/VM; Intel, AMD, NVIDIA, hybrid Intel+NVIDIA or VM/basic Mesa.
- Network: dhcpcd, iwd or NetworkManager; optional multilib; optional noctalia-greeter.

Some choices are conditional in the existing installer. Cosmalia is dinit-only. SwayFX conflicts with Sway and Umbriel. MangoWM, Umbriel, SwayFX and noctalia-greeter are offered only after live package availability checks. This prototype models those options and validates known conflicts, but the live package-discovery module is not implemented yet; the plan therefore calls out those checks as prerequisites.

## Read-only hardware discovery

The `--hardware` mode and the TUI's `d` screen read Linux `/sys`, `/proc` and udev metadata without spawning commands or opening block devices for writing. The inventory includes detected firmware mode, memory, CPU identity, PCI display controllers, network interfaces, whole block devices, their partitions, filesystem metadata where udev has it, and mountpoints.

Disk selection is deliberately conservative: a device is not selectable when it or any child partition is mounted or active swap, when it or a child partition is read-only, or when the disk or any child partition has active holders. eMMC boot/RPMB regions are excluded. If mount, swap or device-state data cannot be inspected, selection fails closed. Removable drives are shown but visibly marked. Selecting a device only stores its path in the TOML plan; it does not partition, format, mount or otherwise modify it. Device inventory is a snapshot and must be rechecked before any future destructive workflow.

## Partition manager (planning only)

Press `m` to open the partition manager. Automatic mode previews the GPT/MBR layout, EFI/root/swap partition order, LUKS2 mapping, Btrfs subvolumes and swap setup. Manual mode assigns already-existing partitions to root, EFI and (optionally) swap roles with `R`, `E` and `W`. Dual-boot mode assigns an existing root and EFI partition and hard-locks EFI policy to preserve.

Manual mode does **not** launch cfdisk or create/resize partitions yet; prepare the partition table separately before assigning the existing partitions in this prototype. The planner is a pure description generator. It has no partitioning, formatting, cryptsetup, mount, mkswap or swapon executor. For custom TOML profiles, manual and dual-boot partition paths must still be independently checked against the selected physical disk; the live topology revalidation module is not implemented.

Btrfs layouts are configurable as flat, standard (`@`, `@home`, `@log`, `@cache`) or snapshots (`@`, `@home`, `@log`, `@cache`, `@snapshots`). Btrfs swapfile plans use a dedicated `@swap` subvolume (or a flat-root swap file) and call out the NOCOW requirement. Compression can be none, zstd or lzo. These are previews only, not executed filesystem operations.

## Safety model

- No shell/process execution API is used in the Rust source.
- No disk-writing workflow is implemented: no partitioning, formatting, LUKS setup, mounting, chroot or package installation.
- `disk.target` defaults to unset. The prototype never selects the first disk automatically.
- A selected target path is only configuration data; the planner does not inspect or modify it.
- Dual-boot mode rejects an EFI format policy. EFI preservation is the default.
- BIOS configurations require GRUB2; BIOS + F2FS is rejected until an explicit compatibility override is designed.
- Passwords and LUKS passphrases are never serialized into TOML or logs.

Read [`docs/SAFETY_MODEL.md`](docs/SAFETY_MODEL.md) before implementing any executor.

## Porting approach

The Rust app lives beside, not instead of, `artix-install.sh`. Port the existing install logic in stages: read-only hardware discovery, package/init compatibility mapping, deterministic plan generation, storage and encryption tests, and only then a command executor. See [`docs/PORTING_PLAN.md`](docs/PORTING_PLAN.md).
