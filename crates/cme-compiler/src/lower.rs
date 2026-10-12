//! Lower a checked, import-resolved program into typed register control flow.
//! No execution backend redoes name resolution or walks expression trees.

use std::collections::HashMap;

use cme_core::ast::*;
use cme_core::ir::{self, Access, BlockId, BoolContext, Exit, ExitKind, Literal, Op, Register};

use crate::Diagnostic;
use crate::check::CheckedProgram;

/// Lowers only a diagnostic-free checked program. Supplemental declarations
/// are the active schema's boundary types, synthesized by `schema`.
pub fn lower(
    checked: &CheckedProgram,
    declarations: &[Stmt],
) -> Result<ir::Program, Vec<Diagnostic>> {
    if !checked.diagnostics().is_empty() {
        return Err(checked.diagnostics().to_vec());
    }
    if let Some(stmt) = checked
        .statements()
        .iter()
        .find(|stmt| stmt.contains_invalid())
    {
        return Err(vec![Diagnostic::type_error(
            "cannot lower a program containing invalid nodes",
            stmt.span,
        )]);
    }
    let registry = Registry::new(checked, declarations);
    let mut functions = Vec::with_capacity(registry.functions.len());
    for (target, stmt) in &registry.functions {
        let StmtKind::FuncDecl {
            name,
            params,
            return_ty,
            body,
        } = &stmt.kind
        else {
            unreachable!()
        };
        let display = target
            .as_ref()
            .map_or_else(|| name.clone(), |target| format!("{target}.{name}"));
        let mut builder = Builder {
            registry: &registry,
            function: ir::Function {
                name: name.clone(),
                target: target.clone(),
                params: params.clone(),
                return_ty: return_ty.clone(),
                registers: Vec::new(),
                blocks: Vec::new(),
                span: stmt.span,
            },
            active: None,
            scopes: vec![HashMap::new()],
        };
        builder.active = Some(builder.block(stmt.span));
        for param in params {
            let reg = builder.reg(param.ty.clone());
            builder.scopes[0].insert(param.name.clone(), reg);
        }
        builder.stmts(&body.stmts).map_err(|error| vec![error])?;
        if builder.active.is_some() {
            if *return_ty == Type::Void {
                let value = builder.constant(Literal::Void, Type::Void, body.span);
                builder.exit(ExitKind::Return(value), body.span);
            } else {
                builder.exit(
                    ExitKind::Fail(format!(
                        "non-void function `{display}` fell off the end without returning a value"
                    )),
                    stmt.span,
                );
            }
        }
        functions.push(builder.function);
    }
    Ok(ir::Program { functions })
}

/// Lower and generate verified bytecode through one diagnostic-preserving gate.
pub fn compile(
    checked: &CheckedProgram,
    declarations: &[Stmt],
) -> Result<cme_interp::bytecode::Bytecode, Vec<Diagnostic>> {
    let ir = lower(checked, declarations)?;
    cme_interp::bytecode::Bytecode::compile(&ir)
        .map_err(|error| vec![Diagnostic::type_error(error.message, error.span)])
}

struct Registry<'a> {
    checked: &'a CheckedProgram,
    functions: Vec<(Option<String>, &'a Stmt)>,
    calls: HashMap<(Option<String>, String), usize>,
    structs: HashMap<String, &'a [FieldDef]>,
    enums: HashMap<String, &'a [VariantDecl]>,
}

impl<'a> Registry<'a> {
    fn new(checked: &'a CheckedProgram, declarations: &'a [Stmt]) -> Self {
        let mut registry = Self {
            checked,
            functions: Vec::new(),
            calls: HashMap::new(),
            structs: HashMap::new(),
            enums: HashMap::new(),
        };
        for stmt in checked.statements().iter().chain(declarations) {
            match &stmt.kind {
                StmtKind::FuncDecl { name, .. } => registry.add_function(None, name, stmt),
                StmtKind::ImplDecl { target, members } => {
                    for member in members {
                        if let StmtKind::FuncDecl { name, .. } = &member.kind {
                            registry.add_function(Some(target.join(".")), name, member);
                        }
                    }
                }
                StmtKind::StructDecl { name, fields, .. } => {
                    registry.structs.entry(name.clone()).or_insert(fields);
                }
                StmtKind::EnumDecl { name, variants, .. } => {
                    registry.enums.entry(name.clone()).or_insert(variants);
                }
                _ => {}
            }
        }
        registry
    }

