use cme_core::Span;
use cme_core::ast::{BinaryOp, Expr, ExprKind, Stmt, StmtKind};

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
        | StmtKind::CompoundAssign { expr, .. } => validate_expression(expr, diagnostics),
        StmtKind::Invalid { .. } => {}
    }
}

fn validate_expression(expr: &Expr, diagnostics: &mut Vec<Diagnostic>) {
    match &expr.kind {
        ExprKind::Paren { expr } => validate_expression(expr, diagnostics),
        ExprKind::Unary { expr, .. } => validate_expression(expr, diagnostics),
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
