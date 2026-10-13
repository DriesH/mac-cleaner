use std::time::SystemTime;

/// Uses powers of 1000, like Finder.
pub fn size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];

    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1000.0 && unit < UNITS.len() - 1 {
        value /= 1000.0;
        unit += 1;
    }

    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

pub fn count(n: usize, noun: &str) -> String {
    if n == 1 {
        format!("1 {noun}")
    } else {
        format!("{n} {noun}s")
    }
}

pub fn age(time: Option<SystemTime>) -> String {
    let Some(days) = time
        .and_then(|time| SystemTime::now().duration_since(time).ok())
        .map(|elapsed| elapsed.as_secs() / 86_400)
    else {
        return "-".to_string();
    };

    match days {
        0 => "today".to_string(),
        1..=13 => format!("{days}d ago"),
        14..=59 => format!("{}w ago", days / 7),
        60..=729 => format!("{}mo ago", days / 30),
        _ => format!("{}y ago", days / 365),
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn size_should_show_bytes_without_decimals() {
        assert_eq!(size(512), "512 B");
    }

    #[test]
    fn size_should_use_powers_of_1000() {
        assert_eq!(size(1_500_000_000), "1.5 GB");
    }

    #[test]
    fn age_should_show_months() {
        let time = SystemTime::now() - Duration::from_secs(90 * 86_400);
        assert_eq!(age(Some(time)), "3mo ago");
    }

    #[test]
    fn age_should_show_dash_when_unknown() {
        assert_eq!(age(None), "-");
    }
}