    fn add_function(&mut self, target: Option<String>, name: &str, stmt: &'a Stmt) {
        let key = (target.clone(), name.to_string());
        if !self.calls.contains_key(&key) {
            self.calls.insert(key, self.functions.len());
            self.functions.push((target, stmt));
        }
    }
}

struct Builder<'a, 'r> {
    registry: &'r Registry<'a>,
    function: ir::Function,
    active: Option<BlockId>,
    scopes: Vec<HashMap<String, Register>>,
}

type LowerResult<T> = Result<T, Diagnostic>;

impl Builder<'_, '_> {
    fn ty(&self, expr: &Expr) -> Type {
        self.registry
            .checked
            .expression_type(expr)
            .cloned()
            .unwrap_or(Type::Infer)
    }

    fn reg(&mut self, ty: Type) -> Register {
        let reg = self.function.registers.len();
        self.function.registers.push(ty);
        reg
    }

    fn block(&mut self, span: Span) -> BlockId {
        let id = self.function.blocks.len();
        self.function.blocks.push(ir::Block {
            instructions: Vec::new(),
            exit: Exit {
                kind: ExitKind::Fail("unreachable block".into()),
                span,
            },
        });
        id
    }

    fn emit(&mut self, op: Op, span: Span) {
        self.function.blocks[self.active.expect("active lowering block")]
            .instructions
            .push(ir::Instruction { op, span });
    }

    fn exit(&mut self, kind: ExitKind, span: Span) {
        if let Some(id) = self.active.take() {
            self.function.blocks[id].exit = Exit { kind, span };
        }
    }

    fn jump(&mut self, target: BlockId, span: Span) {
        self.exit(ExitKind::Jump(target), span);
    }

    fn constant(&mut self, value: Literal, ty: Type, span: Span) -> Register {
        let dst = self.reg(ty);
        self.emit(Op::Constant { dst, value }, span);
        dst
    }

    fn coerce(&mut self, src: Register, ty: &Type, span: Span) -> Register {
        // Nominal types (including a generic field's type parameter) do
        // not convert runtime representations in the reference walker.
        // Do not emit a symbolic/unresolved coercion into typed bytecode.
        if !converts_scalar(ty) {
            return src;
        }
        let dst = self.reg(ty.clone());
        self.emit(
            Op::Coerce {
                dst,
                src,
                ty: ty.clone(),
            },
            span,
        );
        dst
    }

