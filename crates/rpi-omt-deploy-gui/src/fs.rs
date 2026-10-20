//! The workstation's file system, as the page's folder picker sees it: the
//! places to start from, removable media first, and one directory's
//! entries at a time. Nothing here writes.

use std::fmt::Write as _;
use std::path::Path;

use omt_common::json::write_string;
use omt_deploy::sd_card::boot_partition_problem;
use omt_deploy::sys;

/// A directory with more entries than this is listed truncated; nobody
/// picks a boot partition out of more than a screenful anyway.
pub const MAX_ENTRIES: usize = 2000;
/// How many directory entries are examined at most, hidden ones included,
/// so a huge directory costs a bounded amount of work.
const MAX_SCANNED: usize = 20_000;
#[cfg(unix)]
const MOUNTS_LIMIT: usize = 1024 * 1024;
const MAX_ROOTS: usize = 64;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub path: String,
    pub dir: bool,
    /// A directory that is a flashed Alpine Raspberry Pi boot partition.
    pub alpine_boot: bool,
}

#[derive(Clone, Debug, Default)]
pub struct Listing {
    pub path: String,
    pub parent: Option<String>,
    pub entries: Vec<Entry>,
    pub truncated: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Root {
    pub label: String,
    pub path: String,
    pub removable: bool,
    pub alpine_boot: bool,
}

pub fn is_alpine_boot(path: &str) -> bool {
    boot_partition_problem(path).is_none()
}

/// The user's home directory, where a listing with no path starts.
pub fn home() -> Option<String> {
    let name = if sys::ON_WINDOWS { "USERPROFILE" } else { "HOME" };
    sys::getenv(name).filter(|h| sys::is_dir(h))
}

/// `path` without trailing separators, except where they are the root
/// (`/`, `C:\`).
fn tidy(path: &str) -> String {
    let seps: &[char] = if sys::ON_WINDOWS { &['/', '\\'] } else { &['/'] };
    let trimmed = path.trim_end_matches(seps);
    if trimmed.is_empty() {
        return path.chars().next().map_or_else(String::new, String::from);
    }
    if sys::ON_WINDOWS && trimmed.len() == 2 && trimmed.ends_with(':') {
        return format!("{trimmed}\\");
    }
    trimmed.to_owned()
}

/// One directory's entries: directories first, then (when `files`) files,
/// each sorted case-insensitively. Hidden entries are left out unless
/// `hidden`.
pub fn list(path: &str, files: bool, hidden: bool) -> Result<Listing, String> {
    let path = if path.trim().is_empty() {
        home().unwrap_or_else(|| if sys::ON_WINDOWS { "C:\\".into() } else { "/".into() })
    } else {
        tidy(path.trim())
    };
    let reader = std::fs::read_dir(&path).map_err(|e| format!("{path}: {e}"))?;
    let mut listing = Listing {
        parent: Path::new(&path).parent().map(|p| tidy(&p.to_string_lossy())),
        path,
        ..Listing::default()
    };
    for entry in reader.take(MAX_SCANNED) {
        let Ok(entry) = entry else { continue };
        let name = entry.file_name().to_string_lossy().into_owned();
        if !hidden && name.starts_with('.') {
            continue;
        }
        let full = sys::path_join(&listing.path, &name);
        // Followed: a mount point reached through a link is still a place
        // to pick.
        let dir = sys::is_dir(&full);
        if !(dir || files && sys::is_file(&full)) {
            continue;
        }
        if listing.entries.len() == MAX_ENTRIES {
            listing.truncated = true;
            break;
        }
        let alpine_boot = dir && is_alpine_boot(&full);
        listing.entries.push(Entry { name, path: full, dir, alpine_boot });
    }
    listing.entries.sort_by(|a, b| {
        b.dir.cmp(&a.dir).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(listing)
}

/// Undoes the octal escapes /proc/self/mounts writes for space, tab,
/// newline, and backslash.
pub fn unescape_mount_path(field: &str) -> String {
    let b = field.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let octal = b.get(i + 1..i + 4).filter(|d| d.iter().all(|c| (b'0'..=b'7').contains(c)));
        match (b[i], octal) {
            (b'\\', Some(d)) => {
                out.push((d[0] - b'0') * 64 + (d[1] - b'0') * 8 + (d[2] - b'0'));
                i += 4;
            }
            (c, _) => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The mount points in a mounts table that look like removable media: a
/// FAT or exFAT file system (what a Raspberry Pi boot partition is), or
/// anything the desktop mounted under /media, /run/media, or /mnt.
pub fn removable_mounts(table: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in table.lines() {
        let mut f = line.split(' ');
        let (Some(_device), Some(point), Some(kind)) = (f.next(), f.next(), f.next()) else {
            continue;
        };
        let point = unescape_mount_path(point);
        let fat = matches!(kind, "vfat" | "msdos" | "exfat" | "fat");
        let media = ["/media/", "/run/media/", "/mnt/"].iter().any(|p| point.starts_with(p));
        // The workstation's own EFI partition is FAT too; it is never the
        // card.
        let efi = point == "/boot" || point.starts_with("/boot/") || point == "/efi";
        if (fat || media) && !efi && !out.contains(&point) {
            out.push(point);
        }
        if out.len() == MAX_ROOTS {
            break;
        }
    }
    out
}

fn root(label: impl Into<String>, path: String, removable: bool) -> Root {
    let alpine_boot = is_alpine_boot(&path);
    Root { label: label.into(), path, removable, alpine_boot }
}

/// Where the picker starts: removable media (an inserted SD card above
/// all), then the home directory and the file-system roots.
pub fn roots() -> Vec<Root> {
    let mut out = Vec::new();
    #[cfg(windows)]
    for letter in b'A'..=b'Z' {
        let path = format!("{}:\\", char::from(letter));
        if sys::is_dir(&path) {
            // The system drive is not where a card appears.
            let removable = letter != b'C';
            out.push(root(format!("Drive {}:", char::from(letter)), path, removable));
        }
    }
    #[cfg(unix)]
    if let Ok(Some(table)) = omt_common::fsio::read_text("/proc/self/mounts", MOUNTS_LIMIT) {
        for point in removable_mounts(&table) {
            let label = Path::new(&point)
                .file_name()
                .map_or_else(|| point.clone(), |n| n.to_string_lossy().into_owned());
            out.push(root(label, point, true));
        }
    }
    if let Some(h) = home() {
        out.push(root("Home", h, false));
    }
    if !sys::ON_WINDOWS {
        out.push(root("File system", "/".into(), false));
    }
    // A boot partition first: it is what the operator came for.
    out.sort_by_key(|r| (!r.alpine_boot, !r.removable));
    out
}

pub fn listing_json(l: &Listing) -> String {
    let mut out = String::with_capacity(64 + l.entries.len() * 96);
    out.push_str("{\"path\":");
    write_string(&mut out, &l.path);
    out.push_str(",\"parent\":");
    match &l.parent {
        Some(p) => write_string(&mut out, p),
        None => out.push_str("null"),
    }
    let _ = write!(
        out,
        ",\"truncated\":{},\"alpine_boot\":{},\"entries\":[",
        l.truncated,
        is_alpine_boot(&l.path)
    );
    for (i, e) in l.entries.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str("{\"name\":");
        write_string(&mut out, &e.name);
        out.push_str(",\"path\":");
        write_string(&mut out, &e.path);
        let _ = write!(out, ",\"dir\":{},\"alpine_boot\":{}}}", e.dir, e.alpine_boot);
    }
    out.push_str("]}");
    out
}

pub fn roots_json(roots: &[Root]) -> String {
    let mut out = String::from("{\"roots\":[");
    for (i, r) in roots.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str("{\"label\":");
        write_string(&mut out, &r.label);
        out.push_str(",\"path\":");
        write_string(&mut out, &r.path);
        let _ = write!(out, ",\"removable\":{},\"alpine_boot\":{}}}", r.removable, r.alpine_boot);
    }
    out.push_str("]}");
    out
}
