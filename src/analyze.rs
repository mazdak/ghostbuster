use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use encoding_rs::Encoding;
use ruff_python_ast::token::{TokenKind, Tokens};
use ruff_python_ast::visitor::{self, Visitor};
use ruff_python_ast::{
    AnyParameterRef, Expr, ExprContext, Operator, Parameter, Pattern, PySourceType, PythonVersion,
    Stmt,
};
use ruff_python_parser::{Mode, ParseOptions, parse_unchecked};
use ruff_source_file::LineIndex;
use ruff_text_size::{Ranged, TextRange, TextSize};
use thiserror::Error;

use crate::model::{Definition, Finding, FindingKind, Location, SymbolKey};
use crate::policy::Policy;
use crate::reachability;
use crate::whitelists;

/// The result of analyzing one or more Python modules as a single project.
#[derive(Debug, Default)]
pub struct Analysis {
    pub findings: Vec<Finding>,
    pub errors: Vec<AnalyzerError>,
}

impl Analysis {
    pub const fn is_clean(&self) -> bool {
        self.findings.is_empty() && self.errors.is_empty()
    }
}

/// A reusable, immutable project analyzer.
#[derive(Clone, Debug, Default)]
pub struct Analyzer {
    policy: Policy,
}

impl Analyzer {
    pub const fn new(policy: Policy) -> Self {
        Self { policy }
    }

    /// Analyze in-memory modules together, allowing uses in one file to keep definitions in
    /// another file alive.
    pub fn analyze_sources<I, P, S>(&self, sources: I) -> Analysis
    where
        I: IntoIterator<Item = (P, S)>,
        P: Into<PathBuf>,
        S: AsRef<str>,
    {
        let mut project = Project::default();

        for (path, source) in sources {
            self.collect_source(path.into(), source.as_ref(), &mut project);
        }

        project.finish(&self.policy)
    }

    /// Read and analyze Python modules together. A read or parse error in one module does not
    /// prevent valid modules from being checked.
    pub fn analyze_files(&self, paths: &[PathBuf]) -> Analysis {
        let mut project = Project::default();

        for path in paths {
            match read_python_source(path) {
                Ok(source) => self.collect_source(path.clone(), &source, &mut project),
                Err(source) => project.errors.push(AnalyzerError::Read {
                    path: path.clone(),
                    source,
                }),
            }
        }

        project.finish(&self.policy)
    }

    fn collect_source(&self, path: PathBuf, source: &str, project: &mut Project) {
        let line_index = LineIndex::from_source_text(source);
        let options =
            ParseOptions::from(PySourceType::Python).with_target_version(PythonVersion::latest());
        let Some(parsed) = parse_unchecked(source, options).try_into_module() else {
            project.errors.push(AnalyzerError::ParserInvariant {
                path,
                message: "module parser returned an expression tree",
            });
            return;
        };

        if parsed.has_syntax_errors() {
            project.errors.extend(parsed.errors().iter().map(|error| {
                parse_error(
                    &path,
                    source,
                    &line_index,
                    error.location,
                    error.error.to_string(),
                )
            }));
            project
                .errors
                .extend(parsed.unsupported_syntax_errors().iter().map(|error| {
                    parse_error(&path, source, &line_index, error.range(), error.to_string())
                }));
            return;
        }

        let module = parsed.syntax();
        let type_comment_names = match type_comment_names(source, parsed.tokens(), &module.body) {
            Ok(names) => names,
            Err(error) => {
                project.errors.push(parse_error(
                    &path,
                    source,
                    &line_index,
                    error.range,
                    error.message,
                ));
                return;
            }
        };
        let mut collector = Collector::new(&path, source, &line_index, &self.policy);
        collector.visit_body(&module.body);
        for name in type_comment_names {
            match name {
                FormatName::Lexical(name) => collector.use_lexical(&name),
                FormatName::Member(name) => collector.use_member(&name),
            }
        }

        for unreachable in reachability::analyze(&module.body) {
            collector.push_unreachable(unreachable);
        }

        project.definitions.extend(collector.definitions);
        project.uses.extend(collector.uses);
        project.unreachable.extend(collector.unreachable);
    }
}

fn read_python_source(path: &Path) -> io::Result<String> {
    let bytes = fs::read(path)?;
    decode_python_source(&bytes)
}

fn decode_python_source(bytes: &[u8]) -> io::Result<String> {
    const UTF8_BOM: &[u8] = b"\xef\xbb\xbf";

    let (bytes, has_bom) = bytes
        .strip_prefix(UTF8_BOM)
        .map_or((bytes, false), |bytes| (bytes, true));
    let declared = python_encoding_cookie(bytes);
    let normalized = declared
        .as_deref()
        .unwrap_or("utf-8")
        .to_ascii_lowercase()
        .replace('_', "-");

    if has_bom && !matches!(normalized.as_str(), "utf-8" | "utf8" | "utf-8-sig") {
        return Err(invalid_encoding(format!(
            "UTF-8 BOM conflicts with declared {normalized} encoding"
        )));
    }

    if matches!(normalized.as_str(), "utf-8" | "utf8" | "utf-8-sig") {
        return String::from_utf8(bytes.to_vec())
            .map_err(|error| invalid_encoding(error.to_string()));
    }
    if matches!(
        normalized.as_str(),
        "latin-1" | "latin1" | "iso-8859-1" | "iso-latin-1" | "l1" | "cp819" | "ibm819"
    ) {
        return Ok(bytes.iter().map(|byte| char::from(*byte)).collect());
    }

    if matches!(
        normalized.as_str(),
        "ascii" | "us-ascii" | "646" | "ansi-x3.4-1968"
    ) {
        if bytes.is_ascii() {
            return String::from_utf8(bytes.to_vec())
                .map_err(|error| invalid_encoding(error.to_string()));
        }
        return Err(invalid_encoding("source is not valid ASCII".to_owned()));
    }

    if let Some(codepage) = normalized
        .strip_prefix("cp")
        .and_then(|number| number.parse::<u16>().ok())
    {
        return codepage_strings::Coding::new(codepage)
            .and_then(|coding| coding.decode(bytes))
            .map(std::borrow::Cow::into_owned)
            .map_err(|error| invalid_encoding(error.to_string()));
    }

    let Some(label) = python_encoding_rs_label(&normalized) else {
        return Err(invalid_encoding(format!(
            "unknown Python source encoding {normalized:?}"
        )));
    };
    let Some(encoding) = Encoding::for_label(label.as_bytes()) else {
        return Err(invalid_encoding(format!(
            "unsupported Python source encoding {normalized:?}"
        )));
    };
    encoding
        .decode_without_bom_handling_and_without_replacement(bytes)
        .map(std::borrow::Cow::into_owned)
        .ok_or_else(|| invalid_encoding(format!("source is not valid {normalized}")))
}

