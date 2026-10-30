use std::fs;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result, ensure};
use rayon::prelude::*;
use trash::TrashContext;
use trash::macos::{DeleteMethod, TrashContextExtMacos};

use crate::category::Category;
use crate::scan::{Item, ScanConfig};
use crate::simulator;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Trash,
    Permanent,
}

impl Mode {
    pub fn toggle(self) -> Self {
        match self {
            Mode::Trash => Mode::Permanent,
            Mode::Permanent => Mode::Trash,
        }
    }
}

/// A folder we may delete in, and how deep a path must be inside it.
type Root = (PathBuf, usize);

pub struct Cleaner {
    roots: Vec<Root>,
    trash: TrashContext,
}

impl Cleaner {
    pub fn new(config: &ScanConfig) -> Self {
        let mut trash = TrashContext::default();
        // Finder asks for Automation access and plays a sound for every file.
        trash.set_delete_method(DeleteMethod::NsFileManager);

        Self {
            roots: [config.home.clone(), config.temp.clone()]
                .into_iter()
                .chain(config.project_dirs.iter().cloned())
                .map(|root| (root, 2))
                .chain([(config.shared_temp.clone(), 1)])
                .collect(),
            trash,
        }
    }

    /// Permanent deletes add the freed bytes to `removed` while they run.
    pub fn remove(&self, item: &Item, mode: Mode, removed: &AtomicU64) -> Result<()> {
        if item.category == Category::SimulatorRuntimes {
            simulator::delete(&item.path)?;
            removed.fetch_add(item.size, Ordering::Relaxed);
            return Ok(());
        }

        check_safe(&item.path, &self.roots)?;

        let mode = if item.category.always_permanent() {
            Mode::Permanent
        } else {
            mode
        };

        match mode {
            Mode::Trash => self
                .trash
                .delete(&item.path)
                .with_context(|| format!("could not move {} to the Trash", item.path.display())),
            Mode::Permanent => remove_permanently(&item.path, removed),
        }
    }
}

fn remove_permanently(path: &Path, removed: &AtomicU64) -> Result<()> {
    remove_tree(path, removed).with_context(|| format!("could not delete {}", path.display()))
}

/// Like `fs::remove_dir_all`, but counts freed bytes and deletes in parallel.
fn remove_tree(path: &Path, removed: &AtomicU64) -> io::Result<()> {
    let meta = fs::symlink_metadata(path)?;

    if meta.is_dir() {
        let children = fs::read_dir(path)?
            .map(|entry| entry.map(|e| e.path()))
            .collect::<io::Result<Vec<_>>>()?;
        children
            .par_iter()
            .try_for_each(|child| remove_tree(child, removed))?;
        fs::remove_dir(path)?;
    } else {
        fs::remove_file(path)?;
    }

    removed.fetch_add(meta.blocks() * 512, Ordering::Relaxed);
    Ok(())
}

/// Last line of defense: only delete things deep enough inside a scanned root folder.
pub fn check_safe(path: &Path, roots: &[Root]) -> Result<()> {
    ensure!(
        path.is_absolute(),
        "refusing to delete relative path {}",
        path.display()
    );
    ensure!(
        !path.components().any(|part| part == Component::ParentDir),
        "refusing to delete {}: path contains '..'",
        path.display()
    );

    let inside = roots.iter().any(|(root, depth)| {
        path.strip_prefix(root)
            .is_ok_and(|relative| relative.components().count() >= *depth)
    });
    ensure!(
        inside,
        "refusing to delete {}: not inside your home or a temp folder",
        path.display()
    );

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roots() -> Vec<Root> {
        vec![
            (PathBuf::from("/Users/me"), 2),
            (PathBuf::from("/var/folders/x/T"), 2),
            (PathBuf::from("/private/tmp"), 1),
        ]
    }

    #[test]
    fn check_safe_should_allow_cache_folder() {
        assert!(check_safe(Path::new("/Users/me/Library/Caches/com.app"), &roots()).is_ok());
    }

    #[test]
    fn check_safe_should_allow_temp_items() {
        assert!(
            check_safe(
                Path::new("/var/folders/x/T/TemporaryItems/NSIRD_x"),
                &roots()
            )
            .is_ok()
        );
    }

    #[test]
    fn check_safe_should_allow_top_level_tmp_item() {
        assert!(check_safe(Path::new("/private/tmp/build-cache"), &roots()).is_ok());
    }

    #[test]
    fn check_safe_should_refuse_tmp_itself() {
        assert!(check_safe(Path::new("/private/tmp"), &roots()).is_err());
    }

    #[test]
    fn check_safe_should_refuse_home_folder() {
        assert!(check_safe(Path::new("/Users/me"), &roots()).is_err());
    }

    #[test]
    fn check_safe_should_refuse_top_level_home_folder() {
        assert!(check_safe(Path::new("/Users/me/Documents"), &roots()).is_err());
    }

    #[test]
    fn check_safe_should_refuse_paths_outside_roots() {
        assert!(check_safe(Path::new("/System/Library/Caches/x"), &roots()).is_err());
    }

    #[test]
    fn check_safe_should_refuse_parent_dir_tricks() {
        assert!(check_safe(Path::new("/Users/me/Library/../../other"), &roots()).is_err());
    }

    #[test]
    fn remove_permanently_should_delete_folder() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("node_modules");
        fs::create_dir_all(target.join("pkg")).unwrap();
        fs::write(target.join("pkg/index.js"), "x").unwrap();

        remove_permanently(&target, &AtomicU64::new(0)).unwrap();

        assert!(!target.exists());
    }

    #[test]
    fn remove_permanently_should_count_freed_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("cache");
        fs::create_dir_all(&target).unwrap();
        fs::write(target.join("blob"), vec![1u8; 100_000]).unwrap();
        let removed = AtomicU64::new(0);

        remove_permanently(&target, &removed).unwrap();

        assert!(removed.load(Ordering::Relaxed) >= 100_000);
    }
}
