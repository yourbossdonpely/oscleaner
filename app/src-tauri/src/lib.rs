// OS Cleaner v0.2 — directory scanner with hardlink-aware sizing,
// largest/oldest tracking, and hash-based duplicate detection.
// Each unique file is counted once, however many names point at it
// (the WinSxS lesson). Symlinks are never followed.
// Learn more about Tauri commands at https://tauri.app/develop/calling-rust/

use jwalk::WalkDir;
use serde::{Deserialize, Serialize};
use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const TOP_N: usize = 100;
const DUP_GROUP_CAP: usize = 500;
const DUP_FILES_CAP: usize = 100;

#[derive(Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
struct Agg {
    size: u64,
    files: u64,
    dirs: u64,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct FileEntry {
    name: String,
    path: String,
    size: u64,
    modified: u64,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ScanNode {
    name: String,
    path: String,
    size: u64,
    files: u64,
    dirs: u64,
    errors: u64,
    largest: Vec<FileEntry>,
    oldest: Vec<FileEntry>,
    children: Vec<ScanNode>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DupGroup {
    hash: String,
    size: u64,
    files: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DupReport {
    groups: Vec<DupGroup>,
    errors: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Roots {
    roots: Vec<String>,
    home: String,
    temp: String,
}

fn unix_secs(t: SystemTime) -> u64 {
    t.duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Stable identity for a file: (volume, file index).
/// `None` means "no identity available" — count normally.
#[cfg(windows)]
fn file_id(path: &Path) -> Option<(u64, u64)> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION,
    };
    let f = std::fs::File::open(path).ok()?;
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    let handle = f.as_raw_handle() as HANDLE;
    let ok = unsafe { GetFileInformationByHandle(handle, &mut info as *mut _) };
    if ok == 0 {
        return None;
    }
    let index = ((info.nFileIndexHigh as u64) << 32) | (info.nFileIndexLow as u64);
    Some((info.dwVolumeSerialNumber as u64, index))
}

#[cfg(unix)]
fn file_id(path: &Path) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    let md = path.metadata().ok()?;
    Some((md.dev(), md.ino()))
}

#[cfg(not(any(windows, unix)))]
fn file_id(_path: &Path) -> Option<(u64, u64)> {
    None
}

fn node_name(path: &Path) -> String {
    path.file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

fn file_entry(path: &Path, size: u64, modified: u64) -> FileEntry {
    FileEntry {
        name: node_name(path),
        path: path.to_string_lossy().into_owned(),
        size,
        modified,
    }
}

fn scan_path(root: &Path) -> Result<ScanNode, String> {
    let root_md = root
        .metadata()
        .map_err(|e| format!("Cannot read {}: {e}", root.display()))?;
    if root_md.is_file() {
        return Ok(ScanNode {
            name: node_name(root),
            path: root.to_string_lossy().into_owned(),
            size: root_md.len(),
            files: 1,
            dirs: 0,
            errors: 0,
            largest: vec![],
            oldest: vec![],
            children: vec![],
        });
    }
    if !root_md.is_dir() {
        return Err(format!("Not a file or folder: {}", root.display()));
    }

    let mut seen: HashSet<(u64, u64)> = HashSet::new();
    let mut buckets: HashMap<PathBuf, Agg> = HashMap::new();
    let mut largest: BinaryHeap<Reverse<(u64, u64, PathBuf)>> = BinaryHeap::new();
    let mut oldest: BinaryHeap<(SystemTime, PathBuf, u64)> = BinaryHeap::new();
    let mut total = Agg::default();
    let mut errors: u64 = 0;

    for entry in WalkDir::new(root)
        .min_depth(1)
        .follow_links(false)
        .into_iter()
    {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => {
                errors += 1;
                continue;
            }
        };
        let ftype = entry.file_type();
        if ftype.is_symlink() {
            continue;
        }
        let rel = match entry.path().strip_prefix(root) {
            Ok(r) => r.to_path_buf(),
            Err(_) => continue,
        };
        let top: PathBuf = match rel.components().next() {
            Some(c) => root.join(c),
            None => continue,
        };
        let agg = buckets.entry(top).or_default();
        if ftype.is_dir() {
            total.dirs += 1;
            agg.dirs += 1;
        } else if ftype.is_file() {
            let md = match entry.path().metadata() {
                Ok(m) => m,
                Err(_) => {
                    errors += 1;
                    continue;
                }
            };
            // Hardlink-aware: count each unique file once.
            if let Some(key) = file_id(&entry.path()) {
                if !seen.insert(key) {
                    continue;
                }
            }
            total.files += 1;
            total.size += md.len();
            agg.files += 1;
            agg.size += md.len();
            // Track largest files (min-heap on size, capped).
            let mtime = md.modified().map(unix_secs).unwrap_or(0);
            if largest.len() < TOP_N {
                largest.push(Reverse((md.len(), mtime, entry.path())));
            } else if let Some(Reverse((smallest, _, _))) = largest.peek() {
                if md.len() > *smallest {
                    largest.pop();
                    largest.push(Reverse((md.len(), mtime, entry.path())));
                }
            }
            // Track oldest files (max-heap on mtime, capped).
            if let Ok(mtime_t) = md.modified() {
                if oldest.len() < TOP_N {
                    oldest.push((mtime_t, entry.path(), md.len()));
                } else if let Some((newest, _, _)) = oldest.peek() {
                    if mtime_t < *newest {
                        oldest.pop();
                        oldest.push((mtime_t, entry.path(), md.len()));
                    }
                }
            }
        }
        // sockets, pipes, and other special files: ignored deliberately.
    }

    let mut children: Vec<ScanNode> = buckets
        .into_iter()
        .map(|(p, a)| ScanNode {
            name: node_name(&p),
            path: p.to_string_lossy().into_owned(),
            size: a.size,
            files: a.files,
            dirs: a.dirs,
            errors: 0,
            largest: vec![],
            oldest: vec![],
            children: vec![],
        })
        .collect();
    children.sort_by(|a, b| b.size.cmp(&a.size));

    let mut largest: Vec<FileEntry> = largest
        .into_iter()
        .map(|Reverse((size, modified, p))| file_entry(&p, size, modified))
        .collect();
    largest.sort_by(|a, b| b.size.cmp(&a.size));
    let mut oldest: Vec<FileEntry> = oldest
        .into_iter()
        .map(|(t, p, size)| file_entry(&p, size, unix_secs(t)))
        .collect();
    oldest.sort_by(|a, b| a.modified.cmp(&b.modified));

    Ok(ScanNode {
        name: node_name(root),
        path: root.to_string_lossy().into_owned(),
        size: total.size,
        files: total.files,
        dirs: total.dirs,
        errors,
        largest,
        oldest,
        children,
    })
}

fn hash_file(path: &Path) -> std::io::Result<String> {
    use std::io::Read;
    let mut file = std::fs::File::open(path)?;
    let mut hasher = blake3::Hasher::new();
    let mut buf = [0u8; 65536];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize().to_hex().to_string())
}

#[tauri::command]
fn scan_dir(path: String) -> Result<ScanNode, String> {
    scan_path(Path::new(&path))
}

#[tauri::command]
fn list_roots() -> Roots {
    #[cfg(windows)]
    let roots: Vec<String> = ('C'..='Z')
        .map(|c| format!("{c}:\\"))
        .filter(|p| Path::new(p).exists())
        .collect();
    #[cfg(not(windows))]
    let roots = vec!["/".to_string()];
    let home = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .unwrap_or_default();
    Roots {
        roots,
        home,
        temp: std::env::temp_dir().to_string_lossy().into_owned(),
    }
}

#[tauri::command]
fn find_duplicates(path: String) -> Result<DupReport, String> {
    let root = Path::new(&path);
    let md = root
        .metadata()
        .map_err(|e| format!("Cannot read {}: {e}", root.display()))?;
    if !md.is_dir() {
        return Err(format!("Pick a folder: {}", root.display()));
    }
    // Phase 1: group by size (cheap). Hardlinks collapse to one entry.
    let mut by_size: HashMap<u64, Vec<PathBuf>> = HashMap::new();
    let mut seen: HashSet<(u64, u64)> = HashSet::new();
    let mut errors: u64 = 0;
    for entry in WalkDir::new(root)
        .min_depth(1)
        .follow_links(false)
        .into_iter()
    {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => {
                errors += 1;
                continue;
            }
        };
        let ftype = entry.file_type();
        if !ftype.is_file() || ftype.is_symlink() {
            continue;
        }
        let p = entry.path();
        let len = match p.metadata() {
            Ok(m) => m.len(),
            Err(_) => {
                errors += 1;
                continue;
            }
        };
        if len == 0 {
            continue;
        }
        if let Some(key) = file_id(&p) {
            if !seen.insert(key) {
                continue;
            }
        }
        by_size.entry(len).or_default().push(p);
    }
    // Phase 2: hash only same-size candidates, group by digest.
    let mut groups: Vec<DupGroup> = Vec::new();
    for (size, paths) in by_size.into_iter().filter(|(_, v)| v.len() > 1) {
        let mut by_hash: HashMap<String, Vec<String>> = HashMap::new();
        for p in paths {
            match hash_file(&p) {
                Ok(h) => {
                    by_hash
                        .entry(h)
                        .or_default()
                        .push(p.to_string_lossy().into_owned());
                }
                Err(_) => {
                    errors += 1;
                }
            }
        }
        for (hash, mut files) in by_hash.into_iter().filter(|(_, v)| v.len() > 1) {
            files.sort();
            files.truncate(DUP_FILES_CAP);
            groups.push(DupGroup { hash, size, files });
            if groups.len() >= DUP_GROUP_CAP {
                break;
            }
        }
        if groups.len() >= DUP_GROUP_CAP {
            break;
        }
    }
    groups.sort_by(|a, b| b.size.cmp(&a.size));
    Ok(DupReport { groups, errors })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrashItem {
    path: String,
    ok: bool,
    error: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrashReport {
    results: Vec<TrashItem>,
    trashed: u64,
    failed: u64,
}

const TRASH_CAP: usize = 500;

/// Collapse `.` and `..` lexically (no symlink resolution).
fn lexical_normal(p: &Path) -> PathBuf {
    use std::path::Component;
    let mut head = PathBuf::new();
    let mut parts: Vec<std::ffi::OsString> = Vec::new();
    for c in p.components() {
        match c {
            Component::Prefix(p) => head.push(p.as_os_str()),
            Component::RootDir => head.push(std::path::MAIN_SEPARATOR.to_string()),
            Component::CurDir => {}
            Component::ParentDir => {
                parts.pop();
            }
            Component::Normal(s) => parts.push(s.to_os_string()),
        }
    }
    for part in parts {
        head.push(part);
    }
    head
}

/// Directories the trash command will never accept, even when asked.
fn protected_dirs() -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = Vec::new();
    #[cfg(windows)]
    {
        for c in 'C'..='Z' {
            let root = PathBuf::from(format!("{c}:\\"));
            if root.exists() {
                v.push(root);
            }
        }
        for var in ["USERPROFILE", "SystemRoot"] {
            if let Ok(h) = std::env::var(var) {
                v.push(PathBuf::from(h));
            }
        }
        for var in ["ProgramFiles", "ProgramFiles(x86)"] {
            if let Ok(h) = std::env::var(var) {
                v.push(PathBuf::from(h));
            }
        }
        if let Ok(w) = std::env::var("SystemRoot") {
            v.push(PathBuf::from(&w).join("System32"));
            v.push(PathBuf::from(&w).join("SysWOW64"));
        }
    }
    #[cfg(not(windows))]
    {
        for p in ["/", "/bin", "/sbin", "/usr", "/etc", "/System", "/Library"] {
            v.push(PathBuf::from(p));
        }
        if let Ok(h) = std::env::var("HOME") {
            v.push(PathBuf::from(h));
        }
    }
    v.into_iter().map(|p| lexical_normal(&p)).collect()
}

fn refusal(path: &str) -> Option<String> {
    let p = Path::new(path);
    if !p.is_absolute() {
        return Some("Refusing: path must be absolute".to_string());
    }
    let norm = lexical_normal(p);
    let canon = std::fs::canonicalize(p).ok();
    for prot in protected_dirs() {
        if norm == prot {
            return Some(format!("Refusing: {} is protected", prot.display()));
        }
        if let Some(c) = &canon {
            if *c == prot {
                return Some(format!(
                    "Refusing: {} resolves to protected {}",
                    path,
                    prot.display()
                ));
            }
        }
    }
    None
}

/// Move paths to the OS recycle bin / trash. Never a permanent delete.
/// Refuses protected locations, caps batch size, reports per-path results.
#[tauri::command]
fn trash_delete(paths: Vec<String>) -> Result<TrashReport, String> {
    if paths.is_empty() {
        return Err("Nothing to delete.".to_string());
    }
    if paths.len() > TRASH_CAP {
        return Err(format!("Refusing: too many at once ({} max).", TRASH_CAP));
    }
    let mut results: Vec<TrashItem> = Vec::with_capacity(paths.len());
    let mut trashed: u64 = 0;
    let mut failed: u64 = 0;
    for path in &paths {
        if let Some(reason) = refusal(path) {
            results.push(TrashItem {
                path: path.clone(),
                ok: false,
                error: reason,
            });
            failed += 1;
            continue;
        }
        match trash::delete(path) {
            Ok(()) => {
                results.push(TrashItem {
                    path: path.clone(),
                    ok: true,
                    error: String::new(),
                });
                trashed += 1;
            }
            Err(e) => {
                results.push(TrashItem {
                    path: path.clone(),
                    ok: false,
                    error: e.to_string(),
                });
                failed += 1;
            }
        }
    }
    Ok(TrashReport {
        results,
        trashed,
        failed,
    })
}

// ---- Installer orphans (Windows) ----

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct OrphanEntry {
    name: String,
    path: String,
    size: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OrphanReport {
    orphans: Vec<OrphanEntry>,
    total_bytes: u64,
    scanned: u64,
    referenced: u64,
}

fn lower(p: &str) -> String {
    p.replace('/', "\\").to_lowercase()
}

fn expand_env(s: &str) -> String {
    let mut out = s.to_string();
    loop {
        let Some(a) = out.find('%') else {
            break;
        };
        let Some(rel) = out[a + 1..].find('%') else {
            break;
        };
        let b = a + 1 + rel;
        let name = out[a + 1..b].to_string();
        let val = std::env::var(&name).unwrap_or_default();
        out.replace_range(a..=b, &val);
    }
    out
}

#[cfg(windows)]
fn collect_local_packages() -> HashSet<String> {
    use winreg::enums::*;
    use winreg::RegKey;
    fn walk(key: &RegKey, out: &mut HashSet<String>) {
        if let Ok(v) = key.get_value::<String, _>("LocalPackage") {
            out.insert(lower(&expand_env(&v)));
        }
        for name in key.enum_keys().flatten() {
            if let Ok(sub) = key.open_subkey(&name) {
                walk(&sub, out);
            }
        }
    }
    let mut out = HashSet::new();
    for (hive, path) in [
        (
            HKEY_LOCAL_MACHINE,
            r"SOFTWARE\Microsoft\Windows\CurrentVersion\Installer\UserData",
        ),
        (
            HKEY_CURRENT_USER,
            r"SOFTWARE\Microsoft\Windows\CurrentVersion\Installer",
        ),
    ] {
        if let Ok(root) = RegKey::predef(hive).open_subkey(path) {
            walk(&root, &mut out);
        }
    }
    out
}

fn find_orphans(
    cached: &[(String, PathBuf, u64)],
    referenced: &HashSet<String>,
) -> Vec<OrphanEntry> {
    let mut orphans: Vec<OrphanEntry> = cached
        .iter()
        .filter(|(norm, _, _)| !referenced.contains(norm))
        .map(|(_, p, size)| OrphanEntry {
            name: node_name(p),
            path: p.to_string_lossy().into_owned(),
            size: *size,
        })
        .collect();
    orphans.sort_by(|a, b| b.size.cmp(&a.size));
    orphans
}

#[cfg(windows)]
const INSTALLER_DIR: &str = r"C:\Windows\Installer";
#[cfg(windows)]
const ORPHAN_BACKUP_DIR: &str = r"C:\Installer-Orphans-Backup";

#[tauri::command]
fn scan_installer_orphans() -> Result<OrphanReport, String> {
    #[cfg(not(windows))]
    {
        return Err("Installer orphans: Windows only.".to_string());
    }
    #[cfg(windows)]
    {
        let dir = Path::new(INSTALLER_DIR);
        let entries =
            std::fs::read_dir(dir).map_err(|e| format!("Cannot read {}: {e}", dir.display()))?;
        let mut cached: Vec<(String, PathBuf, u64)> = Vec::new();
        for e in entries.flatten() {
            let p = e.path();
            let ext = p
                .extension()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_lowercase();
            if ext != "msi" && ext != "msp" {
                continue;
            }
            let size = e.metadata().map(|m| m.len()).unwrap_or(0);
            cached.push((lower(&p.to_string_lossy()), p, size));
        }
        let referenced = collect_local_packages();
        let scanned = cached.len() as u64;
        let orphans = find_orphans(&cached, &referenced);
        let total_bytes = orphans.iter().map(|o| o.size).sum();
        Ok(OrphanReport {
            orphans,
            total_bytes,
            scanned,
            referenced: referenced.len() as u64,
        })
    }
}

/// Move files into a backup dir. Every path must live under `allowed_root`.
fn move_paths_to_backup(
    paths: &[String],
    backup: &Path,
    allowed_root: &Path,
) -> Result<TrashReport, String> {
    if paths.is_empty() {
        return Err("Nothing to move.".to_string());
    }
    if paths.len() > TRASH_CAP {
        return Err(format!("Refusing: too many at once ({} max).", TRASH_CAP));
    }
    std::fs::create_dir_all(backup)
        .map_err(|e| format!("Cannot create {}: {e}", backup.display()))?;
    let allowed = lexical_normal(allowed_root);
    let mut results: Vec<TrashItem> = Vec::with_capacity(paths.len());
    let mut trashed: u64 = 0;
    let mut failed: u64 = 0;
    for path in paths {
        let p = Path::new(path);
        let norm = lexical_normal(p);
        if !norm.starts_with(&allowed) {
            results.push(TrashItem {
                path: path.clone(),
                ok: false,
                error: format!("Refusing: outside {}", allowed.display()),
            });
            failed += 1;
            continue;
        }
        let dest = backup.join(p.file_name().unwrap_or_default());
        match std::fs::rename(p, &dest) {
            Ok(()) => {
                results.push(TrashItem {
                    path: path.clone(),
                    ok: true,
                    error: String::new(),
                });
                trashed += 1;
            }
            Err(e) => {
                results.push(TrashItem {
                    path: path.clone(),
                    ok: false,
                    error: e.to_string(),
                });
                failed += 1;
            }
        }
    }
    Ok(TrashReport {
        results,
        trashed,
        failed,
    })
}

#[tauri::command]
fn backup_orphans(paths: Vec<String>) -> Result<TrashReport, String> {
    #[cfg(not(windows))]
    {
        let _ = paths;
        return Err("Installer orphans: Windows only.".to_string());
    }
    #[cfg(windows)]
    {
        move_paths_to_backup(&paths, Path::new(ORPHAN_BACKUP_DIR), Path::new(INSTALLER_DIR))
    }
}

#[tauri::command]
fn list_orphan_backup() -> Result<Vec<FileEntry>, String> {
    #[cfg(not(windows))]
    {
        return Err("Installer orphans: Windows only.".to_string());
    }
    #[cfg(windows)]
    {
        let dir = Path::new(ORPHAN_BACKUP_DIR);
        if !dir.exists() {
            return Ok(vec![]);
        }
        let mut out: Vec<FileEntry> = Vec::new();
        for e in std::fs::read_dir(dir)
            .map_err(|e| format!("Cannot read {}: {e}", dir.display()))?
            .flatten()
        {
            let p = e.path();
            if !e.file_type().map(|t| t.is_file()).unwrap_or(false) {
                continue;
            }
            let (size, modified) = e
                .metadata()
                .map(|m| (m.len(), m.modified().map(unix_secs).unwrap_or(0)))
                .unwrap_or((0, 0));
            out.push(file_entry(&p, size, modified));
        }
        out.sort_by(|a, b| b.size.cmp(&a.size));
        Ok(out)
    }
}

#[tauri::command]
fn restore_orphans(paths: Vec<String>) -> Result<TrashReport, String> {
    #[cfg(not(windows))]
    {
        let _ = paths;
        return Err("Installer orphans: Windows only.".to_string());
    }
    #[cfg(windows)]
    {
        move_paths_to_backup(
            &paths,
            Path::new(INSTALLER_DIR),
            Path::new(ORPHAN_BACKUP_DIR),
        )
    }
}

// ---- Remnant sweep ----

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct RemnantEntry {
    name: String,
    path: String,
    size: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemnantReport {
    empty_dirs: Vec<RemnantEntry>,
    stale_files: Vec<RemnantEntry>,
    errors: u64,
}

const REMNANT_CAP: usize = 2000;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemnantOptions {
    root: String,
    min_age_days: u64,
}

#[tauri::command]
fn scan_remnants(options: RemnantOptions) -> Result<RemnantReport, String> {
    let min_age_days = options.min_age_days;
    let root = Path::new(&options.root);
    if !root.is_dir() {
        return Err(format!("Pick a folder: {}", root.display()));
    }
    let cutoff = SystemTime::now()
        .checked_sub(std::time::Duration::from_secs(
            min_age_days.saturating_mul(86400),
        ))
        .unwrap_or(UNIX_EPOCH);
    let mut empty_dirs: Vec<RemnantEntry> = Vec::new();
    let mut stale_files: Vec<RemnantEntry> = Vec::new();
    let mut errors: u64 = 0;
    'walk: for entry in WalkDir::new(root)
        .min_depth(1)
        .follow_links(false)
        .into_iter()
    {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => {
                errors += 1;
                continue;
            }
        };
        let ftype = entry.file_type();
        if ftype.is_symlink() {
            continue;
        }
        if empty_dirs.len() + stale_files.len() >= REMNANT_CAP {
            break 'walk;
        }
        let p = entry.path();
        if ftype.is_dir() {
            match std::fs::read_dir(&p) {
                Ok(mut it) => {
                    if it.next().is_none() {
                        empty_dirs.push(RemnantEntry {
                            name: node_name(&p),
                            path: p.to_string_lossy().into_owned(),
                            size: 0,
                        });
                    }
                }
                Err(_) => {
                    errors += 1;
                }
            }
        } else if ftype.is_file() {
            let ext = p
                .extension()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_lowercase();
            if ext != "tmp" && ext != "log" && ext != "bak" {
                continue;
            }
            match p.metadata() {
                Ok(m) => {
                    let old = m.modified().map(|t| t < cutoff).unwrap_or(false);
                    if old {
                        stale_files.push(RemnantEntry {
                            name: node_name(&p),
                            path: p.to_string_lossy().into_owned(),
                            size: m.len(),
                        });
                    }
                }
                Err(_) => {
                    errors += 1;
                }
            }
        }
    }
    stale_files.sort_by(|a, b| b.size.cmp(&a.size));
    Ok(RemnantReport {
        empty_dirs,
        stale_files,
        errors,
    })
}

// ---- Elevation ----

#[tauri::command]
fn is_elevated() -> bool {
    #[cfg(windows)]
    {
        use windows_sys::Win32::UI::Shell::IsUserAnAdmin;
        unsafe { IsUserAnAdmin() != 0 }
    }
    #[cfg(not(windows))]
    {
        false
    }
}

#[tauri::command]
fn restart_elevated() -> Result<(), String> {
    #[cfg(not(windows))]
    {
        return Err("Elevation relaunch: Windows only.".to_string());
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::UI::Shell::ShellExecuteW;
        use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
        fn wide(s: &str) -> Vec<u16> {
            s.encode_utf16().chain(std::iter::once(0)).collect()
        }
        let exe =
            std::env::current_exe().map_err(|e| format!("No exe path: {e}"))?;
        let op = wide("runas");
        let file = wide(&exe.to_string_lossy());
        let ret = unsafe {
            ShellExecuteW(
                std::ptr::null_mut(),
                op.as_ptr(),
                file.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                SW_SHOWNORMAL,
            )
        };
        if (ret as isize) <= 32 {
            return Err(format!(
                "Elevation relaunch failed (code {}).",
                ret as isize
            ));
        }
        Ok(())
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            scan_dir,
            list_roots,
            find_duplicates,
            trash_delete,
            scan_installer_orphans,
            backup_orphans,
            list_orphan_backup,
            restore_orphans,
            scan_remnants,
            is_elevated,
            restart_elevated
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn fixture(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("oc-test-{}-{tag}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("a").join("deep")).unwrap();
        fs::write(dir.join("a").join("f1.bin"), vec![7u8; 1000]).unwrap();
        fs::write(dir.join("a").join("deep").join("f2.bin"), vec![7u8; 500]).unwrap();
        fs::write(dir.join("top.bin"), vec![7u8; 100]).unwrap();
        dir
    }

    #[test]
    fn aggregates_sizes_and_counts() {
        let dir = fixture("agg");
        let node = scan_path(&dir).unwrap();
        assert_eq!(node.size, 1600);
        assert_eq!(node.files, 3);
        assert!(node.dirs >= 2);
        let a = node.children.iter().find(|c| c.name == "a").unwrap();
        assert_eq!(a.size, 1500);
        assert_eq!(a.files, 2);
        let sizes: Vec<u64> = node.largest.iter().map(|f| f.size).collect();
        assert_eq!(sizes, vec![1000, 500, 100]);
        let mut names: Vec<&str> = node.oldest.iter().map(|f| f.name.as_str()).collect();
        names.sort();
        assert_eq!(names, vec!["f1.bin", "f2.bin", "top.bin"]);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn single_file_scans() {
        let dir = fixture("file");
        let node = scan_path(&dir.join("top.bin")).unwrap();
        assert_eq!(node.size, 100);
        assert_eq!(node.files, 1);
        assert!(node.children.is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_path_errors() {
        let r = scan_path(Path::new("C:\\definitely-not-here-oc-test-12345"));
        assert!(r.is_err());
    }

    #[test]
    fn finds_duplicates() {
        let dir = fixture("dup");
        fs::write(dir.join("copy-of-f1.bin"), vec![7u8; 1000]).unwrap();
        let report = find_duplicates(dir.to_string_lossy().into_owned()).unwrap();
        assert_eq!(report.groups.len(), 1);
        assert_eq!(report.groups[0].size, 1000);
        assert_eq!(report.groups[0].files.len(), 2);
        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(windows)]
    #[test]
    fn hardlink_counted_once() {
        let dir = fixture("hardlink");
        let target = dir.join("a").join("f1.bin");
        let link = dir.join("a").join("f1-link.bin");
        if fs::hard_link(&target, &link).is_err() {
            // Filesystem doesn't support hardlinks; skip.
            let _ = fs::remove_dir_all(&dir);
            return;
        }
        let node = scan_path(&dir).unwrap();
        // f1.bin counted once despite two names: 1600, not 2600.
        assert_eq!(node.size, 1600);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn normalizer_collapses_dots() {
        #[cfg(windows)]
        {
            assert_eq!(
                lexical_normal(Path::new("C:\\a\\..\\b\\.\\c")),
                PathBuf::from("C:\\b\\c")
            );
            assert_eq!(lexical_normal(Path::new("C:\\..")), PathBuf::from("C:\\"));
        }
        #[cfg(not(windows))]
        {
            assert_eq!(
                lexical_normal(Path::new("/a/../b/./c")),
                PathBuf::from("/b/c")
            );
        }
    }

    #[test]
    fn trash_roundtrip_moves_files_away() {
        let dir = fixture("trash");
        let a = dir.join("a").join("f1.bin");
        let b = dir.join("top.bin");
        let report = trash_delete(vec![
            a.to_string_lossy().into_owned(),
            b.to_string_lossy().into_owned(),
        ])
        .unwrap();
        assert_eq!(report.trashed, 2);
        assert_eq!(report.failed, 0);
        assert!(!a.exists());
        assert!(!b.exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn trash_refuses_protected_and_missing() {
        let missing = std::env::temp_dir().join("oc-test-nope-12345-missing.bin");
        let home = std::env::var("USERPROFILE")
            .or_else(|_| std::env::var("HOME"))
            .unwrap_or_default();
        let report = trash_delete(vec![
            "C:\\".to_string(),
            home,
            missing.to_string_lossy().into_owned(),
        ])
        .unwrap();
        assert_eq!(report.trashed, 0);
        assert_eq!(report.failed, 3);
        assert!(report.results.iter().all(|r| !r.ok));
    }

    #[test]
    fn trash_rejects_empty_and_huge_lists() {
        assert!(trash_delete(vec![]).is_err());
        let big = vec!["C:\\x".to_string(); TRASH_CAP + 1];
        assert!(trash_delete(big).is_err());
    }

    #[test]
    fn orphan_matching_is_case_insensitive() {
        let cached = vec![
            (
                "c:\\windows\\installer\\a.msi".to_string(),
                PathBuf::from("C:\\Windows\\Installer\\a.msi"),
                100,
            ),
            (
                "c:\\windows\\installer\\b.msi".to_string(),
                PathBuf::from("C:\\Windows\\Installer\\b.msi"),
                200,
            ),
        ];
        let mut referenced = HashSet::new();
        referenced.insert("c:\\windows\\installer\\b.msi".to_string());
        let orphans = find_orphans(&cached, &referenced);
        assert_eq!(orphans.len(), 1);
        assert_eq!(orphans[0].size, 100);
    }

    #[test]
    fn env_expansion_works() {
        unsafe { std::env::set_var("OC_TEST_VAR", "hello") };
        assert_eq!(expand_env("%OC_TEST_VAR%\\x"), "hello\\x");
        assert_eq!(expand_env("plain"), "plain");
        assert_eq!(expand_env("%OC_TEST_MISSING_zzz%"), "");
    }

    #[test]
    fn backup_roundtrip_stays_inside_allowed_root() {
        let dir = std::env::temp_dir().join(format!("oc-test-{}-backup", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let src = dir.join("src");
        let backup = dir.join("backup");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("x.msi"), vec![1u8; 10]).unwrap();
        let rep = move_paths_to_backup(
            &[src.join("x.msi").to_string_lossy().into_owned()],
            &backup,
            &src,
        )
        .unwrap();
        assert_eq!(rep.trashed, 1);
        assert!(backup.join("x.msi").exists());
        assert!(!src.join("x.msi").exists());
        let outside = std::env::temp_dir().join("oc-test-outside-x.msi");
        fs::write(&outside, vec![1u8; 5]).unwrap();
        let rep2 = move_paths_to_backup(
            &[outside.to_string_lossy().into_owned()],
            &backup,
            &src,
        )
        .unwrap();
        assert_eq!(rep2.failed, 1);
        assert!(outside.exists());
        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_file(&outside);
    }

    #[test]
    fn remnants_find_empty_dirs_and_stale_temps() {
        let dir = std::env::temp_dir().join(format!("oc-test-{}-remnant", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("empty1")).unwrap();
        fs::create_dir_all(dir.join("full")).unwrap();
        fs::write(dir.join("full").join("keep.txt"), b"no").unwrap();
        fs::write(dir.join("old.tmp"), b"tmp").unwrap();
        fs::write(dir.join("old.log"), b"log").unwrap();
        let rep = scan_remnants(RemnantOptions {
            root: dir.to_string_lossy().into_owned(),
            min_age_days: 0,
        })
        .unwrap();
        assert_eq!(rep.empty_dirs.len(), 1);
        assert_eq!(rep.empty_dirs[0].name, "empty1");
        assert_eq!(rep.stale_files.len(), 2);
        let _ = fs::remove_dir_all(&dir);
    }
}