fn python_encoding_rs_label(normalized: &str) -> Option<&str> {
    match normalized {
        "big5" | "big5-hkscs" => Some("big5"),
        "euc-jp" | "eucjp" => Some("euc-jp"),
        "euc-kr" | "euckr" => Some("euc-kr"),
        "gb18030" => Some("gb18030"),
        "gb2312" | "gbk" => Some("gbk"),
        "iso2022-jp" | "iso-2022-jp" => Some("iso-2022-jp"),
        "koi8-r" | "koi8-u" => Some(normalized),
        "mac-cyrillic" => Some("x-mac-cyrillic"),
        "mac-roman" | "macintosh" => Some("macintosh"),
        "shift-jis" | "shiftjis" | "sjis" => Some("shift-jis"),
        "tis-620" => Some("windows-874"),
        label if label.starts_with("iso-8859-") || label.starts_with("windows-125") => Some(label),
        _ => None,
    }
}

fn invalid_encoding(message: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn python_encoding_cookie(bytes: &[u8]) -> Option<String> {
    let mut lines = bytes.split_inclusive(|byte| *byte == b'\n');
    let first = lines.next().unwrap_or_default();
    if let Some(encoding) = encoding_cookie_on_line(first) {
        return Some(encoding);
    }
    if !is_blank_or_comment_line(first) {
        return None;
    }
    encoding_cookie_on_line(lines.next().unwrap_or_default())
}

fn encoding_cookie_on_line(line: &[u8]) -> Option<String> {
    let start = line
        .iter()
        .position(|byte| !matches!(byte, b' ' | b'\t' | 0x0c))?;
    if line[start] != b'#' {
        return None;
    }

    let mut search_start = start + 1;
    while let Some(relative) = line[search_start..]
        .windows(b"coding".len())
        .position(|window| window == b"coding")
    {
        let after_word = search_start + relative + b"coding".len();
        let separator = line.get(after_word)?;
        if !matches!(separator, b':' | b'=') {
            search_start = after_word;
            continue;
        }

        let value_start = line[after_word + 1..]
            .iter()
            .position(|byte| !matches!(byte, b' ' | b'\t'))
            .map_or(line.len(), |offset| after_word + 1 + offset);
        let value_end = line[value_start..]
            .iter()
            .position(|byte| !byte.is_ascii_alphanumeric() && !matches!(byte, b'-' | b'_' | b'.'))
            .map_or(line.len(), |offset| value_start + offset);
        if value_start == value_end {
            return None;
        }
        return std::str::from_utf8(&line[value_start..value_end])
            .ok()
            .map(str::to_owned);
    }
    None
}

fn is_blank_or_comment_line(line: &[u8]) -> bool {
    line.iter()
        .find(|byte| !matches!(byte, b' ' | b'\t' | 0x0c))
        .is_none_or(|byte| matches!(byte, b'#' | b'\r' | b'\n'))
}

#[derive(Debug, Error)]
pub enum AnalyzerError {
    #[error("{path}:{line}:{column}: {message}")]
    Parse {
        path: PathBuf,
        line: usize,
        column: usize,
        message: String,
    },
    #[error("could not read {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("internal parser error for {path}: {message}")]
    ParserInvariant {
        path: PathBuf,
        message: &'static str,
    },
}

#[derive(Default)]
struct Project {
    definitions: Vec<Definition>,
    uses: HashSet<SymbolKey>,
    unreachable: Vec<Finding>,
    errors: Vec<AnalyzerError>,
}

impl Project {
    fn finish(self, policy: &Policy) -> Analysis {
        let mut seen = HashSet::new();
        let mut findings: Vec<_> = self
            .definitions
            .into_iter()
            .filter(|definition| !self.uses.contains(&definition.symbol))
            .map(|definition| definition.finding)
            .chain(self.unreachable)
            .filter(|finding| finding.confidence >= policy.minimum_confidence())
            .filter(|finding| {
                seen.insert((
                    finding.path.clone(),
                    finding.start.line,
                    finding.kind,
                    finding.name.clone(),
                ))
            })
            .collect();

        findings.sort_by(|left, right| {
            left.path
                .to_string_lossy()
                .to_ascii_lowercase()
                .cmp(&right.path.to_string_lossy().to_ascii_lowercase())
                .then_with(|| left.start.byte_offset.cmp(&right.start.byte_offset))
                .then_with(|| left.kind.cmp(&right.kind))
                .then_with(|| left.name.cmp(&right.name))
        });

        Analysis {
            findings,
            errors: self.errors,
        }
    }
}

struct Collector<'a> {
    path: &'a Path,
    source: &'a str,
    line_index: &'a LineIndex,
    policy: &'a Policy,
    definitions: Vec<Definition>,
    uses: HashSet<SymbolKey>,
    unreachable: Vec<Finding>,
    scope: Scope,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Scope {
    Module,
    Class,
    Function,
}

impl<'a> Collector<'a> {
    fn new(path: &'a Path, source: &'a str, line_index: &'a LineIndex, policy: &'a Policy) -> Self {
        Self {
            path,
            source,
            line_index,
            policy,
            definitions: Vec::new(),
            uses: HashSet::new(),
            unreachable: Vec::new(),
            scope: Scope::Module,
        }
    }

    fn define(&mut self, kind: FindingKind, name: &str, range: TextRange, confidence: u8) {
        if name == "*" || self.policy.ignores_definition(self.path, kind, name) {
            return;
        }

        let start = self.location(range.start());
        if suppressed(self.source, range.start(), kind.code()) {
            return;
        }

        let symbol = match kind {
            FindingKind::Attribute | FindingKind::Method | FindingKind::Property => {
                SymbolKey::member(name)
            }
            FindingKind::Class
            | FindingKind::Function
            | FindingKind::Import
            | FindingKind::Variable => SymbolKey::lexical(name),
            FindingKind::UnreachableCode => return,
        };
        let message = format!("unused {} '{name}'", kind.label());
        self.definitions.push(Definition {
            symbol,
            finding: Finding {
                path: self.path.to_path_buf(),
                start,
                end: self.location(range.end()),
                kind,
                code: kind.code(),
                name: name.to_owned(),
                message,
                confidence,
            },
        });
    }

    fn use_lexical(&mut self, name: &str) {
        if !matches!(name, "self" | "object") {
            self.uses.insert(SymbolKey::lexical(name));
        }
    }

    fn use_member(&mut self, name: &str) {
        self.uses.insert(SymbolKey::member(name));
        // `module.Thing` can refer to a top-level class, function, or import in another module.
        self.uses.insert(SymbolKey::lexical(name));
    }

    fn use_import_whitelist(&mut self, name: &str) {
        let Some(whitelist) = whitelists::for_import(name) else {
            return;
        };
        for lexical in whitelist.lexical {
            self.use_lexical(lexical);
        }
        for member in whitelist.members {
            self.use_member(member);
        }
    }

    fn location(&self, offset: TextSize) -> Location {
        let position = self.line_index.line_column(offset, self.source);
        Location {
            line: position.line.get(),
            column: position.column.get(),
            byte_offset: u32::from(offset),
        }
    }

    fn decorated_range<T: Ranged>(
        node: &T,
        decorators: &[ruff_python_ast::Decorator],
    ) -> TextRange {
        let start = decorators
            .first()
            .map_or_else(|| node.start(), Ranged::start);
        TextRange::new(start, node.end())
    }

    fn push_unreachable(&mut self, unreachable: reachability::Unreachable) {
        let kind = FindingKind::UnreachableCode;
        let start = self.location(unreachable.range.start());
        if suppressed(self.source, unreachable.range.start(), kind.code()) {
            return;
        }
        self.unreachable.push(Finding {
            path: self.path.to_path_buf(),
            start,
            end: self.location(unreachable.range.end()),
            kind,
            code: kind.code(),
            name: unreachable.name.to_owned(),
            message: unreachable.message,
            confidence: kind.default_confidence(),
        });
    }

    fn visit_function_definition(&mut self, function: &'a ruff_python_ast::StmtFunctionDef) {
        let decorator_names: Vec<_> = function
            .decorator_list
            .iter()
            .filter_map(|decorator| decorator_name(&decorator.expression))
            .collect();

        if decorator_names
            .iter()
            .any(|name| self.policy.ignores_decorator(name))
        {
            return;
        }

        let first_parameter = function
            .parameters
            .iter()
            .next()
            .map(AnyParameterRef::name)
            .map(ruff_python_ast::Identifier::as_str);
        let kind = if decorator_names.iter().any(|name| name == "@property") {
            FindingKind::Property
        } else if decorator_names
            .iter()
            .any(|name| matches!(name.as_str(), "@staticmethod" | "@classmethod"))
            || first_parameter == Some("self")
        {
            FindingKind::Method
        } else {
            FindingKind::Function
        };
        let range = Self::decorated_range(function, &function.decorator_list);
        self.define(
            kind,
            function.name.as_str(),
            range,
            kind.default_confidence(),
        );
    }

    fn visit_class_definition(&mut self, class: &'a ruff_python_ast::StmtClassDef) {
        let ignored = class.decorator_list.iter().any(|decorator| {
            decorator_name(&decorator.expression)
                .is_some_and(|name| self.policy.ignores_decorator(&name))
        });
        if !ignored {
            let range = Self::decorated_range(class, &class.decorator_list);
            self.define(
                FindingKind::Class,
                class.name.as_str(),
                range,
                FindingKind::Class.default_confidence(),
            );
        }
    }

    fn walk_function_definition(&mut self, function: &'a ruff_python_ast::StmtFunctionDef) {
        for decorator in &function.decorator_list {
            self.visit_decorator(decorator);
        }
        if let Some(type_params) = &function.type_params {
            self.visit_type_params(type_params);
        }
        self.visit_parameters(&function.parameters);
        if let Some(returns) = &function.returns {
            self.visit_annotation(returns);
        }

        let outer_scope = self.scope;
        self.scope = Scope::Function;
        self.visit_body(&function.body);
        self.scope = outer_scope;
    }

    fn walk_class_definition(&mut self, class: &'a ruff_python_ast::StmtClassDef) {
        for decorator in &class.decorator_list {
            self.visit_decorator(decorator);
        }
        if let Some(type_params) = &class.type_params {
            self.visit_type_params(type_params);
        }
        if let Some(arguments) = &class.arguments {
            self.visit_arguments(arguments);
        }

        let outer_scope = self.scope;
        self.scope = Scope::Class;
        self.visit_body(&class.body);
        self.scope = outer_scope;
    }

    fn visit_import(&mut self, import: &'a ruff_python_ast::StmtImport) {
        for alias in &import.names {
            let imported = alias.name.as_str();
            let name = alias.asname.as_ref().map_or_else(
                || imported.split('.').next().unwrap_or(imported),
                ruff_python_ast::Identifier::as_str,
            );
            if alias.asname.is_some() {
                self.use_lexical(imported);
            }
            self.use_import_whitelist(name);
            self.define(
                FindingKind::Import,
                name,
                import.range,
                FindingKind::Import.default_confidence(),
            );
        }
    }

    fn visit_import_from(&mut self, import: &'a ruff_python_ast::StmtImportFrom) {
        if import
            .module
            .as_ref()
            .is_some_and(|module| module == "__future__")
        {
            return;
        }
        for alias in &import.names {
            if alias.asname.is_some() {
                self.use_lexical(alias.name.as_str());
            }
            let name = alias
                .asname
                .as_ref()
                .map_or_else(|| alias.name.as_str(), ruff_python_ast::Identifier::as_str);
            self.use_import_whitelist(name);
            self.define(
                FindingKind::Import,
                name,
                import.range,
                FindingKind::Import.default_confidence(),
            );
        }
    }

    fn visit_special_assignment(&mut self, assignment: &'a ruff_python_ast::StmtAssign) {
        let assigns_all = assignment
            .targets
            .iter()
            .any(|target| matches!(target, Expr::Name(name) if name.id.as_str() == "__all__"));
        if !assigns_all {
            return;
        }

        let elements = match assignment.value.as_ref() {
            Expr::List(list) => Some(list.elts.as_slice()),
            Expr::Tuple(tuple) => Some(tuple.elts.as_slice()),
            _ => None,
        };
        if let Some(elements) = elements {
            for element in elements {
                if let Expr::StringLiteral(string) = element {
                    self.use_lexical(string.value.to_str());
                }
            }
        }
    }

    fn visit_reflective_call(&mut self, call: &'a ruff_python_ast::ExprCall) {
        if let Expr::Name(function) = call.func.as_ref() {
            let valid_arity = match function.id.as_str() {
                "getattr" => matches!(call.arguments.args.len(), 2 | 3),
                "hasattr" => call.arguments.args.len() == 2,
                _ => false,
            };
            if valid_arity && let Some(Expr::StringLiteral(attribute)) = call.arguments.args.get(1)
            {
                self.use_member(attribute.value.to_str());
            }
        }

        if let Expr::Attribute(function) = call.func.as_ref()
            && function.attr.as_str() == "format"
            && let Expr::StringLiteral(template) = function.value.as_ref()
            && call.arguments.keywords.iter().any(|keyword| {
                keyword.arg.is_none() && is_empty_call_named(&keyword.value, "locals")
            })
        {
            for name in format_field_names(template.value.to_str()) {
                match name {
                    FormatName::Lexical(name) => self.use_lexical(&name),
                    FormatName::Member(name) => self.use_member(&name),
                }
            }
        }
    }

    fn visit_percent_format(&mut self, binary: &'a ruff_python_ast::ExprBinOp) {
        if binary.op != Operator::Mod || !is_empty_call_named(&binary.right, "locals") {
            return;
        }
        let Expr::StringLiteral(template) = binary.left.as_ref() else {
            return;
        };
        for name in percent_format_names(template.value.to_str()) {
            self.use_lexical(name);
        }
    }

    fn walk_lambda(&mut self, lambda: &'a ruff_python_ast::ExprLambda) {
        if let Some(parameters) = &lambda.parameters {
            self.visit_parameters(parameters);
        }
        let outer_scope = self.scope;
        self.scope = Scope::Function;
        self.visit_expr(&lambda.body);
        self.scope = outer_scope;
    }

    fn walk_comprehension_expression(
        &mut self,
        generators: &'a [ruff_python_ast::Comprehension],
        results: &[&'a Expr],
    ) {
        let Some((first, remaining)) = generators.split_first() else {
            for result in results {
                self.visit_expr(result);
            }
            return;
        };

        // Python evaluates only the outermost iterable in the enclosing scope.
        self.visit_expr(&first.iter);
        let outer_scope = self.scope;
        self.scope = Scope::Function;
        self.visit_expr(&first.target);
        for condition in &first.ifs {
            self.visit_expr(condition);
        }
        for comprehension in remaining {
            self.visit_comprehension(comprehension);
        }
        for result in results {
            self.visit_expr(result);
        }
        self.scope = outer_scope;
    }
}

impl<'a> Visitor<'a> for Collector<'a> {
    fn visit_stmt(&mut self, stmt: &'a Stmt) {
        match stmt {
            Stmt::FunctionDef(function) => {
                self.visit_function_definition(function);
                self.walk_function_definition(function);
                return;
            }
            Stmt::ClassDef(class) => {
                self.visit_class_definition(class);
                self.walk_class_definition(class);
                return;
            }
            Stmt::Import(import) => self.visit_import(import),
            Stmt::ImportFrom(import) => self.visit_import_from(import),
            Stmt::Assign(assignment) => self.visit_special_assignment(assignment),
            _ => {}
        }
        visitor::walk_stmt(self, stmt);
    }

    fn visit_expr(&mut self, expr: &'a Expr) {
        match expr {
            Expr::Name(name) => match name.ctx {
                ExprContext::Load | ExprContext::Del if self.scope == Scope::Class => {
                    self.use_member(name.id.as_str());
                }
                ExprContext::Load | ExprContext::Del => self.use_lexical(name.id.as_str()),
                ExprContext::Store => self.define(
                    FindingKind::Variable,
                    name.id.as_str(),
                    name.range,
                    FindingKind::Variable.default_confidence(),
                ),
                ExprContext::Invalid => {}
            },
            Expr::Attribute(attribute) => match attribute.ctx {
                ExprContext::Load => self.use_member(attribute.attr.as_str()),
                ExprContext::Store => self.define(
                    FindingKind::Attribute,
                    attribute.attr.as_str(),
                    attribute.range,
                    FindingKind::Attribute.default_confidence(),
                ),
                ExprContext::Del | ExprContext::Invalid => {}
            },
            Expr::Call(call) => self.visit_reflective_call(call),
            Expr::BinOp(binary) => self.visit_percent_format(binary),
            Expr::Lambda(lambda) => {
                self.walk_lambda(lambda);
                return;
            }
            Expr::ListComp(comprehension) => {
                self.walk_comprehension_expression(
                    &comprehension.generators,
                    &[&comprehension.elt],
                );
                return;
            }
            Expr::SetComp(comprehension) => {
                self.walk_comprehension_expression(
                    &comprehension.generators,
                    &[&comprehension.elt],
                );
                return;
            }
            Expr::DictComp(comprehension) => {
                let mut results = Vec::with_capacity(2);
                if let Some(key) = &comprehension.key {
                    results.push(key.as_ref());
                }
                results.push(comprehension.value.as_ref());
                self.walk_comprehension_expression(&comprehension.generators, &results);
                return;
            }
            Expr::Generator(comprehension) => {
                self.walk_comprehension_expression(
                    &comprehension.generators,
                    &[&comprehension.elt],
                );
                return;
            }
            _ => {}
        }
        visitor::walk_expr(self, expr);
    }

    fn visit_parameter(&mut self, parameter: &'a Parameter) {
        self.define(
            FindingKind::Variable,
            parameter.name.as_str(),
            parameter.range,
            100,
        );
        visitor::walk_parameter(self, parameter);
    }

    fn visit_pattern(&mut self, pattern: &'a Pattern) {
        match pattern {
            Pattern::MatchMapping(mapping) => {
                if let Some(name) = &mapping.rest {
                    self.define(
                        FindingKind::Variable,
                        name.as_str(),
                        mapping.range,
                        FindingKind::Variable.default_confidence(),
                    );
                }
            }
            Pattern::MatchClass(class) => {
                for keyword in &class.arguments.keywords {
                    self.use_member(keyword.attr.as_str());
                }
            }
            Pattern::MatchStar(star) => {
                if let Some(name) = &star.name {
                    self.define(
                        FindingKind::Variable,
                        name.as_str(),
                        star.range,
                        FindingKind::Variable.default_confidence(),
                    );
                }
            }
            Pattern::MatchAs(as_pattern) => {
                if let Some(name) = &as_pattern.name {
                    self.define(
                        FindingKind::Variable,
                        name.as_str(),
                        as_pattern.range,
                        FindingKind::Variable.default_confidence(),
                    );
                }
            }
            Pattern::MatchValue(_)
            | Pattern::MatchSingleton(_)
            | Pattern::MatchSequence(_)
            | Pattern::MatchOr(_) => {}
        }
        visitor::walk_pattern(self, pattern);
    }
}

fn parse_error(
    path: &Path,
    source: &str,
    line_index: &LineIndex,
    range: TextRange,
    message: String,
) -> AnalyzerError {
    let position = line_index.line_column(range.start(), source);
    AnalyzerError::Parse {
        path: path.to_path_buf(),
        line: position.line.get(),
        column: position.column.get(),
        message,
    }
}

fn decorator_name(expression: &Expr) -> Option<String> {
    fn push_name(expression: &Expr, output: &mut String) -> bool {
        match expression {
            Expr::Name(name) => {
                output.push_str(name.id.as_str());
                true
            }
            Expr::Attribute(attribute) => {
                if push_name(&attribute.value, output) {
                    output.push('.');
                    output.push_str(attribute.attr.as_str());
                    true
                } else {
                    false
                }
            }
            Expr::Call(call) => push_name(&call.func, output),
            _ => false,
        }
    }

    let mut name = String::from("@");
    push_name(expression, &mut name).then_some(name)
}

fn is_empty_call_named(expression: &Expr, expected: &str) -> bool {
    matches!(
        expression,
        Expr::Call(call)
            if call.arguments.is_empty()
                && matches!(call.func.as_ref(), Expr::Name(name) if name.id.as_str() == expected)
    )
}

enum FormatName {
    Lexical(String),
    Member(String),
}

fn format_field_names(template: &str) -> Vec<FormatName> {
    let mut names = Vec::new();
    let mut remainder = template;

    while let Some(open) = remainder.find('{') {
        remainder = &remainder[open + 1..];
        if remainder.starts_with('{') {
            remainder = &remainder[1..];
            continue;
        }
        let Some(close) = remainder.find('}') else {
            break;
        };
        let field = remainder[..close]
            .split(['!', ':'])
            .next()
            .unwrap_or_default();
        let mut without_indexes = String::with_capacity(field.len());
        let mut bracket_depth = 0_u32;
        for character in field.chars() {
            match character {
                '[' => bracket_depth = bracket_depth.saturating_add(1),
                ']' => bracket_depth = bracket_depth.saturating_sub(1),
                _ if bracket_depth == 0 => without_indexes.push(character),
                _ => {}
            }
        }
        for (index, segment) in without_indexes.split('.').enumerate() {
            if !is_identifier(segment) {
                continue;
            }
            if index == 0 {
                names.push(FormatName::Lexical(segment.to_owned()));
            } else {
                names.push(FormatName::Member(segment.to_owned()));
            }
        }
        remainder = &remainder[close + 1..];
    }

    names
}

#[derive(Clone, Copy)]
enum TypeCommentMode {
    Expression,
    Function,
}

struct TypeComment<'source> {
    range: TextRange,
    annotation: &'source str,
}

