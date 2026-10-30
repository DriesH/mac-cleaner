use std::collections::HashMap;
use std::fs;
use std::io::ErrorKind;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use clap::ValueEnum;
use nix::unistd::getuid;
use serde::Serialize;

use crate::database;
use crate::scan::{ScanConfig, find_project_deps};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, ValueEnum, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Category {
    UserCaches,
    Logs,
    TempItems,
    TempFolders,
    ClaudeScratchpads,
    PackageCaches,
    ProjectDeps,
    DatabaseFiles,
    IdeCaches,
    OldIdeVersions,
    XcodeDerivedData,
    XcodeArchives,
    XcodeDeviceSupport,
    SimulatorCaches,
    SimulatorRuntimes,
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
    ".gradle/wrapper/dists",
    ".android/cache",
    ".m2/repository",
    ".composer/cache",
    "go/pkg/mod/cache",
    ".cocoapods/repos",
];

impl Category {
    pub const ALL: [Category; 18] = [
        Category::UserCaches,
        Category::Logs,
        Category::TempItems,
        Category::TempFolders,
        Category::ClaudeScratchpads,
        Category::PackageCaches,
        Category::ProjectDeps,
        Category::DatabaseFiles,
        Category::IdeCaches,
        Category::OldIdeVersions,
        Category::XcodeDerivedData,
        Category::XcodeArchives,
        Category::XcodeDeviceSupport,
        Category::SimulatorCaches,
        Category::SimulatorRuntimes,
        Category::IosBackups,
        Category::OldDownloads,
        Category::Trash,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Category::UserCaches => "App caches",
            Category::Logs => "Logs",
            Category::TempItems => "Temporary items",
            Category::TempFolders => "Temp folders",
            Category::ClaudeScratchpads => "Claude Code temp files",
            Category::PackageCaches => "Package manager caches",
            Category::ProjectDeps => "Project dependencies",
            Category::DatabaseFiles => "Database files",
            Category::IdeCaches => "IDE caches",
            Category::OldIdeVersions => "Old IDE versions",
            Category::XcodeDerivedData => "Xcode DerivedData",
            Category::XcodeArchives => "Xcode archives",
            Category::XcodeDeviceSupport => "Xcode device support",
            Category::SimulatorCaches => "Simulator caches",
            Category::SimulatorRuntimes => "Simulator runtimes",
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
            Category::TempFolders => {
                "/tmp. Files that apps and scripts leave behind. macOS shows them as System Data. \
                 Programs that still run can break when you remove their files."
            }
            Category::ClaudeScratchpads => {
                "/tmp/claude-*. Scratchpads and output of Claude Code sessions, one item per \
                 session. Remove a session only after it has ended."
            }
            Category::PackageCaches => {
                "Download caches of npm, pnpm, yarn, bun, cargo, gradle, maven, composer, go, \
                 CocoaPods, the Android SDK and ~/.cache. They download again on the next install."
            }
            Category::ProjectDeps => {
                "node_modules, Rust target, composer vendor, Python venvs, Pods, Swift .build, .next and similar \
                 folders inside your projects. Reinstall with npm install, cargo build, ... \
                 Check the age column to find projects you no longer use."
            }
            Category::DatabaseFiles => {
                "PostgreSQL, MySQL and ClickHouse data folders and SQLite files inside your \
                 projects, often from Docker volumes. Stop the database first. \
                 Removing them deletes the data in them."
            }
            Category::IdeCaches => {
                "Caches and indexes of IntelliJ IDEA, PhpStorm, Android Studio and other JetBrains \
                 IDEs. The IDE rebuilds them, but you lose its Local History."
            }
            Category::OldIdeVersions => {
                "Settings and plugins of IDE versions you updated from. JetBrains IDEs and \
                 Android Studio never remove them. The newest version of each IDE is not listed."
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
            Category::SimulatorRuntimes => {
                "iOS, watchOS, ... simulator versions that Xcode downloaded, about 8 GB each. \
                 Xcode can download them again. Deleted with xcrun simctl, never moved to the Trash."
            }
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
            | Category::DatabaseFiles
            | Category::IdeCaches
            | Category::OldIdeVersions
            | Category::TempFolders
            | Category::ClaudeScratchpads
            | Category::SimulatorRuntimes
            | Category::XcodeArchives
            | Category::IosBackups
            | Category::OldDownloads
            | Category::Trash => Safety::Review,
            _ => Safety::Safe,
        }
    }

    /// Items in the Trash and simulator runtimes cannot be moved to the Trash.
    pub fn always_permanent(self) -> bool {
        matches!(self, Category::Trash | Category::SimulatorRuntimes)
    }

