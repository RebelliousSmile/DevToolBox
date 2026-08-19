//! Shared human-readable formatting for UI views.

/// Binary-unit size (o/Kio/Mio/Gio/Tio), one decimal above bytes. Shared by
/// the applications grid and the cleanup rows so both sections of the same
/// screen format sizes identically.
pub(crate) fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["o", "Kio", "Mio", "Gio", "Tio"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_scale_through_binary_units() {
        assert_eq!(human_size(512), "512 o");
        assert_eq!(human_size(1024), "1.0 Kio");
        assert_eq!(human_size(5 * 1024 * 1024 * 1024), "5.0 Gio");
    }
}
