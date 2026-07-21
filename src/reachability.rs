use ruff_python_ast::helpers::Truthiness;
use ruff_python_ast::visitor::{self, Visitor};
use ruff_python_ast::{BoolOp, ExceptHandler, Expr, Stmt, UnaryOp};
use ruff_text_size::{Ranged, TextRange};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Unreachable {
    pub(crate) name: &'static str,
    pub(crate) range: TextRange,
    pub(crate) message: String,
}

/// Find unreachable statement ranges in `body` and every suite nested within it.
pub(crate) fn analyze(body: &[Stmt]) -> Vec<Unreachable> {
    let mut analyzer = Reachability::default();
    analyzer.analyze_suite(body);
    ExpressionReachability {
        findings: &mut analyzer.findings,
    }
    .visit_body(body);
    analyzer
        .findings
        .sort_by_key(|finding| (finding.range.start(), finding.range.end()));
    analyzer.findings
}

struct ExpressionReachability<'a> {
    findings: &'a mut Vec<Unreachable>,
}

impl<'ast> Visitor<'ast> for ExpressionReachability<'_> {
    fn visit_expr(&mut self, expression: &'ast Expr) {
        if let Expr::If(if_expression) = expression {
            match constant_truthiness(&if_expression.test) {
                Some(false) => self.findings.push(Unreachable {
                    name: "ternary",
                    range: TextRange::new(if_expression.range.start(), if_expression.body.end()),
                    message: "unsatisfiable 'ternary' condition".to_owned(),
                }),
                Some(true) => self.findings.push(Unreachable {
                    name: "ternary",
                    range: if_expression.orelse.range(),
                    message: "unreachable 'else' expression".to_owned(),
                }),
                None => {}
            }
        }
        visitor::walk_expr(self, expression);
    }
}

#[derive(Default)]
struct Reachability {
    findings: Vec<Unreachable>,
}

impl Reachability {
    /// Analyze a statement suite and return whether execution can reach its end.
    fn analyze_suite(&mut self, body: &[Stmt]) -> bool {
        let mut first_terminal = None;

        // Analyze every statement, including statements in a dead tail. This mirrors
        // Python's recursive AST walk and still finds dead code in nested definitions.
        for (index, statement) in body.iter().enumerate() {
            let can_fall_through = self.analyze_statement(statement);
            if first_terminal.is_none() && !can_fall_through {
                first_terminal = Some(index);
            }
        }

        let Some(index) = first_terminal else {
            return true;
        };

        if let Some(dead_tail) = body.get(index + 1..)
            && let Some(range) = suite_range(dead_tail)
        {
            let name = statement_name(&body[index]);
            self.findings.push(Unreachable {
                name,
                range,
                message: format!("unreachable code after '{name}'"),
            });
        }

        false
    }

    fn analyze_statement(&mut self, statement: &Stmt) -> bool {
        match statement {
            Stmt::FunctionDef(function) => {
                self.analyze_suite(&function.body);
                true
            }
            Stmt::ClassDef(class) => {
                self.analyze_suite(&class.body);
                true
            }
            Stmt::Return(_) | Stmt::Raise(_) | Stmt::Break(_) | Stmt::Continue(_) => false,
            Stmt::For(for_statement) => {
                self.analyze_suite(&for_statement.body);
                self.analyze_suite(&for_statement.orelse);

                // A for-loop can execute zero times, and a break can skip its else suite.
                true
            }
            Stmt::While(while_statement) => self.analyze_while(while_statement),
            Stmt::If(if_statement) => self.analyze_if(if_statement),
            Stmt::With(with_statement) => {
                self.analyze_suite(&with_statement.body);

                // A context manager can suppress an exception raised by its body. Without
                // type information, treating a with statement as terminal is not sound.
                true
            }
            Stmt::Match(match_statement) => {
                let case_fallthrough: Vec<_> = match_statement
                    .cases
                    .iter()
                    .map(|case| self.analyze_suite(&case.body))
                    .collect();
                let exhaustive = match_statement
                    .cases
                    .iter()
                    .any(|case| case.guard.is_none() && case.pattern.is_irrefutable());

                !exhaustive
                    || case_fallthrough
                        .into_iter()
                        .any(|can_fall_through| can_fall_through)
            }
            Stmt::Try(try_statement) => self.analyze_try(try_statement),
            Stmt::Delete(_)
            | Stmt::TypeAlias(_)
            | Stmt::Assign(_)
            | Stmt::AugAssign(_)
            | Stmt::AnnAssign(_)
            | Stmt::Assert(_)
            | Stmt::Import(_)
            | Stmt::ImportFrom(_)
            | Stmt::Global(_)
            | Stmt::Nonlocal(_)
            | Stmt::Expr(_)
            | Stmt::Pass(_)
            | Stmt::IpyEscapeCommand(_) => true,
        }
    }

