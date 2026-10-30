use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

use crate::category::Category;
use crate::scan::ScanConfig;

/// Find and clean caches, build folders and other junk that fills up your Mac.
#[derive(Parser)]
#[command(version)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,

    /// Folder to search for node_modules, target, vendor, databases, ... (repeatable). Default: your home folder
    #[arg(long = "projects-dir", value_name = "DIR", global = true)]
    pub project_dirs: Vec<PathBuf>,

    /// Only list downloads older than this many days
    #[arg(long, value_name = "DAYS", default_value_t = 30, global = true)]
    pub downloads_days: u64,
}

#[derive(Subcommand)]
pub enum Command {
    /// Open the interactive cleaner (default)
    Tui {
        /// Start in permanent delete mode instead of moving to the Trash
        #[arg(long)]
        permanent: bool,
    },
    /// Show how much space each category uses
    Scan {
        /// Only scan these categories
        #[arg(short, long = "category", value_name = "CATEGORY")]
        categories: Vec<Category>,
        /// List every item, not only the totals
        #[arg(short, long)]
        items: bool,
        /// Print JSON
        #[arg(long)]
        json: bool,
    },
    /// Clean everything in the given categories
    Clean {
        #[arg(required = true, value_name = "CATEGORY")]
        categories: Vec<Category>,
        /// Only clean items not changed for this many days
        #[arg(long, value_name = "DAYS")]
        older_than: Option<u64>,
        /// Delete for good instead of moving to the Trash
        #[arg(long)]
        permanent: bool,
        /// Show what would be cleaned, change nothing
        #[arg(long)]
        dry_run: bool,
        /// Do not ask for confirmation
        #[arg(short, long)]
        yes: bool,
    },
    /// List all categories
    Categories,
}

impl Cli {
    pub fn scan_config(&self) -> Result<ScanConfig> {
        let home = std::env::home_dir().context("could not find your home folder")?;
        let project_dirs = if self.project_dirs.is_empty() {
            vec![home.clone()]
        } else {
            self.project_dirs.clone()
        };

        Ok(ScanConfig {
            home,
            temp: std::env::temp_dir(),
            shared_temp: PathBuf::from("/private/tmp"),
            project_dirs,
            downloads_days: self.downloads_days,
        })
    }
}
