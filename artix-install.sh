#!/bin/bash
set -e
set -o pipefail

# Detect firmware type
[ -d /sys/firmware/efi ] && UEFI=1 || UEFI=0

# Restore terminal and show log if install fails
 trap 'echo ""
       echo "INSTALL FAILED at line $LINENO"
       echo ""
       umount -R /mnt 2>/dev/null || true
       exit 1' ERR

# unmount/cleanup everything from a previous run so the script is re-runnable
swapoff -a 2>/dev/null || true
umount -R /mnt 2>/dev/null || true
# close any leftover LUKS mapping
cryptsetup close cryptroot 2>/dev/null || true

# =========================
# ROOT CHECK
# =========================
if [ "$EUID" -ne 0 ]; then
    echo "Run as root."
    exit 1
fi

clear
TITLE="Artix Master Installer"

declare -A PART_FS
EXTRA_MOUNTS=()

# MangoWM is only offered if it exists in Arch [extra] or the CachyOS repo.
# check_mango_repo (below) fills these in; MANGO_REPO stays empty if neither has it.
MANGO_CHECKED=0
MANGO_REPO=""          # extra | cachyos | ""
MANGO_PKG="mangowm"    # package name found (mangowm, or its old name mangowc)

# noctalia-greeter is only offered if it exists in Arch [extra] or the CachyOS repo
NOCTALIA_GREETER_CHECKED=0
NOCTALIA_GREETER_REPO=""   # extra | cachyos | ""
NOCTALIA_GREETER_SELECTED=""   # "yes" once the user opts in (set in the wizard)

# fs_key: sanitize a device path to use as assoc array key (/dev/sda1 -> dev_sda1)
fs_key() { echo "${1//\//_}" | sed 's/^_//'; }

# =========================
# TEST / FAST MODE
# =========================
# Pass --test as first arg to skip all prompts and do a fast CLI install
# Uses: first disk found, ext4, no swap, no encrypt, dinit, NM, linux kernel,
#       AMD cpu/gpu, GRUB, no DE, hostname=artix, user=user, pw=idk
TEST_MODE=0
if [ "${1:-}" = "--test" ]; then
    TEST_MODE=1
    DISK=$(lsblk -dpno NAME | grep -v loop | head -1)
    FS="ext4"
    SWAP="None"
    ENCRYPT=0; REAL_ROOT=""; LUKS_CMDLINE=""; LUKS_PW=""
    INIT="dinit"
    LOCALE="en_US.UTF-8"
    TIMEZONE="Europe/London"
    KB_LAYOUT="us"; VC_KEYMAP="us"
    HOSTNAME="artix"
    USERNAME="user"
    ROOTPW="idk"; USERPW="idk"
    INSTALL_TYPE="CLI"; DE_CHOICES="CLI"
    KERNEL_CHOICES="linux"; FIRST_KERNEL="linux"
    CPU_VENDOR="amd"; UCODE="amd-ucode"
    GPU_CHOICE="amd"; GPU="mesa vulkan-radeon"
    UEFI_ORIG="$UEFI"
    BOOT="grub"
    NET_CHOICE="NM"
    AUDIO_PKGS=""
    PRIV_ESC="doas"
    MULTILIB=0
    # partition layout
    [[ "$DISK" =~ (nvme|mmcblk) ]] && P="p" || P=""
    if [ "$UEFI" = "1" ]; then
        PART_DEVS=("${DISK}${P}1" "${DISK}${P}2")
        PART_SIZES=("1" "0")
        PART_TYPES=("EFI" "root")
        EFI="${DISK}${P}1"; ROOT="${DISK}${P}2"
    else
        PART_DEVS=("${DISK}${P}1")
        PART_SIZES=("0")
        PART_TYPES=("root")
        EFI=""; ROOT="${DISK}${P}1"
    fi
    DUALBOOT=0; SWAP_PART=""
    EXTRA_MOUNTS=()
    ZFS_ROOT=0
    echo "==> TEST MODE: disk=$DISK boot=$BOOT uefi=$UEFI"
fi

if [ "$TEST_MODE" = "0" ]; then
    whiptail --title "$TITLE" --msgbox "WARNING: This will erase the selected disk.\nMake sure you have backups.\n\nPress Enter to begin." 10 55
fi

# =========================
# HELPERS
# =========================
get_password() {
    local prompt="$1"
    local p1 p2
    while true; do
        p1=$(whiptail --title "$TITLE" --passwordbox "$prompt" 10 60 3>&1 1>&2 2>&3) || exit 1
        p2=$(whiptail --title "$TITLE" --passwordbox "Confirm $prompt" 10 60 3>&1 1>&2 2>&3) || exit 1
        [ "$p1" = "$p2" ] && echo "$p1" && return
        whiptail --title "$TITLE" --msgbox "Passwords do not match." 8 40
    done
}

# no pick() helper needed — using curated menus instead

# ---- MangoWM repo availability ------------------------------------------------
# _db_pkgs <url> — print "name-version-release" for every package in a pacman sync db.
# Never fails (set -e safe): an unreachable mirror just yields an empty list.
_db_pkgs() {
    curl -fsSL --max-time 90 "$1" 2>/dev/null | bsdtar -tf - 2>/dev/null | sed -n -e 's#^\./##' -e 's#/$##p' || true
}

# _arch_extra_has <pkg> — is <pkg> in Arch [extra]?
# Uses the archlinux.org package API; if that is unreachable/blocked, falls back
# to listing the extra.db sync database from the Arch geo mirror.
_arch_extra_has() {
    local _out
    _out=$(curl -fsS --max-time 15 \
        "https://archlinux.org/packages/search/json/?repo=Extra&arch=x86_64&name=$1" 2>/dev/null) || _out=""
    if grep -q '"valid": *true' <<< "$_out"; then
        # API answered — trust it either way
        if grep -q "\"pkgname\": *\"$1\"" <<< "$_out"; then return 0; else return 1; fi
    fi
    if command -v bsdtar >/dev/null 2>&1; then
        [ -z "${_EXTRA_DB_LIST+x}" ] && \
            _EXTRA_DB_LIST=$(_db_pkgs "https://geo.mirror.pkgbuild.com/extra/os/x86_64/extra.db")
        if grep -q "^$1-[0-9]" <<< "${_EXTRA_DB_LIST:-}"; then return 0; fi
    fi
    return 1
}

# check_mango_repo — sets MANGO_REPO / MANGO_PKG. Result is cached so stepping
# back and forth in the wizard doesn't re-query the network.
check_mango_repo() {
    [ "$MANGO_CHECKED" = "1" ] && return 0
    MANGO_CHECKED=1
    command -v curl >/dev/null 2>&1 || return 0
    local _p _cachy
    # 1) Arch [extra] (preferred — nothing but the package itself gets pulled in)
    for _p in mangowm mangowc; do
        if _arch_extra_has "$_p"; then MANGO_REPO="extra"; MANGO_PKG="$_p"; return 0; fi
    done
    # 2) CachyOS repo
    if command -v bsdtar >/dev/null 2>&1; then
        _cachy=$(_db_pkgs "https://mirror.cachyos.org/repo/x86_64/cachyos/cachyos.db")
        for _p in mangowm mangowc; do
            if grep -q "^${_p}-[0-9]" <<< "$_cachy"; then MANGO_REPO="cachyos"; MANGO_PKG="$_p"; return 0; fi
        done
    fi
    return 0
}

# check_noctalia_greeter_repo — sets NOCTALIA_GREETER_REPO. Same caching approach as check_mango_repo.
check_noctalia_greeter_repo() {
    [ "$NOCTALIA_GREETER_CHECKED" = "1" ] && return 0
    NOCTALIA_GREETER_CHECKED=1
    command -v curl >/dev/null 2>&1 || return 0
    if _arch_extra_has "noctalia-greeter"; then NOCTALIA_GREETER_REPO="extra"; return 0; fi
    if command -v bsdtar >/dev/null 2>&1; then
        _cachy=$(_db_pkgs "https://mirror.cachyos.org/repo/x86_64/cachyos/cachyos.db")
        if grep -q "^noctalia-greeter-[0-9]" <<< "$_cachy"; then NOCTALIA_GREETER_REPO="cachyos"; return 0; fi
    fi
    return 0
}

# per-partition filesystem picker
pick_fs() {
    local _title="$1" _default="${2:-ext4}"
    whiptail --title "$TITLE" --menu "$_title" 18 65 10 \
        "ext4"   "Ext4 — solid, widely supported" \
        "btrfs"  "Btrfs — snapshots, compression" \
        "xfs"    "XFS — high performance" \
        "f2fs"   "F2FS — flash-friendly" \
        "zfs"    "ZFS — advanced (needs zfs-dkms)" \
        "jfs"    "JFS — low CPU journaled FS" \
        "nilfs2" "NILFS2 — continuous snapshots" \
        "vfat"   "FAT32 — for EFI/compatibility" \
        "exfat"  "exFAT — large files, cross-platform" \
        "ntfs"   "NTFS — Windows compatibility" \
        3>&1 1>&2 2>&3
}

# =========================
# =========================
# INIT SYSTEM
# =========================
RAM_KB=$(grep MemTotal /proc/meminfo | awk '{print $2}')

if [ "$TEST_MODE" = "0" ]; then

# Step-based Q&A with back navigation
# Each step sets variables; pressing Cancel goes back one step
STEP=1
STEP_MAX=13

# defaults (overwritten by each step)
INIT="dinit"
DISK=""
FS="ext4"
SWAP="None"
SWAP_SIZE_GB=4
SWAP_SIZE_MB=4096
ENCRYPT=0; REAL_ROOT=""; LUKS_CMDLINE=""; LUKS_PW=""
LOCALE="en_US.UTF-8"
TIMEZONE="Europe/London"
KB_LAYOUT="us"
HOSTNAME="artix"
USERNAME="user"
ROOTPW=""; USERPW=""
INSTALL_TYPE="CLI"; DE_CHOICES="CLI"
KERNEL_CHOICES="linux"; FIRST_KERNEL="linux"
CPU_VENDOR="amd"; UCODE="amd-ucode"
GPU_CHOICE="vm"; GPU="mesa"
BOOT="grub"
NET_CHOICE="NM"
PRIV_ESC="doas"
MULTILIB=0

RAM_HALF_GB=$(( (RAM_KB / 1024 / 1024 + 1) / 2 ))
(( RAM_HALF_GB < 1  )) && RAM_HALF_GB=1
(( RAM_HALF_GB > 16 )) && RAM_HALF_GB=16

while true; do
case "$STEP" in

1) # Init system
    _v=$(whiptail --title "$TITLE" --menu "Init System  [1/$STEP_MAX]" 16 65 4 \
        "dinit"  "dinit  — fast, dependency-based (recommended)" \
        "openrc" "openrc — traditional, widely supported" \
        "runit"  "runit  — minimal, supervision-based" \
        "s6"     "s6     — small, fast, supervision-based" \
        3>&1 1>&2 2>&3) || exit 1
    INIT="$_v"; STEP=$(( STEP + 1 )) ;;

2) # Disk
    mapfile -t disklist < <(lsblk -dpno NAME,SIZE | grep -v loop | awk '{print $1; print $2}')
    _v=$(whiptail --title "$TITLE" --menu "Select Disk  [2/$STEP_MAX]" 20 70 10 \
        "${disklist[@]}" 3>&1 1>&2 2>&3) || { STEP=$(( STEP - 1 )); continue; }
    DISK="$_v"; STEP=$(( STEP + 1 )) ;;

3) # Filesystem
    _v=$(whiptail --title "$TITLE" --menu "Root Filesystem  [3/$STEP_MAX]" 16 65 8 \
        "ext4"  "Ext4 — solid, widely supported (recommended)" \
        "btrfs" "Btrfs — snapshots, compression, subvolumes" \
        "xfs"   "XFS — high performance, large files" \
        "f2fs"  "F2FS — flash-friendly (SSDs/NVMe)" \
        "zfs"   "ZFS — advanced, needs zfs-dkms (experimental)" \
        "jfs"   "JFS — IBM journaled FS, low CPU usage" \
        "nilfs2" "NILFS2 — continuous snapshotting" \
        3>&1 1>&2 2>&3) || { STEP=$(( STEP - 1 )); continue; }
    FS="$_v"; STEP=$(( STEP + 1 )) ;;

