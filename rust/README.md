# Artix Granular System Installer

*Rust + Ratatui prototype*

A Rust + Ratatui reimplementation track for [decksters-lab/artix-install](https://github.com/decksters-lab/artix-install).

> **Status: experimental installer prototype.** The Rust binary has guarded storage stages for automatic partitioning, ext4 root formatting, safe EFI preparation, target mounting, UUID-based fstab generation, Artix basestrap installation, and an initial post-install system-identity stage for hostname, timezone, locale generation, console keymap, X11 keyboard layout, and hosts. User/password setup, init-specific service activation, bootloader installation/configuration, and full guided installation are not implemented yet. Keep using the existing `artix-install.sh` for real installs until the Rust port reaches feature parity and passes disposable-VM testing.

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
cargo run -- --write-default agsi.toml
cargo run -- --hardware
cargo run -- --config agsi.toml --configure-system
```

Inside the TUI:

- `↑` / `↓`: select a setting
- `←` / `→`: cycle a setting, or move the cursor in the desktop/kernel multi-selects
- `Space` / `Enter`: toggle the highlighted desktop or kernel
- `p`: preview the generated plan
- `s`: save `./agsi.toml`
- `?`: help
- `d`: open the read-only hardware/disk inventory
- `m`: open the partition manager / layout preview
- On the partition screen: `R` assigns root, `E` assigns EFI, `W` assigns swap (when swap mode is `partition`)
- `q` / `Esc`: quit

## Current configuration coverage

The schema is based on the current shell wizard and covers:

- Init systems: dinit, OpenRC, runit and s6.
- Firmware and boot: UEFI/BIOS, GRUB2, Limine and rEFInd (BIOS is GRUB-only).
- Storage: automatic/manual/dual-boot layout planning; ext4, Btrfs, XFS, F2FS, JFS and NILFS2 root filesystems; optional manual additional mountpoints; LUKS2 intent; zram, swap partition, swapfile, combined zram+swapfile, swap size and zswap intent; configurable Btrfs subvolume/compression plans.
- Locale, timezone, keyboard layout, hostname and username. Passwords/passphrases are deliberately not part of saved profiles.
- Privilege escalation: doas, sudo, or both. Selecting both includes both packages in the install plan; final chroot configuration will configure both without a passwordless pacman exception.
- User services are init-specific: Dinit offers `userspawn-dinit` or `turnstile-dinit`; OpenRC offers native per-user services (OpenRC 0.62+, with XDG_RUNTIME_DIR and PAM session auto-start configured) or `turnstile-openrc`; runit offers `userspawn-runit`; s6 is explicitly marked for manual, user-managed setup. The planner does not install or configure a generic Turnstile service across init systems.
- Multi-select desktop/window-manager choices: Plasma, XFCE, Cinnamon, MATE, LXQt, Moksha, COSMIC, Hyprland, Niri, Wayfire, labwc, Sway, SwayFX, Umbriel, MangoWM, Cosmalia, i3, Openbox, Fluxbox and Qtile.
- Multi-select kernels: linux, linux-lts, linux-zen, linux-lqx and linux-cachyos. `linux` is retained as a guaranteed fallback.
- CPU/GPU selection: Intel, AMD, other/VM; Intel, AMD, NVIDIA, hybrid Intel+NVIDIA or VM/basic Mesa.
- Network: dhcpcd, iwd or NetworkManager; optional multilib; optional noctalia-greeter.

Base development tools are installed as an explicit package list rather than the `base-devel` group. Artix's group includes `sudo` as a dependency, so expanding it explicitly prevents a doas-only selection from silently installing sudo. The privilege choice then adds only `opendoas`, only `sudo`, or both as selected. Some choices are conditional in the existing installer. Cosmalia is dinit-only and follows [grigio/cosmalia-session-dinit](https://github.com/grigio/cosmalia-session-dinit): its launcher runs `cosmic-comp --no-xwayland` and starts Noctalia as the shell. The upstream package manifest does not list a separate `cosmic-osd` component, so the installer treats XWayland as disabled and does not add a standalone COSMIC OSD package. SwayFX conflicts with Sway and Umbriel. MangoWM, Umbriel, SwayFX and noctalia-greeter are offered only after live package availability checks. This prototype models those options and validates known conflicts, but the live package-discovery module is not implemented yet; the plan therefore calls out those checks as prerequisites. The plan preview now includes structured package recipes ported from the shell installer, including init-specific base packages, selected kernel/header packages, CPU microcode, GPU drivers, network and privilege packages, conditional Xorg and PipeWire groups, and desktop package sets. Recipes identify Artix, Arch [extra], Artix [galaxy], live repository-choice, or external-PKGBUILD sources instead of pretending every package is in the default Artix repositories. These are static planning data only; no package queries or installs occur.

## Read-only hardware discovery

The `--hardware` mode and the TUI's `d` screen read Linux `/sys`, `/proc` and udev metadata without spawning commands or opening block devices for writing. The inventory includes detected firmware mode, memory, CPU identity, PCI display controllers, network interfaces, whole block devices, their partitions, filesystem metadata where udev has it, and mountpoints.

Disk selection is deliberately conservative: a device is not selectable when it or any child partition is mounted or active swap, when it or a child partition is read-only, or when the disk or any child partition has active holders. eMMC boot/RPMB regions are excluded. If mount, swap or device-state data cannot be inspected, selection fails closed. Removable drives are shown but visibly marked. Selecting a device only stores its path in the TOML plan; it does not partition, format, mount or otherwise modify it. Device inventory is a snapshot and must be rechecked before any future destructive workflow.

## Partition manager (planning only)

Press `m` to open the partition manager. Automatic mode previews the GPT/MBR layout, EFI/root/swap partition order, LUKS2 mapping, Btrfs subvolumes and swap setup. Manual mode assigns already-existing partitions to root, EFI and (optionally) swap roles with `R`, `E` and `W`; the selected root partition is formatted as the chosen root filesystem. EFI formatting follows the explicit preserve/format policy, and additional mountpoints have per-mount format flags. To reach it, select a target in the `d` disk inventory, change the partitioning setting to Manual or Dual-boot, then press `m`. Changing the target disk clears previous manual partition assignments. Dual-boot mode assigns an existing root and EFI partition and hard-locks EFI policy to preserve. Preserved EFI assignments are accepted in the TUI only when discovery verifies the filesystem is FAT32/vfat. Partition-role assignment rechecks the parent disk's safety state even when the target was loaded from a TOML profile. Additional mountpoints must be normalized absolute paths and cannot collide with the planned EFI mount or Btrfs-managed subvolume mounts.

Manual mode does **not** launch cfdisk or create/resize partitions yet; prepare the partition table separately before assigning the existing partitions in this prototype. The planner is a pure description generator. It has no partitioning, formatting, cryptsetup, mount, mkswap or swapon executor. For custom TOML profiles, manual and dual-boot partition paths must still be independently checked against the selected physical disk; the live topology revalidation module is not implemented. `--plan` prints configuration issues alongside a blocked preview, while `--validate` returns an error for invalid profiles.

Btrfs layouts are configurable as flat, standard (`@`, `@home`, `@log`, `@cache`) or snapshots (`@`, `@home`, `@log`, `@cache`, `@snapshots`). Btrfs swapfile plans use a dedicated `@swap` subvolume (or a flat-root swap file) and call out the NOCOW requirement. Compression can be none, zstd or lzo. These are previews only, not executed filesystem operations.

## Safety model

The storage and basestrap executors are deliberately narrow and guarded. Automatic partitioning, root formatting, EFI preparation, mounting, fstab generation, and base installation each revalidate their target and stop on uncertainty. Existing EFI filesystems and existing fstab files are preserved. The current executable path supports only unencrypted automatic UEFI/ext4 installs without a swap partition. `--configure-system` verifies the configured disk's mounted root and EFI sources before writing hostname, locale, timezone, console keymap, X11 keyboard layout, and hosts files. It is not yet a complete chroot configuration stage; user/password setup, init-specific service activation, bootloader installation/configuration, and final boot verification remain unimplemented.

Read [`docs/SAFETY_MODEL.md`](docs/SAFETY_MODEL.md) for the exact guarantees, limitations, and remaining release requirements.

## Porting approach

The Rust app lives beside, not instead of, `artix-install.sh`. Port the existing install logic in stages: read-only hardware discovery, package/init compatibility mapping, deterministic plan generation, storage and encryption tests, and only then a command executor. See [`docs/PORTING_PLAN.md`](docs/PORTING_PLAN.md).
