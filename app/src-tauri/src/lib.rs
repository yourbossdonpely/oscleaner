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
    os: String,
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
        os: std::env::consts::OS.to_string(),
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

// ---- Stale drivers (Windows, report only in v0.5) ----

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DriverEntry {
    published: String,
    original: String,
    provider: String,
    class_name: String,
    version: String,
    size: u64,
    superseded: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DriverReport {
    drivers: Vec<DriverEntry>,
    total_bytes: u64,
    store_bytes: u64,
}

/// Parse `pnputil /enum-drivers` output into entries (size filled later).
fn parse_pnputil(text: &str) -> Vec<DriverEntry> {
    let mut out: Vec<DriverEntry> = Vec::new();
    let mut cur: HashMap<String, String> = HashMap::new();
    let mut flush = |cur: &mut HashMap<String, String>, out: &mut Vec<DriverEntry>| {
        if let Some(published) = cur.remove("Published Name") {
            out.push(DriverEntry {
                published,
                original: cur.remove("Original Name").unwrap_or_default(),
                provider: cur.remove("Provider Name").unwrap_or_default(),
                class_name: cur.remove("Class Name").unwrap_or_default(),
                version: cur.remove("Driver Version").unwrap_or_default(),
                size: 0,
                superseded: false,
            });
        }
        cur.clear();
    };
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            flush(&mut cur, &mut out);
            continue;
        }
        if let Some((k, v)) = line.split_once(':') {
            cur.insert(k.trim().to_string(), v.trim().to_string());
        }
    }
    flush(&mut cur, &mut out);
    out
}

/// Compare dotted versions numerically ("32.0.15.1" > "31.9.0").
fn compare_versions(a: &str, b: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let norm = |s: &str| {
        s.rsplit(',')
            .next()
            .unwrap_or(s)
            .split('.')
            .map(|p| p.parse::<u64>().unwrap_or(0))
            .collect::<Vec<_>>()
    };
    let (mut av, mut bv) = (norm(a), norm(b));
    let n = av.len().max(bv.len());
    av.resize(n, 0);
    bv.resize(n, 0);
    av.cmp(&bv).then_with(|| a.cmp(b))
}

/// Match `nv_dispi.inf` to every `nv_dispi.inf_amd64_<hash>` store dir
/// (case-insensitive; several can share one original name).
fn match_repo_dirs<'a>(original: &str, dirs: &'a [String]) -> Vec<&'a str> {
    let prefix = format!("{original}_").to_lowercase();
    dirs.iter()
        .map(String::as_str)
        .filter(|d| d.to_lowercase().starts_with(&prefix))
        .collect()
}

/// Flag all-but-newest within each (provider, class) group.
fn flag_superseded(drivers: &mut [DriverEntry]) {
    use std::collections::HashMap;
    let mut groups: HashMap<(String, String), Vec<usize>> = HashMap::new();
    for (i, d) in drivers.iter().enumerate() {
        if !d.published.to_lowercase().starts_with("oem") {
            continue;
        }
        groups
            .entry((d.provider.to_lowercase(), d.class_name.to_lowercase()))
            .or_default()
            .push(i);
    }
    for idxs in groups.values() {
        if idxs.len() < 2 {
            continue;
        }
        let mut best = idxs[0];
        for &i in &idxs[1..] {
            if compare_versions(&drivers[i].version, &drivers[best].version)
                == std::cmp::Ordering::Greater
            {
                best = i;
            }
        }
        for &i in idxs {
            if i != best {
                drivers[i].superseded = true;
            }
        }
    }
}

