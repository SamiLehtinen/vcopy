//! Embedded documentation and user manual.
//! Single source of truth: `docs/MANUAL.md`. Plain-text ASCII output is derived
//! automatically by stripping Markdown formatting.

pub const MANUAL_MARKDOWN: &str = include_str!("../docs/MANUAL.md");

/// Converts the embedded Markdown manual into clean plain-text ASCII by stripping
/// Markdown headings, code fences, bold/italic markers, and inline code backticks.
pub fn render_manual_ascii() -> String {
    let mut out = String::with_capacity(MANUAL_MARKDOWN.len());
    let mut in_code_block = false;

    for raw_line in MANUAL_MARKDOWN.lines() {
        let trimmed = raw_line.trim_start();
        if trimmed.starts_with("```") {
            in_code_block = !in_code_block;
            continue;
        }

        if in_code_block {
            out.push_str("    ");
            out.push_str(&to_pure_ascii(raw_line));
            out.push('\n');
            continue;
        }

        if let Some(h1) = raw_line.strip_prefix("# ") {
            let clean = strip_inline_markdown(h1);
            out.push_str(&clean);
            out.push('\n');
            out.push_str(&"=".repeat(clean.len()));
            out.push('\n');
        } else if let Some(h2) = raw_line.strip_prefix("## ") {
            let clean = strip_inline_markdown(h2);
            out.push_str(&clean);
            out.push('\n');
            out.push_str(&"-".repeat(clean.len()));
            out.push('\n');
        } else if let Some(h3) = raw_line.strip_prefix("### ") {
            let clean = strip_inline_markdown(h3);
            out.push_str(&clean);
            out.push('\n');
        } else {
            out.push_str(&strip_inline_markdown(raw_line));
            out.push('\n');
        }
    }

    out
}

fn to_pure_ascii(line: &str) -> String {
    line.replace("├──", "|--")
        .replace("└──", "`--")
        .replace('│', "|")
}

fn strip_inline_markdown(line: &str) -> String {
    to_pure_ascii(line)
        .replace("**", "")
        .replace("*(", "(")
        .replace(")*", ")")
        .replace("$N$", "N")
        .replace('`', "")
        .replace("<https://", "https://")
        .replace("vcopy>", "vcopy")
}

/// Prints the comprehensive ASCII user manual / man page to standard output.
pub fn print_manual_ascii() {
    print!("{}", render_manual_ascii());
}

/// Prints the complete Markdown user manual to standard output.
pub fn print_manual_markdown() {
    print!("{}", MANUAL_MARKDOWN);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_render_manual_ascii_strips_markdown_and_is_pure_ascii() {
        let ascii = render_manual_ascii();
        assert!(
            ascii.is_ascii(),
            "Rendered ASCII manual must be 100% 7-bit ASCII"
        );
        assert!(ascii.contains("VCOPY(1) User Manual\n===================="));
        assert!(ascii.contains("QUICK START & SIMPLE EXAMPLES\n-----------------------------"));
        assert!(!ascii.contains("```"), "Must not contain code fences");
        assert!(!ascii.contains("**"), "Must not contain bold markers");
        assert!(
            !ascii.contains("*(") && !ascii.contains(")*"),
            "Must not contain italic markers"
        );
        assert!(
            !ascii.contains("$N$"),
            "Must not contain LaTeX math markers"
        );
    }
}
