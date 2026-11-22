use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
};

/// Hardware data collected from Linux sysfs/procfs and the udev database.
/// Discovery is read-only: this module never starts a process or opens a device for writing.
#[derive(Debug, Clone, Default)]
pub struct HardwareSnapshot {
    pub firmware: String,
    pub memory_bytes: Option<u64>,
    pub cpu_vendor: Option<String>,
    pub cpu_model: Option<String>,
    pub gpus: Vec<GpuInfo>,
    pub network_interfaces: Vec<NetworkInterface>,
    pub disks: Vec<BlockDevice>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct GpuInfo {
    pub vendor: String,
    pub vendor_id: String,
    pub device_id: String,
}

#[derive(Debug, Clone)]
pub struct NetworkInterface {
    pub name: String,
    pub mac: Option<String>,
    pub state: Option<String>,
    pub wireless: bool,
}

#[derive(Debug, Clone)]
pub struct BlockDevice {
    pub name: String,
    pub path: PathBuf,
    pub size_bytes: u64,
    pub model: Option<String>,
    pub vendor: Option<String>,
    pub serial: Option<String>,
    pub wwid: Option<String>,
    pub removable: bool,
    pub read_only: bool,
    pub mounted_at: Vec<String>,
    pub active_swap: bool,
    pub safety_checks_complete: bool,
    pub has_holders: bool,
    pub partitions: Vec<BlockPartition>,
}

#[derive(Debug, Clone)]
pub struct BlockPartition {
    pub name: String,
    pub path: PathBuf,
    pub size_bytes: u64,
    pub filesystem: Option<String>,
    pub label: Option<String>,
    pub mounted_at: Vec<String>,
    pub active_swap: bool,
    pub read_only: bool,
    pub has_holders: bool,
    pub safety_checks_complete: bool,
}

impl BlockDevice {
    pub fn is_busy(&self) -> bool {
        !self.mounted_at.is_empty()
            || self.active_swap
            || self.partitions.iter().any(|partition| {
                !partition.mounted_at.is_empty() || partition.active_swap
            })
    }

    pub fn can_be_selected(&self) -> bool {
        self.safety_checks_complete
            && self.partitions.iter().all(|partition| {
                partition.safety_checks_complete && !partition.read_only && !partition.has_holders
            })
            && !self.is_busy()
            && !self.read_only
            && !self.has_holders
    }

    pub fn selection_block_reason(&self) -> Option<&'static str> {
        if !self.safety_checks_complete
            || self.partitions.iter().any(|partition| !partition.safety_checks_complete)
        {
            Some("could not verify device, mount and active-swap state")
        } else if !self.mounted_at.is_empty()
            || self.partitions.iter().any(|partition| !partition.mounted_at.is_empty())
        {
            Some("device or one of its partitions is mounted")
        } else if self.active_swap || self.partitions.iter().any(|partition| partition.active_swap) {
            Some("device or one of its partitions is active swap")
        } else if self.read_only || self.partitions.iter().any(|partition| partition.read_only) {
            Some("device or one of its partitions is read-only")
        } else if self.has_holders || self.partitions.iter().any(|partition| partition.has_holders) {
            Some("device or one of its partitions has active holders (for example device-mapper or RAID)")
        } else {
            None
        }
    }
}

