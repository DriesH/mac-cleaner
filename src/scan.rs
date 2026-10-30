use std::collections::HashSet;
use std::fs::{self, Metadata};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

use rayon::prelude::*;

use crate::category::{Category, added_at};
use crate::simulator;

pub struct ScanConfig {
    pub home: PathBuf,
    pub temp: PathBuf,
    /// `/tmp`, shared by all users. `temp` is your own temp folder.
    pub shared_temp: PathBuf,
    pub project_dirs: Vec<PathBuf>,
    pub downloads_days: u64,
}

impl ScanConfig {
    /// Big folders that never hold code projects. Skipping them keeps the project search fast.
    /// OrbStack shows the files inside Docker there. Docker manages them, not you.
    pub fn project_skip_dirs(&self) -> Vec<PathBuf> {
        [
            "Library",
            "Pictures",
            "Movies",
            "Music",
            "Applications",
            "OrbStack",
        ]
        .iter()
        .map(|dir| self.home.join(dir))
        .collect()
    }
}

#[derive(Clone, Debug)]
pub struct Item {
    pub category: Category,
    pub path: PathBuf,
    pub label: String,
    pub size: u64,
    pub modified: Option<SystemTime>,
}

pub struct CategoryScan {
    pub category: Category,
    pub items: Vec<Item>,
    pub issues: Vec<String>,
}

impl CategoryScan {
    pub fn total(&self) -> u64 {
        self.items.iter().map(|item| item.size).sum()
    }
}

pub fn scan_category(category: Category, config: &ScanConfig) -> CategoryScan {
    if category == Category::SimulatorRuntimes {
        return scan_runtimes();
    }

    let found = category.discover(config);

    let mut items: Vec<Item> = found
        .paths
        .into_par_iter()
        .map(|path| {
            let modified = if category == Category::OldDownloads {
                added_at(&path)
            } else {
                fs::symlink_metadata(&path)
                    .and_then(|meta| meta.modified())
                    .ok()
            };

            Item {
                category,
                label: category.label_for(&path, config),
                size: disk_size(&path),
                modified,
                path,
            }
        })
        .filter(|item| item.size > 0)
        .collect();

    items.sort_by_key(|item| std::cmp::Reverse(item.size));

    CategoryScan {
        category,
        items,
        issues: found.issues,
    }
}

fn scan_runtimes() -> CategoryScan {
    let (runtimes, issues) = match simulator::runtimes() {
        Ok(runtimes) => (runtimes, Vec::new()),
        Err(error) => (Vec::new(), vec![format!("{error:#}")]),
    };

    let mut items: Vec<Item> = runtimes
        .iter()
        .map(|runtime| Item {
            category: Category::SimulatorRuntimes,
            label: runtime.name(),
            size: disk_size(&runtime.path),
            modified: runtime.last_used(),
            path: runtime.path.clone(),
        })
        .collect();
    items.sort_by_key(|item| std::cmp::Reverse(item.size));

    CategoryScan {
        category: Category::SimulatorRuntimes,
        items,
        issues,
    }
}

/// Space the path takes on disk, like `du`. Hard links are counted once.
pub fn disk_size(path: &Path) -> u64 {
    let seen = Mutex::new(HashSet::new());
    size_of(path, &seen)
}

fn size_of(path: &Path, seen: &Mutex<HashSet<(u64, u64)>>) -> u64 {
    let Ok(meta) = fs::symlink_metadata(path) else {
        return 0;
    };
    let own = allocated(&meta, seen);

    if !meta.is_dir() {
        return own;
    }

    let Ok(entries) = fs::read_dir(path) else {
        return own;
    };
    let paths: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .collect();

    own + paths
        .par_iter()
        .map(|path| size_of(path, seen))
        .sum::<u64>()
}

fn allocated(meta: &Metadata, seen: &Mutex<HashSet<(u64, u64)>>) -> u64 {
    if !meta.is_dir() && meta.nlink() > 1 {
        let first_time = seen
            .lock()
            .map(|mut seen| seen.insert((meta.dev(), meta.ino())))
            .unwrap_or(true);
        if !first_time {
            return 0;
        }
    }

    meta.blocks() * 512
}