    pub fn discover(self, config: &ScanConfig) -> Found {
        let home = &config.home;
        let mut found = Found::default();

        match self {
            Category::UserCaches => {
                let ide = Category::IdeCaches.discover(config).paths;
                children_except(&home.join("Library/Caches"), &ide, &mut found);
            }
            Category::Logs => children(&home.join("Library/Logs"), &mut found),
            Category::TempItems => children(&config.temp.join("TemporaryItems"), &mut found),
            Category::TempFolders => {
                children(&config.shared_temp, &mut found);
                let claude = claude_dir(config);
                found
                    .paths
                    .retain(|path| *path != claude && owned_by_me(path));
            }
            Category::ClaudeScratchpads => {
                let mut projects = Found::default();
                children(&claude_dir(config), &mut projects);
                for project in projects.paths.iter().filter(|path| path.is_dir()) {
                    children(project, &mut found);
                }
                found.issues.extend(projects.issues);
            }
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
            Category::DatabaseFiles => {
                // Hidden folders in your home hold tool data, not projects.
                let mut skip = config.project_skip_dirs();
                let mut in_home = Found::default();
                children(home, &mut in_home);
                skip.extend(in_home.paths.into_iter().filter(|path| is_hidden(path)));

                for dir in &config.project_dirs {
                    found.paths.extend(database::find(dir, &skip, 12));
                }
            }
            Category::IdeCaches => found = ide_folders(&home.join("Library/Caches")),
            Category::OldIdeVersions => {
                found = ide_folders(&home.join("Library/Application Support"));
                found.paths = old_versions(found.paths);
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
            // Listed by simctl, see scan::scan_runtimes.
            Category::SimulatorRuntimes => {}
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
        if self == Category::ClaudeScratchpads {
            return scratchpad_label(path, &config.home);
        }

        let relative = match self {
            Category::ProjectDeps | Category::PackageCaches | Category::DatabaseFiles => {
                path.strip_prefix(&config.home).ok()
            }
            _ => None,
        };

        let label = relative
            .or_else(|| path.file_name().map(Path::new))
            .unwrap_or(path)
            .display()
            .to_string();

        match self {
            Category::DatabaseFiles => match database::engine(path) {
                Some(engine) => format!("{} · {label}", engine.name()),
                None => label,
            },
            _ => label,
        }
    }
}

fn claude_dir(config: &ScanConfig) -> PathBuf {
    config.shared_temp.join(format!("claude-{}", getuid()))
}

fn owned_by_me(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|meta| meta.uid() == getuid().as_raw())
}

/// Claude Code names project folders after the project path, like "-Users-me-code-app".
fn scratchpad_label(path: &Path, home: &Path) -> String {
    let name = |path: Option<&Path>| {
        path.and_then(Path::file_name)
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default()
    };
    let session = name(Some(path));
    let project = name(path.parent());
    let home = home.display().to_string().replace('/', "-");

    // Other folders, like "bash-edit-diff", hold files with long unique names.
    let Some(rest) = project.strip_prefix(&home) else {
        return format!("{project} · {session}");
    };
    let project = match rest {
        "" => "~",
        rest => rest.trim_start_matches('-'),
    };
    let session: String = session.chars().take(8).collect();

    format!("{project} · {session}")
}

/// IDE folders like JetBrains/IntelliJIdea2025.2 and Google/AndroidStudio2025.1.
fn ide_folders(base: &Path) -> Found {
    let mut found = Found::default();
    children(&base.join("JetBrains"), &mut found);

    let mut google = Found::default();
    children(&base.join("Google"), &mut google);
    found.paths.extend(google.paths.into_iter().filter(|path| {
        path.file_name()
            .is_some_and(|name| name.to_string_lossy().starts_with("AndroidStudio"))
    }));
    found.issues.extend(google.issues);

    found
}

/// "IntelliJIdea2025.2" becomes ("IntelliJIdea", [2025, 2]).
fn ide_version(path: &Path) -> Option<(String, Vec<u32>)> {
    let name = path.file_name()?.to_str()?;
    let (product, version) = name.split_at(name.find(|c: char| c.is_ascii_digit())?);
    let version = version
        .split('.')
        .map(|part| part.parse().ok())
        .collect::<Option<Vec<u32>>>()?;

    (!product.is_empty()).then(|| (product.to_string(), version))
}

/// Every versioned IDE folder except the newest one of each product.
fn old_versions(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let versioned: Vec<_> = paths
        .into_iter()
        .filter_map(|path| ide_version(&path).map(|(product, version)| (path, product, version)))
        .collect();

    let mut newest: HashMap<&str, &Vec<u32>> = HashMap::new();
    for (_, product, version) in &versioned {
        let entry = newest.entry(product).or_insert(version);
        if version > *entry {
            *entry = version;
        }
    }

    versioned
        .iter()
        .filter(|(_, product, version)| newest[product.as_str()] != version)
        .map(|(path, ..)| path.clone())
        .collect()
}

fn is_hidden(path: &Path) -> bool {
    path.file_name()
        .is_some_and(|name| name.to_string_lossy().starts_with('.'))
}

/// Like `children`, but leaves out `skip`. Folders that hold a skipped path are listed by their children.
fn children_except(dir: &Path, skip: &[PathBuf], found: &mut Found) {
    let mut all = Found::default();
    children(dir, &mut all);
    found.issues.extend(all.issues);

    for path in all.paths {
        if skip.contains(&path) {
            continue;
        }
        if skip.iter().any(|skipped| skipped.starts_with(&path)) {
            children_except(&path, skip, found);
        } else {
            found.paths.push(path);
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn config(shared_temp: &Path) -> ScanConfig {
        ScanConfig {
            home: PathBuf::from("/Users/me"),
            temp: PathBuf::from("/tmp/me"),
            shared_temp: shared_temp.to_path_buf(),
            project_dirs: Vec::new(),
            downloads_days: 30,
        }
    }

    #[test]
    fn temp_folders_should_skip_claude_folder() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("build-cache")).unwrap();
        fs::create_dir_all(dir.path().join(format!("claude-{}", getuid()))).unwrap();

        let found = Category::TempFolders.discover(&config(dir.path()));

        assert_eq!(found.paths, [dir.path().join("build-cache")]);
    }

    #[test]
    fn claude_scratchpads_should_list_sessions() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir
            .path()
            .join(format!("claude-{}", getuid()))
            .join("-Users-me-code-app");
        fs::create_dir_all(project.join("session-a/scratchpad")).unwrap();
        fs::create_dir_all(project.join("session-b")).unwrap();

        let mut found = Category::ClaudeScratchpads
            .discover(&config(dir.path()))
            .paths;
        found.sort();

        assert_eq!(
            found,
            [project.join("session-a"), project.join("session-b")]
        );
    }