impl HardwareSnapshot {
    pub fn report(&self) -> String {
        let mut output = String::from("ARTIX SETUP — READ-ONLY HARDWARE REPORT\n=======================================\n\n");
        output.push_str(&format!("Firmware: {}\n", self.firmware));
        output.push_str(&format!("Memory: {}\n", self.memory_bytes.map_or_else(|| "unknown".into(), format_bytes)));
        output.push_str(&format!("CPU vendor: {}\n", self.cpu_vendor.as_deref().unwrap_or("unknown")));
        output.push_str(&format!("CPU model: {}\n", self.cpu_model.as_deref().unwrap_or("unknown")));
        output.push_str("\nGRAPHICS\n--------\n");
        if self.gpus.is_empty() {
            output.push_str("No PCI display controller found (or sysfs is unavailable).\n");
        }
        for gpu in &self.gpus {
            output.push_str(&format!("- {} (vendor {}, device {})\n", gpu.vendor, gpu.vendor_id, gpu.device_id));
        }
        output.push_str("\nNETWORK INTERFACES\n------------------\n");
        if self.network_interfaces.is_empty() {
            output.push_str("No non-loopback interface found.\n");
        }
        for interface in &self.network_interfaces {
            output.push_str(&format!(
                "- {}  state={}  mac={}  wireless={}\n",
                interface.name,
                interface.state.as_deref().unwrap_or("unknown"),
                interface.mac.as_deref().unwrap_or("unknown"),
                interface.wireless
            ));
        }
        output.push_str("\nBLOCK DEVICES (READ-ONLY)\n-------------------------\n");
        if self.disks.is_empty() {
            output.push_str("No physical block devices found.\n");
        }
        for disk in &self.disks {
            output.push_str(&format!(
                "- {}  {}  model={}  serial={}  {}\n",
                disk.path.display(),
                format_bytes(disk.size_bytes),
                disk.model.as_deref().unwrap_or("unknown"),
                disk.serial.as_deref().or(disk.wwid.as_deref()).unwrap_or("unknown"),
                if disk.can_be_selected() { "available for selection" } else { disk.selection_block_reason().unwrap_or("blocked") }
            ));
            if disk.active_swap {
                output.push_str("    active swap on whole device\n");
            }
            if disk.removable {
                output.push_str("    removable media\n");
            }
            for partition in &disk.partitions {
                output.push_str(&format!(
                    "    {}  {}  fs={}  label={}  mounts={}\n",
                    partition.path.display(),
                    format_bytes(partition.size_bytes),
                    partition.filesystem.as_deref().unwrap_or("unknown"),
                    partition.label.as_deref().unwrap_or("none"),
                    if partition.mounted_at.is_empty() { "none".into() } else { partition.mounted_at.join(", ") }
                ));
                if partition.active_swap {
                    output.push_str("      ACTIVE SWAP — selection blocked\n");
                }
            }
        }
        if !self.warnings.is_empty() {
            output.push_str("\nDISCOVERY WARNINGS\n------------------\n");
            for warning in &self.warnings {
                output.push_str(&format!("- {warning}\n"));
            }
        }
        output.push_str("\nThis report is informational. No disk has been modified.\n");
        output
    }
}

/// Read the current live environment. Individual missing sysfs/procfs attributes
/// are reported as warnings so the TUI can still start on unusual hardware.
pub fn discover() -> HardwareSnapshot {
    let mut snapshot = HardwareSnapshot {
        firmware: if Path::new("/sys/firmware/efi").is_dir() { "UEFI".into() } else { "BIOS / legacy or unavailable".into() },
        ..HardwareSnapshot::default()
    };
    snapshot.memory_bytes = read_memtotal_bytes();
    (snapshot.cpu_vendor, snapshot.cpu_model) = read_cpu_info();
    snapshot.gpus = read_gpus(&mut snapshot.warnings);
    snapshot.network_interfaces = read_network_interfaces(&mut snapshot.warnings);
    snapshot.disks = read_block_devices(&mut snapshot.warnings);
    snapshot
}

pub fn format_bytes(bytes: u64) -> String {
    const GIB: u64 = 1024 * 1024 * 1024;
    const TIB: u64 = GIB * 1024;
    if bytes >= TIB {
        format!("{:.2} TiB", bytes as f64 / TIB as f64)
    } else if bytes >= GIB {
        format!("{:.1} GiB", bytes as f64 / GIB as f64)
    } else {
        format!("{:.0} MiB", bytes as f64 / (1024 * 1024) as f64)
    }
}

fn read_text(path: &Path) -> Option<String> {
    fs::read_to_string(path).ok().map(|value| value.trim().to_owned()).filter(|value| !value.is_empty())
}

fn read_memtotal_bytes() -> Option<u64> {
    let meminfo = fs::read_to_string("/proc/meminfo").ok()?;
    let kib = meminfo.lines().find_map(|line| {
        let (key, rest) = line.split_once(':')?;
        if key != "MemTotal" { return None; }
        rest.split_whitespace().next()?.parse::<u64>().ok()
    })?;
    Some(kib.saturating_mul(1024))
}