/// Folder name and the files that must sit next to it. No files means the name is enough.
const DEPENDENCY_DIRS: &[(&str, &[&str])] = &[
    ("node_modules", &[]),
    ("target", &["Cargo.toml"]),
    ("vendor", &["composer.json"]),
    (".venv", &[]),
    ("venv", &["pyproject.toml", "requirements.txt", "setup.py"]),
    (".next", &["package.json"]),
    (".nuxt", &["package.json"]),
    (".svelte-kit", &["package.json"]),
    (".turbo", &["package.json"]),
    (".parcel-cache", &["package.json"]),
    ("Pods", &["Podfile"]),
    (".build", &["Package.swift"]),
    (
        ".gradle",
        &[
            "build.gradle",
            "build.gradle.kts",
            "settings.gradle",
            "settings.gradle.kts",
        ],
    ),
];

const PACKAGE_EXTENSIONS: &[&str] = &[
    "app",
    "photoslibrary",
    "musiclibrary",
    "tvlibrary",
    "bundle",
    "framework",
    "xcarchive",
];

pub fn find_project_deps(dir: &Path, skip: &[PathBuf], depth: u8) -> Vec<PathBuf> {
    if depth == 0 {
        return Vec::new();
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };

    let subdirs: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .map(|entry| entry.path())
        .collect();

    subdirs
        .par_iter()
        .flat_map(|path| {
            if is_dependency_dir(path) {
                vec![path.clone()]
            } else if should_skip(path, skip) {
                Vec::new()
            } else {
                find_project_deps(path, skip, depth - 1)
            }
        })
        .collect()
}

pub fn is_dependency_dir(path: &Path) -> bool {
    let (Some(name), Some(parent)) = (path.file_name().and_then(|n| n.to_str()), path.parent())
    else {
        return false;
    };

    DEPENDENCY_DIRS
        .iter()
        .find(|(dir, _)| *dir == name)
        .is_some_and(|(_, markers)| {
            markers.is_empty() || markers.iter().any(|marker| parent.join(marker).exists())
        })
}

fn should_skip(path: &Path, skip: &[PathBuf]) -> bool {
    let hidden = path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with('.'));

    hidden || is_package(path) || skip.iter().any(|dir| dir == path)
}

/// Folders macOS shows as one file, like apps and photo libraries.
pub fn is_package(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| PACKAGE_EXTENSIONS.contains(&ext))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch(path: &Path, bytes: usize) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, vec![1u8; bytes]).unwrap();
    }

    #[test]
    fn disk_size_should_sum_nested_files() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("a/b/file"), 100_000);
        touch(&dir.path().join("c"), 100_000);

        assert!(disk_size(dir.path()) >= 200_000);
    }

    #[test]
    fn disk_size_should_count_hard_links_once() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("file"), 100_000);
        let single = disk_size(dir.path());
        fs::hard_link(dir.path().join("file"), dir.path().join("link")).unwrap();

        assert_eq!(disk_size(dir.path()), single);
    }

    #[test]
    fn find_project_deps_should_find_known_folders() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("web/node_modules/x/index.js"), 10);
        touch(&dir.path().join("rust/Cargo.toml"), 10);
        touch(&dir.path().join("rust/target/debug/bin"), 10);
        touch(&dir.path().join("php/composer.json"), 10);
        touch(&dir.path().join("php/vendor/autoload.php"), 10);

        let mut found = find_project_deps(dir.path(), &[], 5);
        found.sort();

        let expected =
            ["php/vendor", "rust/target", "web/node_modules"].map(|p| dir.path().join(p));
        assert_eq!(found, expected);
    }

    #[test]
    fn find_project_deps_should_ignore_target_without_cargo_toml() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("site/target/page.html"), 10);

        assert!(find_project_deps(dir.path(), &[], 5).is_empty());
    }

    #[test]
    fn find_project_deps_should_not_enter_skipped_dirs() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Library/x/node_modules/a"), 10);

        assert!(find_project_deps(dir.path(), &[dir.path().join("Library")], 5).is_empty());
    }
}
