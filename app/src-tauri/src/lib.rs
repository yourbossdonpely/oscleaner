// OS Cleaner v0.2 — directory scanner with hardlink-aware sizing,
// largest/oldest tracking, and hash-based duplicate detection.
// Each unique file is counted once, however many names point at it
// (the WinSxS lesson). Symlinks are never followed.
// Learn more about Tauri commands at https://tauri.app/develop/calling-rust/

use jwalk::WalkDir;
use serde::Serialize;
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
    Roots { roots, home }
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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            scan_dir,
            list_roots,
            find_duplicates,
            trash_delete
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
}