fn read_cpu_info() -> (Option<String>, Option<String>) {
    let Ok(cpuinfo) = fs::read_to_string("/proc/cpuinfo") else {
        return (None, None);
    };
    let mut vendor = None;
    let mut model = None;
    for line in cpuinfo.lines() {
        if vendor.is_none()
            && let Some(value) = line.strip_prefix("vendor_id").and_then(|s| s.split_once(':').map(|(_, v)| v.trim()))
        {
            vendor = Some(value.to_owned());
        }
        if model.is_none()
            && let Some(value) = line.strip_prefix("model name").and_then(|s| s.split_once(':').map(|(_, v)| v.trim()))
        {
            model = Some(value.to_owned());
        }
        if vendor.is_some() && model.is_some() { break; }
    }
    (vendor, model)
}

fn read_gpus(warnings: &mut Vec<String>) -> Vec<GpuInfo> {
    let root = Path::new("/sys/bus/pci/devices");
    let Ok(entries) = fs::read_dir(root) else {
        warnings.push("PCI sysfs is unavailable; graphics detection skipped.".into());
        return Vec::new();
    };
    let mut gpus = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(class) = read_text(&path.join("class")) else { continue; };
        if !class.starts_with("0x03") { continue; }
        let vendor_id = read_text(&path.join("vendor")).unwrap_or_else(|| "unknown".into());
        let device_id = read_text(&path.join("device")).unwrap_or_else(|| "unknown".into());
        let vendor = match vendor_id.as_str() {
            "0x1002" => "AMD / ATI",
            "0x8086" => "Intel",
            "0x10de" => "NVIDIA",
            "0x1af4" => "Virtio",
            _ => "Other PCI graphics device",
        };
        gpus.push(GpuInfo { vendor: vendor.into(), vendor_id, device_id });
    }
    gpus.sort_by(|a, b| a.vendor.cmp(&b.vendor).then(a.device_id.cmp(&b.device_id)));
    gpus
}

fn read_network_interfaces(warnings: &mut Vec<String>) -> Vec<NetworkInterface> {
    let root = Path::new("/sys/class/net");
    let Ok(entries) = fs::read_dir(root) else {
        warnings.push("Network sysfs is unavailable; interface detection skipped.".into());
        return Vec::new();
    };
    let mut interfaces = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name == "lo" { continue; }
        let path = entry.path();
        interfaces.push(NetworkInterface {
            name,
            mac: read_text(&path.join("address")),
            state: read_text(&path.join("operstate")),
            wireless: path.join("wireless").exists(),
        });
    }
    interfaces.sort_by(|a, b| a.name.cmp(&b.name));
    interfaces
}

