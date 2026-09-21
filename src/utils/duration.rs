use std::time::Duration;

/// Formats a duration the same way Go's `time.Duration.String()` does,
/// e.g. `0s`, `1.5s`, `2m0s`, `1h2m3.456s`, `250ms`, `12µs`.
pub fn format_go_duration(d: Duration) -> String {
    let ns = d.as_nanos();
    if ns == 0 {
        return "0s".to_string();
    }
    if ns < 1_000 {
        return format!("{ns}ns");
    }
    if ns < 1_000_000 {
        return format!("{}µs", fmt_frac(ns, 1_000));
    }
    if ns < 1_000_000_000 {
        return format!("{}ms", fmt_frac(ns, 1_000_000));
    }

    let total_secs = ns / 1_000_000_000;
    let frac_ns = ns % 1_000_000_000;
    let hours = total_secs / 3600;
    let mins = (total_secs % 3600) / 60;
    let secs = total_secs % 60;

    let mut out = String::new();
    if hours > 0 {
        out.push_str(&format!("{hours}h"));
    }
    if hours > 0 || mins > 0 {
        out.push_str(&format!("{mins}m"));
    }
    out.push_str(&fmt_frac(secs * 1_000_000_000 + frac_ns, 1_000_000_000));
    out.push('s');
    out
}

fn fmt_frac(value: u128, unit: u128) -> String {
    let whole = value / unit;
    let frac = value % unit;
    if frac == 0 {
        return whole.to_string();
    }
    let width = unit.to_string().len() - 1;
    let mut frac_str = format!("{frac:0width$}");
    while frac_str.ends_with('0') {
        frac_str.pop();
    }
    format!("{whole}.{frac_str}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_go_formatting() {
        let cases = [
            (Duration::ZERO, "0s"),
            (Duration::from_nanos(500), "500ns"),
            (Duration::from_micros(1500), "1.5ms"),
            (Duration::from_millis(250), "250ms"),
            (Duration::from_millis(1500), "1.5s"),
            (Duration::from_secs(60), "1m0s"),
            (Duration::from_secs(3600), "1h0m0s"),
            (Duration::from_millis(3_723_456), "1h2m3.456s"),
            (Duration::from_nanos(1_234_567_891), "1.234567891s"),
        ];
        for (d, want) in cases {
            assert_eq!(format_go_duration(d), want);
        }
    }
}
