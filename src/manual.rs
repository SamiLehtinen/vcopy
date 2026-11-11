//! Embedded documentation and user manual.

pub const MANUAL_ASCII: &str = include_str!("../docs/manual.txt");
pub const MANUAL_MARKDOWN: &str = include_str!("../docs/MANUAL.md");

/// Prints the comprehensive ASCII user manual / man page to standard output.
pub fn print_manual_ascii() {
    print!("{}", MANUAL_ASCII);
}

/// Prints the complete Markdown user manual to standard output.
pub fn print_manual_markdown() {
    print!("{}", MANUAL_MARKDOWN);
}