struct InvalidTypeComment {
    range: TextRange,
    message: String,
}

fn type_comment_names(
    source: &str,
    tokens: &Tokens,
    body: &[Stmt],
) -> Result<Vec<FormatName>, InvalidTypeComment> {
    let comments: Vec<_> = tokens
        .iter()
        .filter(|token| token.kind() == TokenKind::Comment)
        .filter_map(|token| {
            let range = token.range();
            let comment = &source[range.start().to_usize()..range.end().to_usize()];
            let annotation = comment
                .strip_prefix('#')?
                .trim_start()
                .strip_prefix("type:")?
                .trim();
            (!is_type_ignore(annotation)).then_some(TypeComment { range, annotation })
        })
        .collect();
    let mut modes = vec![None; comments.len()];
    TypeCommentLocator {
        source,
        comments: &comments,
        modes: &mut modes,
    }
    .visit_body(body);

    let mut names = Vec::new();
    for (comment, mode) in comments.iter().zip(modes) {
        let Some(mode) = mode else {
            return Err(InvalidTypeComment {
                range: comment.range,
                message: "misplaced type comment".to_owned(),
            });
        };
        let valid = match mode {
            TypeCommentMode::Expression => valid_type_expression(comment.annotation),
            TypeCommentMode::Function => valid_function_type(comment.annotation),
        };
        if !valid {
            return Err(InvalidTypeComment {
                range: comment.range,
                message: "invalid type comment".to_owned(),
            });
        }
        names.extend(annotation_names(comment.annotation));
    }

    Ok(names)
}

