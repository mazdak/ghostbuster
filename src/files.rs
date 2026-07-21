use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use globset::{GlobBuilder, GlobMatcher};
use ignore::WalkBuilder;
use thiserror::Error;

/// Python source files found under a set of input paths, plus non-fatal input
/// and traversal errors.
#[derive(Debug, Default)]
pub struct Discovery {
    pub files: Vec<PathBuf>,
    pub errors: Vec<DiscoveryError>,
}

impl Discovery {
    /// Returns `true` when every input was inspected successfully.
    pub const fn is_clean(&self) -> bool {
        self.errors.is_empty()
    }
}

/// A problem encountered while resolving or walking an input path.
#[derive(Debug, Error)]
pub enum DiscoveryError {
    #[error("path does not exist: {path:?}")]
    MissingPath { path: PathBuf },

    #[error("failed to inspect path {path:?}: {source}")]
    InspectPath {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("path is neither a regular file nor a directory: {path:?}")]
    NotFileOrDirectory { path: PathBuf },

    #[error("invalid exclude pattern {pattern:?}: {source}")]
    InvalidExcludePattern {
        pattern: String,
        #[source]
        source: globset::Error,
    },

    #[error("failed while walking {root:?}: {source}")]
    Walk {
        root: PathBuf,
        #[source]
        source: ignore::Error,
    },
}

/// Resolve explicit files and recursively discover Python files in directories.
///
/// Explicit files are accepted regardless of their extension, matching
/// Vulture's behavior. Directory traversal considers only `.py` files and
/// honors hidden-file, `.ignore`, and Git ignore rules. All returned paths are
/// absolute, canonical, sorted, and unique.
pub fn discover(paths: &[PathBuf], exclude_patterns: &[String]) -> Discovery {
    let (exclude_matchers, mut errors) = compile_exclude_patterns(exclude_patterns);
    let current_dir = fs::canonicalize(".").ok();
    let mut files = BTreeSet::new();

    for input in paths {
        let Some(path) = resolve_input(input, &mut errors) else {
            continue;
        };

        let metadata = match fs::metadata(&path) {
            Ok(metadata) => metadata,
            Err(source) => {
                errors.push(DiscoveryError::InspectPath {
                    path: input.clone(),
                    source,
                });
                continue;
            }
        };

        if metadata.is_file() {
            let root = path.parent().unwrap_or_else(|| Path::new(""));
            if !is_excluded(&path, root, current_dir.as_deref(), &exclude_matchers) {
                files.insert(path);
            }
        } else if metadata.is_dir() {
            discover_directory(
                &path,
                current_dir.as_deref(),
                &exclude_matchers,
                &mut files,
                &mut errors,
            );
        } else {
            errors.push(DiscoveryError::NotFileOrDirectory {
                path: input.clone(),
            });
        }
    }

    Discovery {
        files: files.into_iter().collect(),
        errors,
    }
}

fn resolve_input(input: &Path, errors: &mut Vec<DiscoveryError>) -> Option<PathBuf> {
    match fs::canonicalize(input) {
        Ok(path) => Some(path),
        Err(source) if source.kind() == io::ErrorKind::NotFound => {
            errors.push(DiscoveryError::MissingPath {
                path: input.to_path_buf(),
            });
            None
        }
        Err(source) => {
            errors.push(DiscoveryError::InspectPath {
                path: input.to_path_buf(),
                source,
            });
            None
        }
    }
}

fn discover_directory(
    root: &Path,
    current_dir: Option<&Path>,
    exclude_matchers: &[GlobMatcher],
    files: &mut BTreeSet<PathBuf>,
    errors: &mut Vec<DiscoveryError>,
) {
    let mut builder = WalkBuilder::new(root);
    builder
        .standard_filters(true)
        .require_git(false)
        .follow_links(false)
        .sort_by_file_path(Path::cmp);

    for result in builder.build() {
        match result {
            Ok(entry) => {
                if entry
                    .file_type()
                    .is_none_or(|file_type| !file_type.is_file())
                    || !is_python_file(entry.path())
                    || is_excluded(entry.path(), root, current_dir, exclude_matchers)
                {
                    continue;
                }
                files.insert(entry.into_path());
            }
            Err(source) => errors.push(DiscoveryError::Walk {
                root: root.to_path_buf(),
                source,
            }),
        }
    }
}

fn compile_exclude_patterns(patterns: &[String]) -> (Vec<GlobMatcher>, Vec<DiscoveryError>) {
    let mut matchers = Vec::with_capacity(patterns.len());
    let mut errors = Vec::new();

    for pattern in patterns {
        let normalized = normalize_separators(pattern);
        let prepared = if normalized.contains(['*', '?', '[']) {
            normalized
        } else {
            format!("*{normalized}*")
        };

        match GlobBuilder::new(&prepared)
            .case_insensitive(cfg!(windows))
            .literal_separator(false)
            .build()
        {
            Ok(glob) => matchers.push(glob.compile_matcher()),
            Err(source) => errors.push(DiscoveryError::InvalidExcludePattern {
                pattern: pattern.clone(),
                source,
            }),
        }
    }

    (matchers, errors)
}