4) # Swap
    _v=$(whiptail --title "$TITLE" --menu "Swap  [4/$STEP_MAX]" 14 65 5 \
        "Zram"      "zram — compressed RAM swap (recommended)" \
        "Swapfile"  "Swapfile on root partition" \
        "Both"      "Zram + Swapfile" \
        "Partition" "Dedicated swap partition (auto layout only)" \
        "None"      "No swap" \
        3>&1 1>&2 2>&3) || { STEP=$(( STEP - 1 )); continue; }
    SWAP="$_v"
    if [[ "$SWAP" =~ Swapfile|Both|Partition ]]; then
        SWAP_MENU_ARGS=()
        for SZ in 1 2 4 8 16; do
            (( SZ == RAM_HALF_GB )) \
                && SWAP_MENU_ARGS+=("$SZ" "${SZ}GB  <- recommended") \
                || SWAP_MENU_ARGS+=("$SZ" "${SZ}GB")
        done
        _v=$(whiptail --title "$TITLE" --menu "Swapfile Size  [4/$STEP_MAX]" 13 60 5 \
            "${SWAP_MENU_ARGS[@]}" 3>&1 1>&2 2>&3) || continue
        SWAP_SIZE_GB="$_v"; SWAP_SIZE_MB=$(( SWAP_SIZE_GB * 1024 ))
    fi
    STEP=$(( STEP + 1 )) ;;

5) # Encryption
    if whiptail --title "$TITLE" --yesno \
        "Encryption  [5/$STEP_MAX]\n\nEnable full disk encryption (LUKS2)?\nYou will enter a passphrase on every boot." \
        10 60; then
        ENCRYPT=1
        LUKS_PW=$(get_password "Encryption Passphrase")
    else
        ENCRYPT=0; LUKS_PW=""
    fi
    STEP=$(( STEP + 1 )) ;;

6) # Locale + Timezone + Keyboard (one screen each, all in step 6 — back returns to step 5)
    _v=$(whiptail --title "$TITLE" --menu "Locale  [6/$STEP_MAX]" 20 60 12 \
        "en_US.UTF-8" "English (US)"            "en_GB.UTF-8" "English (UK)" \
        "en_AU.UTF-8" "English (Australia)"     "en_CA.UTF-8" "English (Canada)" \
        "de_DE.UTF-8" "German"                  "fr_FR.UTF-8" "French" \
        "es_ES.UTF-8" "Spanish"                 "it_IT.UTF-8" "Italian" \
        "pt_BR.UTF-8" "Portuguese (Brazil)"     "pt_PT.UTF-8" "Portuguese (Portugal)" \
        "ru_RU.UTF-8" "Russian"                 "pl_PL.UTF-8" "Polish" \
        "nl_NL.UTF-8" "Dutch"                   "sv_SE.UTF-8" "Swedish" \
        "nb_NO.UTF-8" "Norwegian"               "da_DK.UTF-8" "Danish" \
        "fi_FI.UTF-8" "Finnish"                 "hu_HU.UTF-8" "Hungarian" \
        "cs_CZ.UTF-8" "Czech"                   "sk_SK.UTF-8" "Slovak" \
        "hr_HR.UTF-8" "Croatian"                "ro_RO.UTF-8" "Romanian" \
        "uk_UA.UTF-8" "Ukrainian"               "tr_TR.UTF-8" "Turkish" \
        "ja_JP.UTF-8" "Japanese"                "ko_KR.UTF-8" "Korean" \
        "zh_CN.UTF-8" "Chinese (Simplified)" \
        3>&1 1>&2 2>&3) || { STEP=$(( STEP - 1 )); continue; }
    LOCALE="$_v"
    _v=$(whiptail --title "$TITLE" --menu "Timezone  [6/$STEP_MAX]" 20 62 12 \
        "Europe/London"      "UK"                 "Europe/Dublin"     "Ireland" \
        "Europe/Paris"       "France/CET"         "Europe/Berlin"     "Germany" \
        "Europe/Amsterdam"   "Netherlands"        "Europe/Madrid"     "Spain" \
        "Europe/Rome"        "Italy"              "Europe/Warsaw"     "Poland" \
        "Europe/Stockholm"   "Sweden"             "Europe/Oslo"       "Norway" \
        "Europe/Copenhagen"  "Denmark"            "Europe/Helsinki"   "Finland" \
        "Europe/Budapest"    "Hungary"            "Europe/Prague"     "Czech Republic" \
        "Europe/Bratislava"  "Slovakia"           "Europe/Zagreb"     "Croatia" \
        "Europe/Bucharest"   "Romania"            "Europe/Kiev"       "Ukraine" \
        "Europe/Istanbul"    "Turkey"             "Europe/Moscow"     "Russia" \
        "America/New_York"   "US Eastern"         "America/Chicago"   "US Central" \
        "America/Denver"     "US Mountain"        "America/Los_Angeles" "US Pacific" \
        "America/Toronto"    "Canada Eastern"     "America/Sao_Paulo" "Brazil" \
        "Asia/Tokyo"         "Japan"              "Asia/Seoul"        "Korea" \
        "Asia/Shanghai"      "China"              "Australia/Sydney"  "Australia" \
        3>&1 1>&2 2>&3) || { STEP=$(( STEP - 1 )); continue; }
    TIMEZONE="$_v"
    _v=$(whiptail --title "$TITLE" --menu "Keyboard Layout  [6/$STEP_MAX]" 20 60 12 \
        "us" "English (US)"        "gb" "English (UK)" \
        "us-intl" "English (Intl)" "de" "German" \
        "de-latin1" "German (L1)"  "fr" "French" \
        "fr-bepo" "French (Bepo)"  "es" "Spanish" \
        "it" "Italian"             "pt" "Portuguese" \
        "br" "Portuguese (BR)"     "br-abnt2" "Portuguese (ABNT2)" \
        "ru" "Russian"             "pl" "Polish" \
        "nl" "Dutch"               "sv" "Swedish" \
        "no" "Norwegian"           "dk" "Danish" \
        "fi" "Finnish"             "hu" "Hungarian" \
        "cz" "Czech"               "cz-qwerty" "Czech (QWERTY)" \
        "sk" "Slovak"              "hr" "Croatian" \
        "ro" "Romanian"            "bg" "Bulgarian" \
        "gr" "Greek"               "tr" "Turkish" \
        "ua" "Ukrainian"           "dvorak" "Dvorak" \
        "colemak" "Colemak" \
        3>&1 1>&2 2>&3) || { STEP=$(( STEP - 1 )); continue; }
    KB_LAYOUT="$_v"
    STEP=$(( STEP + 1 )) ;;

7) # Hostname + username
    _v=$(whiptail --title "$TITLE" --inputbox "Hostname  [7/$STEP_MAX]" 10 60 "${HOSTNAME:-artix}" \
        3>&1 1>&2 2>&3) || { STEP=$(( STEP - 1 )); continue; }
    [[ "$_v" =~ ^[a-zA-Z0-9\-]+$ ]] && HOSTNAME="$_v" || \
        { whiptail --title "$TITLE" --msgbox "Invalid hostname. Use letters, numbers, hyphens only." 8 50; continue; }
    _v=$(whiptail --title "$TITLE" --inputbox "Username  [7/$STEP_MAX]" 10 60 "${USERNAME:-user}" \
        3>&1 1>&2 2>&3) || { STEP=$(( STEP - 1 )); continue; }
    [[ "$_v" =~ ^[a-z][a-z0-9_\-]*$ ]] && USERNAME="$_v" || \
        { whiptail --title "$TITLE" --msgbox "Invalid username. Use lowercase letters, numbers, _ or -." 8 55; continue; }
    ROOTPW=$(get_password "Root Password  [7/$STEP_MAX]")
    USERPW=$(get_password "User Password  [7/$STEP_MAX]")
    STEP=$(( STEP + 1 )) ;;

8) # Privilege escalation
    _v=$(whiptail --title "$TITLE" --menu "Privilege Escalation  [8/$STEP_MAX]" 12 60 2 \
        "doas"  "doas  — minimal, OpenBSD-style (recommended)" \
        "sudo"  "sudo  — standard, widely compatible" \
        3>&1 1>&2 2>&3) || { STEP=$(( STEP - 1 )); continue; }
    PRIV_ESC="$_v"; STEP=$(( STEP + 1 )) ;;

9) # Desktop
    _v=$(whiptail --title "$TITLE" --menu "Installation Type  [9/$STEP_MAX]" 10 60 2 \
        "DE"  "Desktop Environment / Window Manager" \
        "CLI" "CLI only" \
        3>&1 1>&2 2>&3) || { STEP=$(( STEP - 1 )); continue; }
    INSTALL_TYPE="$_v"
    DE_CHOICES="CLI"
    if [ "$INSTALL_TYPE" = "DE" ]; then
        if [ "$MANGO_CHECKED" = "0" ]; then
            whiptail --title "$TITLE" --infobox "Checking Arch extra / CachyOS repos for MangoWM..." 7 60
        fi
        check_mango_repo
        _de_args=(
            "Plasma"   "KDE Plasma"              OFF
            "XFCE"     "XFCE4"                   OFF
            "Cinnamon" "Cinnamon (X11)"          OFF
            "LXQt"     "LXQt"                    OFF
            "i3"       "i3wm"                    OFF
            "Openbox"  "Openbox"                 OFF
            "Fluxbox"  "Fluxbox"                 OFF
            "IceWM"    "IceWM"                   OFF
            "Hyprland" "Hyprland (Wayland)"      OFF
            "Niri"     "Niri (Wayland)"          OFF
            "Wayfire"  "Wayfire (Wayland)"        OFF
            "Moksha"   "Moksha"                  OFF
            "Cosmic"   "COSMIC [EXPERIMENTAL]"   OFF
        )
        # MangoWM only shows up if Arch extra or the CachyOS repo actually has it
        case "$MANGO_REPO" in
            extra)   _de_args+=("MangoWM" "MangoWM (Wayland) — from Arch extra"  OFF) ;;
            cachyos) _de_args+=("MangoWM" "MangoWM (Wayland) — from CachyOS repo" OFF) ;;
        esac
        _de_n=$(( ${#_de_args[@]} / 3 ))
        DE_CHOICES=$(whiptail --title "$TITLE" --checklist \
            "Select DE/WM  [9/$STEP_MAX]  (space=toggle)" $(( _de_n + 9 )) 70 "$_de_n" \
            "${_de_args[@]}" \
            3>&1 1>&2 2>&3) || { STEP=$(( STEP - 1 )); continue; }
        DE_CHOICES=$(echo "$DE_CHOICES" | tr -d '"')

        # noctalia-greeter: a greetd greeter, so it's offered whenever at least one
        # selected session is one it can hand off to (Hyprland/MangoWM/Niri/Wayfire),
        # even alongside an X11 DE — but not alongside Cosmic, which brings its own
        # dedicated cosmic-greeter and always wins (see the DM/install logic below).
        # Also gated on it actually being available in Arch extra or the CachyOS repo.
        _ng_has_wayland=0
        _ng_has_cosmic=0
        for _d in $DE_CHOICES; do
            case "$_d" in
                Hyprland|MangoWM|Niri|Wayfire) _ng_has_wayland=1 ;;
                Cosmic) _ng_has_cosmic=1 ;;
            esac
        done
        if [ "$_ng_has_wayland" = "1" ] && [ "$_ng_has_cosmic" = "0" ]; then
            if [ "$NOCTALIA_GREETER_CHECKED" = "0" ]; then
                whiptail --title "$TITLE" --infobox "Checking Arch extra / CachyOS repos for noctalia-greeter..." 7 65
            fi
            check_noctalia_greeter_repo
            if [ -n "$NOCTALIA_GREETER_REPO" ]; then
                _ng_msg="Use noctalia-greeter  [9/$STEP_MAX]\n\nA GUI login/session picker (from $NOCTALIA_GREETER_REPO), instead of plain greetd?"
                _ng_mixed=""
                for _d in $DE_CHOICES; do
                    case "$_d" in
                        Hyprland|MangoWM|Niri|Wayfire) ;;
                        *) _ng_mixed="$_d" ;;
                    esac
                done
                if [ -n "$_ng_mixed" ]; then
                    _ng_msg="Use noctalia-greeter  [9/$STEP_MAX]\n\nThis becomes the ONLY login screen, replacing the one $_ng_mixed would otherwise use. It isn't confirmed to list non-Wayland sessions, so $_ng_mixed may not be reachable from it.\n\nUse noctalia-greeter anyway?"
                fi
                if whiptail --title "$TITLE" --yesno "$_ng_msg" 13 72; then
                    NOCTALIA_GREETER_SELECTED="yes"
                else
                    NOCTALIA_GREETER_SELECTED=""
                fi
            else
                NOCTALIA_GREETER_SELECTED=""
            fi
        else
            NOCTALIA_GREETER_SELECTED=""
        fi
        [ -z "$DE_CHOICES" ] && DE_CHOICES="CLI"
    fi
    STEP=$(( STEP + 1 )) ;;

