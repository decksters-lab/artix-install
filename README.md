# artix-install

A whiptail-driven installer for Artix Linux. Originally based on a script from `github.com/feribsd/artix-install`, which no longer exists.

## Requirements

- Booted from the Artix live ISO, with a network connection
- Must be run as root

## Usage (if you are using a graphical iso, run it as root user / su )

```
git clone https://github.com/decksters-lab/artix-install.git
cd artix-install
chmod +x artix-install.sh
./artix-install.sh
```
For a fast, fully-automated test run (AMD CPU/GPU, GRUB, no desktop, hostname `artix`, user `user`):
```

./artix-install.sh --test
```

**`--test` wipes the first disk it finds with no confirmation prompt — only run it inside a VM.**

## The wizard (13 steps)

1. **Init system** — dinit (recommended), openrc, runit, or s6
2. **Disk** — target disk selection
3. **Filesystem** — per-partition: ext4, btrfs, xfs, f2fs, jfs, nilfs2, vfat, exfat, ntfs (cfdisk available for manual partitioning)
4. **Swap** — None / Partition / Swapfile / Both (zram + swapfile)
5. **Encryption** — optional full-disk LUKS2
6. **Locale, timezone, keyboard** — one screen each
7. **Hostname + username**
8. **Privilege escalation** — doas (default) or sudo
9. **Desktop** — see below
10. **Kernel + CPU + GPU** — see below
11. **Bootloader** — on UEFI, choose GRUB2, Limine, or rEFInd; BIOS systems always get GRUB (the other two are UEFI-only)
12. **32-bit / multilib** — optional, for Steam/Wine/32-bit apps
13. **Network stack** — dhcpcd, iwd, or NetworkManager

## Desktop environments

Multi-select checklist. Supported:

- **Plasma, XFCE, Cinnamon, LXQt, Moksha** — X11 sessions; get LightDM (SDDM for Plasma)
- **Cosmic** *(experimental)* — Wayland, ships its own dedicated `cosmic-greeter`
- **Hyprland, Niri, Wayfire** — Wayland-only
- **MangoWM** — Wayland-only, **only offered if found** in Arch `[extra]` or the CachyOS repo — checked live against the package API / sync databases at install time, so it simply won't appear if neither has it
- **i3, Openbox, Fluxbox, IceWM** — bare X11 window managers, get autologin + a generated `.xinitrc`

Display manager is chosen automatically, by priority: Cosmic > noctalia-greeter (if accepted) > Plasma (sddm) > XFCE/Cinnamon/LXQt/Moksha (lightdm) > Hyprland/MangoWM/Niri/Wayfire (greetd). No DM at all for a CLI-only install or a bare WM picked alone.

### noctalia-greeter

Offered whenever at least one of Hyprland / MangoWM / Niri / Wayfire is selected (and Cosmic isn't), and the package is found in Arch `[extra]` or the CachyOS repo. Accepting it makes it the **only** login screen — even if an X11 desktop was also selected — since it isn't confirmed to list non-Wayland sessions. The wizard warns about this before you confirm.

## Kernels

Multi-select checklist — install one or more:

- `linux` — always available as a fallback if your other picks fail to install
- `linux-lts`, `linux-zen`, `linux-lqx` (Liquorix), `linux-cachyos` (adds the CachyOS repo, BORE scheduler)

## Known caveats

Things flagged during review but intentionally left as-is, or not yet verified:

- **doas config** ships `permit nopass :wheel cmd pacman` — effectively passwordless root for anyone in `wheel`, since pacman can run arbitrary code as root (install scriptlets, `--hookdir`, `--config`). Left in deliberately; edit `/etc/doas.conf` after install if you want it removed or pinned to specific args (e.g. `args -Syu`).
- **Dual-boot EFI formatting** — the "do NOT format" warning shown in dual-boot mode isn't currently enforced. The Format EFI step has no dual-boot guard and will format an existing EFI partition — e.g. wiping a Windows bootloader — if you choose "Format" there. Avoid formatting your ESP in dual-boot mode.
- **Untested paths** — Cinnamon, MangoWM, Niri, Wayfire, and noctalia-greeter have been reviewed, syntax-checked, and exercised against mocked network/chroot calls, but not yet run end-to-end on real hardware or a live ISO (noctalia greeter and mangowm working confirmed via vm install). Worth a VM run before relying on them.

## Changes from the original upstream fork

- Added **Cinnamon**, **Plasma**, **Niri**, and **Wayfire** as desktop options
- Added **MangoWM**, gated on live availability in Arch `[extra]` or the CachyOS repo
- Added **noctalia-greeter** as an optional greetd greeter, gated the same way
- Removed **XMonad** — along with it, a `git clone` of `github.com/feribsd/xmonad-dotfiles`, a repo whose owning account no longer exists and could be re-registered by anyone
- Fixed a display-manager/autologin ordering bug where bare window managers combined with another desktop could incorrectly get tty1 autologin
