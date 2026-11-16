//! Spreadsheet-safe CSV cell formatting for metadata exports.

/// Quote a CSV cell and neutralize formula-leading text, including leading whitespace.
/// Doubling quotes preserves commas/newlines without creating extra cells.
pub fn csv_cell(s: &str) -> String {
    let safe = if s.trim_start().starts_with(['=', '+', '-', '@']) {
        format!("'{s}")
    } else {
        s.to_owned()
    };
    format!("\"{}\"", safe.replace('"', "\"\""))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cells_preserve_names_without_executable_formulas() {
        assert_eq!(csv_cell("a,\"b\"\n.txt"), "\"a,\"\"b\"\"\n.txt\"");
        assert_eq!(csv_cell("\t=HYPERLINK(1)"), "\"'\t=HYPERLINK(1)\"");
        assert_eq!(csv_cell("normal.txt"), "\"normal.txt\"");
    }
}
