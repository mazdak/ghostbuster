use std::path::Path;

use globset::{GlobBuilder, GlobMatcher};
use thiserror::Error;

use crate::FindingKind;

const IGNORED_VARIABLE_NAMES: [&str; 2] = ["object", "self"];

/// User-tunable rules for deciding which candidates should be reported.
#[derive(Clone, Debug, Default)]
pub struct Policy {
    minimum_confidence: u8,
    ignored_names: Vec<GlobMatcher>,
    ignored_decorators: Vec<GlobMatcher>,
}

impl Policy {
    pub fn new(
        minimum_confidence: u8,
        ignored_names: &[String],
        ignored_decorators: &[String],
    ) -> Result<Self, PolicyError> {
        if minimum_confidence > 100 {
            return Err(PolicyError::InvalidConfidence(minimum_confidence));
        }
        Ok(Self {
            minimum_confidence,
            ignored_names: compile_patterns(ignored_names)?,
            ignored_decorators: compile_patterns(ignored_decorators)?,
        })
    }

    pub const fn minimum_confidence(&self) -> u8 {
        self.minimum_confidence
    }

    pub(crate) fn ignores_name(&self, name: &str) -> bool {
        self.ignored_names
            .iter()
            .any(|matcher| matcher.is_match(name))
    }

    pub(crate) fn ignores_decorator(&self, name: &str) -> bool {
        self.ignored_decorators
            .iter()
            .any(|matcher| matcher.is_match(name))
    }

    pub(crate) fn ignores_definition(&self, path: &Path, kind: FindingKind, name: &str) -> bool {
        self.ignores_name(name)
            || match kind {
                FindingKind::Import => path.file_name().is_some_and(|file| file == "__init__.py"),
                FindingKind::Class => is_test_file(path) && name.contains("Test"),
                FindingKind::Function => {
                    is_test_file(path)
                        && (name.starts_with("test_")
                            || matches!(
                                name,
                                "setup_module"
                                    | "teardown_module"
                                    | "setup_function"
                                    | "teardown_function"
                            ))
                }
                FindingKind::Method => {
                    is_special_name(name)
                        || (is_test_file(path)
                            && (name.starts_with("test_")
                                || matches!(
                                    name,
                                    "setup_class"
                                        | "teardown_class"
                                        | "setup_method"
                                        | "teardown_method"
                                )))
                }
                FindingKind::Variable => ignore_variable(name),
                FindingKind::Attribute | FindingKind::Property | FindingKind::UnreachableCode => {
                    false
                }
            }
    }
}

#[derive(Debug, Error)]
pub enum PolicyError {
    #[error("confidence must be between 0 and 100, got {0}")]
    InvalidConfidence(u8),
    #[error("invalid glob pattern {pattern:?}: {source}")]
    InvalidPattern {
        pattern: String,
        source: globset::Error,
    },
}

fn compile_patterns(patterns: &[String]) -> Result<Vec<GlobMatcher>, PolicyError> {
    patterns
        .iter()
        .map(|pattern| {
            GlobBuilder::new(pattern)
                .literal_separator(false)
                .build()
                .map(|glob| glob.compile_matcher())
                .map_err(|source| PolicyError::InvalidPattern {
                    pattern: pattern.clone(),
                    source,
                })
        })
        .collect()
}

fn ignore_variable(name: &str) -> bool {
    IGNORED_VARIABLE_NAMES.contains(&name)
        || (name.starts_with('_') && !name.starts_with("__"))
        || is_special_name(name)
}

fn is_special_name(name: &str) -> bool {
    name.starts_with("__") && name.ends_with("__")
}

fn is_test_file(path: &Path) -> bool {
    let normalized = path
        .to_string_lossy()
        .replace('\\', "/")
        .to_ascii_lowercase();
    let filename = normalized.rsplit('/').next().unwrap_or_default();

    normalized
        .split('/')
        .any(|part| part == "test" || part == "tests")
        || (filename.starts_with("test")
            && Path::new(&filename)
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("py")))
        || filename.ends_with("_test.py")
        || filename.ends_with("-test.py")
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{Policy, ignore_variable, is_test_file};
    use crate::FindingKind;

    #[test]
    fn variable_conventions_match_vulture() {
        for ignored in ["_", "_private", "__magic__", "self", "object"] {
            assert!(ignore_variable(ignored), "{ignored}");
        }
        for reported in ["public", "__private", "trailing_"] {
            assert!(!ignore_variable(reported), "{reported}");
        }
    }

    #[test]
    fn recognizes_common_test_paths() {
        assert!(is_test_file(Path::new("tests/helpers.py")));
        assert!(is_test_file(Path::new("src/test_widget.py")));
        assert!(is_test_file(Path::new("src/widget_test.py")));
        assert!(is_test_file(Path::new(r"C:\repo\tests\helpers.py")));
        assert!(is_test_file(Path::new(r"C:\repo\src\test_widget.py")));
        assert!(!is_test_file(Path::new("src/widget.py")));
    }

    #[test]
    fn ignores_test_entry_points() {
        let policy = Policy::default();
        assert!(policy.ignores_definition(
            Path::new("tests/test_widget.py"),
            FindingKind::Function,
            "test_widget"
        ));
        assert!(!policy.ignores_definition(
            Path::new("src/widget.py"),
            FindingKind::Function,
            "test_widget"
        ));
    }

    #[test]
    fn rejects_out_of_range_confidence() {
        assert!(Policy::new(101, &[], &[]).is_err());
    }
}