10) # Kernel + CPU + GPU
    KERNEL_CHOICES=$(whiptail --title "$TITLE" --checklist \
        "Kernel  [10/$STEP_MAX]" 14 70 5 \
        "linux"         "Standard"                                    ON  \
        "linux-lts"     "LTS — long term support"                     OFF \
        "linux-zen"     "Zen — desktop optimised"                     OFF \
        "linux-lqx"     "Liquorix — low latency"                      OFF \
        "linux-cachyos" "CachyOS — BORE scheduler (adds CachyOS repo)" OFF \
        3>&1 1>&2 2>&3) || { STEP=$(( STEP - 1 )); continue; }
    KERNEL_CHOICES=$(echo "$KERNEL_CHOICES" | tr -d '"')
    [ -z "$KERNEL_CHOICES" ] && KERNEL_CHOICES="linux"
    FIRST_KERNEL=$(echo "$KERNEL_CHOICES" | awk '{print $1}')
    _v=$(whiptail --title "$TITLE" --menu "CPU Vendor  [10/$STEP_MAX]" 10 50 3 \
        "intel" "Intel" "amd" "AMD" "other" "Other / VM" \
        3>&1 1>&2 2>&3) || { STEP=$(( STEP - 1 )); continue; }
    CPU_VENDOR="$_v"
    case "$CPU_VENDOR" in
        intel) UCODE="intel-ucode" ;; amd) UCODE="amd-ucode" ;; *) UCODE="" ;;
    esac
    _v=$(whiptail --title "$TITLE" --menu "GPU  [10/$STEP_MAX]" 13 60 5 \
        "intel"  "Intel iGPU" \
        "amd"    "AMD" \
        "nvidia" "Nvidia" \
        "hybrid" "Hybrid Intel+Nvidia" \
        "vm"     "VM / none (mesa only)" \
        3>&1 1>&2 2>&3) || { STEP=$(( STEP - 1 )); continue; }
    GPU_CHOICE="$_v"
    case "$GPU_CHOICE" in
        intel)  GPU="mesa vulkan-intel" ;;
        amd)    GPU="mesa vulkan-radeon" ;;
        nvidia) GPU="mesa nvidia-dkms nvidia-utils" ;;
        hybrid) GPU="mesa vulkan-intel nvidia-dkms nvidia-utils" ;;
        vm)     GPU="mesa" ;;
    esac
    STEP=$(( STEP + 1 )) ;;

11) # Bootloader
    if [ "$UEFI" = "1" ]; then
        _v=$(whiptail --title "$TITLE" --menu "Bootloader  [11/$STEP_MAX]" 12 65 3 \
            "grub"   "GRUB2 — most compatible, required for dual-boot" \
            "limine" "Limine — fast, minimal" \
            "refind" "rEFInd — graphical" \
            3>&1 1>&2 2>&3) || { STEP=$(( STEP - 1 )); continue; }
        BOOT="$_v"
    else
        BOOT="grub"
    fi
    STEP=$(( STEP + 1 )) ;;

12) # 32-bit / multilib
    if whiptail --title "$TITLE" --yesno \
        "32-bit support  [12/$STEP_MAX]\n\nEnable the multilib repo?\n\nRequired for: Steam, Wine, 32-bit games and apps.\nAdds ~1GB of available packages. Safe to skip if unsure." \
        11 60; then
        MULTILIB=1
    else
        MULTILIB=0
    fi
    STEP=$(( STEP + 1 )) ;;

13) # Network
    _v=$(whiptail --title "$TITLE" --menu "Network Stack  [13/$STEP_MAX]" 13 65 3 \
        "dhcpcd" "dhcpcd  — ethernet only, ~2MB" \
        "iwd"    "iwd     — wifi + ethernet, ~5MB" \
        "NM"     "NetworkManager — full featured, ~30MB" \
        3>&1 1>&2 2>&3) || { STEP=$(( STEP - 1 )); continue; }
    NET_CHOICE="$_v"
    STEP=$(( STEP + 1 )) ;;

esac
[ "$STEP" -gt "$STEP_MAX" ] && break
[ "$STEP" -lt 1 ] && STEP=1
done

fi  # end TEST_MODE=0 Q&A

# partition manager
if [ "$TEST_MODE" = "0" ]; then
DISK_SIZE=$(lsblk -bdno SIZE "$DISK" 2>/dev/null || echo 0)
DISK_SIZE_GB=$(( DISK_SIZE / 1024 / 1024 / 1024 ))
EFI=""; ROOT=""; DUALBOOT=0
[[ "$DISK" =~ (nvme|mmcblk) ]] && P="p" || P=""

PART_MODE=$(whiptail --title "$TITLE" --menu \
    "Partitioning — $DISK (${DISK_SIZE_GB}GB)" 13 65 3 \
    "auto"     "Auto — wipe disk, use entire drive (recommended)" \
    "manual"   "Manual — open cfdisk to partition yourself" \
    "dualboot" "Dual-boot — keep existing partitions, pick root" \
    3>&1 1>&2 2>&3) || exit 1

