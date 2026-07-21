use std::fmt;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// A stable category and diagnostic code for a dead-code finding.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FindingKind {
    Attribute,
    Class,
    Function,
    Import,
    Method,
    Property,
    Variable,
    UnreachableCode,
}

impl FindingKind {
    pub const fn code(self) -> &'static str {
        match self {
            Self::Attribute => "V101",
            Self::Class => "V102",
            Self::Function => "V103",
            Self::Import => "V104",
            Self::Method => "V105",
            Self::Property => "V106",
            Self::Variable => "V107",
            Self::UnreachableCode => "V201",
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Attribute => "attribute",
            Self::Class => "class",
            Self::Function => "function",
            Self::Import => "import",
            Self::Method => "method",
            Self::Property => "property",
            Self::Variable => "variable",
            Self::UnreachableCode => "unreachable code",
        }
    }

    pub const fn default_confidence(self) -> u8 {
        match self {
            Self::Import => 90,
            Self::UnreachableCode => 100,
            _ => 60,
        }
    }
}

impl fmt::Display for FindingKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.label())
    }
}

/// A one-based source location and the underlying byte offset.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct Location {
    pub line: usize,
    pub column: usize,
    pub byte_offset: u32,
}

/// One definition or region that Ghostbuster believes is dead.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Finding {
    pub path: PathBuf,
    pub start: Location,
    pub end: Location,
    pub kind: FindingKind,
    pub code: &'static str,
    pub name: String,
    pub message: String,
    pub confidence: u8,
}

impl Finding {
    pub fn line_count(&self) -> usize {
        self.end.line.saturating_sub(self.start.line) + 1
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) struct SymbolKey {
    pub(crate) kind: SymbolNamespace,
    pub(crate) name: String,
}

impl SymbolKey {
    pub(crate) fn lexical(name: impl Into<String>) -> Self {
        Self {
            kind: SymbolNamespace::Lexical,
            name: name.into(),
        }
    }

    pub(crate) fn member(name: impl Into<String>) -> Self {
        Self {
            kind: SymbolNamespace::Member,
            name: name.into(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum SymbolNamespace {
    Lexical,
    Member,
}

#[derive(Clone, Debug)]
pub(crate) struct Definition {
    pub(crate) symbol: SymbolKey,
    pub(crate) finding: Finding,
}