    fn analyze_if(&mut self, statement: &ruff_python_ast::StmtIf) -> bool {
        let mut branches = Vec::with_capacity(statement.elif_else_clauses.len() + 1);
        branches.push(ConditionalBranch {
            test: Some(&statement.test),
            body: &statement.body,
            range: statement.range,
        });
        branches.extend(
            statement
                .elif_else_clauses
                .iter()
                .map(|clause| ConditionalBranch {
                    test: clause.test.as_ref(),
                    body: &clause.body,
                    range: clause.range,
                }),
        );

        let branch_fallthrough: Vec<_> = branches
            .iter()
            .map(|branch| self.analyze_suite(branch.body))
            .collect();

        let mut can_fall_through = false;
        let mut reaches_next_branch = true;

        for (index, branch) in branches.iter().enumerate() {
            if !reaches_next_branch {
                break;
            }

            let Some(test) = branch.test else {
                can_fall_through |= branch_fallthrough[index];
                reaches_next_branch = false;
                continue;
            };

            match constant_truthiness(test) {
                Some(false) => {
                    self.findings.push(Unreachable {
                        name: "if",
                        range: branch.range_through_body(),
                        message: "unsatisfiable 'if' condition".to_owned(),
                    });
                }
                Some(true) => {
                    can_fall_through |= branch_fallthrough[index];
                    reaches_next_branch = false;

                    if let Some(range) = unreachable_branches_range(&branches[index + 1..]) {
                        self.findings.push(Unreachable {
                            name: "else",
                            range,
                            message: "unreachable 'else' block".to_owned(),
                        });
                    } else {
                        self.findings.push(Unreachable {
                            name: "if",
                            range: branch.range_through_body(),
                            message: "redundant if-condition".to_owned(),
                        });
                    }
                }
                None => {
                    can_fall_through |= branch_fallthrough[index];
                }
            }
        }

        // If no else (or definitely-true elif) closes the chain, every condition
        // can be false and execution can continue after the if statement.
        can_fall_through || reaches_next_branch
    }

    fn analyze_while(&mut self, statement: &ruff_python_ast::StmtWhile) -> bool {
        self.analyze_suite(&statement.body);
        let else_can_fall_through = self.analyze_suite(&statement.orelse);

        match constant_truthiness(&statement.test) {
            Some(false) => {
                self.findings.push(Unreachable {
                    name: "while",
                    range: range_through_body(statement.range, &statement.body),
                    message: "unsatisfiable 'while' condition".to_owned(),
                });

                statement.orelse.is_empty() || else_can_fall_through
            }
            Some(true) => {
                if let Some(range) = suite_range(&statement.orelse) {
                    self.findings.push(Unreachable {
                        name: "else",
                        range,
                        message: "unreachable 'else' block".to_owned(),
                    });
                }

                contains_break_for_current_loop(&statement.body)
            }
            None => true,
        }
    }

    fn analyze_try(&mut self, statement: &ruff_python_ast::StmtTry) -> bool {
        let body_can_fall_through = self.analyze_suite(&statement.body);
        let handler_fallthrough: Vec<_> = statement
            .handlers
            .iter()
            .map(|handler| {
                let ExceptHandler::ExceptHandler(handler) = handler;
                self.analyze_suite(&handler.body)
            })
            .collect();
        let else_can_fall_through = self.analyze_suite(&statement.orelse);
        let finally_can_fall_through = self.analyze_suite(&statement.finalbody);

        if !body_can_fall_through && let Some(range) = suite_range(&statement.orelse) {
            self.findings.push(Unreachable {
                name: "else",
                range,
                message: "unreachable 'else' block".to_owned(),
            });
        }

        let normal_path_can_fall_through =
            body_can_fall_through && (statement.orelse.is_empty() || else_can_fall_through);
        let try_can_fall_through = normal_path_can_fall_through
            || handler_fallthrough
                .into_iter()
                .any(|can_fall_through| can_fall_through);

        if statement.finalbody.is_empty() {
            try_can_fall_through
        } else {
            finally_can_fall_through && try_can_fall_through
        }
    }
}

#[derive(Clone, Copy)]
struct ConditionalBranch<'a> {
    test: Option<&'a Expr>,
    body: &'a [Stmt],
    range: TextRange,
}

impl ConditionalBranch<'_> {
    fn range_through_body(self) -> TextRange {
        range_through_body(self.range, self.body)
    }
}