fn is_type_ignore(annotation: &str) -> bool {
    annotation == "ignore"
        || annotation
            .strip_prefix("ignore")
            .is_some_and(|suffix| suffix.trim_start().starts_with('['))
}

struct TypeCommentLocator<'source, 'comments> {
    source: &'source str,
    comments: &'comments [TypeComment<'source>],
    modes: &'comments mut [Option<TypeCommentMode>],
}

impl TypeCommentLocator<'_, '_> {
    fn comment_range(&self, start: TextSize, end: TextSize) -> std::ops::Range<usize> {
        let start = self
            .comments
            .partition_point(|comment| comment.range.start() < start);
        let end = self
            .comments
            .partition_point(|comment| comment.range.start() < end);
        start..end
    }

    fn mark_trailing_assignment(&mut self, assignment: &ruff_python_ast::StmtAssign) {
        let line_end = self.source[assignment.end().to_usize()..]
            .find(['\n', '\r'])
            .map_or(self.source.len(), |offset| {
                assignment.end().to_usize() + offset
            });
        let line_end = TextSize::try_from(line_end).expect("Python source is smaller than 4 GiB");
        for index in self.comment_range(assignment.end(), line_end) {
            if self.modes[index].is_none()
                && same_line(
                    self.source,
                    assignment.end(),
                    self.comments[index].range.start(),
                )
            {
                self.modes[index] = Some(TypeCommentMode::Expression);
            }
        }
    }