    fn lookup(&self, name: &str, span: Span) -> LowerResult<Register> {
        self.scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name).copied())
            .ok_or_else(|| {
                Diagnostic::type_error(format!("unknown name `{name}` during lowering"), span)
            })
    }

    fn stmts(&mut self, stmts: &[Stmt]) -> LowerResult<()> {
        for stmt in stmts {
            if self.active.is_none() {
                break;
            }
            self.stmt(stmt)?;
        }
        Ok(())
    }

    fn scoped_block(&mut self, block: &Block) -> LowerResult<()> {
        self.scopes.push(HashMap::new());
        let result = self.stmts(&block.stmts);
        self.scopes.pop();
        result
    }

    fn stmt(&mut self, stmt: &Stmt) -> LowerResult<()> {
        let span = stmt.span;
        match &stmt.kind {
            StmtKind::Expression { expr } => {
                self.expr(expr)?;
            }
            StmtKind::VarDecl { ty, name, expr } => {
                let value = self.expr(expr)?;
                let value = self.coerce(value, ty, span);
                let local_ty = if *ty == Type::Infer {
                    self.ty(expr)
                } else {
                    ty.clone()
                };
                let dst = self.reg(local_ty);
                self.emit(Op::Copy { dst, src: value }, span);
                self.scopes.last_mut().unwrap().insert(name.clone(), dst);
            }
            StmtKind::Assign { target, expr } => {
                // Plain assignment evaluates the RHS before the target indices.
                let value = self.expr(expr)?;
                let (base, path) = self.path(target, span)?;
                self.emit(Op::WritePath { base, path, value }, span);
            }
            StmtKind::CompoundAssign { target, op, expr } => {
                // Compound assignment resolves indices once, then reads, evaluates
                // the RHS and writes through the same path.
                let (base, path) = self.path(target, span)?;
                let slot_ty = self
                    .registry
                    .checked
                    .lvalue_type(target)
                    .cloned()
                    .unwrap_or(Type::Infer);
                let current = self.reg(slot_ty.clone());
                self.emit(
                    Op::ReadPath {
                        dst: current,
                        base,
                        path: path.clone(),
                    },
                    span,
                );
                let rhs = self.expr(expr)?;
                let right = self.reg(slot_ty);
                self.emit(
                    Op::ByteLiteral {
                        dst: right,
                        src: rhs,
                        against: current,
                    },
                    span,
                );
                let value = self.reg(self.function.registers[current].clone());
                let op = match op {
                    CompoundOp::Add => BinaryOp::Add,
                    CompoundOp::Sub => BinaryOp::Sub,
                    CompoundOp::Mul => BinaryOp::Mul,
                    CompoundOp::Div => BinaryOp::Div,
                    CompoundOp::Rem => BinaryOp::Rem,
                };
                self.emit(
                    Op::Binary {
                        dst: value,
                        op,
                        left: current,
                        right,
                    },
                    span,
                );
                self.emit(Op::WritePath { base, path, value }, span);
            }
            StmtKind::Return { value } => {
                let value = match value {
                    Some(expr) => self.expr(expr)?,
                    None => self.constant(Literal::Void, Type::Void, span),
                };
                self.exit(ExitKind::Return(value), span);
            }
            StmtKind::Block(block) => self.scoped_block(block)?,
            StmtKind::If {
                cond,
                then_branch,
                else_branch,
            } => {
                let condition = self.expr(cond)?;
                let yes = self.block(span);
                let no = self.block(span);
                let done = self.block(span);
                self.exit(
                    ExitKind::Branch {
                        cond: condition,
                        yes,
                        no,
                        context: BoolContext::If,
                    },
                    cond.span,
                );
                self.active = Some(yes);
                self.scoped_block(then_branch)?;
                self.jump(done, span);
                self.active = Some(no);
                if let Some(other) = else_branch {
                    self.stmt(other)?;
                }
                self.jump(done, span);
                self.active = Some(done);
            }
            StmtKind::While { cond, body } => {
                let test = self.block(cond.span);
                let step = self.block(body.span);
                let done = self.block(span);
                self.jump(test, span);
                self.active = Some(test);
                let condition = self.expr(cond)?;
                self.exit(
                    ExitKind::Branch {
                        cond: condition,
                        yes: step,
                        no: done,
                        context: BoolContext::While,
                    },
                    cond.span,
                );
                self.active = Some(step);
                self.scoped_block(body)?;
                self.jump(test, span);
                self.active = Some(done);
            }
            StmtKind::For {
                elem_ty,
                elem_name,
                iterable,
                body,
            } => {
                let source = self.expr(iterable)?;
                let collection = self.reg(Type::Array(Box::new(elem_ty.clone())));
                self.emit(
                    Op::Iterate {
                        dst: collection,
                        collection: source,
                    },
                    iterable.span,
                );
                let int = Type::Prim(PrimitiveType::Int);
                let index = self.constant(Literal::Int(0), int.clone(), span);
                let one = self.constant(Literal::Int(1), int.clone(), span);
                let length = self.reg(int.clone());
                self.emit(
                    Op::Field {
                        dst: length,
                        obj: collection,
                        name: "length".into(),
                    },
                    iterable.span,
                );
                let test = self.block(span);
                let step = self.block(body.span);
                let done = self.block(span);
                self.jump(test, span);
                self.active = Some(test);
                let condition = self.reg(Type::Prim(PrimitiveType::Bool));
                self.emit(
                    Op::Binary {
                        dst: condition,
                        op: BinaryOp::Lt,
                        left: index,
                        right: length,
                    },
                    span,
                );
                self.exit(
                    ExitKind::Branch {
                        cond: condition,
                        yes: step,
                        no: done,
                        context: BoolContext::Internal,
                    },
                    span,
                );
                self.active = Some(step);
                let raw = self.reg(elem_ty.clone());
                self.emit(
                    Op::Index {
                        dst: raw,
                        obj: collection,
                        index,
                    },
                    iterable.span,
                );
                let element = self.coerce(raw, elem_ty, iterable.span);
                self.scopes
                    .push(HashMap::from([(elem_name.clone(), element)]));
                self.stmts(&body.stmts)?;
                self.scopes.pop();
                if self.active.is_some() {
                    self.emit(
                        Op::Binary {
                            dst: index,
                            op: BinaryOp::Add,
                            left: index,
                            right: one,
                        },
                        span,
                    );
                    self.jump(test, span);
                }
                self.active = Some(done);
            }
            StmtKind::Match { scrutinee, arms } => {
                let value = self.expr(scrutinee)?;
                let done = self.block(span);
                for arm in arms {
                    let next = self.match_arm(value, &arm.pattern, scrutinee.span)?;
                    self.stmts(&arm.body.stmts)?;
                    self.scopes.pop();
                    self.jump(done, span);
                    self.active = Some(next);
                }
                self.exit(
                    ExitKind::Fail("match fell through without a matching arm".into()),
                    span,
                );
                self.active = Some(done);
            }
            _ => {
                return Err(Diagnostic::type_error(
                    "cannot lower an invalid executable statement",
                    span,
                ));
            }
        }
        Ok(())
    }

    fn path(&mut self, target: &LValue, span: Span) -> LowerResult<(Register, Vec<Access>)> {
        // Match the reference walker: resolve the outermost index first,
        // then traverse the base, finally reverse into read/write order.
        let mut ops = Vec::new();
        let mut cursor = target;
        loop {
            match cursor {
                LValue::Var { name } => {
                    ops.reverse();
                    return Ok((self.lookup(name, span)?, ops));
                }
                LValue::Field { base, name } => {
                    ops.push(Access::Field(name.clone()));
                    cursor = base;
                }
                LValue::Index { base, index } => {
                    ops.push(Access::Index(self.expr(index)?));
                    cursor = base;
                }
            }
        }
    }

    fn match_arm(
        &mut self,
        value: Register,
        pattern: &Pattern,
        span: Span,
    ) -> LowerResult<BlockId> {
        let variant = match pattern {
            Pattern::Wildcard => None,
            Pattern::Variant { variant, .. } => Some(variant.clone()),
        };
        let condition = self.reg(Type::Prim(PrimitiveType::Bool));
        self.emit(
            Op::IsVariant {
                dst: condition,
                value,
                variant,
            },
            span,
        );
        let yes = self.block(span);
        let next = self.block(span);
        self.exit(
            ExitKind::Branch {
                cond: condition,
                yes,
                no: next,
                context: BoolContext::Internal,
            },
            span,
        );
        self.active = Some(yes);
        self.scopes.push(HashMap::new());
        if let Pattern::Variant { bindings, .. } = pattern {
            for (index, binding) in bindings.iter().enumerate() {
                let dst = self.reg(binding.ty.clone());
                self.emit(Op::Payload { dst, value, index }, span);
                self.scopes
                    .last_mut()
                    .unwrap()
                    .insert(binding.name.clone(), dst);
            }
        }
        Ok(next)
    }

    fn expr(&mut self, expr: &Expr) -> LowerResult<Register> {
        let span = expr.span;
        let ty = self.ty(expr);
        let dst = self.reg(ty.clone());
        let op = match &expr.kind {
            // Literals are int-shaped until a declared slot or a direct
            // byte arithmetic operand crystallizes them, like the walker.
            ExprKind::IntLit(value) => {
                self.function.registers[dst] = Type::Prim(PrimitiveType::Int);
                Op::Constant {
                    dst,
                    value: Literal::Int(*value),
                }
            }
            ExprKind::FloatLit(value) => Op::Constant {
                dst,
                value: Literal::Float(*value),
            },
            ExprKind::BoolLit(value) => Op::Constant {
                dst,
                value: Literal::Bool(*value),
            },
            ExprKind::StrLit(value) => Op::Constant {
                dst,
                value: Literal::Str(value.clone()),
            },
            ExprKind::Ident(name) => Op::Copy {
                dst,
                src: self.lookup(name, span)?,
            },
            ExprKind::Paren { expr: inner } => Op::Copy {
                dst,
                src: self.expr(inner)?,
            },
            ExprKind::Unary { op, expr: inner } => Op::Unary {
                dst,
                op: *op,
                value: self.expr(inner)?,
            },
            ExprKind::Binary {
                op: BinaryOp::And | BinaryOp::Or,
                lhs,
                rhs,
            } => {
                let left = self.expr(lhs)?;
                let right_block = self.block(rhs.span);
                let short_block = self.block(span);
                let done = self.block(span);
                let is_and = matches!(
                    &expr.kind,
                    ExprKind::Binary {
                        op: BinaryOp::And,
                        ..
                    }
                );
                let (yes, no) = if is_and {
                    (right_block, short_block)
                } else {
                    (short_block, right_block)
                };
                self.exit(
                    ExitKind::Branch {
                        cond: left,
                        yes,
                        no,
                        context: if is_and {
                            BoolContext::AndLeft
                        } else {
                            BoolContext::OrLeft
                        },
                    },
                    span,
                );
                self.active = Some(short_block);
                self.emit(Op::Copy { dst, src: left }, span);
                self.jump(done, span);
                self.active = Some(right_block);
                let right = self.expr(rhs)?;
                self.emit(
                    Op::Bool {
                        dst,
                        value: right,
                        context: if is_and {
                            BoolContext::AndRight
                        } else {
                            BoolContext::OrRight
                        },
                    },
                    span,
                );
                self.jump(done, span);
                self.active = Some(done);
                return Ok(dst);
            }
            ExprKind::Binary { op, lhs, rhs } => {
                let mut left = self.expr(lhs)?;
                let mut right = self.expr(rhs)?;
                if matches!(rhs.kind, ExprKind::IntLit(_)) {
                    let literal = self.reg(self.ty(rhs));
                    self.emit(
                        Op::ByteLiteral {
                            dst: literal,
                            src: right,
                            against: left,
                        },
                        span,
                    );
                    right = literal;
                }
                if matches!(lhs.kind, ExprKind::IntLit(_)) {
                    let literal = self.reg(self.ty(lhs));
                    self.emit(
                        Op::ByteLiteral {
                            dst: literal,
                            src: left,
                            against: right,
                        },
                        span,
                    );
                    left = literal;
                }
                Op::Binary {
                    dst,
                    op: *op,
                    left,
                    right,
                }
            }
            ExprKind::Field { obj, name } => Op::Field {
                dst,
                obj: self.expr(obj)?,
                name: name.clone(),
            },
            ExprKind::Index { obj, index } => {
                let obj = self.expr(obj)?;
                let index = self.expr(index)?;
                Op::Index { dst, obj, index }
            }
            ExprKind::ArrayLit { elements } => Op::Array {
                dst,
                items: elements
                    .iter()
                    .map(|e| self.expr(e))
                    .collect::<LowerResult<_>>()?,
            },
            ExprKind::MapLit { entries } => {
                let mut values = Vec::with_capacity(entries.len());
                for (key, value) in entries {
                    values.push((self.expr(key)?, self.expr(value)?));
                }
                Op::Map {
                    dst,
                    entries: values,
                }
            }
            ExprKind::Interpolated { parts } => {
                self.emit(
                    Op::Constant {
                        dst,
                        value: Literal::Str(String::new()),
                    },
                    span,
                );
                for part in parts {
                    let suffix = match part {
                        InterpPart::Literal(text) => self.constant(
                            Literal::Str(text.clone()),
                            Type::Prim(PrimitiveType::Str),
                            span,
                        ),
                        InterpPart::Expr(island) => {
                            let value = self.expr(island)?;
                            let text = self.reg(Type::Prim(PrimitiveType::Str));
                            self.emit(Op::Stringify { dst: text, value }, island.span);
                            text
                        }
                    };
                    self.emit(
                        Op::Binary {
                            dst,
                            op: BinaryOp::Add,
                            left: dst,
                            right: suffix,
                        },
                        span,
                    );
                }
                return Ok(dst);
            }
            ExprKind::Try { expr: inner } => {
                let value = self.expr(inner)?;
                let next = self.block(span);
                self.exit(ExitKind::Try { value, dst, next }, span);
                self.active = Some(next);
                return Ok(dst);
            }
            ExprKind::Match { scrutinee, arms } => {
                let value = self.expr(scrutinee)?;
                let done = self.block(span);
                for arm in arms {
                    let next = self.match_arm(value, &arm.pattern, scrutinee.span)?;
                    let result = self.expr(&arm.body)?;
                    self.emit(Op::Copy { dst, src: result }, arm.body.span);
                    self.scopes.pop();
                    self.jump(done, span);
                    self.active = Some(next);
                }
                self.exit(
                    ExitKind::Fail("match fell through without a matching arm".into()),
                    span,
                );
                self.active = Some(done);
                return Ok(dst);
            }
            ExprKind::Call { name, args } => {
                return self.call(dst, None, name, args, span);
            }
            ExprKind::VariantCall {
                enum_name,
                variant,
                args,
            } => {
                return self.call(dst, Some(enum_name), variant, args, span);
            }
            ExprKind::PathCall { path, args } => {
                let member = path
                    .last()
                    .ok_or_else(|| Diagnostic::type_error("empty call path", span))?;
                let target = path[..path.len() - 1].join(".");
                if let Some(&function) = self
                    .registry
                    .calls
                    .get(&(Some(target.clone()), member.clone()))
                {
                    let values = self.function_args(function, args)?;
                    Op::Call {
                        dst,
                        function,
                        args: values,
                    }
                } else {
                    Op::Capability {
                        dst,
                        path: path[..path.len() - 1].to_vec(),
                        member: member.clone(),
                        args: self.args(args)?,
                    }
                }
            }
            ExprKind::Invalid { .. } => {
                return Err(Diagnostic::type_error(
                    "cannot lower an invalid expression",
                    span,
                ));
            }
        };
        self.emit(op, span);
        Ok(dst)
    }

    fn args(&mut self, args: &[CallArg]) -> LowerResult<Vec<Register>> {
        args.iter()
            .map(|arg| {
                self.expr(match arg {
                    CallArg::Positional(e) | CallArg::Named { expr: e, .. } => e,
                })
            })
            .collect()
    }

    fn function_args(&mut self, function: usize, args: &[CallArg]) -> LowerResult<Vec<Register>> {
        // Evaluate in source order, then permute into parameter order.
        let values = self.args(args)?;
        if args.iter().all(|arg| matches!(arg, CallArg::Positional(_))) {
            return Ok(values);
        }
        let StmtKind::FuncDecl { params, .. } = &self.registry.functions[function].1.kind else {
            unreachable!()
        };
        params
            .iter()
            .map(|param| {
                args.iter()
                    .position(
                        |arg| matches!(arg, CallArg::Named { name, .. } if *name == param.name),
                    )
                    .map(|i| values[i])
                    .ok_or_else(|| {
                        Diagnostic::type_error("missing checked call argument", self.function.span)
                    })
            })
            .collect()
    }

    fn call(
        &mut self,
        dst: Register,
        target: Option<&str>,
        name: &str,
        args: &[CallArg],
        span: Span,
    ) -> LowerResult<Register> {
        if target.is_none() {
            if let Some(&function) = self.registry.calls.get(&(None, name.to_string())) {
                let values = self.function_args(function, args)?;
                self.emit(
                    Op::Call {
                        dst,
                        function,
                        args: values,
                    },
                    span,
                );
                return Ok(dst);
            }
            if let Some(&fields) = self.registry.structs.get(name) {
                let mut values = Vec::with_capacity(fields.len());
                // Struct fields evaluate in declaration order, not argument order.
                for field in fields {
                    let expr = args
                        .iter()
                        .find_map(|arg| match arg {
                            CallArg::Named { name, expr } if *name == field.name => Some(expr),
                            _ => None,
                        })
                        .ok_or_else(|| {
                            Diagnostic::type_error("missing checked struct field", span)
                        })?;
                    let value = self.expr(expr)?;
                    let value = self.coerce(value, &field.ty, expr.span);
                    values.push((field.name.clone(), value));
                }
                self.emit(
                    Op::Struct {
                        dst,
                        name: name.to_string(),
                        fields: values,
                    },
                    span,
                );
                return Ok(dst);
            }
        }
        let builtin = match (target, name) {
            (None, "Some" | "None") => Some("option"),
            (None, "Ok" | "Err") => Some("result"),
            (Some("option" | "result"), _) => target,
            _ => None,
        };
        if let Some(enum_name) = builtin {
            let payload = self.args(args)?;
            self.emit(
                Op::Enum {
                    dst,
                    name: enum_name.into(),
                    variant: name.into(),
                    payload,
                },
                span,
            );
            return Ok(dst);
        }
        if let Some(target) = target {
            if let Some(&variants) = self.registry.enums.get(target)
                && let Some(variant) = variants.iter().find(|v| v.name == name)
            {
                let mut payload = Vec::with_capacity(args.len());
                for (arg, field) in args.iter().zip(&variant.fields) {
                    let expr = match arg {
                        CallArg::Positional(e) | CallArg::Named { expr: e, .. } => e,
                    };
                    let value = self.expr(expr)?;
                    payload.push(self.coerce(value, &field.ty, expr.span));
                }
                self.emit(
                    Op::Enum {
                        dst,
                        name: target.into(),
                        variant: name.into(),
                        payload,
                    },
                    span,
                );
                return Ok(dst);
            }
            if let Some(&function) = self.registry.calls.get(&(Some(target.into()), name.into())) {
                let values = self.function_args(function, args)?;
                self.emit(
                    Op::Call {
                        dst,
                        function,
                        args: values,
                    },
                    span,
                );
                return Ok(dst);
            }
        }
        if target == Some("cm") {
            // The checker admits compile-time helper bodies into expanded
            // programs. Runtime invocation fails exactly like a walker with
            // no CtHost attached; expansion still uses the reference walker.
            self.emit(Op::Fail("unknown enum `cm`".into()), span);
            return Ok(dst);
        }
        Err(Diagnostic::type_error(
            "unresolved checked call during lowering",
            span,
        ))
    }
}

fn converts_scalar(ty: &Type) -> bool {
    match ty {
        Type::Prim(PrimitiveType::Int | PrimitiveType::Byte) => true,
        Type::Array(elem) => converts_scalar(elem),
        Type::Map { key, value } => converts_scalar(key) || converts_scalar(value),
        Type::Named { name, args } if name == "option" || name == "result" => {
            args.iter().any(converts_scalar)
        }
        _ => false,
    }
}
