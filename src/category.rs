use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use clap::ValueEnum;
use serde::Serialize;

use crate::scan::{ScanConfig, find_project_deps};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, ValueEnum, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Category {
    UserCaches,
    Logs,
    TempItems,
    PackageCaches,
    ProjectDeps,
    XcodeDerivedData,
    XcodeArchives,
    XcodeDeviceSupport,
    SimulatorCaches,
    IosBackups,
    OldDownloads,
    Trash,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Safety {
    /// Gets rebuilt or downloaded again when needed.
    Safe,
    /// Check what you select. Some of it cannot be recovered.
    Review,
}

#[derive(Default)]
pub struct Found {
    pub paths: Vec<PathBuf>,
    pub issues: Vec<String>,
}

const PACKAGE_CACHES: &[&str] = &[
    ".npm/_cacache",
    ".npm/_npx",
    ".pnpm-store",
    "Library/pnpm/store",
    ".yarn/berry/cache",
    ".bun/install/cache",
    ".cargo/registry/cache",
    ".cargo/registry/src",
    ".cargo/git/checkouts",
    ".gradle/caches",
    ".m2/repository",
    ".composer/cache",
    "go/pkg/mod/cache",
    ".cocoapods/repos",
];

impl Category {
    pub const ALL: [Category; 12] = [
        Category::UserCaches,
        Category::Logs,
        Category::TempItems,
        Category::PackageCaches,
        Category::ProjectDeps,
        Category::XcodeDerivedData,
        Category::XcodeArchives,
        Category::XcodeDeviceSupport,
        Category::SimulatorCaches,
        Category::IosBackups,
        Category::OldDownloads,
        Category::Trash,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Category::UserCaches => "App caches",
            Category::Logs => "Logs",
            Category::TempItems => "Temporary items",
            Category::PackageCaches => "Package manager caches",
            Category::ProjectDeps => "Project dependencies",
            Category::XcodeDerivedData => "Xcode DerivedData",
            Category::XcodeArchives => "Xcode archives",
            Category::XcodeDeviceSupport => "Xcode device support",
            Category::SimulatorCaches => "Simulator caches",
            Category::IosBackups => "iPhone/iPad backups",
            Category::OldDownloads => "Old downloads",
            Category::Trash => "Trash",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Category::UserCaches => {
                "~/Library/Caches. Apps store downloaded and computed data here to start faster. \
                 Apps rebuild it when needed, so the first launch after cleaning can be slower."
            }
            Category::Logs => {
                "~/Library/Logs. Old log and crash files. Only useful when you debug a problem."
            }
            Category::TempItems => {
                "$TMPDIR/TemporaryItems. Temporary copies macOS makes, like the screenshot files \
                 (NSIRD_screencaptureui_*) you get when you drag a screenshot thumbnail."
            }
            Category::PackageCaches => {
                "Download caches of npm, pnpm, yarn, bun, cargo, gradle, maven, composer, go, \
                 CocoaPods and ~/.cache. They download again on the next install."
            }
            Category::ProjectDeps => {
                "node_modules, Rust target, composer vendor, Python venvs, Pods, .next and similar \
                 folders inside your projects. Reinstall with npm install, cargo build, ... \
                 Check the age column to find projects you no longer use."
            }
            Category::XcodeDerivedData => {
                "Xcode build output. Xcode rebuilds it on the next build."
            }
            Category::XcodeArchives => {
                "App archives made with Product > Archive. You need them to symbolicate crash \
                 reports of released builds. Keep the ones you still ship."
            }
            Category::XcodeDeviceSupport => {
                "Debug symbols Xcode copies from every iOS/watchOS version you connected. \
                 Old versions are safe to remove."
            }
            Category::SimulatorCaches => "Caches of the iOS simulators. Rebuilt when needed.",
            Category::IosBackups => {
                "Local backups of your iPhone or iPad made with Finder. Removing them cannot be undone. \
                 Needs Full Disk Access."
            }
            Category::OldDownloads => "Files in ~/Downloads you downloaded a while ago.",
            Category::Trash => "Files in your Trash. Cleaning this always deletes them for good.",
        }
    }

    pub fn safety(self) -> Safety {
        match self {
            Category::ProjectDeps
            | Category::XcodeArchives
            | Category::IosBackups
            | Category::OldDownloads
            | Category::Trash => Safety::Review,
            _ => Safety::Safe,
        }
    }

    /// Moving files from the Trash to the Trash makes no sense.
    pub fn always_permanent(self) -> bool {
        self == Category::Trash
    }

    pub fn discover(self, config: &ScanConfig) -> Found {
        let home = &config.home;
        let mut found = Found::default();

        match self {
            Category::UserCaches => children(&home.join("Library/Caches"), &mut found),
            Category::Logs => children(&home.join("Library/Logs"), &mut found),
            Category::TempItems => children(&config.temp.join("TemporaryItems"), &mut found),
            Category::PackageCaches => {
                found.paths.extend(
                    PACKAGE_CACHES
                        .iter()
                        .map(|path| home.join(path))
                        .filter(|path| path.exists()),
                );
                children(&home.join(".cache"), &mut found);
            }
            Category::ProjectDeps => {
                let skip = config.project_skip_dirs();
                for dir in &config.project_dirs {
                    found.paths.extend(find_project_deps(dir, &skip, 12));
                }
            }
            Category::XcodeDerivedData => {
                children(
                    &home.join("Library/Developer/Xcode/DerivedData"),
                    &mut found,
                );
            }
            Category::XcodeArchives => {
                children(&home.join("Library/Developer/Xcode/Archives"), &mut found);
            }
            Category::XcodeDeviceSupport => {
                for platform in ["iOS", "watchOS", "tvOS", "visionOS"] {
                    let dir = format!("Library/Developer/Xcode/{platform} DeviceSupport");
                    children(&home.join(dir), &mut found);
                }
            }
            Category::SimulatorCaches => {
                children(
                    &home.join("Library/Developer/CoreSimulator/Caches"),
                    &mut found,
                );
            }
            Category::IosBackups => {
                children(
                    &home.join("Library/Application Support/MobileSync/Backup"),
                    &mut found,
                );
            }
            Category::OldDownloads => {
                children(&home.join("Downloads"), &mut found);
                let cutoff =
                    SystemTime::now() - Duration::from_secs(config.downloads_days * 86_400);
                found
                    .paths
                    .retain(|path| added_at(path).is_some_and(|added| added < cutoff));
            }
            Category::Trash => children(&home.join(".Trash"), &mut found),
        }

        found
    }

    pub fn label_for(self, path: &Path, config: &ScanConfig) -> String {
        let relative = match self {
            Category::ProjectDeps | Category::PackageCaches => path.strip_prefix(&config.home).ok(),
            _ => None,
        };

        relative
            .or_else(|| path.file_name().map(Path::new))
            .unwrap_or(path)
            .display()
            .to_string()
    }
}

fn children(dir: &Path, found: &mut Found) {
    match fs::read_dir(dir) {
        Ok(entries) => found.paths.extend(
            entries
                .filter_map(Result::ok)
                .filter(|entry| !matches!(entry.file_name().to_str(), Some(".DS_Store" | ".localized")))
                .map(|entry| entry.path()),
        ),
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) if error.kind() == ErrorKind::PermissionDenied => found.issues.push(format!(
            "No access to {}. Give your terminal Full Disk Access in System Settings > Privacy & Security.",
            dir.display()
        )),
        Err(error) => found.issues.push(format!("{}: {error}", dir.display())),
    }
}

/// When a file landed in Downloads. The modified time often comes from the server.
pub fn added_at(path: &Path) -> Option<SystemTime> {
    let meta = fs::symlink_metadata(path).ok()?;
    meta.created().or_else(|_| meta.modified()).ok()
}
