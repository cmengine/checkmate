//! Post-parse validation for the expression-level rules of Appendix A §A.3
//! that the parser's local recursive descent cannot see. The validator walks
//! the complete statement forest — every expression owned by every statement
//! at any nesting depth (function bodies, if/else branches, while bodies,
//! plain blocks, return values, conditions) and every call argument inside
//! those expressions — so a violation anywhere in the program is reported.
//!
//! It runs exactly once, after the whole program has parsed
//! ([`Parser::parse_program_with_errors`](crate::parser::Parser)), so each
//! violation produces exactly one diagnostic.

use cme_core::Span;
use cme_core::ast::{BinaryOp, Block, CallArg, Expr, ExprKind, Stmt, StmtKind};

use crate::diagnostics::Diagnostic;

pub fn validate_statements(statements: &[Stmt]) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    for statement in statements {
        validate_statement(statement, &mut diagnostics);
    }
    diagnostics
}

fn validate_statement(statement: &Stmt, diagnostics: &mut Vec<Diagnostic>) {
    match &statement.kind {
        StmtKind::VarDecl { expr, .. }
        | StmtKind::Assign { expr, .. }
        | StmtKind::CompoundAssign { expr, .. }
        | StmtKind::Expression { expr } => validate_expression(expr, diagnostics),
        StmtKind::Return { value } => {
            if let Some(expr) = value {
                validate_expression(expr, diagnostics);
            }
        }
        StmtKind::If {
            cond,
            then_branch,
            else_branch,
        } => {
            validate_expression(cond, diagnostics);
            validate_block(then_branch, diagnostics);
            if let Some(else_stmt) = else_branch {
                validate_statement(else_stmt, diagnostics);
            }
        }
        StmtKind::While { cond, body } => {
            validate_expression(cond, diagnostics);
            validate_block(body, diagnostics);
        }
        StmtKind::FuncDecl { body, .. } => validate_block(body, diagnostics),
        StmtKind::ImplDecl { members, .. } => {
            // Every member is a function declaration (§10.4): its body's
            // expression-level rules (§A.3) apply like any function's.
            for member in members {
                validate_statement(member, diagnostics);
            }
        }
        StmtKind::StructDecl { .. } | StmtKind::EnumDecl { .. } => {}
        StmtKind::For { iterable, body, .. } => {
            validate_expression(iterable, diagnostics);
            validate_block(body, diagnostics);
        }
        StmtKind::Match { scrutinee, arms } => {
            validate_expression(scrutinee, diagnostics);
            for arm in arms {
                validate_block(&arm.body, diagnostics);
            }
        }
        StmtKind::Block(block) => validate_block(block, diagnostics),
        // An import declares no executable code (§2.3): nothing to validate.
        StmtKind::Import { .. } => {}
        StmtKind::Invalid { .. } => {}
    }
}

fn validate_block(block: &Block, diagnostics: &mut Vec<Diagnostic>) {
    for statement in &block.stmts {
        validate_statement(statement, diagnostics);
    }
}

fn validate_expression(expr: &Expr, diagnostics: &mut Vec<Diagnostic>) {
    match &expr.kind {
        ExprKind::Paren { expr } => validate_expression(expr, diagnostics),
        ExprKind::Unary { expr, .. } => validate_expression(expr, diagnostics),
        ExprKind::Call { args, .. } => {
            for arg in args {
                match arg {
                    CallArg::Positional(expr) | CallArg::Named { expr, .. } => {
                        validate_expression(expr, diagnostics);
                    }
                }
            }
        }
        ExprKind::VariantCall { args, .. } => {
            for arg in args {
                match arg {
                    CallArg::Positional(expr) | CallArg::Named { expr, .. } => {
                        validate_expression(expr, diagnostics);
                    }
                }
            }
        }
        ExprKind::PathCall { args, .. } => {
            for arg in args {
                match arg {
                    CallArg::Positional(expr) | CallArg::Named { expr, .. } => {
                        validate_expression(expr, diagnostics);
                    }
                }
            }
        }
        ExprKind::Match { scrutinee, arms } => {
            validate_expression(scrutinee, diagnostics);
            for arm in arms {
                validate_expression(&arm.body, diagnostics);
            }
        }
        ExprKind::ArrayLit { elements } => {
            for element in elements {
                validate_expression(element, diagnostics);
            }
        }
        ExprKind::MapLit { entries } => {
            for (key, value) in entries {
                validate_expression(key, diagnostics);
                validate_expression(value, diagnostics);
            }
        }
        ExprKind::Interpolated { parts } => {
            for part in parts {
                if let cme_core::ast::InterpPart::Expr(expr) = part {
                    validate_expression(expr, diagnostics);
                }
            }
        }
        ExprKind::Try { expr } => validate_expression(expr, diagnostics),
        ExprKind::Field { obj, .. } => validate_expression(obj, diagnostics),
        ExprKind::Index { obj, index } => {
            validate_expression(obj, diagnostics);
            validate_expression(index, diagnostics);
        }
        ExprKind::Binary { op, lhs, rhs } => {
            match op {
                BinaryOp::And => {
                    check_operand(lhs, BinaryOp::Or, diagnostics);
                    check_operand(rhs, BinaryOp::Or, diagnostics);
                }
                BinaryOp::Or => {
                    check_operand(lhs, BinaryOp::And, diagnostics);
                    check_operand(rhs, BinaryOp::And, diagnostics);
                }
                _ => {}
            }
            validate_expression(lhs, diagnostics);
            validate_expression(rhs, diagnostics);
        }
        _ => {}
    }
}

fn check_operand(expr: &Expr, forbidden: BinaryOp, diagnostics: &mut Vec<Diagnostic>) {
    if contains_unparenthesized(expr, forbidden) {
        diagnostics.push(Diagnostic::parse(
            "mixed && and || require parentheses",
            expr.span,
        ));
    }
}

fn contains_unparenthesized(expr: &Expr, op: BinaryOp) -> bool {
    match &expr.kind {
        ExprKind::Paren { .. } => false,
        ExprKind::Binary { op: binary_op, .. } => *binary_op == op,
        ExprKind::Unary { expr, .. } => contains_unparenthesized(expr, op),
        _ => false,
    }
}

pub fn span_of(expr: &Expr) -> Span {
    expr.span
}
