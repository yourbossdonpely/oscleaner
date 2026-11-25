// OS Cleaner v0.1 — directory scanner with hardlink-aware sizing.
// Each unique file is counted once, however many names point at it
// (the WinSxS lesson). Symlinks are never followed.
// Learn more about Tauri commands at https://tauri.app/develop/calling-rust/

use jwalk::WalkDir;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

#[derive(Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
struct Agg {
    size: u64,
    files: u64,
    dirs: u64,
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
    children: Vec<ScanNode>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Roots {
    roots: Vec<String>,
    home: String,
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
            children: vec![],
        });
    }
    if !root_md.is_dir() {
        return Err(format!("Not a file or folder: {}", root.display()));
    }

    let mut seen: HashSet<(u64, u64)> = HashSet::new();
    let mut buckets: HashMap<PathBuf, Agg> = HashMap::new();
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
            children: vec![],
        })
        .collect();
    children.sort_by(|a, b| b.size.cmp(&a.size));

    Ok(ScanNode {
        name: node_name(root),
        path: root.to_string_lossy().into_owned(),
        size: total.size,
        files: total.files,
        dirs: total.dirs,
        errors,
        children,
    })
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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![scan_dir, list_roots])
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
}
