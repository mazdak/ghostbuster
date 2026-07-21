use std::cmp::Ordering;
use std::fs;
use std::io::{self, Write};
use std::path::Path;

use clap::ValueEnum;

use crate::Finding;

/// The machine- or human-readable finding representation.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, ValueEnum)]
pub enum OutputFormat {
    #[default]
    Text,
    Json,
}

/// Write findings in a stable order to an arbitrary output stream.
pub fn write_findings<W: Write>(
    mut writer: W,
    findings: &[Finding],
    format: OutputFormat,
) -> io::Result<()> {
    let mut ordered: Vec<&Finding> = findings.iter().collect();
    ordered.sort_by(|left, right| compare_findings(left, right));

    match format {
        OutputFormat::Text => write_text(&mut writer, &ordered),
        OutputFormat::Json => {
            serde_json::to_writer_pretty(&mut writer, &ordered).map_err(|error| {
                if let Some(kind) = error.io_error_kind() {
                    io::Error::new(kind, error)
                } else {
                    io::Error::other(error)
                }
            })?;
            writer.write_all(b"\n")
        }
    }
}

fn write_text(writer: &mut impl Write, findings: &[&Finding]) -> io::Result<()> {
    let current_dir = fs::canonicalize(".").ok();

    for finding in findings {
        let path = display_path(&finding.path, current_dir.as_deref());
        writeln!(
            writer,
            "{}:{}: {} {} ({}% confidence)",
            path.display(),
            finding.start.line,
            finding.code,
            finding.message,
            finding.confidence,
        )?;
    }

    Ok(())
}

fn display_path<'path>(path: &'path Path, current_dir: Option<&Path>) -> &'path Path {
    current_dir
        .and_then(|current_dir| path.strip_prefix(current_dir).ok())
        .filter(|relative| !relative.as_os_str().is_empty())
        .unwrap_or(path)
}

fn compare_findings(left: &Finding, right: &Finding) -> Ordering {
    left.path
        .to_string_lossy()
        .to_ascii_lowercase()
        .cmp(&right.path.to_string_lossy().to_ascii_lowercase())
        .then_with(|| left.path.cmp(&right.path))
        .then_with(|| left.start.line.cmp(&right.start.line))
        .then_with(|| left.start.column.cmp(&right.start.column))
        .then_with(|| left.end.line.cmp(&right.end.line))
        .then_with(|| left.end.column.cmp(&right.end.column))
        .then_with(|| left.kind.cmp(&right.kind))
        .then_with(|| left.name.cmp(&right.name))
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::path::PathBuf;

    use pretty_assertions::assert_eq;

    use super::{OutputFormat, write_findings};
    use crate::{Finding, FindingKind, Location};

    fn finding(name: &str, line: usize) -> Finding {
        let location = Location {
            line,
            column: 1,
            byte_offset: u32::try_from(line).expect("test line fits in u32"),
        };
        Finding {
            path: PathBuf::from("src/example.py"),
            start: location,
            end: location,
            kind: FindingKind::Function,
            code: FindingKind::Function.code(),
            name: name.to_owned(),
            message: format!("unused function '{name}'"),
            confidence: 60,
        }
    }

    fn finding_at(path: &str, name: &str) -> Finding {
        let mut finding = finding(name, 1);
        finding.path = PathBuf::from(path);
        finding
    }

    #[test]
    fn text_output_matches_the_vulture_shape_and_is_sorted() {
        let findings = [finding("later", 10), finding("earlier", 2)];
        let mut output = Vec::new();

        write_findings(&mut output, &findings, OutputFormat::Text).expect("write text findings");

        assert_eq!(
            String::from_utf8(output).expect("output is UTF-8"),
            concat!(
                "src/example.py:2: V103 unused function 'earlier' (60% confidence)\n",
                "src/example.py:10: V103 unused function 'later' (60% confidence)\n",
            )
        );
    }

    #[test]
    fn json_output_is_pretty_stable_and_newline_terminated() {
        let findings = [finding("later", 10), finding("earlier", 2)];
        let mut output = Vec::new();

        write_findings(&mut output, &findings, OutputFormat::Json).expect("write JSON findings");

        assert_eq!(output.last(), Some(&b'\n'));
        let value: serde_json::Value =
            serde_json::from_slice(&output).expect("parse JSON findings");
        assert_eq!(value[0]["name"], "earlier");
        assert_eq!(value[1]["name"], "later");
        assert!(
            String::from_utf8(output)
                .expect("output is UTF-8")
                .contains("\n  {")
        );
    }

    #[test]
    fn output_paths_are_sorted_like_vulture() {
        let findings = [finding_at("B.py", "upper"), finding_at("a.py", "lower")];
        let mut output = Vec::new();

        write_findings(&mut output, &findings, OutputFormat::Text).expect("write findings");

        let output = String::from_utf8(output).expect("output is UTF-8");
        assert!(output.starts_with("a.py:"), "{output}");
    }

    #[test]
    fn json_preserves_broken_pipe_errors() {
        struct BrokenWriter;

        impl io::Write for BrokenWriter {
            fn write(&mut self, _buffer: &[u8]) -> io::Result<usize> {
                Err(io::ErrorKind::BrokenPipe.into())
            }

            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }

        let error = write_findings(BrokenWriter, &[finding("dead", 1)], OutputFormat::Json)
            .expect_err("writing JSON should fail");
        assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
    }
}