fn is_python_file(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            if cfg!(windows) {
                extension.eq_ignore_ascii_case("py")
            } else {
                extension == "py"
            }
        })
}

fn is_excluded(
    path: &Path,
    root: &Path,
    current_dir: Option<&Path>,
    matchers: &[GlobMatcher],
) -> bool {
    if matchers.is_empty() {
        return false;
    }

    let full_path = normalized_path(path);
    if matches_any(&full_path, matchers) {
        return true;
    }

    if path
        .strip_prefix(root)
        .is_ok_and(|relative| matches_any(&normalized_path(relative), matchers))
    {
        return true;
    }

    current_dir.is_some_and(|current_dir| {
        path.strip_prefix(current_dir)
            .is_ok_and(|relative| matches_any(&normalized_path(relative), matchers))
    })
}

fn matches_any(path: &str, matchers: &[GlobMatcher]) -> bool {
    matchers.iter().any(|matcher| matcher.is_match(path))
}

fn normalized_path(path: &Path) -> String {
    normalize_separators(&path.to_string_lossy())
}

fn normalize_separators(value: &str) -> String {
    value.replace(std::path::MAIN_SEPARATOR, "/")
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use pretty_assertions::assert_eq;
    use tempfile::TempDir;

    use super::{DiscoveryError, discover};

    fn write_file(root: &Path, relative: &str) -> PathBuf {
        let path = root.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create test directory");
        }
        fs::write(&path, "pass\n").expect("write test file");
        path
    }

    #[test]
    fn discovers_sorted_unique_python_files_with_standard_ignores() {
        let temp = TempDir::new().expect("create temporary directory");
        let root = temp.path();
        let keep = write_file(root, "src/keep.py");
        let explicit_without_extension = write_file(root, "script");
        write_file(root, "src/notes.txt");
        write_file(root, ".hidden.py");
        write_file(root, "gitignored.py");
        write_file(root, "excluded/generated.py");
        fs::write(root.join(".gitignore"), "gitignored.py\n").expect("write gitignore file");

        let discovery = discover(
            &[
                root.to_path_buf(),
                explicit_without_extension.clone(),
                root.to_path_buf(),
            ],
            &["excluded/**".to_owned()],
        );

        assert!(discovery.is_clean());
        assert_eq!(
            discovery.files,
            vec![
                fs::canonicalize(explicit_without_extension).expect("resolve explicit file"),
                fs::canonicalize(keep).expect("resolve Python file"),
            ]
        );
    }

    #[test]
    fn explicit_hidden_and_ignored_files_are_authoritative() {
        let temp = TempDir::new().expect("create temporary directory");
        let hidden = write_file(temp.path(), ".hidden.py");
        fs::write(temp.path().join(".gitignore"), ".hidden.py\n").expect("write gitignore file");

        let discovery = discover(std::slice::from_ref(&hidden), &[]);

        assert!(discovery.is_clean());
        assert_eq!(
            discovery.files,
            vec![fs::canonicalize(hidden).expect("resolve hidden file")]
        );
    }

    #[test]
    fn reports_missing_inputs_and_bad_patterns_without_aborting() {
        let temp = TempDir::new().expect("create temporary directory");
        let keep = write_file(temp.path(), "keep.py");
        let missing = temp.path().join("missing.py");

        let discovery = discover(&[keep.clone(), missing.clone()], &["[".to_owned()]);

        assert_eq!(
            discovery.files,
            vec![fs::canonicalize(keep).expect("resolve Python file")]
        );
        assert_eq!(discovery.errors.len(), 2);
        assert!(matches!(
            &discovery.errors[0],
            DiscoveryError::InvalidExcludePattern { pattern, .. } if pattern == "["
        ));
        assert!(matches!(
            &discovery.errors[1],
            DiscoveryError::MissingPath { path } if path == &missing
        ));
    }

    #[test]
    fn exclude_case_sensitivity_matches_the_platform() {
        let temp = TempDir::new().expect("create temporary directory");
        let source = write_file(temp.path(), "CaseSensitive.py");

        let discovery = discover(
            std::slice::from_ref(&source),
            &["casesensitive.py".to_owned()],
        );

        assert!(discovery.errors.is_empty());
        if cfg!(windows) {
            assert!(discovery.files.is_empty());
        } else {
            assert_eq!(
                discovery.files,
                vec![fs::canonicalize(source).expect("resolve Python file")]
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn reports_inputs_that_are_not_files_or_directories() {
        use std::os::unix::net::UnixListener;

        let temp = TempDir::new().expect("create temporary directory");
        let socket = temp.path().join("ghostbuster.sock");
        let _listener = UnixListener::bind(&socket).expect("create Unix socket");

        let discovery = discover(std::slice::from_ref(&socket), &[]);

        assert!(discovery.files.is_empty());
        assert!(matches!(
            discovery.errors.as_slice(),
            [DiscoveryError::NotFileOrDirectory { path }] if path == &socket
        ));
    }
}