case "$PART_MODE" in
    auto)
        if [ "$UEFI" = "1" ]; then
            if [ "$SWAP" = "Partition" ]; then
                PART_DEVS=( "${DISK}${P}1" "${DISK}${P}2" "${DISK}${P}3" )
                PART_SIZES=( "1" "$SWAP_SIZE_GB" "0" )
                PART_TYPES=( "EFI" "swap" "root" )
                EFI="${DISK}${P}1"; ROOT="${DISK}${P}3"
            else
                PART_DEVS=( "${DISK}${P}1" "${DISK}${P}2" )
                PART_SIZES=( "1" "0" )
                PART_TYPES=( "EFI" "root" )
                EFI="${DISK}${P}1"; ROOT="${DISK}${P}2"
            fi
        else
            if [ "$SWAP" = "Partition" ]; then
                PART_DEVS=( "${DISK}${P}1" "${DISK}${P}2" )
                PART_SIZES=( "$SWAP_SIZE_GB" "0" )
                PART_TYPES=( "swap" "root" )
                ROOT="${DISK}${P}2"
            else
                PART_DEVS=( "${DISK}${P}1" )
                PART_SIZES=( "0" )
                PART_TYPES=( "root" )
                ROOT="${DISK}${P}1"
            fi
        fi
        ;;
    manual)
        whiptail --title "$TITLE" --msgbox \
            "cfdisk will open now.\n\nCreate your partitions and write the table.\nAfter exiting you select which partitions to use and their filesystems." \
            10 62
        cfdisk "$DISK"
        udevadm settle
        mapfile -t _parts < <(lsblk -pno NAME,SIZE,FSTYPE "$DISK" | grep -v "^$DISK " | \
            awk '{print $1; printf "%s %s\n", $2, ($3=="" ? "unformatted" : $3)}')
        if [ "$UEFI" = "1" ]; then
            EFI=$(whiptail --title "$TITLE" --menu "Select EFI partition" \
                16 62 8 "${_parts[@]}" 3>&1 1>&2 2>&3) || exit 1
            _efi_fs=$(pick_fs "Filesystem for EFI partition" "vfat") || _efi_fs="vfat"
            PART_FS["$(fs_key "$EFI")"]="$_efi_fs"
        fi
        ROOT=$(whiptail --title "$TITLE" --menu "Select root partition" \
            16 62 8 "${_parts[@]}" 3>&1 1>&2 2>&3) || exit 1
        _root_fs=$(pick_fs "Filesystem for root partition" "$FS") || _root_fs="$FS"
        FS="$_root_fs"
        PART_FS["$(fs_key "$ROOT")"]="$_root_fs"
        # offer extra partitions (home, data, etc.)
        while true; do
            _remain_args=("skip" "Done — no more partitions")
            for _pp in "${_parts[@]}"; do
                [[ "$_pp" == /dev/* ]] && [ "$_pp" != "$EFI" ] && [ "$_pp" != "$ROOT" ] && \
                    _remain_args+=("$_pp" "$(lsblk -dno SIZE "$_pp" 2>/dev/null)")
            done
            [ ${#_remain_args[@]} -le 2 ] && break
            _extra=$(whiptail --title "$TITLE" --menu \
                "Assign more partitions? (e.g. /home)" \
                16 62 8 "${_remain_args[@]}" 3>&1 1>&2 2>&3) || break
            [ "$_extra" = "skip" ] && break
            _extra_mp=$(whiptail --title "$TITLE" --inputbox \
                "Mount point for $_extra" 10 55 "/home" 3>&1 1>&2 2>&3) || break
            _extra_fs=$(pick_fs "Filesystem for $_extra ($_extra_mp)") || _extra_fs="ext4"
            PART_FS["$(fs_key "$_extra")"]="$_extra_fs"
            EXTRA_MOUNTS+=("$_extra:$_extra_mp")
        done
        PART_DEVS=(); PART_SIZES=(); PART_TYPES=()
        [ "$UEFI" = "1" ] && { PART_DEVS+=("$EFI"); PART_SIZES+=("0"); PART_TYPES+=("EFI"); }
        PART_DEVS+=("$ROOT"); PART_SIZES+=("0"); PART_TYPES+=("root")
        ;;
    dualboot)
        DUALBOOT=1
        mapfile -t allparts < <(lsblk -pno NAME,SIZE,FSTYPE,PARTTYPE | \
            grep -v '^/dev/[a-z]*[[:space:]]' | \
            awk '{print $1; printf "%s %s %s\n", $2, ($3=="" ? "unformatted" : $3), ($4=="" ? "" : "[EFI]")}')
        EFI=$(whiptail --title "$TITLE" --menu \
            "Select your existing EFI partition (do NOT format it)" \
            18 72 10 "${allparts[@]}" 3>&1 1>&2 2>&3) || exit 1
        mapfile -t rootparts < <(lsblk -pno NAME,SIZE,FSTYPE "$DISK" | \
            grep -v "^$DISK " | \
            awk '{print $1; printf "%s %s\n", $2, ($3=="" ? "unformatted" : $3)}')
        ROOT=$(whiptail --title "$TITLE" --menu \
            "Select partition for Artix root (will be formatted as $FS)" \
            18 72 10 "${rootparts[@]}" 3>&1 1>&2 2>&3) || exit 1
        PART_DEVS=("$ROOT"); PART_SIZES=("0"); PART_TYPES=("root")
        ;;
esac
fi  # end TEST_MODE=0 partition manager

if [ "$DUALBOOT" = "0" ] && [ "${PART_MODE:-auto}" != "manual" ]; then
    # Fresh install — wipe and write new partition table via sfdisk
    wipefs -af "$DISK"
    {
        [ "$UEFI" = "1" ] && echo "label: gpt" || echo "label: dos"
        echo "label-id: $(cat /proc/sys/kernel/random/uuid)"
        for i in "${!PART_DEVS[@]}"; do
            SZ="${PART_SIZES[$i]}"
            TYPE="${PART_TYPES[$i]}"
            if [ "$UEFI" = "1" ]; then
                case "$TYPE" in
                    EFI)  TYPECODE="C12A7328-F81F-11D2-BA4B-00A0C93EC93B" ;;
                    swap) TYPECODE="0657FD6D-A4AB-43C4-84E5-0933C84B4F4F" ;;
                    *)    TYPECODE="0FC63DAF-8483-4772-8E79-3D69D8477DE4" ;;
                esac
                [ "$SZ" = "0" ] \
                    && echo "size=+, type=$TYPECODE" \
                    || echo "size=+${SZ}G, type=$TYPECODE"
            else
                case "$TYPE" in
                    swap) TYPECODE="82" ;;
                    *)    TYPECODE="83" ;;
                esac
                BOOTFLAG=""; [ "$TYPE" = "root" ] && BOOTFLAG=", bootable"
                [ "$SZ" = "0" ] \
                    && echo "size=+, type=$TYPECODE$BOOTFLAG" \
                    || echo "size=+${SZ}G, type=$TYPECODE$BOOTFLAG"
            fi
        done
    } | sfdisk "$DISK"
    # Give kernel time to re-read partition table — NVMe needs this
    partprobe "$DISK" 2>/dev/null || true
    udevadm settle --timeout=10
    sleep 1
    # Format swap partition now — activate after mount so fstabgen sees it
    SWAP_PART=""
    for i in "${!PART_TYPES[@]}"; do
        if [ "${PART_TYPES[$i]}" = "swap" ]; then
            SWAP_PART="${PART_DEVS[$i]}"
            mkswap "$SWAP_PART"
        fi
    done
else
    # Dual-boot or manual — partition table already written, just settle
    udevadm settle
    SWAP_PART=""
fi

if [ "$ENCRYPT" = "1" ]; then
    command -v cryptsetup &>/dev/null || pacman -Sy --noconfirm cryptsetup
    echo -n "$LUKS_PW" | cryptsetup luksFormat --type luks2 "$ROOT" -
    echo -n "$LUKS_PW" | cryptsetup open "$ROOT" cryptroot -
    REAL_ROOT="$ROOT"
    ROOT="/dev/mapper/cryptroot"
    PART_FS["$(fs_key "$ROOT")"]="${PART_FS[$(fs_key "$REAL_ROOT")]:-$FS}"
fi

# format_part <device> <fstype>
format_part() {
    local _dev="$1" _fs="${2:-ext4}"
    case "$_fs" in
        ext4)   mkfs.ext4   -F  "$_dev" ;;
        btrfs)  mkfs.btrfs  -f  "$_dev" ;;
        xfs)    mkfs.xfs    -f  "$_dev" ;;
        f2fs)   mkfs.f2fs   -f  "$_dev" ;;
        jfs)    mkfs.jfs    -q  "$_dev" ;;
        nilfs2) mkfs.nilfs2 -f  "$_dev" ;;
        vfat)   mkfs.fat -F32  "$_dev" ;;
        exfat)  mkfs.exfat      "$_dev" ;;
        ntfs)   mkfs.ntfs  -f  "$_dev" ;;
        zfs)
            # ZFS needs zfs-dkms on the live ISO
            if ! command -v zpool &>/dev/null; then
                pacman -Sy --noconfirm zfs-dkms zfs-utils 2>/dev/null || \
                pacman -Sy --noconfirm zfs-linux 2>/dev/null || true
                modprobe zfs 2>/dev/null || true
            fi
            # pool name: root pool = zroot, others use last path component
            local _pool="zroot"
            zpool create -f -o ashift=12 \
                -O acltype=posixacl -O xattr=sa \
                -O dnodesize=auto -O compression=lz4 \
                -O normalization=formD -O relatime=on \
                -O mountpoint=none \
                "$_pool" "$_dev"
            zfs create -o mountpoint=/ "${_pool}/root"
            # export/import so it mounts under /mnt
            zpool export "$_pool"
            zpool import -d /dev -R /mnt "$_pool"
            return
            ;;
        *) echo "==> Unknown FS '$_fs', defaulting to ext4"; mkfs.ext4 -F "$_dev" ;;
    esac
}

# Format EFI (always vfat unless user picked something exotic)
[ "$UEFI" = "1" ] && [ -n "$EFI" ] && {
    _efi_fs="${PART_FS[$(fs_key "$EFI")]:-vfat}"
    format_part "$EFI" "$_efi_fs"
}

# Format root
_root_fs="${PART_FS[$(fs_key "$ROOT")]:-$FS}"
if [ "$_root_fs" = "zfs" ]; then
    format_part "$ROOT" "zfs"
    # ZFS import already mounted /mnt — skip normal mount below
    ZFS_ROOT=1
else
    ZFS_ROOT=0
    format_part "$ROOT" "$_root_fs"
    mount "$ROOT" /mnt
fi

mkdir -p /mnt/boot
[ "$UEFI" = "1" ] && mount "$EFI" /mnt/boot

# Mount extra partitions (from manual mode)
for _em in "${EXTRA_MOUNTS[@]+"${EXTRA_MOUNTS[@]}"}"; do
    [ -z "$_em" ] && continue
    _em_dev="${_em%%:*}"
    _em_mp="${_em##*:}"
    _em_fs="${PART_FS[$(fs_key "$_em_dev")]:-ext4}"
    [ "$_em_fs" != "zfs" ] && format_part "$_em_dev" "$_em_fs"
    mkdir -p "/mnt${_em_mp}"
    [ "$_em_fs" != "zfs" ] && mount "$_em_dev" "/mnt${_em_mp}"
done

# Activate swap partition now so fstabgen picks it up
[ -n "${SWAP_PART:-}" ] && swapon "$SWAP_PART"

# section headers during install
gauge() {
    echo ""
    echo "==> ${2}"
    echo ""
}

# svc_pkg <svc> — returns init-specific package name
svc_pkg() {
    case "$INIT" in
        dinit)  echo "${1}-dinit" ;;
        openrc) echo "${1}-openrc" ;;
        runit)  echo "${1}-runit" ;;
        s6)     echo "${1}-s6" ;;
    esac
}

# svc_enable <svc> — enable service in installed system
# NOTE: for runit/s6/dinit, /run is a tmpfs not yet mounted in chroot,
# so we link into persistent dirs (runsvdir/default or adminsv/default/contents.d)
svc_enable() {
    local _s="$1"
    case "$INIT" in
        dinit)
            if [ -f "/mnt/etc/dinit.d/$_s" ]; then
                artix-chroot /mnt ln -sf "/etc/dinit.d/$_s" /etc/dinit.d/boot.d/
            else
                echo "Warning: dinit service $_s not found"
            fi
            ;;
        openrc)
            artix-chroot /mnt rc-update add "$_s" default 2>/dev/null || \
                echo "Warning: openrc service $_s not found"
            ;;
        runit)
            # Link into runsvdir/default — NOT /run/runit/service (tmpfs, not mounted in chroot)
            if [ -d "/mnt/etc/runit/sv/$_s" ]; then
                mkdir -p /mnt/etc/runit/runsvdir/default
                artix-chroot /mnt ln -sf "/etc/runit/sv/$_s" /etc/runit/runsvdir/default/
            else
                echo "Warning: runit service $_s not found in /etc/runit/sv/"
            fi
            ;;
        s6)
            # Add to default bundle contents.d; s6-db-reload compiles at end
            if [ -d "/mnt/etc/s6/sv/${_s}-srv" ] || [ -d "/mnt/etc/s6/adminsv/$_s" ]; then
                mkdir -p /mnt/etc/s6/adminsv/default/contents.d
                touch "/mnt/etc/s6/adminsv/default/contents.d/$_s"
            else
                echo "Note: s6 service $_s not found in sv/adminsv (may be in base bundle)"
            fi
            ;;
    esac
}

# enable_arch_extra_chroot — add Arch [extra] to the installed system
# (same mechanism MangoWM uses). Returns non-zero on failure.
enable_arch_extra_chroot() {
    if ! grep -q '^\[extra\]' /mnt/etc/pacman.conf; then
        artix-chroot /mnt pacman -S --noconfirm --needed artix-archlinux-support || return 1
        printf '\n# Arch repos\n[extra]\nInclude = /etc/pacman.d/mirrorlist-arch\n' >> /mnt/etc/pacman.conf
        artix-chroot /mnt pacman-key --populate archlinux || return 1
    fi
    artix-chroot /mnt pacman -Sy --noconfirm
}

# enable_cachyos_repo_chroot — add the CachyOS repo to the installed system
# (no-op if the linux-cachyos kernel step already did it). Returns non-zero on
# failure and leaves pacman.conf as it found it.
enable_cachyos_repo_chroot() {
    grep -q '^\[cachyos\]' /mnt/etc/pacman.conf && return 0
    local _sig _ok=1
    _sig=$(grep '^SigLevel' /mnt/etc/pacman.conf | head -1)
    [ -z "$_sig" ] && _sig="SigLevel = Required DatabaseOptional"
    # keyring package can't be verified before we trust the keys — same trick as the kernel path
    sed -i 's/^SigLevel.*/SigLevel = Never/' /mnt/etc/pacman.conf
    printf '\n[cachyos]\nServer = https://mirror.cachyos.org/repo/x86_64/cachyos\n' >> /mnt/etc/pacman.conf
    artix-chroot /mnt pacman -Sy --noconfirm || _ok=0
    if [ "$_ok" = "1" ]; then
        artix-chroot /mnt pacman -S --noconfirm cachyos-keyring cachyos-mirrorlist || _ok=0
    fi
    sed -i "s/^SigLevel.*/${_sig}/" /mnt/etc/pacman.conf
    if [ "$_ok" = "0" ]; then
        sed -i '/^\[cachyos\]/,/^$/d' /mnt/etc/pacman.conf
        return 1
    fi
    artix-chroot /mnt pacman-key --populate cachyos || return 1
    sed -i '/^\[cachyos\]/{n; s|^Server = .*|Include = /etc/pacman.d/cachyos-mirrorlist|}' /mnt/etc/pacman.conf
    artix-chroot /mnt pacman -Sy --noconfirm
}

# swapfile
if [[ "$SWAP" =~ Swapfile|Both ]]; then
    if [[ "$FS" == "btrfs" ]]; then
        truncate -s 0 /mnt/swapfile
        chattr +C /mnt/swapfile
        fallocate -l "${SWAP_SIZE_GB}G" /mnt/swapfile
    else
        dd if=/dev/zero of=/mnt/swapfile bs=1M count="$SWAP_SIZE_MB" status=progress
    fi
    chmod 600 /mnt/swapfile
    mkswap /mnt/swapfile
    swapon /mnt/swapfile  # activate so fstabgen picks it up
fi

# icewm doesnt need mesa/llvm so skip gpu entirely to save ~200mb
BARE_WM_ONLY=1
for _de in Plasma XFCE Cinnamon LXQt Moksha Cosmic Hyprland MangoWM Niri Wayfire i3 Openbox Fluxbox; do
    echo "$DE_CHOICES" | grep -qw "$_de" && BARE_WM_ONLY=0 && break
done
[ "$DE_CHOICES" = "CLI" ] && BARE_WM_ONLY=0
[ "$BARE_WM_ONLY" = "1" ] && GPU=""

# Standard Xorg — XLibre pulled from the Artix repos after the XLibre/Artix dispute
# Skipped only when every selected session is Wayland-only (Cosmic/Hyprland/MangoWM/Niri/Wayfire)
XORG_PKGS=""
if [ "$DE_CHOICES" != "CLI" ]; then
    for _de in $DE_CHOICES; do
        case "$_de" in
            Cosmic|Hyprland|MangoWM|Niri|Wayfire) ;;
            *) XORG_PKGS="xorg-server xorg-server-common xf86-input-libinput xorg-xinit"; break ;;
        esac
    done
fi

# Only install audio stack for DEs that actually use it
AUDIO_PKGS=""
AUDIO_DES="Plasma XFCE Cinnamon LXQt Moksha Cosmic Hyprland MangoWM Niri Wayfire"
for _de in $AUDIO_DES; do
    if echo "$DE_CHOICES" | grep -qw "$_de"; then
        AUDIO_PKGS="pipewire pipewire-pulse pipewire-alsa wireplumber"
        break
    fi
done

# =========================
gauge 15 "Configuring repositories..."
# LIQUORIX REPO (if needed)
# =========================
if echo "$KERNEL_CHOICES" | grep -qw "linux-lqx"; then
    pacman-key --keyserver hkps://keyserver.ubuntu.com --recv-keys 9AE4078033F8024D
    pacman-key --lsign-key 9AE4078033F8024D
    grep -q 'liquorix.net' /etc/pacman.conf || \
        printf '\n[liquorix]\nServer = https://liquorix.net/archlinux/$repo/$arch\n' >> /etc/pacman.conf
    pacman -Sy
fi

# cachyos repo — only needed for linux-cachyos kernel
if echo "$KERNEL_CHOICES" | grep -qw "linux-cachyos"; then
    _cachy_ok=0
    set +e

    # Temporarily set global SigLevel = Never so pacman can install the keyring
    # package before we have CachyOS keys trusted. Per-repo SigLevel is unreliable
    # across pacman versions — global override is the only thing that always works.
    _orig_siglevel=$(grep '^SigLevel' /etc/pacman.conf | head -1)
    [ -z "$_orig_siglevel" ] && _orig_siglevel="SigLevel = Required DatabaseOptional"
    sed -i "s/^SigLevel.*/${_orig_siglevel}/" /etc/pacman.conf  # ensure it exists first
    sed -i 's/^SigLevel.*/SigLevel = Never/' /etc/pacman.conf

    # Add the repo (with a plain Server line, no per-repo SigLevel needed now)
    grep -q '\[cachyos\]' /etc/pacman.conf || printf '\n[cachyos]\nServer = https://mirror.cachyos.org/repo/x86_64/cachyos\n' >> /etc/pacman.conf

    pacman -Sy --noconfirm 2>/dev/null
    pacman -S --noconfirm cachyos-keyring cachyos-mirrorlist

    # Restore global SigLevel and populate the keyring properly
    sed -i "s/^SigLevel.*/${_orig_siglevel}/" /etc/pacman.conf
    pacman-key --populate cachyos

    # Switch repo to use mirrorlist include now that mirrorlist is installed
    sed -i '/^\[cachyos\]/{n; s|^Server = .*|Include = /etc/pacman.d/cachyos-mirrorlist|}' /etc/pacman.conf

    pacman -Sy --noconfirm && pacman -Si linux-cachyos &>/dev/null && _cachy_ok=1

    set -e
    if [ "$_cachy_ok" = "0" ]; then
        # Restore original SigLevel in case it got left as Never
        sed -i "s/^SigLevel.*/${_orig_siglevel}/" /etc/pacman.conf
        sed -i '/^\[cachyos\]/,/^$/d' /etc/pacman.conf
        whiptail --title "$TITLE" --msgbox \
            "CachyOS repo setup failed.\nFalling back to linux kernel." 10 60
        KERNEL_CHOICES=$(echo "$KERNEL_CHOICES" | sed 's/linux-cachyos/linux/g' | tr -s ' ')
        FIRST_KERNEL=$(echo "$KERNEL_CHOICES" | awk '{print $1}')
    fi