    fn mark_compound_header(&mut self, start: TextSize, body: &[Stmt]) {
        let Some(body_start) = body.first().map(Ranged::start) else {
            return;
        };
        for index in self.comment_range(start, body_start) {
            let comment = &self.comments[index];
            if self.modes[index].is_none()
                && line_prefix(self.source, comment.range.start())
                    .trim_end()
                    .ends_with(':')
            {
                self.modes[index] = Some(TypeCommentMode::Expression);
            }
        }
    }

    fn mark_function_header(&mut self, function: &ruff_python_ast::StmtFunctionDef) {
        let Some(body_start) = function.body.first().map(Ranged::start) else {
            return;
        };
        let mut has_function_comment = false;
        for index in self.comment_range(function.start(), body_start) {
            if self.modes[index].is_some() {
                continue;
            }
            let comment = &self.comments[index];
            let prefix = line_prefix(self.source, comment.range.start());
            if prefix.trim_end().ends_with(':')
                || (prefix.trim().is_empty()
                    && previous_nonempty_line(self.source, function.start(), comment.range.start())
                        .is_some_and(|line| line.trim_end().ends_with(':')))
            {
                if !has_function_comment {
                    self.modes[index] = Some(TypeCommentMode::Function);
                    has_function_comment = true;
                }
            } else if function.parameters.iter_source_order().any(|parameter| {
                let parameter_end = parameter.end();
                if parameter_end > comment.range.start()
                    || !same_line(self.source, parameter_end, comment.range.start())
                {
                    return false;
                }
                let suffix =
                    &self.source[parameter_end.to_usize()..comment.range.start().to_usize()];
                suffix.trim().trim_end_matches(',').trim().is_empty()
            }) {
                self.modes[index] = Some(TypeCommentMode::Expression);
            }
        }
    }
}

impl<'ast> Visitor<'ast> for TypeCommentLocator<'_, '_> {
    fn visit_stmt(&mut self, statement: &'ast Stmt) {
        match statement {
            Stmt::Assign(assignment) => self.mark_trailing_assignment(assignment),
            Stmt::For(for_statement) => {
                self.mark_compound_header(for_statement.start(), &for_statement.body);
            }
            Stmt::With(with_statement) => {
                self.mark_compound_header(with_statement.start(), &with_statement.body);
            }
            Stmt::FunctionDef(function) => self.mark_function_header(function),
            _ => {}
        }
        visitor::walk_stmt(self, statement);
    }
}

