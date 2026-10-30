mod category;
mod clean;
mod cli;
mod database;
mod disk;
mod format;
mod scan;
mod simulator;
mod tui;

use std::io::{self, Write};
use std::sync::atomic::AtomicU64;
use std::time::{Duration, SystemTime};

use anyhow::Result;
use clap::{Parser, ValueEnum};
use rayon::prelude::*;
use serde::Serialize;

use crate::category::{Category, Safety};
use crate::clean::{Cleaner, Mode};
use crate::cli::{Cli, Command};
use crate::scan::{CategoryScan, Item, ScanConfig, scan_category};

fn main() -> Result<()> {
    let cli = Cli::parse();
    let config = cli.scan_config()?;

    match cli.command.unwrap_or(Command::Tui { permanent: false }) {
        Command::Tui { permanent } => tui::run(
            config,
            if permanent {
                Mode::Permanent
            } else {
                Mode::Trash
            },
        ),
        Command::Scan {
            categories,
            items,
            json,
        } => scan(&config, &categories, items, json),
        Command::Clean {
            categories,
            older_than,
            permanent,
            dry_run,
            yes,
        } => {
            let mode = if permanent {
                Mode::Permanent
            } else {
                Mode::Trash
            };
            clean(&config, &categories, older_than, mode, dry_run, yes)
        }
        Command::Categories => {
            for category in Category::ALL {
                let name = category
                    .to_possible_value()
                    .map(|v| v.get_name().to_string());
                println!("{:<22} {}", name.unwrap_or_default(), category.name());
            }
            Ok(())
        }
    }
}

fn scan_many(config: &ScanConfig, categories: &[Category]) -> Vec<CategoryScan> {
    let categories = if categories.is_empty() {
        &Category::ALL[..]
    } else {
        categories
    };

    let scans: Vec<CategoryScan> = categories
        .par_iter()
        .map(|category| scan_category(*category, config))
        .collect();

    for issue in scans.iter().flat_map(|scan| &scan.issues) {
        eprintln!("warning: {issue}");
    }

    scans
}

#[derive(Serialize)]
struct JsonItem<'a> {
    category: Category,
    path: &'a std::path::Path,
    size: u64,
    modified: Option<u64>,
}

fn scan(config: &ScanConfig, categories: &[Category], show_items: bool, json: bool) -> Result<()> {
    let scans = scan_many(config, categories);

    if json {
        let items: Vec<JsonItem> = scans
            .iter()
            .flat_map(|scan| &scan.items)
            .map(|item| JsonItem {
                category: item.category,
                path: &item.path,
                size: item.size,
                modified: item
                    .modified
                    .and_then(|time| time.duration_since(SystemTime::UNIX_EPOCH).ok())
                    .map(|elapsed| elapsed.as_secs()),
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&items)?);
        return Ok(());
    }

    for scan in &scans {
        let safety = match scan.category.safety() {
            Safety::Safe => "",
            Safety::Review => "  (review before deleting)",
        };
        println!(
            "{:<24} {:>10} {:>10}{safety}",
            scan.category.name(),
            format::size(scan.total()),
            format::count(scan.items.len(), "item")
        );

        if show_items {
            for item in &scan.items {
                println!(
                    "    {:>10}  {:<9} {}",
                    format::size(item.size),
                    format::age(item.modified),
                    item.label
                );
            }
        }
    }

    let total: u64 = scans.iter().map(CategoryScan::total).sum();
    println!("{:<24} {:>10}", "Total", format::size(total));

    Ok(())
}

fn clean(
    config: &ScanConfig,
    categories: &[Category],
    older_than: Option<u64>,
    mode: Mode,
    dry_run: bool,
    yes: bool,
) -> Result<()> {
    let cutoff = older_than.map(|days| SystemTime::now() - Duration::from_secs(days * 86_400));
    let items: Vec<Item> = scan_many(config, categories)
        .into_iter()
        .flat_map(|scan| scan.items)
        .filter(|item| match (cutoff, item.modified) {
            (Some(cutoff), Some(modified)) => modified < cutoff,
            (Some(_), None) => false,
            (None, _) => true,
        })
        .collect();

    let total: u64 = items.iter().map(|item| item.size).sum();
    if items.is_empty() {
        println!("Nothing to clean.");
        return Ok(());
    }

    for item in &items {
        println!("{:>10}  {}", format::size(item.size), item.path.display());
    }

    let action = match mode {
        Mode::Trash => "Move to the Trash",
        Mode::Permanent => "Delete for good",
    };
    println!(
        "\n{action}: {}, {}",
        format::count(items.len(), "item"),
        format::size(total)
    );
    let permanent = items
        .iter()
        .filter(|item| item.category.always_permanent())
        .count();
    if mode == Mode::Trash && permanent > 0 {
        println!(
            "{} cannot go to the Trash and are deleted for good.",
            format::count(permanent, "item")
        );
    }

    if dry_run {
        println!("Dry run, nothing changed.");
        return Ok(());
    }
    if !yes && !confirm("Continue? [y/N] ")? {
        println!("Cancelled.");
        return Ok(());
    }

    let cleaner = Cleaner::new(config);
    let mut freed = 0;
    for item in &items {
        match cleaner.remove(item, mode, &AtomicU64::new(0)) {
            Ok(()) => freed += item.size,
            Err(error) => eprintln!("error: {error:#}"),
        }
    }

    println!("Cleaned {}.", format::size(freed));
    if mode == Mode::Trash {
        println!("Empty the Trash to get the space back.");
    }

    Ok(())
}

fn confirm(question: &str) -> Result<bool> {
    print!("{question}");
    io::stdout().flush()?;

    let mut answer = String::new();
    io::stdin().read_line(&mut answer)?;

    Ok(matches!(answer.trim(), "y" | "Y" | "yes"))
}
