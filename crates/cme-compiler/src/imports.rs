//! File-scoped import bindings. The front end keeps source spelling in its
//! AST; this pass resolves it to the canonical names used by the checker and
//! interpreter while preserving every original span.

use std::collections::{HashMap, HashSet};

use cme_core::Span;
use cme_core::ast::{
    Block, CallArg, Expr, ExprKind, ImportBinding, InterpPart, LValue, Pattern, Stmt, StmtKind,
    Type,
};
use cme_core::schema::SchemaItem;

use crate::diagnostics::Diagnostic;
use crate::mods::ModuleRange;
use crate::schema::SchemaContext;

pub struct Resolution {
    pub statements: Vec<Stmt>,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Clone)]
enum Binding {
    Alias(Vec<String>),
    Member(Vec<String>),
}

#[derive(Default)]
struct Scope {
    imports: HashSet<Vec<String>>,
    bindings: HashMap<String, Binding>,
    origins: HashMap<String, String>,
}

struct Resolver<'a> {
    ranges: &'a [ModuleRange],
    schema: Option<&'a SchemaContext>,
    declarations: HashMap<String, usize>,
    scopes: Vec<Scope>,
    diagnostics: Vec<Diagnostic>,
}

pub fn resolve(
    statements: &[Stmt],
    ranges: &[ModuleRange],
    schema: Option<&SchemaContext>,
) -> Resolution {
    let mut resolver = Resolver {
        ranges,
        schema,
        declarations: HashMap::new(),
        scopes: (0..ranges.len().max(1)).map(|_| Scope::default()).collect(),
        diagnostics: Vec::new(),
    };
    for statement in statements {
        if let Some(name) = declaration_name(statement) {
            let owner = resolver.owner(statement.span);
            resolver
                .declarations
                .entry(name.to_string())
                .or_insert(owner);
            resolver.scopes[owner]
                .origins
                .insert(name.to_string(), format!("declaration `{name}`"));
        }
    }
    for scope in &mut resolver.scopes {
        for name in ["option", "result", "map", "Some", "None", "Ok", "Err"] {
            scope
                .origins
                .entry(name.into())
                .or_insert_with(|| format!("built-in `{name}`"));
        }
        if let Some(schema) = schema {
            for file in schema.set.namespaces() {
                if schema.target(&file.namespace).is_none() {
                    continue;
                }
                scope
                    .origins
                    .entry(file.namespace.clone())
                    .or_insert_with(|| format!("schema namespace `{}`", file.namespace));
                for item in &file.items {
                    if matches!(item, SchemaItem::Struct(_) | SchemaItem::Enum(_)) {
                        let name = item.name();
                        scope
                            .origins
                            .entry(name.into())
                            .or_insert_with(|| format!("schema type `{}.{name}`", file.namespace));
                    }
                }
            }
        }
    }
    for statement in statements {
        let StmtKind::Import { path, binding } = &statement.kind else {
            continue;
        };
        let owner = resolver.owner(statement.span);
        resolver.scopes[owner].imports.insert(path.clone());
        match binding {
            Some(ImportBinding::Alias(alias)) => {
                resolver.bind(
                    owner,
                    alias,
                    Binding::Alias(path.clone()),
                    path,
                    statement.span,
                );
            }
            Some(ImportBinding::Glob) => {
                if path.first().map(String::as_str) == Some("self") {
                    if let Some(target) = ranges.iter().position(|range| range.module_path == *path)
                    {
                        for declaration in statements {
                            if resolver.owner(declaration.span) == target
                                && let Some(name) = declaration_name(declaration)
                            {
                                resolver.bind(
                                    owner,
                                    name,
                                    Binding::Member(path.clone()),
                                    path,
                                    statement.span,
                                );
                            }
                        }
                    }
                } else if let Some(schema) = schema {
                    if path.len() == 2
                        && let Some(version) = schema.target(&path[0])
                        && let Some(file) = schema.set.namespace(&path[0])
                        && let Some(capability) = file.capability(&path[1])
                    {
                        for member in &capability.members {
                            if member.visible_at(version) {
                                resolver.bind(
                                    owner,
                                    &member.name,
                                    Binding::Member(path.clone()),
                                    path,
                                    statement.span,
                                );
                            }
                        }
                    }
                } else {
                    resolver.diagnostics.push(Diagnostic::parse(
                        format!(
                            "`import {} as *` needs a registered schema to enumerate capability members",
                            path.join(".")
                        ),
                        statement.span,
                    ));
                }
            }
            None => {}
        }
    }
    let mut lowered = statements.to_vec();
    for statement in &mut lowered {
        let owner = resolver.owner(statement.span);
        resolver.statement(statement, owner);
    }
    Resolution {
        statements: lowered,
        diagnostics: resolver.diagnostics,
    }
}