fn read_block_devices(warnings: &mut Vec<String>) -> Vec<BlockDevice> {
    let root = Path::new("/sys/class/block");
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) => {
            warnings.push(format!("Block-device sysfs is unavailable: {error}"));
            return Vec::new();
        }
    };
    let (mountpoints, mounts_verified) = read_mountpoints(warnings);
    let (active_swaps, swaps_verified) = read_active_swap_names(warnings);
    let mut raw = Vec::new();
    let mut enumeration_verified = true;
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                warnings.push(format!("Could not enumerate every block device: {error}; disk selection is disabled for safety."));
                enumeration_verified = false;
                continue;
            }
        };
        let name = entry.file_name().to_string_lossy().into_owned();
        let class_path = entry.path();
        let is_partition = class_path.join("partition").exists();
        if !is_partition && !is_physical_disk_candidate(&name, &class_path) { continue; }
        let canonical = match fs::canonicalize(&class_path) {
            Ok(path) => path,
            Err(error) => {
                warnings.push(format!("Could not resolve sysfs identity for {name}: {error}; disk selection is disabled for safety."));
                enumeration_verified = false;
                if is_partition {
                    continue;
                }
                class_path.clone()
            }
        };
        let parent_name = if is_partition {
            canonical.parent().and_then(Path::file_name).map(|v| v.to_string_lossy().into_owned())
        } else { None };
        if is_partition && parent_name.is_none() {
            warnings.push(format!("Could not resolve the parent disk for {name}; partition excluded from disk selection."));
            continue;
        }
        let major_minor = read_text(&class_path.join("dev")).unwrap_or_default();
        let device_mounts = mountpoints.get(&major_minor).cloned().unwrap_or_default();
        let ro_value = read_text(&class_path.join("ro"));
        let partition_read_only = ro_value.as_deref() == Some("1");
        let size_sectors = read_text(&class_path.join("size"))
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(0);
        let size_verified = size_sectors > 0;
        let device_state_verified = mounts_verified
            && swaps_verified
            && !major_minor.is_empty()
            && ro_value.is_some()
            && size_verified
            && enumeration_verified;
        let udev = read_udev_properties(&major_minor);
        let filesystem = udev.get("ID_FS_TYPE").cloned();
        let label = udev.get("ID_FS_LABEL").cloned().or_else(|| udev.get("ID_FS_LABEL_ENC").cloned());
        let partition = BlockPartition {
            name: name.clone(),
            path: PathBuf::from("/dev").join(&name),
            size_bytes: size_sectors.saturating_mul(512),
            filesystem,
            label,
            mounted_at: device_mounts.clone(),
            active_swap: active_swaps.contains(&name),
            read_only: partition_read_only,
            has_holders: false,
            safety_checks_complete: device_state_verified,
        };
        let (holders, holders_verified) = match fs::read_dir(class_path.join("holders")) {
            Ok(mut entries) => (entries.next().is_some(), true),
            Err(_) => (false, false),
        };
        raw.push((name, parent_name, partition, holders, holders_verified, class_path));
    }

    let mut disks = Vec::new();
    let mut partitions_by_parent: HashMap<String, Vec<BlockPartition>> = HashMap::new();
    for (_name, parent, partition, holders, holders_verified, _path) in &raw {
        if let Some(parent) = parent {
            let mut child = partition.clone();
            child.has_holders = *holders;
            child.safety_checks_complete &= *holders_verified && enumeration_verified;
            partitions_by_parent.entry(parent.clone()).or_default().push(child);
        }
    }
    for (name, parent, partition, holders, holders_verified, class_path) in raw {
        if parent.is_some() { continue; }
        let model = read_text(&class_path.join("device/model"));
        let vendor = read_text(&class_path.join("device/vendor"));
        let serial = read_text(&class_path.join("device/serial"));
        let wwid = read_text(&class_path.join("wwid")).or_else(|| read_text(&class_path.join("device/wwid")));
        let removable = read_text(&class_path.join("removable")).as_deref() == Some("1");
        let read_only = partition.read_only;
        let size_bytes = partition.size_bytes;
        let mut children = partitions_by_parent.remove(&name).unwrap_or_default();
        children.sort_by(|a, b| a.name.cmp(&b.name));
        disks.push(BlockDevice {
            name: name.clone(),
            path: PathBuf::from("/dev").join(&name),
            size_bytes,
            model,
            vendor,
            serial,
            wwid,
            removable,
            read_only,
            mounted_at: partition.mounted_at,
            active_swap: partition.active_swap,
            safety_checks_complete: partition.safety_checks_complete && holders_verified && enumeration_verified,
            has_holders: holders,
            partitions: children,
        });
    }
    disks.sort_by(|a, b| a.name.cmp(&b.name));
    disks
}

fn is_physical_disk_candidate(name: &str, class_path: &Path) -> bool {
    if ["loop", "ram", "zram", "dm-", "md", "fd", "sr"].iter().any(|prefix| name.starts_with(prefix)) {
        return false;
    }
    // eMMC boot0/boot1 and RPMB are special hardware regions, not general-purpose disks.
    if name.starts_with("mmcblk") && (name.contains("boot") || name.ends_with("rpmb")) {
        return false;
    }
    class_path.join("device").exists()
        || name.starts_with("nvme")
        || name.starts_with("mmcblk")
        || name.starts_with("sd")
        || name.starts_with("vd")
        || name.starts_with("xvd")
        || name.starts_with("hd")
}