fi

gauge 20 "Installing base system (this takes a while)..."
# =========================
# BASESTRAP
# =========================
_do_basestrap() {
    local _k="$1"
    basestrap /mnt \
        base base-devel "$_k" linux-firmware $UCODE \
        $(case "$INIT" in dinit) echo "dinit elogind-dinit dbus-dinit";; openrc) echo "openrc elogind-openrc dbus-openrc";; runit) echo "runit elogind-runit dbus-runit";; s6) echo "s6-base elogind-s6 dbus-s6";; esac) \
        $([ "$NET_CHOICE" = "NM" ] && echo "networkmanager $(svc_pkg networkmanager)") \
        $([ "$PRIV_ESC" = "sudo" ] && echo "sudo" || echo "doas") $([ -n "$AUDIO_PKGS" ] && echo rtkit) \
        ttf-dejavu ttf-liberation noto-fonts \
        $XORG_PKGS \
        $AUDIO_PKGS \
        $GPU
}
if ! _do_basestrap "$FIRST_KERNEL"; then
    echo "==> $FIRST_KERNEL failed, falling back to linux"
    FIRST_KERNEL="linux"
    KERNEL_CHOICES="linux"
    _do_basestrap linux
fi

gauge 35 "Writing fstab..."
if [ "${ZFS_ROOT:-0}" = "1" ]; then
    # ZFS root: generate fstab for non-ZFS mounts only, ZFS handles itself
    fstabgen -U /mnt | grep -v ' / ' >> /mnt/etc/fstab
    # install ZFS support in the target
    artix-chroot /mnt pacman -S --noconfirm zfs-dkms zfs-utils 2>/dev/null || \
        artix-chroot /mnt pacman -S --noconfirm zfs-linux 2>/dev/null || true
    # enable zfs service
    svc_enable zfs-import 2>/dev/null || true
    svc_enable zfs-mount  2>/dev/null || true
else
    fstabgen -U /mnt >> /mnt/etc/fstab
fi

# Encryption setup inside installed system
if [ "$ENCRYPT" = "1" ]; then
    # zstd mkinitcpio builds fail in some chroot environments — gzip is universal
    sed -i 's/^#*COMPRESSION=.*/COMPRESSION="gzip"/' /mnt/etc/mkinitcpio.conf
    grep -q '^COMPRESSION=' /mnt/etc/mkinitcpio.conf || echo 'COMPRESSION="gzip"' >> /mnt/etc/mkinitcpio.conf
    # Set HOOKS before installing cryptsetup so the pacman post-hook builds a
    # correct initramfs. consolefont is omitted — no font is configured at this
    # stage (vconsole.conf is written later) and mkinitcpio 41+ errors on it.
    sed -i 's/^HOOKS=.*/HOOKS=(base udev autodetect microcode modconf kms keyboard keymap block encrypt filesystems fsck)/' /mnt/etc/mkinitcpio.conf
    # Pin btrfs module so autodetect can't filter it out in a chroot environment
    [ "$FS" = "btrfs" ] && \
        sed -i 's/^MODULES=.*/MODULES=(btrfs)/' /mnt/etc/mkinitcpio.conf
    # Ensure cryptsetup is in the installed system
    artix-chroot /mnt pacman -S --noconfirm cryptsetup
    # crypttab — maps cryptroot on boot
    LUKS_UUID=$(blkid -s UUID -o value "$REAL_ROOT")
    echo "cryptroot UUID=$LUKS_UUID none luks" >> /mnt/etc/crypttab
    artix-chroot /mnt mkinitcpio -P
    # Store LUKS UUID for bootloader cmdline
    LUKS_CMDLINE="cryptdevice=UUID=$LUKS_UUID:cryptroot root=/dev/mapper/cryptroot"
fi

# pacman tweaks
sed -i 's/^#Color/Color\nILoveCandy/' /mnt/etc/pacman.conf
sed -i 's/^#ParallelDownloads.*/ParallelDownloads = 10/' /mnt/etc/pacman.conf
# Enable multilib if requested
if [ "$MULTILIB" = "1" ]; then
    grep -q '\[lib32\]' /mnt/etc/pacman.conf || \
        printf '\n[lib32]\nInclude = /etc/pacman.d/mirrorlist\n' >> /mnt/etc/pacman.conf
    artix-chroot /mnt pacman -Sy --noconfirm
fi

# Block legacy xf86-video DDX drivers — modesetting handles everything
# Prevents DE metapackages from pulling them in as optional deps
grep -q 'xf86-video-amdgpu' /mnt/etc/pacman.conf || \
    sed -i '/^\[options\]/a IgnorePkg = xf86-video-amdgpu xf86-video-intel xf86-video-nouveau xf86-video-fbdev xf86-video-vesa' \
    /mnt/etc/pacman.conf

# liquorix repo in the installed system
if echo "$KERNEL_CHOICES" | grep -qw "linux-lqx"; then
    artix-chroot /mnt pacman-key --keyserver hkps://keyserver.ubuntu.com --recv-keys 9AE4078033F8024D
    artix-chroot /mnt pacman-key --lsign-key 9AE4078033F8024D
    grep -q 'liquorix.net' /mnt/etc/pacman.conf || \
        printf '\n[liquorix]\nServer = https://liquorix.net/archlinux/$repo/$arch\n' >> /mnt/etc/pacman.conf
    artix-chroot /mnt pacman -Sy --noconfirm
fi

# cachyos repo in the installed system
if echo "$KERNEL_CHOICES" | grep -qw "linux-cachyos"; then
    _orig_siglevel=$(grep '^SigLevel' /mnt/etc/pacman.conf | head -1)
    [ -z "$_orig_siglevel" ] && _orig_siglevel="SigLevel = Required DatabaseOptional"
    sed -i 's/^SigLevel.*/SigLevel = Never/' /mnt/etc/pacman.conf
    grep -q '\[cachyos\]' /mnt/etc/pacman.conf || \
        printf '\n[cachyos]\nServer = https://mirror.cachyos.org/repo/x86_64/cachyos\n' >> /mnt/etc/pacman.conf
    artix-chroot /mnt pacman -Sy --noconfirm
    artix-chroot /mnt pacman -S --noconfirm cachyos-keyring cachyos-mirrorlist
    sed -i "s/^SigLevel.*/${_orig_siglevel}/" /mnt/etc/pacman.conf
    artix-chroot /mnt pacman-key --populate cachyos
    sed -i '/^\[cachyos\]/{n; s|^Server = .*|Include = /etc/pacman.d/cachyos-mirrorlist|}' /mnt/etc/pacman.conf
    artix-chroot /mnt pacman -Sy --noconfirm
fi

# Install headers for lqx/cachyos if selected as first kernel
if [ "$FIRST_KERNEL" = "linux-lqx" ]; then
    artix-chroot /mnt pacman -S --noconfirm linux-lqx-headers
elif [ "$FIRST_KERNEL" = "linux-cachyos" ]; then
    : # cachyos bundles headers