fn constant_truthiness(expression: &Expr) -> Option<bool> {
    // Vulture evaluates an unknown expression as true when trying to prove a
    // condition false, and as false when trying to prove it true. This also
    // lets short-circuiting `and`, `or`, and `not` expose constant branches.
    if !safe_truthiness(expression, true) {
        Some(false)
    } else if safe_truthiness(expression, false) {
        Some(true)
    } else {
        None
    }
}

fn safe_truthiness(expression: &Expr, unknown: bool) -> bool {
    match expression {
        Expr::BoolOp(boolean) => match boolean.op {
            BoolOp::And => boolean
                .values
                .iter()
                .all(|value| safe_truthiness(value, unknown)),
            BoolOp::Or => boolean
                .values
                .iter()
                .any(|value| safe_truthiness(value, unknown)),
        },
        Expr::UnaryOp(unary) if unary.op == UnaryOp::Not => {
            !safe_truthiness(&unary.operand, !unknown)
        }
        Expr::UnaryOp(unary)
            if matches!(unary.op, UnaryOp::UAdd | UnaryOp::USub)
                && matches!(unary.operand.as_ref(), Expr::NumberLiteral(_)) =>
        {
            Truthiness::from_expr(&unary.operand, |_| false)
                .into_bool()
                .unwrap_or(unknown)
        }
        _ => Truthiness::from_expr(expression, |_| false)
            .into_bool()
            .unwrap_or(unknown),
    }
}

fn range_through_body(header: TextRange, body: &[Stmt]) -> TextRange {
    TextRange::new(
        header.start(),
        body.last().map_or(header.end(), Ranged::end),
    )
}

fn suite_range(body: &[Stmt]) -> Option<TextRange> {
    Some(TextRange::new(body.first()?.start(), body.last()?.end()))
}