fn read_active_swap_names(
    warnings: &mut Vec<String>,
) -> (std::collections::HashSet<String>, bool) {
    let Ok(contents) = fs::read_to_string("/proc/swaps") else {
        warnings.push("Active swap list is unavailable; disk selection is disabled for safety.".into());
        return (std::collections::HashSet::new(), false);
    };
    let names = parse_proc_swaps(&contents)
        .into_iter()
        .filter_map(|path| {
            let resolved = fs::canonicalize(&path).unwrap_or(path);
            resolved.file_name().map(|name| name.to_string_lossy().into_owned())
        })
        .collect();
    (names, true)
}

fn parse_proc_swaps(contents: &str) -> Vec<PathBuf> {
    contents
        .lines()
        .skip(1)
        .filter_map(|line| line.split_whitespace().next())
        .map(decode_mount_field)
        .map(PathBuf::from)
        .collect()
}

fn read_mountpoints(
    warnings: &mut Vec<String>,
) -> (HashMap<String, Vec<String>>, bool) {
    let Ok(contents) = fs::read_to_string("/proc/self/mountinfo") else {
        warnings.push("Mount table is unavailable; disk selection is disabled for safety.".into());
        return (HashMap::new(), false);
    };
    (parse_mountinfo(&contents), true)
}

/// Return mounted paths keyed by the kernel major:minor device number.
fn parse_mountinfo(contents: &str) -> HashMap<String, Vec<String>> {
    let mut result: HashMap<String, Vec<String>> = HashMap::new();
    for line in contents.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 6 { continue; }
        let device = fields[2].to_owned();
        let mountpoint = decode_mount_field(fields[4]);
        result.entry(device).or_default().push(mountpoint);
    }
    result
}

fn decode_mount_field(value: &str) -> String {
    value.replace("\\040", " ").replace("\\011", "\t").replace("\\012", "\n").replace("\\134", "\\")
}

fn read_udev_properties(major_minor: &str) -> HashMap<String, String> {
    if major_minor.is_empty() { return HashMap::new(); }
    let path = PathBuf::from("/run/udev/data").join(format!("b{major_minor}"));
    let Ok(contents) = fs::read_to_string(path) else { return HashMap::new(); };
    parse_udev_properties(&contents)
}

