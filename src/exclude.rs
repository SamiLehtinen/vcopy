use glob::Pattern;
use std::fs::File;
use std::io::{self, BufRead, BufReader};
use std::path::Path;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExcludeFilter {
    raw_patterns: Vec<String>,
    #[doc(hidden)]
    compiled: Vec<Rule>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Rule {
    raw: String,
    only_dir: bool,
    match_basename_only: bool,
    pattern: Pattern,
}

impl ExcludeFilter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn raw_patterns(&self) -> &[String] {
        &self.raw_patterns
    }

    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.compiled.is_empty()
    }

    pub fn add_pattern(&mut self, pat: &str) {
        let trimmed = pat.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            return;
        }

        let only_dir = trimmed.ends_with('/') || trimmed.ends_with('\\');
        let no_trail = trimmed.trim_end_matches(['/', '\\']);
        if no_trail.is_empty() {
            return;
        }

        let (no_lead, match_basename_only) = if let Some(stripped) = no_trail.strip_prefix('/') {
            (stripped, false)
        } else if let Some(stripped) = no_trail.strip_prefix('\\') {
            (stripped, false)
        } else if !no_trail.contains('/') && !no_trail.contains('\\') {
            (no_trail, true)
        } else {
            (no_trail, false)
        };

        // Normalize separators to '/'
        let normalized = no_lead.replace('\\', "/");

        if let Ok(pattern) = Pattern::new(&normalized) {
            self.raw_patterns.push(trimmed.to_string());
            self.compiled.push(Rule {
                raw: normalized,
                only_dir,
                match_basename_only,
                pattern,
            });
        }
    }

    pub fn add_patterns_from_file(&mut self, path: &Path) -> io::Result<()> {
        self.add_patterns_from_file_with_prefix(path, "")
    }

    pub fn add_patterns_from_file_with_prefix(
        &mut self,
        path: &Path,
        prefix: &str,
    ) -> io::Result<()> {
        let file = File::open(path)?;
        let reader = BufReader::new(file);
        for line in reader.lines() {
            let l = line?;
            let trimmed = l.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            if prefix.is_empty() {
                self.add_pattern(trimmed);
            } else {
                let anchored = trimmed.starts_with('/') || trimmed.starts_with('\\');
                let clean_p = trimmed.trim_start_matches(['/', '\\']);
                if clean_p.is_empty() {
                    continue;
                }
                self.add_pattern(&format!("{}/{}", prefix, clean_p));
                let no_trail = clean_p.trim_end_matches(['/', '\\']);
                if !anchored && !no_trail.contains('/') && !no_trail.contains('\\') {
                    self.add_pattern(&format!("{}/**/{}", prefix, clean_p));
                }
            }
        }
        Ok(())
    }

    fn matches_direct(&self, rel_path: &str, is_dir: bool) -> bool {
        let norm_path = rel_path.trim_matches('/');
        let basename = norm_path.rsplit('/').next().unwrap_or(norm_path);

        for rule in &self.compiled {
            if rule.only_dir && !is_dir {
                continue;
            }

            if rule.match_basename_only {
                if rule.pattern.matches(basename) {
                    return true;
                }
            } else if rule.pattern.matches(norm_path) {
                return true;
            }
        }
        false
    }

    pub fn is_excluded(&self, rel_path: &str, is_dir: bool) -> bool {
        if self.compiled.is_empty() {
            return false;
        }

        let norm_path = rel_path.replace('\\', "/");
        let trimmed = norm_path.trim_matches('/');
        if trimmed.is_empty() {
            return false;
        }

        // 1. Direct match on current path
        if self.matches_direct(trimmed, is_dir) {
            return true;
        }

        // 2. Check each ancestor directory
        let mut cur = trimmed;
        while let Some(idx) = cur.rfind('/') {
            cur = &cur[..idx];
            if self.matches_direct(cur, true) {
                return true;
            }
        }

        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exclude_basename_patterns() {
        let mut filter = ExcludeFilter::new();
        filter.add_pattern("*.tmp");
        filter.add_pattern(".DS_Store");
        filter.add_pattern("node_modules/");

        assert!(filter.is_excluded("file.tmp", false));
        assert!(filter.is_excluded("sub/dir/file.tmp", false));
        assert!(!filter.is_excluded("file.tmpx", false));

        assert!(filter.is_excluded(".DS_Store", false));
        assert!(filter.is_excluded("nested/.DS_Store", false));

        assert!(filter.is_excluded("node_modules", true));
        assert!(filter.is_excluded("nested/node_modules", true));
        assert!(filter.is_excluded("nested/node_modules/package.json", false));
        // file named node_modules shouldn't match node_modules/
        assert!(!filter.is_excluded("node_modules", false));
    }

    #[test]
    fn test_exclude_path_patterns() {
        let mut filter = ExcludeFilter::new();
        filter.add_pattern("build/*.o");
        filter.add_pattern("/top_only.txt");
        filter.add_pattern("docs/drafts/");

        assert!(filter.is_excluded("build/main.o", false));
        assert!(!filter.is_excluded("other/build/main.o", false));

        assert!(filter.is_excluded("top_only.txt", false));
        assert!(!filter.is_excluded("sub/top_only.txt", false));

        assert!(filter.is_excluded("docs/drafts", true));
        assert!(filter.is_excluded("docs/drafts/ch1.txt", false));
    }

    #[test]
    fn test_exclude_comments_and_empty() {
        let mut filter = ExcludeFilter::new();
        filter.add_pattern("");
        filter.add_pattern("   ");
        filter.add_pattern("# This is a comment");
        filter.add_pattern("valid.txt");

        assert_eq!(filter.raw_patterns(), &["valid.txt"]);
        assert!(filter.is_excluded("valid.txt", false));
        assert!(!filter.is_excluded("other.txt", false));
    }

    #[test]
    fn test_exclude_nested_prefix() {
        let dir = std::env::temp_dir().join(format!("vcopy_exclude_test_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let ignore_file = dir.join(".vcopyignore");
        std::fs::write(&ignore_file, "*.tmp\n/anchored.txt\ncache/\n").unwrap();

        let mut filter = ExcludeFilter::new();
        filter
            .add_patterns_from_file_with_prefix(&ignore_file, "sub")
            .unwrap();
        let _ = std::fs::remove_dir_all(&dir);

        assert!(filter.is_excluded("sub/a.tmp", false));
        assert!(filter.is_excluded("sub/deep/b.tmp", false));
        assert!(!filter.is_excluded("other/a.tmp", false));

        assert!(filter.is_excluded("sub/anchored.txt", false));
        assert!(!filter.is_excluded("sub/deep/anchored.txt", false));

        assert!(filter.is_excluded("sub/cache", true));
        assert!(filter.is_excluded("sub/deep/cache", true));
        assert!(filter.is_excluded("sub/deep/cache/item.bin", false));
    }
}