fn same_line(source: &str, start: TextSize, end: TextSize) -> bool {
    !source[start.to_usize()..end.to_usize()].contains(['\n', '\r'])
}

fn line_prefix(source: &str, offset: TextSize) -> &str {
    let offset = offset.to_usize();
    let start = source[..offset]
        .rfind(['\n', '\r'])
        .map_or(0, |index| index + 1);
    &source[start..offset]
}

fn previous_nonempty_line(source: &str, start: TextSize, end: TextSize) -> Option<&str> {
    source[start.to_usize()..end.to_usize()]
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty() && !line.trim_start().starts_with('#'))
}

fn valid_type_expression(annotation: &str) -> bool {
    parse_unchecked(
        annotation,
        ParseOptions::from(Mode::Expression).with_target_version(PythonVersion::latest()),
    )
    .try_into_expression()
    .is_some_and(|parsed| !parsed.has_syntax_errors())
}

fn valid_function_type(annotation: &str) -> bool {
    let parsed = parse_unchecked(
        annotation,
        ParseOptions::from(Mode::Expression).with_target_version(PythonVersion::latest()),
    );
    let significant: Vec<_> = parsed
        .tokens()
        .iter()
        .filter(|token| {
            !matches!(
                token.kind(),
                TokenKind::Comment
                    | TokenKind::Newline
                    | TokenKind::NonLogicalNewline
                    | TokenKind::Indent
                    | TokenKind::Dedent
                    | TokenKind::EndOfFile
            )
        })
        .collect();
    if significant
        .first()
        .is_none_or(|token| token.kind() != TokenKind::Lpar)
    {
        return false;
    }

    let mut delimiters = Vec::new();
    let mut closing = None;
    for (index, token) in significant.iter().enumerate() {
        match token.kind() {
            TokenKind::Lpar | TokenKind::Lsqb | TokenKind::Lbrace => {
                delimiters.push(token.kind());
            }
            TokenKind::Rpar | TokenKind::Rsqb | TokenKind::Rbrace => {
                let Some(opening) = delimiters.pop() else {
                    return false;
                };
                if !delimiters_match(opening, token.kind()) {
                    return false;
                }
                if delimiters.is_empty() {
                    closing = Some(index);
                    break;
                }
            }
            _ => {}
        }
    }
    let Some(closing) = closing else {
        return false;
    };
    let Some(arrow) = significant.get(closing + 1) else {
        return false;
    };
    if arrow.kind() != TokenKind::Rarrow || significant.get(closing + 2).is_none() {
        return false;
    }
    let closing_token = significant[closing];
    let parameters = &annotation[significant[0].end().to_usize()..closing_token.start().to_usize()];
    let returns = annotation[arrow.end().to_usize()..].trim();
    valid_type_parameters(parameters) && valid_type_expression(returns)
}

fn valid_type_parameters(parameters: &str) -> bool {
    if parameters.trim().is_empty() {
        return true;
    }
    let parsed = parse_unchecked(
        parameters,
        ParseOptions::from(Mode::Expression).with_target_version(PythonVersion::latest()),
    );
    let mut delimiters = Vec::new();
    let mut start = 0;
    let mut seen_star = false;
    let mut seen_double_star = false;
    for token in parsed.tokens() {
        match token.kind() {
            TokenKind::Lpar | TokenKind::Lsqb | TokenKind::Lbrace => {
                delimiters.push(token.kind());
            }
            TokenKind::Rpar | TokenKind::Rsqb | TokenKind::Rbrace => {
                let Some(opening) = delimiters.pop() else {
                    return false;
                };
                if !delimiters_match(opening, token.kind()) {
                    return false;
                }
            }
            TokenKind::Comma if delimiters.is_empty() => {
                if !valid_type_parameter(
                    &parameters[start..token.start().to_usize()],
                    &mut seen_star,
                    &mut seen_double_star,
                ) {
                    return false;
                }
                start = token.end().to_usize();
            }
            _ => {}
        }
    }
    if !delimiters.is_empty() {
        return false;
    }
    let tail = parameters[start..].trim();
    valid_type_parameter(tail, &mut seen_star, &mut seen_double_star)
}

fn valid_type_parameter(
    parameter: &str,
    seen_star: &mut bool,
    seen_double_star: &mut bool,
) -> bool {
    let parameter = parameter.trim();
    let (parameter, is_star, is_double_star) = if let Some(parameter) = parameter.strip_prefix("**")
    {
        (parameter, false, true)
    } else if let Some(parameter) = parameter.strip_prefix('*') {
        (parameter, true, false)
    } else {
        (parameter, false, false)
    };

    if *seen_double_star || (*seen_star && !is_double_star) {
        return false;
    }
    *seen_star |= is_star;
    *seen_double_star |= is_double_star;

    let parameter = parameter.trim();
    !parameter.is_empty() && valid_type_expression(parameter)
}

const fn delimiters_match(opening: TokenKind, closing: TokenKind) -> bool {
    matches!(
        (opening, closing),
        (TokenKind::Lpar, TokenKind::Rpar)
            | (TokenKind::Lsqb, TokenKind::Rsqb)
            | (TokenKind::Lbrace, TokenKind::Rbrace)
    )
}

fn annotation_names(annotation: &str) -> Vec<FormatName> {
    let parsed = parse_unchecked(
        annotation,
        ParseOptions::from(Mode::Expression).with_target_version(PythonVersion::latest()),
    );
    let significant: Vec<_> = parsed
        .tokens()
        .iter()
        .filter(|token| !token.kind().is_trivia() && token.kind() != TokenKind::EndOfFile)
        .collect();
    let mut names = Vec::new();
    for (index, token) in significant.iter().enumerate() {
        if token.kind() == TokenKind::Name {
            if significant
                .get(index + 1)
                .is_some_and(|next| next.kind() == TokenKind::Equal)
            {
                continue;
            }
            let range = token.range();
            let name = annotation[range.start().to_usize()..range.end().to_usize()].to_owned();
            if index > 0 && significant[index - 1].kind() == TokenKind::Dot {
                names.push(FormatName::Member(name));
            } else {
                names.push(FormatName::Lexical(name));
            }
        }
    }
    names
}

fn percent_format_names(template: &str) -> Vec<&str> {
    let mut names = Vec::new();
    let mut remainder = template;

    while let Some(open) = remainder.find("%(") {
        remainder = &remainder[open + 2..];
        let Some(close) = remainder.find(')') else {
            break;
        };
        let name = &remainder[..close];
        if !name.is_empty()
            && name
                .chars()
                .all(|character| character == '_' || character.is_alphanumeric())
        {
            names.push(name);
        }
        remainder = &remainder[close + 1..];
    }

    names
}

