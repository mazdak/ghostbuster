use std::{collections::BTreeSet, path::PathBuf};

use ghostbuster::{Analyzer, FindingKind, Policy};
use pretty_assertions::assert_eq;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct Corpus {
    schema_version: u8,
    upstream: Upstream,
    cases: Vec<Case>,
}

#[derive(Debug, Deserialize)]
struct Upstream {
    repository: String,
    revision: String,
    describe: String,
}

#[derive(Debug, Deserialize)]
struct Case {
    id: String,
    files: Vec<SourceFile>,
    parse_valid: bool,
    #[serde(default)]
    min_confidence: u8,
    #[serde(default)]
    ignore_names: Vec<String>,
    #[serde(default)]
    ignore_decorators: Vec<String>,
    code_projection: Option<Vec<String>>,
    expected: Vec<ExpectedFinding>,
}

#[derive(Debug, Deserialize)]
struct SourceFile {
    path: PathBuf,
    source: String,
}

#[derive(Debug, Deserialize, Eq, PartialEq)]
struct ExpectedFinding {
    path: PathBuf,
    start_line: usize,
    end_line: usize,
    kind: FindingKind,
    code: String,
    name: String,
    message: String,
    confidence: u8,
}

#[test]
fn matches_the_vulture_compatibility_corpus() {
    let corpus: Corpus = serde_json::from_str(include_str!("vulture/cases.json"))
        .expect("Vulture corpus is valid JSON");
    assert_eq!(corpus.schema_version, 1);
    assert_eq!(
        corpus.upstream.repository,
        "https://github.com/jendrikseipp/vulture"
    );
    assert_eq!(
        corpus.upstream.revision,
        "2c21cb0ae2afa657e36f6a397cb573608a65d79e"
    );
    assert_eq!(corpus.upstream.describe, "v2.16-3-g2c21cb0");

    let mut full_result_case_count = 0;
    let mut full_result_codes = BTreeSet::new();

    for case in corpus.cases {
        assert!(case.parse_valid, "corpus case {} must be valid", case.id);
        if case.code_projection.is_none() {
            full_result_case_count += 1;
            full_result_codes.extend(case.expected.iter().map(|finding| finding.code.clone()));
        }
        let policy = Policy::new(
            case.min_confidence,
            &case.ignore_names,
            &case.ignore_decorators,
        )
        .expect("corpus policy is valid");
        let analysis = Analyzer::new(policy).analyze_sources(
            case.files
                .iter()
                .map(|file| (file.path.as_path(), file.source.as_str())),
        );
        assert!(
            analysis.errors.is_empty(),
            "case {} produced parse errors: {:?}",
            case.id,
            analysis.errors
        );

        let actual: Vec<_> = analysis
            .findings
            .into_iter()
            .filter(|finding| {
                case.code_projection
                    .as_ref()
                    .is_none_or(|codes| codes.iter().any(|code| code == finding.code))
            })
            .map(|finding| ExpectedFinding {
                path: finding.path,
                start_line: finding.start.line,
                end_line: finding.end.line,
                kind: finding.kind,
                code: finding.code.to_owned(),
                name: finding.name,
                message: finding.message,
                confidence: finding.confidence,
            })
            .collect();

        assert_eq!(actual, case.expected, "Vulture case: {}", case.id);
    }

    assert!(
        full_result_case_count >= 8,
        "the corpus must retain at least eight unprojected, full-result cases"
    );
    assert_eq!(
        full_result_codes,
        [
            "V101".to_owned(),
            "V102".to_owned(),
            "V103".to_owned(),
            "V104".to_owned(),
            "V105".to_owned(),
            "V106".to_owned(),
            "V107".to_owned(),
            "V201".to_owned(),
        ]
        .into_iter()
        .collect(),
        "full-result cases must cover every supported finding code"
    );
}