fn unreachable_branches_range(branches: &[ConditionalBranch<'_>]) -> Option<TextRange> {
    let first = branches.first()?;
    let last = branches.last()?;

    // CPython represents `elif` as a nested if. Match Vulture's ranges by
    // retaining the `elif` header while omitting a plain `else` header.
    let start = if first.test.is_some() {
        first.range.start()
    } else {
        first.body.first()?.start()
    };
    let end = last.body.last().map_or(last.range.end(), Ranged::end);
    Some(TextRange::new(start, end))
}

fn contains_break_for_current_loop(body: &[Stmt]) -> bool {
    body.iter().any(|statement| match statement {
        Stmt::Break(_) => true,

        // Loop control in any of these scopes cannot exit the current loop.
        Stmt::FunctionDef(_) | Stmt::ClassDef(_) | Stmt::For(_) | Stmt::While(_) => false,

        Stmt::If(if_statement) => {
            contains_break_for_current_loop(&if_statement.body)
                || if_statement
                    .elif_else_clauses
                    .iter()
                    .any(|clause| contains_break_for_current_loop(&clause.body))
        }
        Stmt::With(with_statement) => contains_break_for_current_loop(&with_statement.body),
        Stmt::Match(match_statement) => match_statement
            .cases
            .iter()
            .any(|case| contains_break_for_current_loop(&case.body)),
        Stmt::Try(try_statement) => {
            contains_break_for_current_loop(&try_statement.body)
                || try_statement.handlers.iter().any(|handler| {
                    let ExceptHandler::ExceptHandler(handler) = handler;
                    contains_break_for_current_loop(&handler.body)
                })
                || contains_break_for_current_loop(&try_statement.orelse)
                || contains_break_for_current_loop(&try_statement.finalbody)
        }
        Stmt::Return(_)
        | Stmt::Delete(_)
        | Stmt::TypeAlias(_)
        | Stmt::Assign(_)
        | Stmt::AugAssign(_)
        | Stmt::AnnAssign(_)
        | Stmt::Raise(_)
        | Stmt::Assert(_)
        | Stmt::Import(_)
        | Stmt::ImportFrom(_)
        | Stmt::Global(_)
        | Stmt::Nonlocal(_)
        | Stmt::Expr(_)
        | Stmt::Pass(_)
        | Stmt::Continue(_)
        | Stmt::IpyEscapeCommand(_) => false,
    })
}

fn statement_name(statement: &Stmt) -> &'static str {
    match statement {
        Stmt::Return(_) => "return",
        Stmt::Raise(_) => "raise",
        Stmt::Break(_) => "break",
        Stmt::Continue(_) => "continue",
        Stmt::If(_) => "if",
        Stmt::While(_) => "while",
        Stmt::Try(_) => "try",
        Stmt::Match(_) => "match",
        Stmt::FunctionDef(_)
        | Stmt::ClassDef(_)
        | Stmt::Delete(_)
        | Stmt::TypeAlias(_)
        | Stmt::Assign(_)
        | Stmt::AugAssign(_)
        | Stmt::AnnAssign(_)
        | Stmt::For(_)
        | Stmt::With(_)
        | Stmt::Assert(_)
        | Stmt::Import(_)
        | Stmt::ImportFrom(_)
        | Stmt::Global(_)
        | Stmt::Nonlocal(_)
        | Stmt::Expr(_)
        | Stmt::Pass(_)
        | Stmt::IpyEscapeCommand(_) => "statement",
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;
    use ruff_python_parser::parse_module;

    use super::{Unreachable, analyze};

    fn findings(source: &str) -> Vec<Unreachable> {
        let parsed = parse_module(source).expect("test source should parse");
        analyze(parsed.suite())
    }

    fn covered_source<'a>(source: &'a str, finding: &Unreachable) -> &'a str {
        &source[usize::from(finding.range.start())..usize::from(finding.range.end())]
    }

    #[test]
    fn reports_one_range_for_a_dead_tail() {
        let source = "def answer():\n    return 42\n    first = 1\n    second = 2\n";
        let findings = findings(source);

        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].name, "return");
        assert_eq!(findings[0].message, "unreachable code after 'return'");
        assert_eq!(
            covered_source(source, &findings[0]),
            "first = 1\n    second = 2"
        );
    }

    #[test]
    fn reports_loop_control_tails_recursively() {
        let source = r#"def consume(items):
    for item in items:
        if item:
            continue
            after_continue(item)
        break
        after_break()
    after_loop()
"#;
        let findings = findings(source);

        assert_eq!(
            findings
                .iter()
                .map(|finding| finding.name)
                .collect::<Vec<_>>(),
            ["continue", "break"]
        );
        assert_eq!(covered_source(source, &findings[0]), "after_continue(item)");
        assert_eq!(covered_source(source, &findings[1]), "after_break()");
    }

    #[test]
    fn nested_loop_break_does_not_exit_outer_loop() {
        let source = r#"while True:
    for item in items:
        break
    consume(item)
after_outer_loop()
"#;
        let findings = findings(source);

        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].name, "while");
        assert_eq!(covered_source(source, &findings[0]), "after_outer_loop()");
    }

    #[test]
    fn reports_constant_if_branches() {
        let source = r#"if False:
    dead_if()
else:
    live_else()

if True:
    live_if()
else:
    dead_else()
"#;
        let findings = findings(source);

        assert_eq!(findings.len(), 2);
        assert_eq!(findings[0].message, "unsatisfiable 'if' condition");
        assert_eq!(
            covered_source(source, &findings[0]),
            "if False:\n    dead_if()"
        );
        assert_eq!(findings[1].message, "unreachable 'else' block");
        assert_eq!(covered_source(source, &findings[1]), "dead_else()");
    }

    #[test]
    fn evaluates_boolean_and_unary_constant_conditions() {
        let source = r#"if not True:
    dead_not()
if False and unknown:
    dead_and()
if True or unknown:
    live_or()
else:
    dead_or_else()
if -0:
    dead_negative_zero()
"#;
        let findings = findings(source);

        assert_eq!(findings.len(), 4);
        assert_eq!(
            findings
                .iter()
                .map(|finding| finding.message.as_str())
                .collect::<Vec<_>>(),
            [
                "unsatisfiable 'if' condition",
                "unsatisfiable 'if' condition",
                "unreachable 'else' block",
                "unsatisfiable 'if' condition",
            ]
        );
    }

    #[test]
    fn all_if_branches_terminal_make_following_code_unreachable() {
        let source = r#"def choose(flag):
    if flag:
        return 1
    else:
        raise ValueError
    dead()
"#;
        let findings = findings(source);

        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].name, "if");
        assert_eq!(covered_source(source, &findings[0]), "dead()");
    }

    #[test]
    fn all_try_paths_terminal_make_following_code_unreachable() {
        let source = r#"def choose():
    try:
        return 1
    except ValueError:
        raise
    dead()
"#;
        let findings = findings(source);

        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].name, "try");
        assert_eq!(covered_source(source, &findings[0]), "dead()");
    }

    #[test]
    fn constant_while_reports_dead_body_and_infinite_tail() {
        let source = r#"while False:
    never()

while True:
    spin()
after_loop()
"#;
        let findings = findings(source);

        assert_eq!(findings.len(), 2);
        assert_eq!(findings[0].message, "unsatisfiable 'while' condition");
        assert_eq!(findings[1].name, "while");
        assert_eq!(covered_source(source, &findings[1]), "after_loop()");
    }
}
