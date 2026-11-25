//! Terminal dimensions for bounded, readable human output.

const DEFAULT_COLUMNS: usize = 80;
const MAX_COLUMNS: usize = 160;

/// Prefer the output terminal's current size; `COLUMNS` also supports piped
/// output and systems without a native size query. A bound keeps very wide
/// displays (or an accidental environment value) from allocating huge rows.
pub(super) fn columns() -> usize {
    let declared = std::env::var("COLUMNS").ok();
    resolve_columns(native_columns(), declared.as_deref())
}

#[cfg(all(
    unix,
    not(any(target_os = "horizon", target_os = "espidf", target_os = "wasi"))
))]
fn native_columns() -> Option<usize> {
    rustix::termios::tcgetwinsize(std::io::stdout())
        .ok()
        .map(|size| usize::from(size.ws_col))
}

#[cfg(not(all(
    unix,
    not(any(target_os = "horizon", target_os = "espidf", target_os = "wasi"))
)))]
fn native_columns() -> Option<usize> {
    None
}

fn resolve_columns(actual: Option<usize>, declared: Option<&str>) -> usize {
    actual
        .filter(|width| *width > 0)
        .or_else(|| {
            declared
                .and_then(|value| value.trim().parse::<usize>().ok())
                .filter(|width| *width > 0)
        })
        .unwrap_or(DEFAULT_COLUMNS)
        .min(MAX_COLUMNS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn actual_terminal_wins_over_stale_environment_without_widening_small_panes() {
        assert_eq!(resolve_columns(Some(24), Some("120")), 24);
        assert_eq!(resolve_columns(Some(8), None), 8);
        assert_eq!(resolve_columns(Some(240), Some("80")), MAX_COLUMNS);
    }

    #[test]
    fn pipes_use_valid_columns_and_invalid_or_huge_values_remain_bounded() {
        assert_eq!(resolve_columns(None, Some(" 40 ")), 40);
        assert_eq!(resolve_columns(Some(0), Some("60")), 60);
        assert_eq!(resolve_columns(None, Some("999999")), MAX_COLUMNS);
        for value in [
            None,
            Some(""),
            Some("0"),
            Some("-1"),
            Some("wide"),
            Some("999999999999999999999999999999999999999999"),
        ] {
            assert_eq!(resolve_columns(None, value), DEFAULT_COLUMNS);
        }
    }
}