fn is_identifier(value: &str) -> bool {
    let mut characters = value.chars();
    characters
        .next()
        .is_some_and(|first| first == '_' || first.is_alphabetic())
        && characters.all(|character| character == '_' || character.is_alphanumeric())
}

fn suppressed(source: &str, offset: TextSize, code: &str) -> bool {
    let offset = offset.to_usize().min(source.len());
    let line_start = source[..offset]
        .rfind(['\n', '\r'])
        .map_or(0, |index| index + 1);
    let line_end = source[offset..]
        .find(['\n', '\r'])
        .map_or(source.len(), |index| offset + index);
    let source_line = &source[line_start..line_end];
    let lowercase = source_line.to_ascii_lowercase();
    let Some(noqa_index) = lowercase.find("# noqa") else {
        return false;
    };
    let after_noqa = &source_line[noqa_index + "# noqa".len()..];
    let Some(mut listed_codes) = after_noqa.strip_prefix(':') else {
        return true;
    };
    if let Some(whitespace) = listed_codes
        .chars()
        .next()
        .filter(|character| character.is_whitespace())
    {
        listed_codes = &listed_codes[whitespace.len_utf8()..];
    }
    let alias = match code {
        "V104" => Some("F401"),
        "V107" => Some("F841"),
        _ => None,
    };
    let bytes = listed_codes.as_bytes();
    let mut offset = 0;
    let mut found_code = false;
    while offset < bytes.len() {
        let start = offset;
        while bytes.get(offset).is_some_and(u8::is_ascii_alphabetic) {
            offset += 1;
        }
        let digits = offset;
        while bytes.get(offset).is_some_and(u8::is_ascii_digit) {
            offset += 1;
        }
        if start == digits || digits == offset {
            break;
        }
        found_code = true;
        let listed = &listed_codes[start..offset];
        if listed.eq_ignore_ascii_case(code)
            || alias.is_some_and(|alias| listed.eq_ignore_ascii_case(alias))
        {
            return true;
        }
        while bytes
            .get(offset)
            .is_some_and(|byte| *byte == b',' || byte.is_ascii_whitespace())
        {
            offset += 1;
        }
    }

    !found_code
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use pretty_assertions::assert_eq;

    use super::{Analyzer, valid_function_type};
    use crate::{FindingKind, Policy};

    fn findings(source: &str) -> Vec<(FindingKind, String, usize, u8)> {
        Analyzer::new(Policy::default())
            .analyze_sources([(PathBuf::from("module.py"), source)])
            .findings
            .into_iter()
            .map(|finding| {
                (
                    finding.kind,
                    finding.name,
                    finding.start.line,
                    finding.confidence,
                )
            })
            .collect()
    }

    #[test]
    fn finds_vulture_core_categories() {
        let source = r#"import missing

class Unused:
    def method(self, unused_arg):
        self.dead_attr = 1

    @property
    def dead_property(self):
        return 1

def dead_function():
    dead_variable = 1
"#;
        assert_eq!(
            findings(source),
            vec![
                (FindingKind::Import, "missing".to_owned(), 1, 90),
                (FindingKind::Class, "Unused".to_owned(), 3, 60),
                (FindingKind::Method, "method".to_owned(), 4, 60),
                (FindingKind::Variable, "unused_arg".to_owned(), 4, 100),
                (FindingKind::Attribute, "dead_attr".to_owned(), 5, 60),
                (FindingKind::Property, "dead_property".to_owned(), 7, 60),
                (FindingKind::Function, "dead_function".to_owned(), 11, 60),
                (FindingKind::Variable, "dead_variable".to_owned(), 12, 60),
            ]
        );
    }

    #[test]
    fn resolves_lexical_and_member_uses_across_files() {
        let analysis = Analyzer::default().analyze_sources([
            ("definitions.py", "def alive(): pass\nclass Widget: pass\n"),
            ("uses.py", "alive()\nmodule.Widget()\n"),
        ]);
        assert!(analysis.errors.is_empty());
        assert!(analysis.findings.is_empty());
    }

    #[test]
    fn supports_all_and_reflection() {
        let source = r#"def exported(): pass
__all__ = ["exported"]

class Thing:
    def reflected(self): pass

getattr(Thing(), "reflected")
"#;
        assert!(findings(source).is_empty());
    }

    #[test]
    fn honors_confidence_and_noqa_aliases() {
        let policy = Policy::new(80, &[], &[]).expect("valid policy");
        let source = "import foo  # noqa: F401\ndef dead():\n    value = 1\n";
        let analysis = Analyzer::new(policy).analyze_sources([("module.py", source)]);
        assert_eq!(
            analysis
                .findings
                .iter()
                .map(|finding| (finding.kind, finding.name.as_str()))
                .collect::<Vec<_>>(),
            Vec::<(FindingKind, &str)>::new()
        );
    }

    #[test]
    fn reports_parse_errors_without_recovery_findings() {
        let analysis =
            Analyzer::default().analyze_sources([("broken.py", "def broken(:\n    pass")]);
        assert!(analysis.findings.is_empty());
        assert!(!analysis.errors.is_empty());
    }

    #[test]
    fn only_accepts_flake8_shaped_noqa_comments() {
        let source = "exact = 1  # noqa\ninvalid_codes = 1  # noqa: 123V\nno_space = 1  #noqa\nprose = 1  # Hello, noqa\n";
        assert_eq!(
            findings(source),
            vec![
                (FindingKind::Variable, "no_space".to_owned(), 3, 60),
                (FindingKind::Variable, "prose".to_owned(), 4, 60),
            ]
        );
    }

    #[test]
    fn percent_format_with_locals_marks_fields_used() {
        let source = "visible = 1\nprint('%(visible)s' % locals())\nunused = 2\n";
        assert_eq!(
            findings(source),
            vec![(FindingKind::Variable, "unused".to_owned(), 3, 60)]
        );
    }

    #[test]
    fn dotted_format_fields_mark_members_used() {
        let source = "class Box:\n    pass\nbox = Box()\nbox.dynamic = 1\nbox.numeric = 2\nprint('{box.dynamic}'.format(**locals()))\nprint('{0.numeric}'.format(**locals()))\n";
        assert!(findings(source).is_empty());
    }

    #[test]
    fn type_comments_mark_imports_used() {
        let source = r#"from typing import Any, Dict, List, Text, Tuple

def plain(arg):
    # explanatory comment
    # type: (Text) -> None
    pass
async def asynchronous(arg):
    # type: (List[int]) -> None
    pass
value = {}  # type: Dict[str, str]
for item in []:  # type: Any
    print(item)
pair = (1, 2)  # type: Tuple[int, int]
"#;
        let analysis = Analyzer::default().analyze_sources([("module.py", source)]);
        assert!(
            analysis
                .findings
                .iter()
                .all(|finding| finding.kind != FindingKind::Import),
            "{:?}",
            analysis.findings
        );
    }

    #[test]
    fn rejects_misplaced_and_invalid_function_type_comments() {
        for source in [
            "from x import Missing\n\n# type: Missing\n",
            "def bad():\n    # type: bogus\n    pass\nbad()\n",
            "def bad(arg):\n    # type: (int,) -> None\n    pass\nbad(1)\n",
            "def bad(arg):\n    # type: (int] -> None\n    pass\nbad(1)\n",
            "def bad(arg):\n    # type: (**int, str) -> None\n    pass\nbad(1)\n",
            "def bad(arg):\n    # type: (*int, *str) -> None\n    pass\nbad(1)\n",
            "def bad(arg):\n    # type: (**int, **str) -> None\n    pass\nbad(1)\n",
            "def bad(arg):\n    # type: (int) -> None\n    # type: (int) -> None\n    pass\nbad(1)\n",
            "def bad() -> (\n    int  # type: int\n):\n    return 1\nprint(bad())\n",
            "def bad(\n    arg=(1  # type: int\n    ),\n):\n    return arg\nprint(bad())\n",
            "def bad(\n    *,  # type: int\n    arg,\n):\n    return arg\nprint(bad(arg=1))\n",
        ] {
            let analysis = Analyzer::default().analyze_sources([("module.py", source)]);
            assert!(analysis.findings.is_empty());
            assert_eq!(analysis.errors.len(), 1, "{source:?}");
        }
    }

    #[test]
    fn validates_function_type_parameter_grammar() {
        for annotation in [
            "() -> None",
            "(int) -> None",
            "(int, str) -> None",
            "(*int) -> None",
            "(**int) -> None",
            "(int, *str) -> None",
            "(int, **str) -> None",
            "(*int, **str) -> None",
            "(int, *str, **bytes) -> None",
        ] {
            assert!(valid_function_type(annotation), "{annotation:?}");
        }
    }

    #[test]
    fn parameter_type_comments_only_attach_to_parameters() {
        let source = r#"from typing import Dict, List

def valid(
    first,  # type: Dict[str, int]
    second  # type: List[int]
):
    return first, second

print(valid({}, []))
"#;
        let analysis = Analyzer::default().analyze_sources([("module.py", source)]);
        assert!(analysis.errors.is_empty(), "{:?}", analysis.errors);
        assert!(analysis.findings.is_empty(), "{:?}", analysis.findings);
    }

    #[test]
    fn applies_vulture_protocol_whitelists() {
        let source = r#"import ast
import unittest

class Visitor(ast.NodeVisitor):
    def visit_Name(self, node):
        return node

class TestWidget(unittest.TestCase):
    def setUp(self):
        self.value = 1
"#;
        let analysis = Analyzer::default().analyze_sources([("test_protocols.py", source)]);
        assert!(
            analysis
                .findings
                .iter()
                .all(|finding| !matches!(finding.name.as_str(), "visit_Name" | "setUp")),
            "{:?}",
            analysis.findings
        );
    }

    #[test]
    fn type_comment_attributes_mark_members_used() {
        let source = r#"class Namespace:
    pass
Namespace.member = int
value = None  # type: Namespace.member
print(value)
"#;
        assert!(findings(source).is_empty());
    }

    #[test]
    fn type_comments_ignore_keyword_labels_and_type_ignore_tags() {
        let source = r#"from external import Foo, Baz
keyword = 1
ignored = 2
value = None  # type: Foo(keyword=Baz)
other = None  # type: ignore [assignment]
print(value, other)
"#;
        assert_eq!(
            findings(source),
            vec![
                (FindingKind::Variable, "keyword".to_owned(), 2, 60),
                (FindingKind::Variable, "ignored".to_owned(), 3, 60),
            ]
        );
    }

    #[test]
    fn class_body_aliases_mark_methods_used() {
        let source = r#"class Bar:
    def func(self):
        return 1
    other_name_for_func = func

Bar().other_name_for_func()
"#;
        assert!(findings(source).is_empty());
    }

    #[test]
    fn nested_expression_scopes_do_not_mark_methods_used() {
        let source = r#"class C:
    def dead(self):
        return 1
    keep = lambda dead: dead
    values = [dead for dead in (1, 2)]

C.keep(1)
print(C.values)
"#;
        assert_eq!(
            findings(source),
            vec![(FindingKind::Method, "dead".to_owned(), 2, 60)]
        );
    }

    #[test]
    fn aliased_imports_mark_original_cross_file_names_used() {
        let analysis = Analyzer::default().analyze_sources([
            ("library.py", "def helper():\n    return 1\n"),
            (
                "application.py",
                "from library import helper as alias\nprint(alias())\n",
            ),
        ]);
        assert!(analysis.errors.is_empty());
        assert!(analysis.findings.is_empty(), "{:?}", analysis.findings);
    }

    #[test]
    fn except_handler_aliases_match_vulture() {
        let source = "try:\n    pass\nexcept Exception as error:\n    pass\n";
        assert!(findings(source).is_empty());
    }

    #[test]
    fn reads_pep_263_latin1_source_files() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("latin1.py");
        fs::write(&path, b"# -*- coding: latin-1 -*-\nunused = 'caf\xe9'\n")
            .expect("write Latin-1 source");

        let analysis = Analyzer::default().analyze_files(&[path]);
        assert!(analysis.errors.is_empty(), "{:?}", analysis.errors);
        assert_eq!(analysis.findings.len(), 1);
        assert_eq!(analysis.findings[0].name, "unused");
    }

    #[test]
    fn supports_python_codepages_and_rejects_non_python_labels() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let cp437 = directory.path().join("cp437.py");
        fs::write(&cp437, b"# coding: cp437\nprint('caf\x82')\n").expect("write CP437 source");
        let analysis = Analyzer::default().analyze_files(&[cp437]);
        assert!(analysis.errors.is_empty(), "{:?}", analysis.errors);

        for encoding in ["mac-roman", "mac-cyrillic"] {
            let path = directory.path().join(format!("{encoding}.py"));
            fs::write(&path, format!("# coding: {encoding}\nprint('clean')\n"))
                .expect("write Macintosh source");
            let analysis = Analyzer::default().analyze_files(&[path]);
            assert!(
                analysis.errors.is_empty(),
                "{encoding}: {:?}",
                analysis.errors
            );
        }

        for (name, source) in [
            (
                "unknown.py",
                b"# coding: x-user-defined\nprint('clean')\n".as_slice(),
            ),
            (
                "invalid_ascii.py",
                b"# coding: ascii\nprint('\x80')\n".as_slice(),
            ),
        ] {
            let path = directory.path().join(name);
            fs::write(&path, source).expect("write invalid source");
            let analysis = Analyzer::default().analyze_files(&[path]);
            assert_eq!(analysis.errors.len(), 1, "{name}");
        }
    }
}