fi
# copy NM wifi profiles when NM is selected
if [ "$NET_CHOICE" = "NM" ] && [ -d /etc/NetworkManager/system-connections ]; then
    mkdir -p /mnt/etc/NetworkManager/system-connections
    cp /etc/NetworkManager/system-connections/* \
        /mnt/etc/NetworkManager/system-connections/ 2>/dev/null || true
    chmod 600 /mnt/etc/NetworkManager/system-connections/* 2>/dev/null || true
fi

# Extra kernels — skip silently if they fail
for K in $KERNEL_CHOICES; do
    [ "$K" = "$FIRST_KERNEL" ] && continue
    if [ "$K" = "linux-cachyos" ]; then
        artix-chroot /mnt pacman -S --noconfirm linux-cachyos || echo "==> Warning: linux-cachyos failed, skipping"
    elif [ "$K" = "linux-lqx" ]; then
        artix-chroot /mnt pacman -S --noconfirm linux-lqx linux-lqx-headers || echo "==> Warning: linux-lqx failed, skipping"
    else
        artix-chroot /mnt pacman -S --noconfirm "$K" "${K}-headers" || echo "==> Warning: $K failed, skipping"
    fi
done

# locale and timezone
gauge 40 "Setting locale..."
artix-chroot /mnt bash -c "echo '$LOCALE UTF-8' >> /etc/locale.gen && locale-gen"
artix-chroot /mnt bash -c "echo 'LANG=$LOCALE' > /etc/locale.conf"
gauge 45 "Setting timezone..."
artix-chroot /mnt bash -c "ln -sf /usr/share/zoneinfo/$TIMEZONE /etc/localtime && hwclock --systohc"

# keyboard layout
# Map X11 layout name to vconsole keymap (they differ for some layouts)
case "$KB_LAYOUT" in
    us-intl)   VC_KEYMAP="us" ;;
    cz-qwerty) VC_KEYMAP="cz-qwerty" ;;
    fr-bepo)   VC_KEYMAP="fr-bepo" ;;
    br-abnt2)  VC_KEYMAP="br-abnt2" ;;
    de-latin1) VC_KEYMAP="de-latin1" ;;
    jp106)     VC_KEYMAP="jp106" ;;
    *)         VC_KEYMAP="$KB_LAYOUT" ;;
esac
cat > /mnt/etc/vconsole.conf << EOF
KEYMAP=$VC_KEYMAP
FONT=default
EOF
mkdir -p /mnt/etc/X11/xorg.conf.d
cat > /mnt/etc/X11/xorg.conf.d/00-keyboard.conf << KBEOF
Section "InputClass"
    Identifier "system-keyboard"
    MatchIsKeyboard "on"
    Option "XkbLayout" "$KB_LAYOUT"
EndSection
KBEOF

# hostname
gauge 50 "Configuring hostname..."
echo "$HOSTNAME" > /mnt/etc/hostname
printf "127.0.0.1\tlocalhost\n127.0.1.1\t%s\n::1\t\tlocalhost\n" "$HOSTNAME" > /mnt/etc/hosts

# Passwords — read directly from files inside chroot, no encoding needed
# Passwords — pass directly as env vars, no files, no subshells
ROOTPW_B64=$(printf '%s' "$ROOTPW" | base64)
USERPW_B64=$(printf '%s' "$USERPW" | base64)
artix-chroot /mnt bash -c "echo root:\$(echo $ROOTPW_B64 | base64 -d) | chpasswd"
gauge 55 "Creating user account..."
artix-chroot /mnt bash -c "useradd -m -G wheel,audio,video,storage,input '$USERNAME'"
artix-chroot /mnt bash -c "echo $USERNAME:\$(echo $USERPW_B64 | base64 -d) | chpasswd"
USER_UID=$(grep "^${USERNAME}:" /mnt/etc/passwd | cut -d: -f3)
USER_GID=$(grep "^${USERNAME}:" /mnt/etc/passwd | cut -d: -f4)

# privilege escalation config
if [ "$PRIV_ESC" = "sudo" ]; then
    _sudo_ok=0
    if [ -f /mnt/etc/sudoers ]; then
        sed -i 's/^# %wheel ALL=(ALL:ALL) ALL/%wheel ALL=(ALL:ALL) ALL/' /mnt/etc/sudoers
        sed -i 's/^# %wheel ALL=(ALL) ALL/%wheel ALL=(ALL) ALL/' /mnt/etc/sudoers
        grep -q '%wheel.*ALL' /mnt/etc/sudoers && _sudo_ok=1
    else
        mkdir -p /mnt/etc/sudoers.d
        echo "%wheel ALL=(ALL:ALL) ALL" > /mnt/etc/sudoers.d/wheel
        chmod 0440 /mnt/etc/sudoers.d/wheel
        _sudo_ok=1
    fi
    if [ "$_sudo_ok" = "0" ]; then
        echo "==> Warning: sudo config failed, falling back to doas"
        PRIV_ESC="doas"
    fi
fi
# doas — either as primary choice or fallback
if [ "$PRIV_ESC" = "doas" ]; then
    # ensure doas is installed (may have been skipped if sudo was chosen initially)
    [ ! -f /mnt/usr/bin/doas ] && artix-chroot /mnt pacman -S --noconfirm doas 2>/dev/null || true
    cat > /mnt/etc/doas.conf << 'EOF'
permit persist :wheel
permit nopass :wheel cmd pacman
EOF
    chmod 0400 /mnt/etc/doas.conf
    [ ! -e /mnt/usr/bin/sudo ] && artix-chroot /mnt ln -s /usr/bin/doas /usr/bin/sudo || true
fi

# xdg dirs — create standard dirs directly, avoids chroot session issues
for d in Desktop Documents Downloads Music Pictures Public Templates Videos; do
    mkdir -p "/mnt/home/$USERNAME/$d"
done
chown -R "${USER_UID}:${USER_GID}" /mnt/home/"$USERNAME"

# =========================
# PIPEWIRE AUTOSTART (only for DEs that use audio)
# =========================
AUDIO_DES_CHECK="Plasma XFCE Cinnamon LXQt Moksha Cosmic Hyprland MangoWM Niri Wayfire"
NEED_AUDIO=0
for _de in $AUDIO_DES_CHECK; do
    echo "$DE_CHOICES" | grep -qw "$_de" && NEED_AUDIO=1 && break
done

mkdir -p /mnt/usr/local/bin
if [ "$NEED_AUDIO" = "1" ]; then

cat > /mnt/usr/local/bin/start-pipewire << 'EOF'
#!/bin/bash
export XDG_RUNTIME_DIR="/run/user/$(id -u)"

# Kill any stale instances first so we start clean
pkill -u "$USER" -fx /usr/bin/pipewire-pulse 2>/dev/null || true
pkill -u "$USER" -fx /usr/bin/wireplumber     2>/dev/null || true
pkill -u "$USER" -fx /usr/bin/pipewire        2>/dev/null || true
sleep 0.5

# Start pipewire and wait for its socket — not a fixed sleep
/usr/bin/pipewire &
i=0
while [ ! -S "$XDG_RUNTIME_DIR/pipewire-0" ] && [ $i -lt 10 ]; do
    sleep 1; i=$((i+1))
done

# Now safe to start wireplumber and pipewire-pulse
/usr/bin/wireplumber &
i=0
while [ "$(pgrep -fx /usr/bin/wireplumber)" = "" ] && [ $i -lt 10 ]; do
    sleep 1; i=$((i+1))
done
/usr/bin/pipewire-pulse &
EOF
chmod +x /mnt/usr/local/bin/start-pipewire

# XDG autostart — works for Plasma X11, XFCE, LXQt
mkdir -p /mnt/home/"$USERNAME"/.config/autostart
cat > /mnt/home/"$USERNAME"/.config/autostart/pipewire.desktop << 'EOF'
[Desktop Entry]
Type=Application
Name=PipeWire
Exec=/usr/local/bin/start-pipewire
X-KDE-autostart-phase=1
EOF

# Note: autostart-scripts/ intentionally omitted — KDE auto-converts scripts
# in that directory into broken .desktop files pointing to wrong paths

if echo "$DE_CHOICES" | grep -qw "Moksha"; then
    mkdir -p /mnt/home/"$USERNAME"/.e/e/applications/startup
    cat > /mnt/home/"$USERNAME"/.e/e/applications/startup/pipewire.desktop << 'EOF'
[Desktop Entry]
Type=Application
Name=PipeWire
Exec=/usr/local/bin/start-pipewire
EOF
fi
fi # end NEED_AUDIO

chown -R "${USER_UID}:${USER_GID}" /mnt/home/"$USERNAME"

# =========================
# DISPLAY MANAGER SELECTION
# =========================
# Decided here (before the bare-WM autologin block below) so that block can
# actually see whether a DM is being installed.
# DM priority: Cosmic/Hyprland need greetd, Plasma needs sddm, X11 DEs use lightdm.
# MangoWM alone uses greetd; alongside an X11 DE it's registered as a wayland-session
# in that DE's DM instead (sddm reads /usr/share/wayland-sessions; so do recent lightdm).
# Accepting noctalia-greeter in the wizard overrides all of that — it becomes the
# sole login screen even when an X11 DE was also picked (Cosmic still wins outright,
# since it was never offered alongside noctalia-greeter in the first place).
if echo "$DE_CHOICES" | grep -qw "Cosmic"; then
    DM="greetd"
elif [ "$NOCTALIA_GREETER_SELECTED" = "yes" ]; then
    DM="greetd"
elif echo "$DE_CHOICES" | grep -qw "Plasma"; then
    DM="sddm"
elif echo "$DE_CHOICES" | grep -qwE "XFCE|Cinnamon|LXQt|Moksha"; then
    DM="lightdm"
elif echo "$DE_CHOICES" | grep -qwE "Hyprland|MangoWM|Niri|Wayfire"; then
    DM="greetd"
else
    DM=""
fi

# bare WMs autologin on tty1 and startx
BARE_WMS="i3 Openbox Fluxbox IceWM"

# Write .desktop session files for bare WMs so DMs (lightdm, sddm) can launch them
for _wm in $BARE_WMS; do
    if echo "$DE_CHOICES" | grep -qw "$_wm"; then
        mkdir -p /mnt/usr/share/xsessions
        case "$_wm" in
            i3)      _wm_exec="i3" ;;
            Openbox) _wm_exec="openbox-session" ;;
            Fluxbox) _wm_exec="startfluxbox" ;;
            IceWM)   _wm_exec="icewm-session" ;;
        esac
        cat > "/mnt/usr/share/xsessions/${_wm}.desktop" << EOF
[Desktop Entry]
Name=$_wm
Exec=$_wm_exec
Type=Application
EOF
    fi
done

# Only set up autologin + startx if no DM is being installed
# If a DM is present it owns the session — startx in .bash_profile would conflict
_has_bare_wm=0
for _wm in $BARE_WMS; do
    echo "$DE_CHOICES" | grep -qw "$_wm" && _has_bare_wm=1 && break
done

if [ "$_has_bare_wm" = "1" ] && [ -z "$DM" ]; then
    # pure bare WM setup — autologin on tty1 and startx
    cat >> /mnt/home/"$USERNAME"/.bash_profile << 'EOF'
[[ -z $DISPLAY && $XDG_VTNR -eq 1 ]] && exec startx
EOF
    chown "${USER_UID}:${USER_GID}" /mnt/home/"$USERNAME"/.bash_profile
    case "$INIT" in
        dinit)
            cat > /mnt/etc/dinit.d/agetty-tty1 << EOF
type = process
command = /sbin/agetty --autologin $USERNAME --noclear tty1 38400 linux
restart = true
depends-on = elogind
EOF
            ;;
        openrc)
            mkdir -p /mnt/etc/conf.d
            cat > /mnt/etc/conf.d/agetty.tty1 << EOF
agetty_options="--autologin $USERNAME --noclear"
EOF
            for pam_file in login system-auth; do
                PAM_PATH="/mnt/etc/pam.d/$pam_file"
                [ -f "$PAM_PATH" ] || continue
                sed -i 's/pam_unix.so$/pam_unix.so nullok/' "$PAM_PATH" 2>/dev/null || true
                grep -q 'pam_elogind.so' "$PAM_PATH" || \
                    echo 'session optional pam_elogind.so' >> "$PAM_PATH"
            done
            ;;
        runit)
            mkdir -p /mnt/etc/runit/sv/agetty-tty1
            printf '#!/bin/sh\nexec agetty --autologin %s --noclear tty1 linux\n' "$USERNAME" \
                > /mnt/etc/runit/sv/agetty-tty1/run
            chmod +x /mnt/etc/runit/sv/agetty-tty1/run
            artix-chroot /mnt ln -sf /etc/runit/sv/agetty-tty1 /etc/runit/runsvdir/default/ 2>/dev/null || true
            ;;
        s6)
            # s6: create a custom longrun service for autologin getty
            mkdir -p /mnt/etc/s6/adminsv/agetty-tty1
            printf 'longrun\n' > /mnt/etc/s6/adminsv/agetty-tty1/type
            printf '#!/bin/execlineb -P\nagetty --autologin %s --noclear tty1 linux\n' "$USERNAME" \
                > /mnt/etc/s6/adminsv/agetty-tty1/run
            chmod +x /mnt/etc/s6/adminsv/agetty-tty1/run
            mkdir -p /mnt/etc/s6/adminsv/default/contents.d
            touch /mnt/etc/s6/adminsv/default/contents.d/agetty-tty1
            ;;
    esac
elif [ "$_has_bare_wm" = "1" ] && [ -n "$DM" ]; then
    # mixed: bare WMs + a DM — DM handles login, session files written above
    echo "==> Bare WMs registered as DM sessions — select them from $DM login screen"
fi

gauge 60 "Configuring swap..."
# zram swap
if [[ "$SWAP" =~ Zram|Both ]]; then
    ZRAM_MB=$(( RAM_KB / 1024 / 2 ))
    (( ZRAM_MB > 8192 )) && ZRAM_MB=8192

    cat > /mnt/usr/local/bin/zram-setup << EOF
#!/bin/bash
modprobe zram
echo lz4 > /sys/block/zram0/comp_algorithm 2>/dev/null || echo lzo > /sys/block/zram0/comp_algorithm
echo ${ZRAM_MB}M > /sys/block/zram0/disksize
mkswap /dev/zram0
swapon -p 100 /dev/zram0
EOF
    chmod +x /mnt/usr/local/bin/zram-setup

    cat > /mnt/usr/local/bin/zram-teardown << 'EOF'
#!/bin/bash
swapoff /dev/zram0 2>/dev/null
echo 1 > /sys/block/zram0/reset
modprobe -r zram
EOF
    chmod +x /mnt/usr/local/bin/zram-teardown

    case "$INIT" in
        dinit)
            cat > /mnt/etc/dinit.d/zram << 'EOF'
type = scripted
command = /usr/local/bin/zram-setup
stop-command = /usr/local/bin/zram-teardown
EOF
            ;;
        openrc)
            cat > /mnt/etc/init.d/zram << 'EOF'
#!/sbin/openrc-run
description="zram swap"
command="/usr/local/bin/zram-setup"
stop() { /usr/local/bin/zram-teardown; }
EOF
            chmod +x /mnt/etc/init.d/zram
            ;;
        runit)
            mkdir -p /mnt/etc/runit/sv/zram
            printf '#!/bin/sh\nexec /usr/local/bin/zram-setup\n' > /mnt/etc/runit/sv/zram/run
            printf '#!/bin/sh\nexec /usr/local/bin/zram-teardown\n' > /mnt/etc/runit/sv/zram/finish
            chmod +x /mnt/etc/runit/sv/zram/run /mnt/etc/runit/sv/zram/finish
            ;;
        s6)
            mkdir -p /mnt/etc/s6/adminsv/zram
            printf 'longrun\n' > /mnt/etc/s6/adminsv/zram/type
            printf '#!/bin/execlineb -P\n/usr/local/bin/zram-setup\n' > /mnt/etc/s6/adminsv/zram/run
            chmod +x /mnt/etc/s6/adminsv/zram/run
            ;;
    esac
fi

gauge 65 "Installing desktop environment..."
_failed_des=""
for DE in $DE_CHOICES; do
    _de_ok=1
    case "$DE" in
        Plasma)
            artix-chroot /mnt pacman -S --noconfirm \
                plasma-desktop kwin plasma-pa plasma-nm \
                powerdevil kscreen kde-gtk-config \
                breeze breeze-gtk knotifications \
                polkit-kde-agent xdg-desktop-portal-kde \
                dolphin konsole spectacle ark gwenview \
                plasma-systemmonitor ksystemstats bluedevil || _de_ok=0
            ;;
        XFCE)
            artix-chroot /mnt pacman -S --noconfirm \
                xfce4 xfce4-goodies xdg-desktop-portal-gtk \
                pavucontrol thunar-archive-plugin || _de_ok=0
            ;;
        Cinnamon)
            artix-chroot /mnt pacman -S --noconfirm \
                cinnamon nemo-fileroller gnome-terminal \
                xdg-desktop-portal-gtk || _de_ok=0
            ;;
        LXQt)
            artix-chroot /mnt pacman -S --noconfirm lxqt || _de_ok=0
            ;;
        i3)
            artix-chroot /mnt pacman -S --noconfirm i3-wm dmenu xterm || _de_ok=0
            ;;
        Openbox)
            artix-chroot /mnt pacman -S --noconfirm openbox xterm dmenu || _de_ok=0
            ;;
        Fluxbox)
            artix-chroot /mnt pacman -S --noconfirm fluxbox xterm dmenu || _de_ok=0
            ;;
        IceWM)
            artix-chroot /mnt pacman -S --noconfirm icewm xterm || _de_ok=0
            mkdir -p /mnt/home/"$USERNAME"/.config/icewm
            ;;
        Hyprland)
            artix-chroot /mnt pacman -S --noconfirm hyprland xdg-desktop-portal-hyprland || { _de_ok=0; }
            if [ "$_de_ok" = "1" ]; then
                mkdir -p /mnt/home/"$USERNAME"/.config/hypr
                cat >> /mnt/home/"$USERNAME"/.config/hypr/hyprland.conf << 'HYPREOF'
exec-once = /usr/local/bin/start-pipewire
HYPREOF
                chown -R "${USER_UID}:${USER_GID}" /mnt/home/"$USERNAME"/.config/hypr
                mkdir -p /mnt/usr/share/wayland-sessions
                cat > /mnt/usr/share/wayland-sessions/hyprland.desktop << 'EOF'
[Desktop Entry]
Name=Hyprland
Comment=A dynamic tiling Wayland compositor
Exec=Hyprland
Type=Application
EOF
            fi
            ;;
        Niri)
            artix-chroot /mnt pacman -S --noconfirm niri xdg-desktop-portal-gnome || { _de_ok=0; }
            if [ "$_de_ok" = "1" ]; then
                # Niri generates a full default config.kdl on first run; ship the
                # package's own copy of that default and just append our autostart
                # line, rather than hand-writing a (possibly incomplete) config.
                mkdir -p /mnt/home/"$USERNAME"/.config/niri
                _niri_default=""
                for _f in /mnt/usr/share/doc/niri/default-config.kdl /mnt/etc/niri/config.kdl; do
                    [ -f "$_f" ] && { _niri_default="$_f"; break; }
                done
                if [ -n "$_niri_default" ]; then
                    cp "$_niri_default" /mnt/home/"$USERNAME"/.config/niri/config.kdl
                    printf '\nspawn-at-startup "/usr/local/bin/start-pipewire"\n' \
                        >> /mnt/home/"$USERNAME"/.config/niri/config.kdl
                else
                    echo "==> Note: no default niri config found — ~/.config/niri/config.kdl will be auto-generated on first login; add: spawn-at-startup \"/usr/local/bin/start-pipewire\""
                fi
                chown -R "${USER_UID}:${USER_GID}" /mnt/home/"$USERNAME"/.config/niri
                # niri.desktop is shipped by the package already
            fi
            ;;
        Wayfire)
            artix-chroot /mnt pacman -S --noconfirm wayfire wf-shell xdg-desktop-portal-wlr || { _de_ok=0; }
            if [ "$_de_ok" = "1" ]; then
                mkdir -p /mnt/home/"$USERNAME"/.config
                _wayfire_default=""
                for _f in /mnt/usr/share/wayfire/wayfire.ini /mnt/usr/share/examples/wayfire/wayfire.ini; do
                    [ -f "$_f" ] && { _wayfire_default="$_f"; break; }
                done
                if [ -n "$_wayfire_default" ]; then
                    cp "$_wayfire_default" /mnt/home/"$USERNAME"/.config/wayfire.ini
                else
                    printf '[core]\nplugins = autostart\n' > /mnt/home/"$USERNAME"/.config/wayfire.ini
                    echo "==> Note: no default wayfire.ini found — using a minimal one; re-check plugin list"
                fi
                printf '\n[autostart]\npipewire = /usr/local/bin/start-pipewire\n' \
                    >> /mnt/home/"$USERNAME"/.config/wayfire.ini
                chown "${USER_UID}:${USER_GID}" /mnt/home/"$USERNAME"/.config/wayfire.ini
                # wayfire.desktop is shipped by the package already
            fi
            ;;
        MangoWM)
            # Only offered when Arch [extra] or the CachyOS repo has it (see check_mango_repo)
            case "${MANGO_REPO:-}" in
                extra)   enable_arch_extra_chroot   || _de_ok=0 ;;
                cachyos) enable_cachyos_repo_chroot || _de_ok=0 ;;
                *)       _de_ok=0 ;;
            esac
            [ "$_de_ok" = "1" ] && { artix-chroot /mnt pacman -S --noconfirm "${MANGO_PKG:-mangowm}" || _de_ok=0; }
            if [ "$_de_ok" = "1" ]; then
                # Terminal + launcher so the first login isn't a blank screen (best effort)
                artix-chroot /mnt pacman -S --noconfirm foot wmenu || \
                    echo "==> Warning: couldn't install foot/wmenu — install a terminal + launcher manually"

                # Session entry for lightdm/sddm (skip if the package already ships one)
                mkdir -p /mnt/usr/share/wayland-sessions
                _has_mango_session=0
                for _f in /mnt/usr/share/wayland-sessions/mango*.desktop; do
                    [ -f "$_f" ] && _has_mango_session=1
                done
                if [ "$_has_mango_session" = "0" ]; then
                    cat > /mnt/usr/share/wayland-sessions/mangowm.desktop << 'EOF'
[Desktop Entry]
Name=MangoWM
Comment=Tiling Wayland compositor based on dwl
Exec=mango
Type=Application
EOF
                fi

                # Per-user config: a user config.conf REPLACES the system one, so start
                # from the shipped default (keeps binds etc.) and append the pipewire autostart
                mkdir -p /mnt/home/"$USERNAME"/.config/mango
                _mango_default=""
                for _f in /mnt/etc/mango/config.conf /mnt/usr/share/defaults/mango/config.conf; do
                    [ -f "$_f" ] && { _mango_default="$_f"; break; }
                done
                if [ -n "$_mango_default" ]; then
                    cp "$_mango_default" /mnt/home/"$USERNAME"/.config/mango/config.conf
                    printf '\n# Audio (added by installer)\nexec-once=/usr/local/bin/start-pipewire\n' \
                        >> /mnt/home/"$USERNAME"/.config/mango/config.conf
                else
                    echo "==> Note: no default mango config found — copy one to ~/.config/mango/config.conf and add: exec-once=/usr/local/bin/start-pipewire"
                fi
                chown -R "${USER_UID}:${USER_GID}" /mnt/home/"$USERNAME"/.config/mango
            fi
            ;;
        Moksha)
            artix-chroot /mnt pacman -S --noconfirm moksha-artix || _de_ok=0
            ;;
        Cosmic)
            artix-chroot /mnt bash -c "
                grep -q '\[galaxy\]' /etc/pacman.conf || printf '\n[galaxy]\nInclude = /etc/pacman.d/mirrorlist\n' >> /etc/pacman.conf
                pacman -Sy --noconfirm
            "
            artix-chroot /mnt pacman -S --noconfirm \
                cosmic-session cosmic-comp cosmic-greeter \
                greetd "$(svc_pkg greetd)" \
                xdg-desktop-portal-cosmic cosmic-terminal \
                cosmic-files cosmic-text-editor cosmic-settings \
                cosmic-screenshot cosmic-store upower pavucontrol || _de_ok=0
            # Create cosmic-greeter system user if missing
            artix-chroot /mnt id cosmic-greeter >/dev/null 2>&1 || \
                artix-chroot /mnt useradd -r -M -G video,audio,input cosmic-greeter
            # PAM elogind session registration
            for pam_file in system-login greetd; do
                PAM_PATH="/mnt/etc/pam.d/$pam_file"
                if [ -f "$PAM_PATH" ] && ! grep -q "pam_elogind.so" "$PAM_PATH"; then
                    echo "session required pam_elogind.so" >> "$PAM_PATH"
                fi
            done

            # Stub out systemd dbus interfaces that cosmic-osd/cosmic-settings poll for
            # Without these stubs they spin at 99% CPU waiting for a response that never comes
            mkdir -p /mnt/usr/share/dbus-1/services
            for svc in org.freedesktop.systemd1 org.freedesktop.login1; do
                cat > "/mnt/usr/share/dbus-1/services/${svc}.service" << DBUSEOF
[D-BUS Service]
Name=${svc}
Exec=/bin/false
DBUSEOF
            done
            mkdir -p /mnt/etc/greetd
            cat > /mnt/etc/greetd/config.toml << 'EOF'
[terminal]
vt = 1

[default_session]
command = "cosmic-comp cosmic-greeter"
user = "cosmic-greeter"
EOF
            ;;
    esac
    [ "$_de_ok" = "0" ] && _failed_des="$_failed_des $DE"
done

[ -n "$_failed_des" ] && echo "==> Warning: failed to install:$_failed_des — install manually after boot"

# write .xinitrc for bare WMs — done after loop so all WMs are installed
BARE_WMS_SELECTED=""
for _wm in i3 Openbox Fluxbox IceWM; do
    echo "$DE_CHOICES" | grep -qw "$_wm" && BARE_WMS_SELECTED="$BARE_WMS_SELECTED $_wm"
done
BARE_WMS_SELECTED="${BARE_WMS_SELECTED# }"

if [ -n "$BARE_WMS_SELECTED" ]; then
    wm_exec() {
        case "$1" in
            i3)      echo "exec i3" ;;
            Openbox) echo "exec openbox-session" ;;
            Fluxbox) echo "exec startfluxbox" ;;
            IceWM)   echo "exec icewm-session" ;;
        esac
    }
    WM_COUNT=$(echo "$BARE_WMS_SELECTED" | wc -w)
    {
        cat << 'XINITRC_HEADER'
#!/bin/sh
rm -f /tmp/.X*-lock /tmp/.X11-unix/X*
xset s off; xset -dpms; xset s noblank
xset fp+ /usr/share/fonts/TTF 2>/dev/null
xset fp+ /usr/share/fonts/dejavu 2>/dev/null
xset fp rehash 2>/dev/null
XINITRC_HEADER
        if [ "$WM_COUNT" -eq 1 ]; then
            wm_exec "$BARE_WMS_SELECTED"
        else
            # generate a whiptail picker that runs at login
            printf 'WMLIST=('
            for _wm in $BARE_WMS_SELECTED; do printf '"%s" "" ' "$_wm"; done
            printf ')\n'
            cat << 'WMPICKER'
_wm_choice=$(whiptail --title "Window Manager" --menu "Select WM to launch:" 15 50 8 "${WMLIST[@]}" 3>&1 1>&2 2>&3)
[ -z "$_wm_choice" ] && _wm_choice="${WMLIST[0]}"
WMPICKER
            for _wm in $BARE_WMS_SELECTED; do
                echo "[ \"\$_wm_choice\" = \"$_wm\" ] && $(wm_exec $_wm)"
            done
            wm_exec "$(echo "$BARE_WMS_SELECTED" | awk '{print $1}')"
        fi
    } > /mnt/home/"$USERNAME"/.xinitrc
    chmod +x /mnt/home/"$USERNAME"/.xinitrc
    chown "${USER_UID}:${USER_GID}" /mnt/home/"$USERNAME"/.xinitrc
fi

if [ -n "$DM" ]; then
    if [[ "$DM" == "greetd" ]]; then
        artix-chroot /mnt pacman -S --noconfirm greetd "$(svc_pkg greetd)"
        mkdir -p /mnt/etc/greetd
        if echo "$DE_CHOICES" | grep -qw "Cosmic"; then
            # Cosmic owns greetd — its own greeter handles session selection
            cat > /mnt/etc/greetd/config.toml << 'EOF'
[terminal]
vt = 1

[default_session]
command = "cosmic-comp cosmic-greeter"
user = "cosmic-greeter"
EOF
        elif [ "$NOCTALIA_GREETER_SELECTED" = "yes" ] && [ -n "${NOCTALIA_GREETER_REPO:-}" ]; then
            # Only reached when the user opted in during the wizard (which requires at
            # least one of Hyprland/MangoWM/Niri/Wayfire selected, no Cosmic, and the
            # package confirmed present — see check_noctalia_greeter_repo). May also be
            # mixed with an X11 DE; if so this still becomes the sole login screen, per
            # the warning shown in the wizard, and if install fails below the fallback
            # just drops to plain greetd on whichever Wayland WM was picked, leaving any
            # X11 DE without a DM pointed at it.
            _ng_ok=1
            case "$NOCTALIA_GREETER_REPO" in
                extra)   enable_arch_extra_chroot   || _ng_ok=0 ;;
                cachyos) enable_cachyos_repo_chroot || _ng_ok=0 ;;
                *)       _ng_ok=0 ;;
            esac
            [ "$_ng_ok" = "1" ] && { artix-chroot /mnt pacman -S --noconfirm noctalia-greeter || _ng_ok=0; }
            if [ "$_ng_ok" = "1" ]; then
                # greetd creates the "greeter" system user on install; create it
                # ourselves if that didn't happen, same defensive check as cosmic-greeter above
                artix-chroot /mnt id greeter >/dev/null 2>&1 || \
                    artix-chroot /mnt useradd -r -M -G video,input -s /bin/false greeter
                # Patches greetd's PAM stack for XDG_RUNTIME_DIR and creates
                # /var/lib/noctalia-greeter owned by the greeter user — safe to re-run
                if [ -x /mnt/usr/share/noctalia-greeter/setup_greeter_system.sh ]; then
                    artix-chroot /mnt /usr/share/noctalia-greeter/setup_greeter_system.sh || \
                        echo "==> Warning: noctalia-greeter setup script failed — check PAM/greeter user manually"
                fi
                cat > /mnt/etc/greetd/config.toml << 'EOF'
[terminal]
vt = 1

[default_session]
command = "noctalia-greeter-session"
user = "greeter"
EOF
            fi
            if [ "$_ng_ok" != "1" ]; then
                echo "==> Warning: failed to install noctalia-greeter — falling back to plain greetd"
                NOCTALIA_GREETER_SELECTED=""
            fi
        fi
        if [ "$NOCTALIA_GREETER_SELECTED" != "yes" ] && ! echo "$DE_CHOICES" | grep -qw "Cosmic"; then
            # Hyprland > Niri > Wayfire > MangoWM — whichever comes first lands in
            # greetd's single default_session; the others are still installed and
            # reachable by hand (e.g. `niri` from a tty) just not auto-launched
            GREETD_CMD=""
            for _w in Hyprland Niri Wayfire MangoWM; do
                if echo "$DE_CHOICES" | grep -qw "$_w"; then
                    case "$_w" in
                        MangoWM) GREETD_CMD="mango" ;;
                        *)       GREETD_CMD="$_w" ;;
                    esac
                    break
                fi
            done
            # bare WMs alone (no Wayland DE picked) also land here via DM=greetd — shouldn't
            # normally happen since DM is only "greetd" when one of the above was selected
            [ -z "$GREETD_CMD" ] && GREETD_CMD="mango"
            cat > /mnt/etc/greetd/config.toml << EOF
[terminal]
vt = 1

[default_session]
command = "$GREETD_CMD"
user = "$USERNAME"
EOF
        fi
    elif [[ "$DM" == "sddm" ]]; then
        artix-chroot /mnt pacman -S --noconfirm sddm "$(svc_pkg sddm)"
        # sddm picks up all installed .desktop session files automatically
    elif [[ "$DM" == "lightdm" ]]; then
        artix-chroot /mnt pacman -S --noconfirm lightdm lightdm-gtk-greeter "$(svc_pkg lightdm)"
        # lightdm picks up all installed .desktop session files automatically
    fi
fi


gauge 85 "Enabling services..."
# enable services

# CPU governor
cat > /mnt/etc/cpupower.conf << 'EOF'
governor='schedutil'
EOF

# Install chosen network stack + migrate NM wifi profiles if needed
case "$NET_CHOICE" in
    dhcpcd)
        artix-chroot /mnt pacman -S --noconfirm dhcpcd "$(svc_pkg dhcpcd)"
        NET_SVC="dhcpcd"
        ;;
    iwd)
        artix-chroot /mnt pacman -S --noconfirm iwd "$(svc_pkg iwd)"
        NET_SVC="iwd"
        if [ -d /etc/NetworkManager/system-connections ]; then
            mkdir -p /mnt/var/lib/iwd
            for nmconf in /etc/NetworkManager/system-connections/*.nmconnection; do
                [ -f "$nmconf" ] || continue
                SSID=$(awk -F= '/^ssid=/{print $2}' "$nmconf")
                PSK=$(awk -F= '/^psk=/{print $2}' "$nmconf")
                [ -z "$SSID" ] && continue
                IWDFILE="/mnt/var/lib/iwd/${SSID}.psk"
                [ -n "$PSK" ] && printf '[Security]\nPassphrase=%s\n' "$PSK" > "$IWDFILE" \
                              || printf '[Security]\n' > "$IWDFILE"
                chmod 600 "$IWDFILE"
                echo "Migrated wifi: $SSID"
            done
        fi
        ;;
    NM)
        NET_SVC="NetworkManager"
        ;;
esac

artix-chroot /mnt pacman -S --noconfirm cpupower "$(svc_pkg cpupower)"

SVCS="dbus $NET_SVC elogind cpupower"
echo "$DE_CHOICES" | grep -qw "Cosmic" && SVCS="$SVCS upower"
[ -n "$DM" ] && SVCS="$SVCS $DM"

case "$INIT" in
    dinit)
        mkdir -p /mnt/etc/dinit.d/boot.d
        [ -f /mnt/etc/dinit.d/rtkit-daemon ] && SVCS="$SVCS rtkit-daemon"             || { [ -f /mnt/etc/dinit.d/rtkit ] && SVCS="$SVCS rtkit"; }
        echo "$DE_CHOICES" | grep -qw "Cosmic" && SVCS="$SVCS turnstiled"
        for svc in $SVCS; do svc_enable "$svc"; done
        [[ "$SWAP" =~ Zram|Both ]] && svc_enable zram || true
        for tty in 2 3 4 5 6; do
            rm -f /mnt/etc/dinit.d/boot.d/getty@tty${tty} 2>/dev/null || true
            artix-chroot /mnt dinitctl disable getty@tty${tty} 2>/dev/null || true
        done
        ;;
    openrc)
        [ -f /mnt/etc/init.d/rtkit ] && SVCS="$SVCS rtkit"
        for svc in $SVCS; do svc_enable "$svc"; done
        [[ "$SWAP" =~ Zram|Both ]] && artix-chroot /mnt rc-update add zram boot 2>/dev/null || true
        for tty in 2 3 4 5 6; do
            artix-chroot /mnt rc-update del agetty.tty${tty} 2>/dev/null || true
        done
        ;;
    runit)
        mkdir -p /mnt/etc/runit/runsvdir/default
        [ -f /mnt/etc/runit/sv/rtkit/run ] && SVCS="$SVCS rtkit"
        for svc in $SVCS; do svc_enable "$svc"; done
        [[ "$SWAP" =~ Zram|Both ]] && svc_enable zram || true
        ;;
    s6)
        [ -d /mnt/etc/s6/sv/rtkit-srv ] && SVCS="$SVCS rtkit"
        for svc in $SVCS; do svc_enable "$svc"; done
        [[ "$SWAP" =~ Zram|Both ]] && svc_enable zram || true
        # Compile the s6-rc database so services are active on next boot
        artix-chroot /mnt s6-db-reload 2>/dev/null ||             artix-chroot /mnt s6-rc-db-update 2>/dev/null || true
        ;;
esac

gauge 90 "Installing bootloader..."
# =========================
# BOOTLOADER
# =========================
case "$BOOT" in
    grub)
        artix-chroot /mnt pacman -S --noconfirm grub
        [ "$UEFI" = "1" ] && artix-chroot /mnt pacman -S --noconfirm efibootmgr
        if [ "$DUALBOOT" = "1" ]; then
            # os-prober detects other OSes (Windows etc) for GRUB menu
            artix-chroot /mnt pacman -S --noconfirm os-prober
            sed -i 's/^#GRUB_DISABLE_OS_PROBER=.*/GRUB_DISABLE_OS_PROBER=false/' /mnt/etc/default/grub
            grep -q 'GRUB_DISABLE_OS_PROBER' /mnt/etc/default/grub || \
                echo 'GRUB_DISABLE_OS_PROBER=false' >> /mnt/etc/default/grub
            # Mount other partitions so os-prober can find them
            mount --bind /dev  /mnt/dev
            mount --bind /proc /mnt/proc
            mount --bind /sys  /mnt/sys
        fi
        if [ "$UEFI" = "1" ]; then
            artix-chroot /mnt grub-install \
                --target=x86_64-efi \
                --efi-directory=/boot \
                --bootloader-id=Artix \
                --recheck

            # Also install to the fallback path /EFI/BOOT/BOOTX64.EFI
            # Many boards (especially cheaper/older ones) ignore NVRAM entries
            # and only boot from this hardcoded fallback location
            mkdir -p /mnt/boot/EFI/BOOT
            cp /mnt/boot/EFI/Artix/grubx64.efi /mnt/boot/EFI/BOOT/BOOTX64.EFI 2>/dev/null || true

            # Register an explicit NVRAM boot entry — grub-install does this but
            # some firmware clears it on reboot; efibootmgr makes it stick
            EFI_PART_NUM=$(echo "$EFI" | grep -o '[0-9]*$')
            efibootmgr --create \
                --disk "$DISK" \
                --part "$EFI_PART_NUM" \
                --label "Artix Linux" \
                --loader '\EFI\Artix\grubx64.efi' 2>/dev/null || true
        else
            artix-chroot /mnt grub-install --target=i386-pc --recheck "$DISK"
        fi
        if [ "$ENCRYPT" = "1" ]; then
            GRUB_LUKS="cryptdevice=UUID=$(blkid -s UUID -o value "$REAL_ROOT"):cryptroot root=/dev/mapper/cryptroot"
            sed -i "s|^GRUB_CMDLINE_LINUX=.*|GRUB_CMDLINE_LINUX=\"$GRUB_LUKS\"|" /mnt/etc/default/grub
            sed -i 's/^#GRUB_ENABLE_CRYPTODISK=.*/GRUB_ENABLE_CRYPTODISK=y/' /mnt/etc/default/grub
        fi
        artix-chroot /mnt grub-mkconfig -o /boot/grub/grub.cfg
        # Unmount bind mounts used by os-prober
        [ "$DUALBOOT" = "1" ] && { umount /mnt/sys /mnt/proc /mnt/dev 2>/dev/null || true; }
        ;;
    limine)
        artix-chroot /mnt pacman -S --noconfirm limine efibootmgr limine-mkinitcpio-hook limine-snapper-sync

        # Get the EFI partition number from its device path
        EFI_PART_NUM=$(echo "$EFI" | grep -o '[0-9]*$')

        # Install Limine EFI binary
        mkdir -p /mnt/boot/EFI/limine
        # Limine ships the EFI binary at this path
        cp /mnt/usr/share/limine/BOOTX64.EFI /mnt/boot/EFI/limine/ 2>/dev/null || \
        cp /mnt/usr/share/limine/limine-uefi.efi /mnt/boot/EFI/limine/BOOTX64.EFI 2>/dev/null || true

        efibootmgr --create \
            --disk "$DISK" \
            --part "$EFI_PART_NUM" \
            --label "Limine" \
            --loader '\EFI\limine\BOOTX64.EFI' 2>/dev/null || true

        # Fallback path — boards that ignore NVRAM entries boot from here
        mkdir -p /mnt/boot/EFI/BOOT
        cp /mnt/boot/EFI/limine/BOOTX64.EFI /mnt/boot/EFI/BOOT/BOOTX64.EFI 2>/dev/null || true

        if [ "$ENCRYPT" = "1" ]; then
            PART_UUID=$(blkid -s UUID -o value "$REAL_ROOT")
            LIMINE_CMDLINE="cryptdevice=UUID=$PART_UUID:cryptroot root=/dev/mapper/cryptroot rw quiet"
        else
            ROOT_UUID=$(blkid -s UUID -o value "$ROOT")
            LIMINE_CMDLINE="root=UUID=$ROOT_UUID rw quiet"
        fi

        # Limine config — key names from official CONFIG.md
        # protocol, path, cmdline, module_path (NOT kernel_path/kernel_cmdline)
        cat > /mnt/boot/limine.conf << EOF
