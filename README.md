# artix-install

A whiptail-driven installer for Artix Linux. Originally based on a script from `github.com/feribsd/artix-install`, which no longer exists.
(Feel free to fork this repo.. I wish I had done that with the original script. Luckily I found the script on my drive and adapted it from there. This is why my install instructions direct you to download the script rather than just piping it into bash.)

## Requirements

- Booted from the Artix live ISO, with a network connection
- Must be run as root

## Usage (if you are using a graphical iso, run it as root user / su )

```
su
pacman -Syy
pacman -S git
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

noctalia-greeter is now offered for any desktop/WM selection — including pure X11 picks like Plasma alone — with only Cosmic excluded (it brings its own dedicated cosmic-greeter, which always wins anyway). The uncertain-X11-support warning is gone since x11 sessions have been confirmed working.
A fallback bug this surfaced: with the old restriction, a failed noctalia-greeter install always had a real Wayland WM to fall back to. With it gone, I found two cases where the old fallback would’ve pointed greetd at a binary that was never installed — Plasma-only (would’ve tried mango) and a bare-WM-only pick like i3-only (same problem). Both now correctly fall back to actually installing the right thing instead: sddm/lightdm get installed live if that’s what would’ve been used, and the bare-WM case now falls back to the WM’s real command (e.g. i3) rather than mango.


## Kernels

Multi-select checklist — install one or more:

- `linux` — always available as a fallback if your other picks fail to install
- `linux-lts`, `linux-zen`, `linux-lqx` (Liquorix), `linux-cachyos` (adds the CachyOS repo, BORE scheduler)

## Known caveats

Things flagged during review but intentionally left as-is, or not yet verified:

- **doas config** ships `permit nopass :wheel cmd pacman` — effectively passwordless root for anyone in `wheel`, since pacman can run arbitrary code as root (install scriptlets, `--hookdir`, `--config`). Left in deliberately; edit `/etc/doas.conf` after install if you want it removed or pinned to specific args (e.g. `args -Syu`).
- **Dual-boot EFI formatting** — the "do NOT format" warning shown in dual-boot mode isn't currently enforced. The Format EFI step has no dual-boot guard and will format an existing EFI partition — e.g. wiping a Windows bootloader — if you choose "Format" there. Avoid formatting your ESP in dual-boot mode.


## Changes from the original upstream fork

- Removed **Xlibre**, this was added to the original script when xlibre was the artix default x session. I switched it back to **X11** to match artix team recommendation on using it.
- Added **Cinnamon**, **Plasma**, **Niri**, and **Wayfire** as desktop options
- Added **MangoWM**, gated on live availability in Arch `[extra]` or the CachyOS repo
- Added **noctalia-greeter** as an optional greetd greeter, gated the same way
- Removed **XMonad** — along with it, a `git clone` of `github.com/feribsd/xmonad-dotfiles`, a repo whose owning account no longer exists and could be re-registered by anyone
- Fixed a display-manager/autologin ordering bug where bare window managers combined with another desktop could incorrectly get tty1 autologin
