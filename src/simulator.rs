use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result, ensure};
use serde::Deserialize;

#[derive(Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Runtime {
    pub identifier: String,
    pub runtime_identifier: String,
    pub version: String,
    pub build: String,
    pub path: PathBuf,
    pub deletable: bool,
    pub last_used_at: Option<String>,
}

impl Runtime {
    /// "com.apple.CoreSimulator.SimRuntime.iOS-26-5" becomes "iOS 26.5 (23F77)".
    pub fn name(&self) -> String {
        let platform = self
            .runtime_identifier
            .rsplit('.')
            .next()
            .and_then(|id| id.split('-').next())
            .unwrap_or("Simulator");
        format!("{platform} {} ({})", self.version, self.build)
    }

    pub fn last_used(&self) -> Option<SystemTime> {
        self.last_used_at.as_deref().and_then(parse_utc)
    }
}

/// Runtimes `simctl` can delete. Empty when Xcode is not installed.
pub fn runtimes() -> Result<Vec<Runtime>> {
    let Ok(output) = Command::new("xcrun")
        .args(["simctl", "runtime", "list", "-j"])
        .output()
    else {
        return Ok(Vec::new());
    };
    if !output.status.success() {
        return Ok(Vec::new());
    }

    let runtimes: HashMap<String, Runtime> = serde_json::from_slice(&output.stdout)
        .context("could not read the output of xcrun simctl runtime list")?;

    Ok(runtimes
        .into_values()
        .filter(|runtime| runtime.deletable && !runtime.path.as_os_str().is_empty())
        .collect())
}

pub fn delete(path: &Path) -> Result<()> {
    let runtime = runtimes()?
        .into_iter()
        .find(|runtime| runtime.path == path)
        .with_context(|| format!("no simulator runtime found at {}", path.display()))?;

    let output = Command::new("xcrun")
        .args(["simctl", "runtime", "delete", &runtime.identifier])
        .output()
        .context("could not run xcrun")?;
    ensure!(
        output.status.success(),
        "could not delete {}: {}",
        runtime.name(),
        String::from_utf8_lossy(&output.stderr).trim()
    );

    Ok(())
}

/// Parses "2026-08-30T09:26:26Z".
fn parse_utc(text: &str) -> Option<SystemTime> {
    let (date, time) = text.strip_suffix('Z')?.split_once('T')?;
    let time = time.split('.').next()?;

    let date: Vec<i64> = date
        .split('-')
        .map(str::parse)
        .collect::<Result<_, _>>()
        .ok()?;
    let time: Vec<i64> = time
        .split(':')
        .map(str::parse)
        .collect::<Result<_, _>>()
        .ok()?;
    let (&[year, month, day], &[hour, minute, second]) = (date.as_slice(), time.as_slice()) else {
        return None;
    };

    let seconds = days_from_civil(year, month, day) * 86_400 + hour * 3600 + minute * 60 + second;
    Some(SystemTime::UNIX_EPOCH + Duration::from_secs(u64::try_from(seconds).ok()?))
}

/// Days since 1970-01-01, from Howard Hinnant's date algorithms.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let day_of_year = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_utc_should_read_simctl_dates() {
        assert_eq!(
            parse_utc("2026-08-30T09:26:26Z"),
            Some(SystemTime::UNIX_EPOCH + Duration::from_secs(1_788_081_986))
        );
    }

    #[test]
    fn parse_utc_should_ignore_fractional_seconds() {
        assert_eq!(
            parse_utc("1970-01-02T00:00:00.250Z"),
            Some(SystemTime::UNIX_EPOCH + Duration::from_secs(86_400))
        );
    }

    #[test]
    fn parse_utc_should_reject_other_formats() {
        assert_eq!(parse_utc("30/08/2026"), None);
    }

    #[test]
    fn name_should_show_platform_version_and_build() {
        let runtime = Runtime {
            runtime_identifier: "com.apple.CoreSimulator.SimRuntime.iOS-26-5".to_string(),
            version: "26.5".to_string(),
            build: "23F77".to_string(),
            ..Runtime::default()
        };

        assert_eq!(runtime.name(), "iOS 26.5 (23F77)");
    }
}
