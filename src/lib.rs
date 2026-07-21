//! Dead Python code detection powered by Ruff's parser and syntax tree.

mod analyze;
mod files;
mod model;
mod policy;
mod reachability;
mod report;
mod whitelists;

pub use analyze::{Analysis, Analyzer, AnalyzerError};
pub use files::{Discovery, DiscoveryError, discover};
pub use model::{Finding, FindingKind, Location};
pub use policy::{Policy, PolicyError};
pub use report::{OutputFormat, write_findings};
