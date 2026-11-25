# artix-install
<img width="1920" height="1080" alt="image" src="https://github.com/user-attachments/assets/b205cfde-ecda-4768-b45a-7ac6ae2faec3" />

A pair of installers for Artix Linux. The original whiptail-driven Bash installer is the established option; a separate Rust + Ratatui implementation is being developed alongside it.

## Choose your installer

### 1. Bash installer — stable, recommended

This is the original, fully featured installer and remains the recommended option for real installations. Its script and existing usage instructions are preserved.

[Jump to Bash installation instructions](#bash-installer-requirements-and-usage)

### 2. Artix Granular System Installer — experimental

The Rust implementation is a separate project under `rust/`. It is being developed toward feature parity and does **not** replace or modify the Bash installer.

- Designed to run as a precompiled executable: **Rust and Cargo are not required on the Artix live ISO**.
- GitHub Actions builds x86_64 GNU and musl binaries and attaches archives plus SHA-256 checksums to versioned GitHub Releases.
- The musl build is statically linked and has been launched and smoke-tested on an Artix live ISO without Rust or Cargo.
- It now has separately guarded storage and base-install stages, but it is **not a complete installer** and is not ready for real installations. See the Rust prototype status and safety notes below.

When a tested release is available, download and extract the appropriate archive from [GitHub Releases](https://github.com/decksters-lab/artix-install/releases), then run `./agsi` from a terminal. Until then, use the Bash installer for actual installations.

---

## Goal

This installer is meant for experienced and new Artix users alike but you should be somewhat familiar with linux and comfortable with a terminal to install apps. It is deliberately minimal: you get what you choose in the wizard, plus what those choices need to work, and no extra apps, themes or utilities on top. You start with a clean, uncluttered system and build it up the way you want. If you already know exactly what you want, the wizard gets you there quickly; if you're newer to Artix, the defaults are sensible and every step explains what it's asking.

Window managers and Wayland compositors (Hyprland, MangoWM, Niri, Wayfire, labwc, Sway, and so on) are **installed, not configured**. You get the compositor and what it needs to start, and nothing more: no bar, launcher, wallpaper, theme or keybind setup. At most the installer drops in the package's own stock config plus one line that starts audio, so the rest of the setup is yours to build the way you like it.

## Bash installer requirements and usage

The following requirements and commands apply to the stable Bash installer only. The experimental Rust executable has separate build and run instructions below.

- Booted from the Artix live ISO, with a network connection (can be found at https://artixlinux.org/download.php .. you can use any weekly or official iso. Use one with a graphical session if its easier for you. It doesn't matter which kind you choose.) note: with a graphical installer it makes it easy to copy the install script, or the folder containing the install script to another location like a second hdd or a usb drive that isn't running the iso then the script is always available to you locally.
- Script Must be run as root either with su before all other commands or with sudo right before running ./artix-install.sh

## Usage ( run ./artix-install.sh with sudo or as root user / su )

Copy this one line, paste it, hit enter. It calls `su -c` for you, so there is no longer a separate "enter su first" step:

```bash
su -c 'pacman -Sy; pacman -S --needed --noconfirm git; git clone https://github.com/decksters-lab/artix-install.git; cd artix-install; chmod +x artix-install.sh; ./artix-install.sh'
```

`su` will ask for the root password before anything else runs (on the official Artix live ISOs that password is `artix`; check your ISO's release notes if it isn't). Everything after that runs as root, in the same terminal, so the wizard still works normally. The `git clone` means you still end up with a local copy of the script — nothing is piped straight into bash.

Already root, or prefer to call it from a sudo shell? Then just paste the inner part:

```bash
pacman -Sy; pacman -S --needed --noconfirm git; git clone https://github.com/decksters-lab/artix-install.git; cd artix-install; chmod +x artix-install.sh; ./artix-install.sh
```

Because the whole thing runs in one shell, `cd artix-install` sticks and `./artix-install.sh` launches from the right directory. If you close the terminal halfway through, the clone is still on disk — just `cd artix-install` and run `./artix-install.sh` again.

### Only if you hit a keyring or signature error

If the command above stops with `unknown trust`, `invalid or corrupted package (PGP signature)`, or a `keyring` complaint, use this version instead. It refreshes the keyrings before doing anything else:

```bash
su -c 'pacman -Syy; pacman -S --needed --noconfirm artix-keyring archlinux-keyring; pacman-key --init; pacman-key --populate artix archlinux; pacman -S --needed --noconfirm git; git clone https://github.com/decksters-lab/artix-install.git; cd artix-install; chmod +x artix-install.sh; ./artix-install.sh'
```

You don't need this otherwise — on a recent ISO the plain command works fine, and running the keyring steps when nothing is wrong just adds a minute of waiting.

For a fast, fully-automated test run (AMD CPU/GPU, GRUB, no desktop, hostname `artix`, user `user`):

```bash
./artix-install.sh --test
```

**`--test` wipes the first disk it finds with no confirmation prompt — only run it inside a VM.**
## The wizard (13 steps)

1. **Init system** — dinit (recommended), openrc, runit, or s6
2. **Disk** — target disk selection 
<img width="1920" height="1080" alt="2026-11-18_23-22-53" src="https://github.com/user-attachments/assets/6fedcb94-9405-4b6d-b272-e082c8bc1b28" />

3. **Filesystem** — root: ext4, btrfs, xfs, f2fs, jfs, or nilfs2. Other partitions (manual mode) can also be vfat, exfat or ntfs. The EFI partition is always FAT32 (cfdisk available for manual partitioning). The userland tools for each filesystem you use are installed into the new system
4. **Swap** — None / Zram / Partition / Swapfile / Both (zram + swapfile). With a partition or swapfile you are also asked whether to enable **zswap** (adds `zswap.enabled=1` to the kernel command line for GRUB, Limine and rEFInd). zswap is built into the kernel and needs a real swap device, so it is not offered with zram. See [zswap vs zram](https://chrisdown.name/2026/03/24/zswap-vs-zram-when-to-use-what.html) and the [Arch wiki](https://wiki.archlinux.org/title/Zswap)
5. **Encryption** — optional full-disk LUKS2
6. **Locale, timezone, keyboard** — one screen each
7. **Hostname + username**
8. **Privilege escalation** — doas (default) or sudo
9. **User services** 
<img width="1920" height="1080" alt="2026-11-18_23-23-26" src="https://github.com/user-attachments/assets/f796d668-a79c-41f0-b9d5-9a44a8df412c" />
— what starts each user's service manager at login (lets user services like pipewire or Cosmalia run): **userspawn** (`userspawn` + `userspawn-<init>`), **turnstile** (`turnstile` + `turnstile-<init>`), or neither. Asked before the desktop and applies whichever desktop you pick. Only the options that actually exist in the Artix repos for your init are shown (checked live), and the two conflict, so only one is installed. With userspawn on dinit, `wireplumber` and `pipewire-pulse` are also enabled for every user. On runit/s6, userspawn may need per-user setup after install. Then **Desktop** — see below
10. **Kernel + CPU + GPU** — see below
11. **Bootloader** — on UEFI, choose GRUB2, Limine, or rEFInd; BIOS systems always get GRUB (the other two are UEFI-only)
12. **32-bit / multilib** — optional, for Steam/Wine/32-bit apps
13. **Network stack** — dhcpcd, iwd, or NetworkManager

## Desktop environments
<img width="1920" height="1080" alt="2026-11-18_23-23-38" src="https://github.com/user-attachments/assets/83f185ff-f7d0-4a88-8372-55ccb9f2c6f8" />


Multi-select checklist. Supported:

- **Plasma, XFCE, Cinnamon, MATE, LXQt, Moksha** — X11 sessions; get LightDM (SDDM for Plasma)
- **Cosmic** *(experimental)* — Wayland, ships its own dedicated `cosmic-greeter`
- **Hyprland, Niri, Wayfire** — Wayland-only. Wayfire is installed with `wf-shell` and `wcm` (the Wayfire Config Manager)
- **Sway** — Wayland-only i3-style tiling compositor, from the Artix repos. Installed with `foot` and `wmenu` (the terminal and launcher its stock config uses) and `xdg-desktop-portal-wlr`; its stock config is untouched apart from a drop-in that starts audio
- **SwayFX** — Sway with visual effects (blur, rounded corners, shadows). Not in Arch or Artix; it lives in the CachyOS repo, so like MangoWM it is **only offered if found** there (checked live at install time). It replaces Sway, so picking both installs only SwayFX
- **labwc** — Wayland-only stacking compositor in the Openbox style, from the Artix repos. Installed with `foot` (so its root menu has a terminal) and `xdg-desktop-portal-wlr`, and nothing else — no panel or launcher. Its config lives in `~/.config/labwc`
- **Cosmalia** *(dinit only, recommended)* — minimal Wayland desktop: `cosmic-comp` + Noctalia shell, from [grigio/cosmalia-session-dinit](https://github.com/grigio/cosmalia-session-dinit). Only shown in the list when dinit is the init system. Its dependencies come from Arch `[extra]`, plus `cosmic-terminal` so you have a terminal; the session package itself is built from its PKGBUILD during install (git clone + `makepkg`). It runs on dinit user services, so pick userspawn or turnstile in the User services step (picking neither means you set it up yourself before logging in)
- **Umbriel** — Wayland-only compositor from the Noctalia project. Like MangoWM it is **only offered if found** in Arch `[extra]` or the CachyOS repo (checked live at install time), so it appears on its own once it is packaged there. Installed with no config
- **MangoWM** — Wayland-only, **only offered if found** in Arch `[extra]` or the CachyOS repo — checked live against the package API / sync databases at install time, so it simply won't appear if neither has it
- **i3, Openbox, Fluxbox, Qtile** — bare X11 window managers (Qtile is configured in Python), get autologin + a generated `.xinitrc`. Qtile is installed with `xterm` so its terminal binding works

Display manager is chosen automatically, by priority: Cosmic > noctalia-greeter (if accepted) > Plasma (sddm) > XFCE/Cinnamon/MATE/LXQt/Moksha (lightdm) > Hyprland/MangoWM/Umbriel/Niri/Wayfire/Cosmalia/labwc/Sway/SwayFX (greetd). No DM at all for a CLI-only install or a bare WM picked alone.

### noctalia-greeter

noctalia-greeter is now offered for any desktop/WM selection — including pure X11 picks like Cinnamon alone — with only Cosmic excluded (it brings its own dedicated cosmic-greeter, which always wins anyway). 

## Kernels

Multi-select checklist — install one or more:

- `linux` — always available as a fallback if your other picks fail to install
- `linux-lts`, `linux-zen`, `linux-lqx` (Liquorix), `linux-cachyos` (adds the CachyOS repo, BORE scheduler)

## Known caveats

- **Signature complaints after `pacman -Syy` on the live ISO.** Forcing a second `-y` refreshes the sync databases right away, so pacman starts seeing packages that are newer than the keyring baked into the ISO. You can get something like:

  ```
  error: <package>: signature from "..." is unknown trust
  error: <package>: invalid or corrupted package (PGP signature)
  error: failed to commit transaction (invalid or corrupted package (PGP signature))
  :: File /var/cache/pacman/pkg/<package> is corrupted (invalid or corrupted package (PGP signature)).
  Do you want to delete it? [Y/n]
  ```

  This is almost always a stale keyring versus fresh mirrors, not a broken mirror. The quickest fix is the [keyring variant of the install command](#only-if-you-hit-a-keyring-or-signature-error). By hand, fix it in this order:

  1. Update the keyrings first: `pacman -Sy --needed artix-keyring archlinux-keyring`
  2. If trust still fails: `pacman-key --init && pacman-key --populate artix archlinux && pacman-key --refresh-keys`
  3. Clear anything half-downloaded: `pacman -Scc`, then retry the install command
  4. If a single package still refuses, retry it, then try another mirror in `/etc/pacman.d/mirrorlist` — a mirror can serve a fresh database next to a stale package

  Downloading a newer weekly ISO is the permanent fix. Don't disable signature checking (`SigLevel = Never`) to get past it.

Things flagged during review but intentionally left as-is:

- **doas config** ships `permit nopass :wheel cmd pacman` — effectively passwordless root for anyone in `wheel`, since pacman can run arbitrary code as root (install scriptlets, `--hookdir`, `--config`). Left in deliberately; edit `/etc/doas.conf` and remove the second line (it should just read as permit persist :wheel)  after install if you want it removed or pinned to specific args (e.g. `args -Syu`)WARNING: IF YOU DON"T REMOVE THIS AFTER INSTALL PACMAN WILL NO LONGER PROMPT FOR PASSWORD!!!! Only applicable to doas though.
- **Dual-boot EFI formatting** — the "do NOT format" warning shown in dual-boot mode isn't currently enforced. The Format EFI step has no dual-boot guard and will format an existing EFI partition — e.g. wiping a Windows bootloader — if you choose "Format" there. Avoid formatting your ESP in dual-boot mode.

## Changes from the original upstream fork

- Removed **Xlibre**, this was added to the original script when xlibre was the artix default x session. I switched it back to **X11** as xlibre is no longer officially supported on artix.
- Added **Cinnamon**, **MATE**, **Plasma**, **Niri**, **Wayfire**, **labwc**, **Sway**, and **Qtile** as desktop options
- Added **MangoWM**, **Umbriel** and **SwayFX**, each gated on live availability in Arch `[extra]` or the CachyOS repo
- Added **Cosmalia** (dinit only), a minimal COSMIC + Noctalia desktop from [grigio/cosmalia-session-dinit](https://github.com/grigio/cosmalia-session-dinit). Its dependencies come from Arch `[extra]`, and the session package is built from its PKGBUILD during install
- Added **noctalia-greeter** as an optional greetd greeter, gated the same way
- Added an optional **zswap** prompt to the swap step
- Added a **User services** step (userspawn or turnstile, matching what Artix packages for your init)
- Installs the per-init pipewire/wireplumber service packages (e.g. `pipewire-dinit`, `wireplumber-dinit`) when they exist for your init
- Removed **ZFS**. ZFS isn't part of the Linux kernel, so its modules have to be built against each kernel version (DKMS) and can lag behind the newest kernels. It also isn't in Artix's main repos (it lives in `[universe]` or the third-party archzfs repo), and a ZFS root needs an initramfs hook, boot parameters and import services that this installer never set up. Choosing it would format the disk and then leave a system that couldn't boot, so I took the option out rather than ship a broken one. If you want ZFS, follow the [Artix ZFS installation guide](https://wiki.artixlinux.org/Main/InstallationOnZFS)
- Removed **XMonad** — along with it, a `git clone` of `github.com/feribsd/xmonad-dotfiles`, a repo whose owning account no longer exists and could be re-registered by anyone
- Fixed a display-manager/autologin ordering bug where bare window managers combined with another desktop could incorrectly get tty1 autologin


## Rust installer prototype (experimental)

The Rust implementation lives in `rust/` and is still under development; it does not replace the Bash installer yet. It is built ahead of time for the live environment, so **users do not need Rust, Cargo, or a compiler installed on the Artix ISO**.

The GitHub Actions workflow [Build standalone Linux binaries](.github/workflows/standalone-binaries.yml) builds x86_64 GNU and musl executables, smoke-tests them, and packages each with a SHA-256 checksum. The workflow runs on pushes to this feature branch and can also be started manually from the Actions tab to produce downloadable workflow artifacts. Push a version tag beginning with `v` to publish both archives as GitHub Release assets.

The musl build is intended to be statically linked and is checked for an ELF interpreter in CI. It has now been tested on an Artix live ISO: the executable launched, its SHA-256 checksum verified, configuration validation passed, and all 17 packages in the test profile were found in the enabled sync databases. This test did not run any disk-writing or package-installation stage. The GNU build is provided as a compatibility fallback.

For source-build instructions, checksum guidance, and verification of GitHub's signed build-provenance attestations, see [SUPPLY_CHAIN.md](rust/docs/SUPPLY_CHAIN.md). A committed Cargo.lock, locked builds, reproducible-build checks, and an Artix package recipe are still outstanding.

The executable is named `agsi`. Extract the archive and run it from a terminal:

```sh
tar -xzf agsi-x86_64-linux-musl.tar.gz
cd agsi-musl
./agsi
```

This prototype currently supports guarded automatic partition-table writing, unencrypted ext4 root formatting, EFI preparation, mounting, fstab generation, and a separately confirmed Artix basestrap stage for automatic UEFI/ext4 targets. It does not yet configure the installed system, users, bootloader, services, or reboot, so it is not a complete installer. Do not use it for an actual installation yet.
