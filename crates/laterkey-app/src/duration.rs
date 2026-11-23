//! `--for` 的时长写法：`30s`、`5m`、`1h`、`90`（秒）。

use std::time::Duration;

pub fn parse(text: &str) -> Option<Duration> {
    let text = text.trim();
    let split = text
        .find(|c: char| c.is_ascii_alphabetic())
        .unwrap_or(text.len());
    let (number, unit) = text.split_at(split);
    let n: u64 = number.trim().parse().ok()?;
    let unit = unit.trim().to_ascii_lowercase();
    let seconds = match unit.as_str() {
        "" | "s" | "sec" | "secs" => Some(n),
        "m" | "min" | "mins" => n.checked_mul(60),
        "h" | "hr" | "hrs" => n.checked_mul(3600),
        _ => None,
    }?;
    Some(Duration::from_secs(seconds))
}

pub fn human(d: Duration) -> String {
    let secs = d.as_secs();
    if secs == 0 {
        return "不到 1 秒".into();
    }
    if secs.is_multiple_of(3600) {
        format!("{} 小时", secs / 3600)
    } else if secs.is_multiple_of(60) {
        format!("{} 分钟", secs / 60)
    } else {
        format!("{secs} 秒")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_accepts_units_and_bare_seconds() {
        assert_eq!(parse("30s"), Some(Duration::from_secs(30)));
        assert_eq!(parse("5m"), Some(Duration::from_secs(300)));
        assert_eq!(parse("1h"), Some(Duration::from_secs(3600)));
        assert_eq!(parse("90"), Some(Duration::from_secs(90)));
        assert_eq!(parse(" 2 Min "), Some(Duration::from_secs(120)));
    }

    #[test]
    fn parse_rejects_nonsense() {
        assert_eq!(parse("abc"), None);
        assert_eq!(parse("5x"), None);
        assert_eq!(parse("-5s"), None);
        assert_eq!(parse(""), None);
        assert_eq!(parse("99999999999999999999m"), None, "溢出");
    }

    #[test]
    fn human_reads_naturally() {
        assert_eq!(human(Duration::from_secs(300)), "5 分钟");
        assert_eq!(human(Duration::from_secs(3600)), "1 小时");
        assert_eq!(human(Duration::from_secs(45)), "45 秒");
        assert_eq!(human(Duration::ZERO), "不到 1 秒");
    }
}