fn declaration_name(statement: &Stmt) -> Option<&str> {
    match &statement.kind {
        StmtKind::FuncDecl { name, .. }
        | StmtKind::StructDecl { name, .. }
        | StmtKind::EnumDecl { name, .. } => Some(name),
        _ => None,
    }
}

impl Resolver<'_> {
    fn owner(&self, span: Span) -> usize {
        self.ranges
            .iter()
            .position(|range| range.start <= span.start && span.start < range.end)
            .unwrap_or(0)
    }

    fn bind(&mut self, owner: usize, name: &str, binding: Binding, path: &[String], span: Span) {
        if matches!(name, "self" | "cm") {
            self.diagnostics.push(Diagnostic::parse(
                format!("`{name}` is a reserved namespace root and cannot be an import binding"),
                span,
            ));
            return;
        }
        if let Some(previous) = self.scopes[owner].origins.get(name) {
            self.diagnostics.push(Diagnostic::parse(
                format!(
                    "import binding `{name}` from `{}` collides with {previous} in this file",
                    path.join(".")
                ),
                span,
            ));
            return;
        }
        self.scopes[owner]
            .origins
            .insert(name.to_string(), format!("import `{}`", path.join(".")));
        self.scopes[owner]
            .bindings
            .insert(name.to_string(), binding);
    }

    fn need_import(&mut self, name: &str, owner: usize, span: Span) {
        if let Some(target) = self.declarations.get(name)
            && *target != owner
            && !matches!(
                self.scopes[owner].bindings.get(name),
                Some(Binding::Member(_))
            )
        {
            let source = self
                .ranges
                .get(*target)
                .map(|range| range.module_path.join("."));
            if let Some(source) = source {
                let imported = self.scopes[owner]
                    .imports
                    .iter()
                    .any(|path| path.join(".") == source);
                self.diagnostics.push(Diagnostic::parse(
                    if imported {
                        format!(
                            "`{name}` is declared in `{source}`; use `{source}.{name}` or import it with `as *`"
                        )
                    } else {
                        format!("`{name}` is declared in `{source}`; import that module in this file")
                    },
                    span,
                ));
            }
        }
    }

    fn expand_path(&mut self, path: &[String], owner: usize, span: Span) -> Vec<String> {
        let expanded =
            if let Some(Binding::Alias(prefix)) = self.scopes[owner].bindings.get(&path[0]) {
                prefix
                    .iter()
                    .cloned()
                    .chain(path[1..].iter().cloned())
                    .collect::<Vec<_>>()
            } else {
                path.to_vec()
            };
        if expanded.first().map(String::as_str) == Some("self") {
            if let Some(target) = self
                .ranges
                .iter()
                .filter(|range| {
                    expanded.starts_with(&range.module_path)
                        && expanded.len() > range.module_path.len()
                })
                .max_by_key(|range| range.module_path.len())
                && !self.scopes[owner].imports.contains(&target.module_path)
            {
                self.diagnostics.push(Diagnostic::parse(
                    format!(
                        "`{}` needs `import {}` in this file",
                        path.join("."),
                        target.module_path.join(".")
                    ),
                    span,
                ));
            }
        } else if expanded.len() >= 3 && self.schema.is_some() {
            let capability = expanded[..2].to_vec();
            if !self.scopes[owner].imports.contains(&capability) {
                self.diagnostics.push(Diagnostic::parse(
                    format!(
                        "`{}` needs `import {}` in this file",
                        path.join("."),
                        capability.join(".")
                    ),
                    span,
                ));
            }
        }
        expanded
    }

    fn self_symbol(&mut self, path: &[String], span: Span) -> Option<Vec<String>> {
        let (target_index, target) = self
            .ranges
            .iter()
            .enumerate()
            .filter(|(_, range)| {
                path.starts_with(&range.module_path) && path.len() > range.module_path.len()
            })
            .max_by_key(|(_, range)| range.module_path.len())?;
        let suffix = path[target.module_path.len()..].to_vec();
        let name = &suffix[0];
        if self.declarations.get(name).copied() != Some(target_index) {
            self.diagnostics.push(Diagnostic::parse(
                format!(
                    "`{name}` is not declared in `{}`",
                    target.module_path.join(".")
                ),
                span,
            ));
        }
        Some(suffix)
    }

    fn ty(&mut self, ty: &mut Type, owner: usize, span: Span) {
        match ty {
            Type::Named { name, args } => {
                if name.contains('.') {
                    let path: Vec<String> = name.split('.').map(str::to_string).collect();
                    let expanded = self.expand_path(&path, owner, span);
                    if expanded.first().map(String::as_str) == Some("self")
                        && let Some(suffix) = self.self_symbol(&expanded, span)
                        && suffix.len() == 1
                    {
                        *name = suffix[0].clone();
                    }
                } else {
                    self.need_import(name, owner, span);
                }
                for arg in args {
                    self.ty(arg, owner, span);
                }
            }
            Type::Array(element) => self.ty(element, owner, span),
            Type::Map { key, value } => {
                self.ty(key, owner, span);
                self.ty(value, owner, span);
            }
            _ => {}
        }
    }

    fn args(&mut self, args: &mut [CallArg], owner: usize) {
        for arg in args {
            match arg {
                CallArg::Positional(expr) | CallArg::Named { expr, .. } => self.expr(expr, owner),
            }
        }
    }

    fn expr(&mut self, expr: &mut Expr, owner: usize) {
        let span = expr.span;
        match &mut expr.kind {
            ExprKind::Call { name, args } => {
                self.args(args, owner);
                match self.scopes[owner].bindings.get(name) {
                    Some(Binding::Member(prefix))
                        if prefix.first().map(String::as_str) != Some("self") =>
                    {
                        let mut path = prefix.clone();
                        path.push(name.clone());
                        expr.kind = ExprKind::PathCall {
                            path,
                            args: std::mem::take(args),
                        };
                    }
                    Some(Binding::Member(_)) => {}
                    _ => self.need_import(name, owner, span),
                }
            }
            ExprKind::VariantCall {
                enum_name,
                variant,
                args,
            } => {
                self.args(args, owner);
                let head = enum_name.clone();
                if let Some(Binding::Alias(prefix)) = self.scopes[owner].bindings.get(&head) {
                    let mut path = prefix.clone();
                    path.push(variant.clone());
                    if path.first().map(String::as_str) == Some("self") {
                        if let Some(suffix) = self.self_symbol(&path, span)
                            && suffix.len() == 1
                        {
                            expr.kind = ExprKind::Call {
                                name: suffix[0].clone(),
                                args: std::mem::take(args),
                            };
                        }
                    } else {
                        expr.kind = ExprKind::PathCall {
                            path,
                            args: std::mem::take(args),
                        };
                    }
                } else {
                    self.need_import(&head, owner, span);
                }
            }
            ExprKind::PathCall { path, args } => {
                self.args(args, owner);
                let expanded = self.expand_path(path, owner, span);
                if expanded.first().map(String::as_str) == Some("self") {
                    if let Some(suffix) = self.self_symbol(&expanded, span) {
                        match suffix.as_slice() {
                            [name] => {
                                expr.kind = ExprKind::Call {
                                    name: name.clone(),
                                    args: std::mem::take(args),
                                }
                            }
                            [name, member] => {
                                expr.kind = ExprKind::VariantCall {
                                    enum_name: name.clone(),
                                    variant: member.clone(),
                                    args: std::mem::take(args),
                                }
                            }
                            _ => {}
                        }
                    }
                } else {
                    *path = expanded;
                }
            }
            ExprKind::Binary { lhs, rhs, .. } => {
                self.expr(lhs, owner);
                self.expr(rhs, owner);
            }
            ExprKind::Unary { expr, .. } | ExprKind::Paren { expr } | ExprKind::Try { expr } => {
                self.expr(expr, owner)
            }
            ExprKind::Field { obj, .. } => self.expr(obj, owner),
            ExprKind::Index { obj, index } => {
                self.expr(obj, owner);
                self.expr(index, owner);
            }
            ExprKind::Match { scrutinee, arms } => {
                self.expr(scrutinee, owner);
                for arm in arms {
                    self.expr(&mut arm.body, owner);
                }
            }
            ExprKind::ArrayLit { elements } => {
                for element in elements {
                    self.expr(element, owner);
                }
            }
            ExprKind::MapLit { entries } => {
                for (key, value) in entries {
                    self.expr(key, owner);
                    self.expr(value, owner);
                }
            }
            ExprKind::Interpolated { parts } => {
                for part in parts {
                    if let InterpPart::Expr(value) = part {
                        self.expr(value, owner);
                    }
                }
            }
            _ => {}
        }
    }

    fn lvalue(&mut self, target: &mut LValue, owner: usize) {
        match target {
            LValue::Var { .. } => {}
            LValue::Field { base, .. } => self.lvalue(base, owner),
            LValue::Index { base, index } => {
                self.lvalue(base, owner);
                self.expr(index, owner);
            }
        }
    }

    fn block(&mut self, block: &mut Block, owner: usize) {
        for statement in &mut block.stmts {
            self.statement(statement, owner);
        }
    }

    fn statement(&mut self, statement: &mut Stmt, owner: usize) {
        let span = statement.span;
        match &mut statement.kind {
            StmtKind::VarDecl { ty, expr, .. } => {
                self.ty(ty, owner, span);
                self.expr(expr, owner);
            }
            StmtKind::Assign { target, expr } | StmtKind::CompoundAssign { target, expr, .. } => {
                self.lvalue(target, owner);
                self.expr(expr, owner);
            }
            StmtKind::Expression { expr } => self.expr(expr, owner),
            StmtKind::FuncDecl {
                params,
                return_ty,
                body,
                ..
            } => {
                for param in params {
                    self.ty(&mut param.ty, owner, span);
                }
                self.ty(return_ty, owner, span);
                self.block(body, owner);
            }
            StmtKind::StructDecl { fields, .. } => {
                for field in fields {
                    self.ty(&mut field.ty, owner, span);
                }
            }
            StmtKind::EnumDecl { variants, .. } => {
                for variant in variants {
                    for field in &mut variant.fields {
                        self.ty(&mut field.ty, owner, span);
                    }
                }
            }
            StmtKind::ImplDecl { target, members } => {
                if target.len() == 1 {
                    self.need_import(&target[0], owner, span);
                } else {
                    let expanded = self.expand_path(target, owner, span);
                    if expanded.first().map(String::as_str) == Some("self") {
                        if let Some(suffix) = self.self_symbol(&expanded, span)
                            && suffix.len() == 1
                        {
                            *target = suffix;
                        }
                    } else {
                        *target = expanded;
                    }
                }
                for member in members {
                    self.statement(member, owner);
                }
            }
            StmtKind::If {
                cond,
                then_branch,
                else_branch,
            } => {
                self.expr(cond, owner);
                self.block(then_branch, owner);
                if let Some(other) = else_branch {
                    self.statement(other, owner);
                }
            }
            StmtKind::While { cond, body } => {
                self.expr(cond, owner);
                self.block(body, owner);
            }
            StmtKind::For {
                elem_ty,
                iterable,
                body,
                ..
            } => {
                self.ty(elem_ty, owner, span);
                self.expr(iterable, owner);
                self.block(body, owner);
            }
            StmtKind::Match { scrutinee, arms } => {
                self.expr(scrutinee, owner);
                for arm in arms {
                    if let Pattern::Variant { bindings, .. } = &mut arm.pattern {
                        for field in bindings {
                            self.ty(&mut field.ty, owner, span);
                        }
                    }
                    self.block(&mut arm.body, owner);
                }
            }
            StmtKind::Return { value } => {
                if let Some(value) = value {
                    self.expr(value, owner);
                }
            }
            StmtKind::Block(block) => self.block(block, owner),
            StmtKind::Import { .. } | StmtKind::Invalid { .. } => {}
        }
    }
}