timeout: 5
verbose: no

/Artix Linux
    protocol: linux
    path: boot():/vmlinuz-$FIRST_KERNEL
    cmdline: $LIMINE_CMDLINE
    module_path: boot():/initramfs-$FIRST_KERNEL.img
EOF
        ;;
    refind)
        artix-chroot /mnt pacman -S --noconfirm refind efibootmgr
        artix-chroot /mnt refind-install
        # Explicit NVRAM entry — refind-install does this but reinforcing helps on picky firmware
        EFI_PART_NUM=$(echo "$EFI" | grep -o '[0-9]*$')
        efibootmgr --create \
            --disk "$DISK" \
            --part "$EFI_PART_NUM" \
            --label "rEFInd" \
            --loader '\\EFI\\refind\\refind_x64.efi' 2>/dev/null || true
        ROOT_UUID=$(blkid -s UUID -o value "${REAL_ROOT:-$ROOT}")
        if [ "$ENCRYPT" = "1" ]; then
            printf '"Boot with standard options"  "%s root=/dev/mapper/cryptroot rw quiet"\n' \
                "$LUKS_CMDLINE" > /mnt/boot/refind_linux.conf
        else
            printf '"Boot with standard options"  "root=UUID=%s rw quiet"\n"Boot to terminal"            "root=UUID=%s rw init=/sbin/$INIT"\n"Boot with minimal options"   "root=UUID=%s rw"\n' \
                "$ROOT_UUID" "$ROOT_UUID" "$ROOT_UUID" > /mnt/boot/refind_linux.conf
        fi
        ;;
esac


gauge 100 "Installation complete!"

umount -R /mnt 2>/dev/null || true

whiptail --title "$TITLE" --yesno "Installation complete!\n\nReboot now?" 10 50 \
    && reboot || true