    #[test]
    fn scratchpad_label_should_strip_home_from_project() {
        let path = Path::new("/tmp/claude-501/-Users-me-code-app/4e2c1973-9d3a-4801");

        assert_eq!(
            scratchpad_label(path, Path::new("/Users/me")),
            "code-app · 4e2c1973"
        );
    }

    #[test]
    fn scratchpad_label_should_show_home_project_as_tilde() {
        let path = Path::new("/tmp/claude-501/-Users-me/4e2c1973-9d3a-4801");

        assert_eq!(
            scratchpad_label(path, Path::new("/Users/me")),
            "~ · 4e2c1973"
        );
    }

    #[test]
    fn old_versions_should_keep_newest_of_each_ide() {
        let paths = [
            "IntelliJIdea2025.1",
            "IntelliJIdea2025.10",
            "IntelliJIdea2025.2",
            "PhpStorm2024.3",
            "AndroidStudio2024.2",
            "AndroidStudioPreview2025.1",
            "Toolbox",
        ]
        .map(PathBuf::from);

        let mut old = old_versions(paths.to_vec());
        old.sort();

        assert_eq!(
            old,
            [
                PathBuf::from("IntelliJIdea2025.1"),
                PathBuf::from("IntelliJIdea2025.2")
            ]
        );
    }

    #[test]
    fn user_caches_should_leave_out_ide_caches() {
        let home = tempfile::tempdir().unwrap();
        let caches = home.path().join("Library/Caches");
        for dir in [
            "JetBrains/PhpStorm2025.2",
            "Google/AndroidStudio2025.1",
            "Google/Chrome",
            "com.apple.Safari",
        ] {
            fs::create_dir_all(caches.join(dir)).unwrap();
        }
        let config = ScanConfig {
            home: home.path().to_path_buf(),
            ..config(Path::new("/tmp"))
        };

        let mut found = Category::UserCaches.discover(&config).paths;
        found.sort();

        assert_eq!(
            found,
            [
                caches.join("Google/Chrome"),
                caches.join("com.apple.Safari")
            ]
        );
    }

    #[test]
    fn scratchpad_label_should_keep_full_name_outside_projects() {
        let path = Path::new("/tmp/claude-501/bash-edit-diff/1054-1420-05f4a7db");

        assert_eq!(
            scratchpad_label(path, Path::new("/Users/me")),
            "bash-edit-diff · 1054-1420-05f4a7db"
        );
    }
}
