use std::fmt::Display;
use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;
use ghostbuster::{Analyzer, DiscoveryError, OutputFormat, Policy, discover, write_findings};

const EXIT_INVALID_INPUT: u8 = 1;
const EXIT_INVALID_ARGUMENTS: u8 = 2;
const EXIT_DEAD_CODE: u8 = 3;

#[derive(Debug, Parser)]
#[command(version, about)]
struct Cli {
    /// Python files or directories to inspect.
    #[arg(required = true)]
    paths: Vec<PathBuf>,

    /// Comma-separated glob patterns for paths to skip.
    #[arg(long, value_delimiter = ',')]
    exclude: Vec<String>,

    /// Comma-separated glob patterns for definition names to ignore.
    #[arg(long, value_delimiter = ',')]
    ignore_names: Vec<String>,

    /// Comma-separated glob patterns for decorators that keep definitions alive.
    #[arg(long, value_delimiter = ',')]
    ignore_decorators: Vec<String>,

    /// Only report findings at or above this confidence percentage.
    #[arg(long, default_value_t = 0, value_parser = parse_confidence)]
    min_confidence: u8,

    /// Select human-readable text or machine-readable JSON.
    #[arg(long, value_enum, default_value_t)]
    format: OutputFormat,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let policy = match Policy::new(
        cli.min_confidence,
        &cli.ignore_names,
        &cli.ignore_decorators,
    ) {
        Ok(policy) => policy,
        Err(error) => {
            write_error(&error);
            return ExitCode::from(EXIT_INVALID_ARGUMENTS);
        }
    };

    let discovery = discover(&cli.paths, &cli.exclude);
    for error in &discovery.errors {
        write_error(error);
    }
    if discovery
        .errors
        .iter()
        .any(|error| matches!(error, DiscoveryError::InvalidExcludePattern { .. }))
    {
        return ExitCode::from(EXIT_INVALID_ARGUMENTS);
    }

    let analysis = Analyzer::new(policy).analyze_files(&discovery.files);
    for error in &analysis.errors {
        write_error(error);
    }

    let stdout = io::stdout();
    if let Err(error) = write_findings(stdout.lock(), &analysis.findings, cli.format) {
        if error.kind() == io::ErrorKind::BrokenPipe {
            return ExitCode::SUCCESS;
        }
        write_error(&error);
        return ExitCode::from(EXIT_INVALID_INPUT);
    }

    if !discovery.errors.is_empty() || !analysis.errors.is_empty() {
        ExitCode::from(EXIT_INVALID_INPUT)
    } else if analysis.findings.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(EXIT_DEAD_CODE)
    }
}

fn parse_confidence(value: &str) -> Result<u8, String> {
    let confidence = value
        .parse::<u8>()
        .map_err(|_| "confidence must be an integer between 0 and 100".to_owned())?;
    if confidence <= 100 {
        Ok(confidence)
    } else {
        Err("confidence must be between 0 and 100".to_owned())
    }
}

fn write_error(error: &impl Display) {
    let stderr = io::stderr();
    let _ignored = writeln!(stderr.lock(), "ghostbuster: {error}");
}

#[cfg(test)]
mod tests {
    use super::parse_confidence;

    #[test]
    fn validates_confidence() {
        assert_eq!(parse_confidence("0"), Ok(0));
        assert_eq!(parse_confidence("100"), Ok(100));
        assert!(parse_confidence("101").is_err());
        assert!(parse_confidence("high").is_err());
    }
}
