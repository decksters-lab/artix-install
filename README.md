# artix-install

A whiptail-driven installer for Artix Linux. Originally based on a script from `github.com/feribsd/artix-install`, which no longer exists.
(Feel free to fork this repo.. I wish I had done that with the original script. Luckily I found the script on my drive and adapted it from there. This is why my install instructions direct you to download the script rather than just piping it into bash. When you have a local copy nobody can delete a repo and take it away from you)

## Requirements

- Booted from the Artix live ISO, with a network connection (can be found at https://artixlinux.org/download.php .. you can use any weekly or official iso. Use one with a graphical session if its easier for you. It doesn't matter which kind you choose.) note: with a graphical installer it makes it easy to copy the install script, or the folder containing the install script to another location like a second hdd or a usb drive that isn't running the iso then the script is always available to you locally.
- Script Must be run as root either with su before all other commands or with sudo right before running ./artix-install.sh

## Usage ( run ./artix-install.sh with sudo or as root user / su )

Copy this one line, paste it, hit enter. It calls `su -c` for you, so there is no longer a separate "enter su first" step:

```bash
su -c 'pacman -Syy; pacman -S --needed --noconfirm git; git clone https://github.com/decksters-lab/artix-install.git; cd artix-install; chmod +x artix-install.sh; ./artix-install.sh'
```

`su` will ask for the root password before anything else runs (on the official Artix live ISOs that password is `artix`; check your ISO's release notes if it isn't). Everything after that runs as root, in the same terminal, so the wizard still works normally. The `git clone` means you still end up with a local copy of the script — nothing is piped straight into bash.

Already root, or prefer to call it from a sudo shell? Then just paste the inner part:

```bash
pacman -Syy; pacman -S --needed --noconfirm git; git clone https://github.com/decksters-lab/artix-install.git; cd artix-install; chmod +x artix-install.sh; ./artix-install.sh
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
- Added **Cinnamon**, **Plasma**, **Niri**, and **Wayfire** as desktop options
- Added **MangoWM**, gated on live availability in Arch `[extra]` or the CachyOS repo
- Added **noctalia-greeter** as an optional greetd greeter, gated the same way
- Removed **XMonad** — along with it, a `git clone` of `github.com/feribsd/xmonad-dotfiles`, a repo whose owning account no longer exists and could be re-registered by anyone
- Fixed a display-manager/autologin ordering bug where bare window managers combined with another desktop could incorrectly get tty1 autologin