#[tauri::command]
fn scan_drivers() -> Result<DriverReport, String> {
    #[cfg(not(windows))]
    {
        return Err("Stale drivers: Windows only.".to_string());
    }
    #[cfg(windows)]
    {
        let out = std::process::Command::new("pnputil")
            .arg("/enum-drivers")
            .output()
            .map_err(|e| format!("pnputil failed to run: {e}"))?;
        if !out.status.success() {
            return Err(format!(
                "pnputil failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }
        let mut drivers = parse_pnputil(&String::from_utf8_lossy(&out.stdout));
        // One walk of the driver store: full-store total plus a per-dir
        // map joined to packages by original-name prefix.
        let repo = Path::new(r"C:\Windows\System32\DriverStore\FileRepository");
        let mut sizes: HashMap<String, u64> = HashMap::new();
        let mut store_bytes: u64 = 0;
        if let Ok(entries) = std::fs::read_dir(repo) {
            for e in entries.flatten() {
                let p = e.path();
                if !e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                    continue;
                }
                let name = p
                    .file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let mut total: u64 = 0;
                for f in WalkDir::new(&p).min_depth(1).follow_links(false).into_iter().flatten()
                {
                    if f.file_type().is_file() {
                        total += f.path().metadata().map(|m| m.len()).unwrap_or(0);
                    }
                }
                store_bytes += total;
                sizes.insert(name, total);
            }
        }
        let names: Vec<String> = sizes.keys().cloned().collect();
        for d in &mut drivers {
            d.size = match_repo_dirs(&d.original, &names)
                .iter()
                .map(|dir| sizes.get(*dir).copied().unwrap_or(0))
                .sum();
        }
        flag_superseded(&mut drivers);
        drivers.sort_by(|a, b| b.size.cmp(&a.size));
        let total_bytes = drivers.iter().map(|d| d.size).sum();
        Ok(DriverReport {
            drivers,
            total_bytes,
            store_bytes,
        })
    }
}

// ---- AppX cleanup (Windows) ----

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AppxEntry {
    name: String,
    full_name: String,
    version: String,
    size: u64,
    provisioned: bool,
    blocked: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppxReport {
    apps: Vec<AppxEntry>,
    total_bytes: u64,
}

/// Packages that must never be removed, even when asked.
fn appx_blocked(name: &str, is_framework: bool) -> Option<&'static str> {
    if is_framework {
        return Some("framework package");
    }
    const NEVER: &[&str] = &[
        "microsoft.windows.shellexperiencehost",
        "microsoft.windows.startmenuexperiencehost",
        "microsoft.windows.search",
        "microsoft.windows.cortana",
        "windows.immersivecontrolpanel",
        "microsoft.windowsstore",
        "microsoft.storepurchaseapp",
        "microsoft.desktopappinstaller",
        "microsoft.microsoftedge.stable",
        "microsoft.microsoftedge",
        "microsoft.ui.xaml.2.7",
        "microsoft.ui.xaml.2.8",
        "microsoft.vclibs.140.00",
        "microsoft.vclibs.140.00.uwpdesktop",
        "microsoft.windowsappsdk.1.5",
        "microsoft.windowsappsdk.1.6",
        "windows.devicesflow",
        "sechealthui",
    ];
    let lower = name.to_lowercase();
    if NEVER.iter().any(|n| lower.starts_with(n)) {
        return Some("system package");
    }
    None
}

fn safe_arg(s: &str) -> bool {
    !s.is_empty()
        && s.len() < 300
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

fn parse_appx_list(json_text: &str) -> Vec<(String, String, String, bool)> {
    let v: serde_json::Value = match serde_json::from_str(json_text) {
        Ok(v) => v,
        Err(_) => return vec![],
    };
    let items: Vec<&serde_json::Value> = match &v {
        serde_json::Value::Array(a) => a.iter().collect(),
        serde_json::Value::Object(_) => vec![&v],
        _ => vec![],
    };
    let mut out = Vec::new();
    for it in items {
        let get = |k: &str| it.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
        let name = get("Name");
        if name.is_empty() {
            continue;
        }
        let is_framework = it
            .get("IsFramework")
            .and_then(|x| x.as_bool())
            .unwrap_or(false);
        out.push((name, get("PackageFullName"), get("Version"), is_framework));
    }
    out
}

fn dir_size(path: &Path) -> u64 {
    let mut total: u64 = 0;
    for f in WalkDir::new(path)
        .min_depth(1)
        .follow_links(false)
        .into_iter()
        .flatten()
    {
        if f.file_type().is_file() {
            total += f.path().metadata().map(|m| m.len()).unwrap_or(0);
        }
    }
    total
}

#[tauri::command]
fn scan_appx() -> Result<AppxReport, String> {
    #[cfg(not(windows))]
    {
        return Err("AppX cleanup: Windows only.".to_string());
    }
    #[cfg(windows)]
    {
        let ps = r#"Get-AppxPackage | Select-Object Name,PackageFullName,Version,IsFramework | ConvertTo-Json -Compress -Depth 3"#;
        let out = std::process::Command::new("powershell")
            .args(["-NoProfile", "-Command", ps])
            .output()
            .map_err(|e| format!("AppX query failed to run: {e}"))?;
        if !out.status.success() {
            return Err(format!(
                "AppX query failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }
        let prov: HashSet<String> = std::process::Command::new("powershell")
            .args([
                "-NoProfile",
                "-Command",
                "Get-AppxProvisionedPackage -Online | Select-Object -ExpandProperty DisplayName",
            ])
            .output()
            .ok()
            .map(|o| {
                String::from_utf8_lossy(&o.stdout)
                    .lines()
                    .map(|l| l.trim().to_lowercase())
                    .filter(|l| !l.is_empty())
                    .collect()
            })
            .unwrap_or_default();
        let mut apps: Vec<AppxEntry> = Vec::new();
        for (name, full_name, version, is_framework) in
            parse_appx_list(&String::from_utf8_lossy(&out.stdout))
        {
            // Size from the install location is expensive per app; do a
            // bounded lookup via the package dir name when present.
            let mut size: u64 = 0;
            let base = Path::new(r"C:\Program Files\WindowsApps").join(&full_name);
            if base.is_dir() {
                size = dir_size(&base);
            }
            apps.push(AppxEntry {
                blocked: appx_blocked(&name, is_framework)
                    .unwrap_or("")
                    .to_string(),
                provisioned: prov.contains(&name.to_lowercase()),
                name,
                full_name,
                version,
                size,
            });
        }
        apps.sort_by(|a, b| b.size.cmp(&a.size));
        let total_bytes = apps.iter().map(|a| a.size).sum();
        Ok(AppxReport { apps, total_bytes })
    }
}

#[tauri::command]
fn remove_appx(packages: Vec<String>) -> Result<TrashReport, String> {
    if packages.is_empty() {
        return Err("Nothing selected.".to_string());
    }
    if packages.len() > TRASH_CAP {
        return Err(format!("Refusing: too many at once ({} max).", TRASH_CAP));
    }
    let mut results: Vec<TrashItem> = Vec::with_capacity(packages.len());
    let mut trashed: u64 = 0;
    let mut failed: u64 = 0;
    for full in &packages {
        let name = full.split('_').next().unwrap_or(full);
        let mut error = String::new();
        if !safe_arg(full) {
            error = "Refusing: unexpected package name.".to_string();
        } else if let Some(reason) = appx_blocked(name, false) {
            error = format!("Refusing: {reason}.");
        } else {
            let ps = format!("Remove-AppxPackage -Package '{full}'");
            match std::process::Command::new("powershell")
                .args(["-NoProfile", "-Command", &ps])
                .output()
            {
                Ok(o) if o.status.success() => {}
                Ok(o) => {
                    error = String::from_utf8_lossy(&o.stderr).trim().to_string();
                    if error.is_empty() {
                        error = "Removal failed.".to_string();
                    }
                }
                Err(e) => {
                    error = e.to_string();
                }
            }
        }
        if error.is_empty() {
            results.push(TrashItem {
                path: full.clone(),
                ok: true,
                error: String::new(),
            });
            trashed += 1;
        } else {
            results.push(TrashItem {
                path: full.clone(),
                ok: false,
                error,
            });
            failed += 1;
        }
    }
    Ok(TrashReport {
        results,
        trashed,
        failed,
    })
}

// ---- Component store (Windows, DISM, admin only) ----

fn dism_args() -> Vec<&'static str> {
    // StartComponentCleanup WITHOUT ResetBase: keeps updates uninstallable.
    // Never runs AnalyzeComponentStore (known-flaky hang path).
    vec![
        "/Online",
        "/Cleanup-Image",
        "/StartComponentCleanup",
    ]
}

#[tauri::command]
fn run_component_cleanup() -> Result<TrashReport, String> {
    #[cfg(not(windows))]
    {
        return Err("Component store: Windows only.".to_string());
    }
    #[cfg(windows)]
    {
        if !is_elevated() {
            return Err("Administrator rights needed. Restart elevated first.".to_string());
        }
        let system_root =
            std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".to_string());
        let dism = format!("{system_root}\\System32\\Dism.exe");
        let t0 = std::time::Instant::now();
        let out = std::process::Command::new(&dism)
            .args(dism_args())
            .output()
            .map_err(|e| format!("DISM failed to run: {e}"))?;
        let secs = t0.elapsed().as_secs();
        let text = String::from_utf8_lossy(&out.stdout).into_owned();
        let tail: String = text.chars().rev().take(1500).collect::<String>().chars().rev().collect();
        let ok = out.status.success();
        Ok(TrashReport {
            results: vec![TrashItem {
                path: format!("Component cleanup ({}s)", secs),
                ok,
                error: if ok {
                    tail
                } else {
                    format!("exit {}: {tail}", out.status)
                },
            }],
            trashed: u64::from(ok),
            failed: u64::from(!ok),
        })
    }
}

// ---- System monitors ----

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct DiskStat {
    mount: String,
    total: u64,
    free: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SysStats {
    cpu: f32,
    mem_used: u64,
    mem_total: u64,
    disks: Vec<DiskStat>,
}

#[tauri::command]
fn system_stats() -> Result<SysStats, String> {
    let mut sys =
        sysinfo::System::new().map_err(|e| format!("stats unavailable: {e}"))?;
    // CPU usage needs two samples to form a delta.
    sys.refresh_cpu_all();
    std::thread::sleep(std::time::Duration::from_millis(200));
    sys.refresh_cpu_all();
    sys.refresh_memory();
    let disks = sysinfo::Disks::new_with_refreshed_list()
        .map_err(|e| format!("disk list unavailable: {e}"))?;
    Ok(SysStats {
        cpu: sys.global_cpu_usage(),
        mem_used: sys.used_memory(),
        mem_total: sys.total_memory(),
        disks: disks
            .iter()
            .take(16)
            .map(|d| DiskStat {
                mount: d.mount_point().to_string_lossy().into_owned(),
                total: d.total_space(),
                free: d.available_space(),
            })
            .collect(),
    })
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            use tauri::menu::{Menu, MenuItem};
            use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
            use tauri::Manager;
            let show = MenuItem::with_id(app, "show", "Show OS Cleaner", true, None::<&str>)?;
            let hide = MenuItem::with_id(app, "hide", "Hide", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show, &hide, &quit])?;
            TrayIconBuilder::with_id("main")
                .icon(app.default_window_icon().unwrap().clone())
                .menu(&menu)
                .tooltip("OS Cleaner")
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "show" => {
                        if let Some(w) = app.get_webview_window("main") {
                            let _ = w.show();
                            let _ = w.set_focus();
                        }
                    }
                    "hide" => {
                        if let Some(w) = app.get_webview_window("main") {
                            let _ = w.hide();
                        }
                    }
                    "quit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        let app = tray.app_handle();
                        if let Some(w) = app.get_webview_window("main") {
                            let _ = w.show();
                            let _ = w.set_focus();
                        }
                    }
                })
                .build(app)?;
            Ok(())
        })
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
            restart_elevated,
            scan_drivers,
            scan_appx,
            remove_appx,
            run_component_cleanup,
            system_stats
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
    fn pnputil_parsing_and_superseded_flags() {
        let sample = "Published Name:     oem9.inf\nOriginal Name:      nv_dispi.inf\nProvider Name:      NVIDIA\nClass Name:         Display adapters\nDriver Version:     07/10/2024,32.0.15.6600\n\nPublished Name:     oem62.inf\nOriginal Name:      nv_dispi.inf\nProvider Name:      NVIDIA\nClass Name:         Display adapters\nDriver Version:     01/12/2024,31.0.15.1234\n";
        let mut drivers = parse_pnputil(sample);
        assert_eq!(drivers.len(), 2);
        assert_eq!(drivers[0].published, "oem9.inf");
        flag_superseded(&mut drivers);
        let old = drivers.iter().find(|d| d.published == "oem62.inf").unwrap();
        let new = drivers.iter().find(|d| d.published == "oem9.inf").unwrap();
        assert!(old.superseded);
        assert!(!new.superseded);
        assert_eq!(
            compare_versions("32.0.15.6600", "31.0.15.1234"),
            std::cmp::Ordering::Greater
        );
        let dirs = vec![
            "nv_dispi.inf_amd64_deadbeef".to_string(),
            "NV_DISPI.inf_amd64_cafe1234".to_string(),
        ];
        assert_eq!(
            match_repo_dirs("nv_dispi.inf", &dirs),
            vec![
                "nv_dispi.inf_amd64_deadbeef",
                "NV_DISPI.inf_amd64_cafe1234"
            ]
        );
        assert!(match_repo_dirs("other.inf", &dirs).is_empty());
    }

    #[test]
    fn appx_parsing_and_blocklist() {
        let json = r#"[{"Name":"Some.App","PackageFullName":"Some.App_1.0_x64__abc","Version":"1.0.0.0","IsFramework":false},{"Name":"Microsoft.Windows.ShellExperienceHost","PackageFullName":"Microsoft.Windows.ShellExperienceHost_1.0_neutral__xyz","Version":"1.0.0.0","IsFramework":false}]"#;
        let apps = parse_appx_list(json);
        assert_eq!(apps.len(), 2);
        assert_eq!(apps[0].0, "Some.App");
        assert!(appx_blocked("Some.App", false).is_none());
        assert!(appx_blocked("Microsoft.Windows.ShellExperienceHost", false).is_some());
        assert!(appx_blocked("Anything", true).is_some());
        assert!(safe_arg("Some.App_1.0_x64__abc"));
        assert!(!safe_arg("a'b;rm"));
        assert!(!safe_arg(""));
    }

    #[test]
    fn dism_args_have_no_resetbase_or_analyze() {
        let args = dism_args();
        assert!(args.contains(&"/StartComponentCleanup"));
        assert!(!args.iter().any(|a| *a == "/ResetBase"));
        assert!(!args.iter().any(|a| a.contains("Analyze")));
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

    #[test]
    fn system_stats_are_sane() {
        let s = system_stats().unwrap();
        assert!(s.mem_total > 0);
        assert!(s.mem_total >= s.mem_used);
        assert!(!s.disks.is_empty());
        assert!(s.cpu >= 0.0);
    }
}