fn parse_udev_properties(contents: &str) -> HashMap<String, String> {
    contents.lines().filter_map(|line| {
        let (raw_key, value) = line.split_once('=')?;
        let key = raw_key.strip_prefix("E:").unwrap_or(raw_key);
        if key.starts_with("ID_FS_") { Some((key.to_owned(), value.to_owned())) } else { None }
    }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn udev_filesystem_properties_accept_environment_prefix() {
        let properties = parse_udev_properties("E:ID_FS_TYPE=btrfs\nE:ID_FS_LABEL=ArtixRoot\nE:ID_FS_UUID=abcd\nS:disk/by-label/ArtixRoot");
        assert_eq!(properties.get("ID_FS_TYPE").map(String::as_str), Some("btrfs"));
        assert_eq!(properties.get("ID_FS_LABEL").map(String::as_str), Some("ArtixRoot"));
        assert_eq!(properties.get("ID_FS_UUID").map(String::as_str), Some("abcd"));
        assert_eq!(properties.len(), 3);
    }

    #[test]
    fn emmc_boot_and_rpmb_regions_are_not_offered_as_disks() {
        assert!(!is_physical_disk_candidate("mmcblk0boot0", Path::new("/sys/class/block/mmcblk0boot0")));
        assert!(!is_physical_disk_candidate("mmcblk0rpmb", Path::new("/sys/class/block/mmcblk0rpmb")));
        assert!(is_physical_disk_candidate("mmcblk0", Path::new("/sys/class/block/mmcblk0")));
    }

    #[test]
    fn mountinfo_parser_maps_major_minor_and_unescapes_mountpoint() {
        let input = "42 25 8:1 / /mnt/My\\040Drive rw,relatime - ext4 /dev/sda1 rw\n";
        let mounts = parse_mountinfo(input);
        assert_eq!(mounts.get("8:1").unwrap(), &vec!["/mnt/My Drive".to_owned()]);
    }

    #[test]
    fn block_device_is_busy_when_child_partition_is_mounted() {
        let disk = BlockDevice {
            name: "sda".into(), path: "/dev/sda".into(), size_bytes: 1,
            model: None, vendor: None, serial: None, wwid: None, removable: false,
            read_only: false, mounted_at: vec![], active_swap: false, safety_checks_complete: true, has_holders: false,
            partitions: vec![BlockPartition {
                name: "sda1".into(), path: "/dev/sda1".into(), size_bytes: 1,
                filesystem: Some("vfat".into()), label: None, mounted_at: vec!["/boot".into()], active_swap: false, read_only: false, has_holders: false, safety_checks_complete: true,
            }],
        };
        assert!(disk.is_busy());
        assert!(!disk.can_be_selected());
        assert_eq!(disk.selection_block_reason(), Some("device or one of its partitions is mounted"));
    }

    #[test]
    fn proc_swaps_parser_finds_active_swap_paths() {
        let input = "Filename Type Size Used Priority\n/dev/sda3 partition 8388604 0 -2\n";
        assert_eq!(parse_proc_swaps(input), vec![PathBuf::from("/dev/sda3")]);
    }

    #[test]
    fn active_swap_partition_blocks_its_parent_disk() {
        let disk = BlockDevice {
            name: "sda".into(), path: "/dev/sda".into(), size_bytes: 1,
            model: None, vendor: None, serial: None, wwid: None, removable: false,
            read_only: false, mounted_at: vec![], active_swap: false, safety_checks_complete: true, has_holders: false,
            partitions: vec![BlockPartition {
                name: "sda2".into(), path: "/dev/sda2".into(), size_bytes: 1,
                filesystem: None, label: None, mounted_at: vec![], active_swap: true, read_only: false, has_holders: false, safety_checks_complete: true,
            }],
        };
        assert!(disk.is_busy());
        assert!(!disk.can_be_selected());
        assert_eq!(disk.selection_block_reason(), Some("device or one of its partitions is active swap"));
    }

    #[test]
    fn incomplete_safety_data_fails_closed() {
        let disk = BlockDevice {
            name: "sdb".into(), path: "/dev/sdb".into(), size_bytes: 1,
            model: None, vendor: None, serial: None, wwid: None, removable: false,
            read_only: false, mounted_at: vec![], active_swap: false, safety_checks_complete: false,
            has_holders: false, partitions: vec![],
        };
        assert!(!disk.can_be_selected());
        assert_eq!(disk.selection_block_reason(), Some("could not verify device, mount and active-swap state"));
    }

    #[test]
    fn partition_device_mapper_holders_block_parent_disk() {
        let disk = BlockDevice {
            name: "nvme0n1".into(), path: "/dev/nvme0n1".into(), size_bytes: 1,
            model: None, vendor: None, serial: None, wwid: None, removable: false,
            read_only: false, mounted_at: vec![], active_swap: false, safety_checks_complete: true,
            has_holders: false,
            partitions: vec![BlockPartition {
                name: "nvme0n1p2".into(), path: "/dev/nvme0n1p2".into(), size_bytes: 1,
                filesystem: None, label: None, mounted_at: vec![], active_swap: false,
                read_only: false, has_holders: true, safety_checks_complete: true,
            }],
        };
        assert!(!disk.can_be_selected());
        assert_eq!(disk.selection_block_reason(), Some("device or one of its partitions has active holders (for example device-mapper or RAID)"));
    }

    #[test]
    fn mounted_or_readonly_disks_are_never_selectable() {
        let mut disk = BlockDevice {
            name: "nvme0n1".into(), path: "/dev/nvme0n1".into(), size_bytes: 1,
            model: None, vendor: None, serial: None, wwid: None, removable: false,
            read_only: true, mounted_at: vec![], active_swap: false, safety_checks_complete: true, has_holders: false, partitions: vec![],
        };
        assert!(!disk.can_be_selected());
        disk.read_only = false;
        disk.has_holders = true;
        assert!(!disk.can_be_selected());
    }
}
