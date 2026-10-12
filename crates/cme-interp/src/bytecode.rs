//! Register bytecode generation and synchronous execution (§5.1).
//!
//! Bytecode is verified, immutable and AST-free. Each invocation owns an
//! explicit frame stack and register files; script calls never recurse on the
//! native stack. Each executed instruction (including branches and returns)
//! consumes one unit of fuel. Host operations are synchronous; no frames are
//! parked for `suspend` in this milestone.

use std::cell::Cell;
use std::collections::HashMap;
use std::time::Instant;

use cme_core::ast::{Param, Span, Type};
use cme_core::ir::{self, Access, BoolContext, ExitKind, Literal, Op, Register};

use crate::{CapabilityHost, InterpError, MAX_CALL_DEPTH, PathOp, Value};
use crate::{
    apply_binary, apply_unary, coerce_to_declared, read_field, read_index, read_path, stringify,
    write_path,
};

#[derive(Debug, Clone)]
enum Code {
    Op(Op),
    Jump(usize),
    Branch {
        cond: Register,
        yes: usize,
        no: usize,
        context: BoolContext,
    },
    Return(Register),
    Try {
        value: Register,
        dst: Register,
        next: usize,
    },
    Fail(String),
}

#[derive(Debug, Clone)]
struct Instruction {
    code: Code,
    span: Span,
}

#[derive(Debug, Clone)]
struct Function {
    name: String,
    params: Vec<Param>,
    return_ty: Type,
    registers: Vec<Type>,
    code: Vec<Instruction>,
}

/// A verified, reusable bytecode program. Only generation can construct it;
/// instruction/register indices cannot be mutated by callers after verification.
#[derive(Debug, Clone)]
pub struct Bytecode {
    functions: Vec<Function>,
    entries: HashMap<String, usize>,
    members: HashMap<(String, String), usize>,
}

impl Bytecode {
    /// Flatten basic blocks and resolve their labels into instruction addresses.
    /// Malformed hand-built IR is rejected before execution, never indexed
    /// blindly by the VM. This is an in-memory format, not an artifact ABI.
    pub fn compile(program: &ir::Program) -> Result<Self, InterpError> {
        let mut functions = Vec::with_capacity(program.functions.len());
        let mut entries = HashMap::new();
        let mut members = HashMap::new();
        for (id, function) in program.functions.iter().enumerate() {
            verify(function, program)?;
            let mut offsets = Vec::with_capacity(function.blocks.len());
            let mut offset = 0;
            for block in &function.blocks {
                offsets.push(offset);
                offset += block.instructions.len() + 1;
            }
            let mut code = Vec::with_capacity(offset);
            for block in &function.blocks {
                for instruction in &block.instructions {
                    code.push(Instruction {
                        code: Code::Op(instruction.op.clone()),
                        span: instruction.span,
                    });
                }
                let exit = match &block.exit.kind {
                    ExitKind::Jump(target) => Code::Jump(offsets[*target]),
                    ExitKind::Branch {
                        cond,
                        yes,
                        no,
                        context,
                    } => Code::Branch {
                        cond: *cond,
                        yes: offsets[*yes],
                        no: offsets[*no],
                        context: *context,
                    },
                    ExitKind::Return(value) => Code::Return(*value),
                    ExitKind::Try { value, dst, next } => Code::Try {
                        value: *value,
                        dst: *dst,
                        next: offsets[*next],
                    },
                    ExitKind::Fail(message) => Code::Fail(message.clone()),
                };
                code.push(Instruction {
                    code: exit,
                    span: block.exit.span,
                });
            }
            let display = if let Some(target) = &function.target {
                if members
                    .insert((target.clone(), function.name.clone()), id)
                    .is_some()
                {
                    return Err(InterpError::new(
                        "duplicate bytecode impl member",
                        function.span,
                    ));
                }
                format!("{target}.{}", function.name)
            } else {
                if entries.insert(function.name.clone(), id).is_some() {
                    return Err(InterpError::new("duplicate bytecode entry", function.span));
                }
                function.name.clone()
            };
            functions.push(Function {
                name: display,
                params: function.params.clone(),
                return_ty: function.return_ty.clone(),
                registers: function.registers.clone(),
                code,
            });
        }
        Ok(Self {
            functions,
            entries,
            members,
        })
    }

    pub fn function_count(&self) -> usize {
        self.functions.len()
    }
    pub fn instruction_count(&self) -> usize {
        self.functions.iter().map(|f| f.code.len()).sum()
    }
}

fn verify(function: &ir::Function, program: &ir::Program) -> Result<(), InterpError> {
    let fail = || {
        InterpError::new(
            "invalid bytecode IR: register, block, or function reference",
            function.span,
        )
    };
    if function.blocks.is_empty() || function.params.len() > function.registers.len() {
        return Err(fail());
    }
    for block in &function.blocks {
        for instruction in &block.instructions {
            if instruction
                .op
                .registers()
                .iter()
                .any(|r| *r >= function.registers.len())
            {
                return Err(fail());
            }
            if let Op::Call {
                function: id, args, ..
            } = &instruction.op
            {
                let callee = program.functions.get(*id).ok_or_else(fail)?;
                if args.len() != callee.params.len() {
                    return Err(fail());
                }
            }
            if let Op::Capability { path, member, .. } = &instruction.op
                && (path.is_empty() || member.is_empty())
            {
                return Err(fail());
            }
        }
        let (regs, blocks) = match &block.exit.kind {
            ExitKind::Jump(target) => (vec![], vec![*target]),
            ExitKind::Branch { cond, yes, no, .. } => (vec![*cond], vec![*yes, *no]),
            ExitKind::Return(value) => (vec![*value], vec![]),
            ExitKind::Try { value, dst, next } => (vec![*value, *dst], vec![*next]),
            ExitKind::Fail(_) => (vec![], vec![]),
        };
        if regs.iter().any(|r| *r >= function.registers.len())
            || blocks.iter().any(|b| *b >= function.blocks.len())
        {
            return Err(fail());
        }
    }
    Ok(())
}

/// Invocation controls mirror the reference interpreter. A fresh invocation
/// owns frames/registers; sharing a VM or bytecode across calls does not share
/// execution state. Callers choose whether to share the optional fuel cell.
pub struct Vm<'a> {
    program: &'a Bytecode,
    capabilities: Option<&'a dyn CapabilityHost>,
    fuel: Option<&'a Cell<u64>>,
    deadline: Option<Instant>,
    call_depth_limit: usize,
}

impl<'a> Vm<'a> {
    pub fn new(program: &'a Bytecode) -> Self {
        Self {
            program,
            capabilities: None,
            fuel: None,
            deadline: None,
            call_depth_limit: MAX_CALL_DEPTH,
        }
    }
    pub fn with_capabilities(mut self, capabilities: &'a dyn CapabilityHost) -> Self {
        self.capabilities = Some(capabilities);
        self
    }
    pub fn with_fuel(mut self, fuel: &'a Cell<u64>) -> Self {
        self.fuel = Some(fuel);
        self
    }
    pub fn with_deadline(mut self, deadline: Instant) -> Self {
        self.deadline = Some(deadline);
        self
    }
    pub fn with_call_depth_limit(mut self, limit: usize) -> Self {
        if limit > 0 && limit < MAX_CALL_DEPTH {
            self.call_depth_limit = limit;
        }
        self
    }

    pub fn invoke(&self, name: &str, args: &[Value]) -> Result<Value, InterpError> {
        let id = self.program.entries.get(name).copied().ok_or_else(|| {
            InterpError::unknown_entry(format!("unknown function `{name}`"), Span::new(0, 0))
        })?;
        self.run(id, args)
    }

    pub fn invoke_member(
        &self,
        target: &str,
        member: &str,
        args: &[Value],
    ) -> Result<Value, InterpError> {
        let id = self
            .program
            .members
            .get(&(target.into(), member.into()))
            .copied()
            .ok_or_else(|| {
                InterpError::unknown_entry(
                    format!("unknown impl member `{target}.{member}`"),
                    Span::new(0, 0),
                )
            })?;
        self.run(id, args)
    }

    fn frame(
        &self,
        id: usize,
        args: Vec<Value>,
        call_span: Span,
        return_to: Option<Register>,
    ) -> Result<Frame, InterpError> {
        let function = &self.program.functions[id];
        if args.len() != function.params.len() {
            return Err(InterpError::new(
                format!(
                    "wrong number of arguments to `{}`: expected {}, found {}",
                    function.name,
                    function.params.len(),
                    args.len()
                ),
                call_span,
            ));
        }
        let mut registers = vec![Value::Void; function.registers.len()];
        for (index, (param, value)) in function.params.iter().zip(args).enumerate() {
            registers[index] = coerce_to_declared(value, &param.ty, call_span)?;
        }
        Ok(Frame {
            function: id,
            pc: 0,
            registers,
            call_span,
            return_to,
        })
    }

    fn safepoint(&self, span: Span) -> Result<(), InterpError> {
        if let Some(deadline) = self.deadline
            && Instant::now() >= deadline
        {
            return Err(InterpError::deadline(span));
        }
        if let Some(fuel) = self.fuel {
            if fuel.get() == 0 {
                let mut error = InterpError::budget(span);
                error.message = "fuel budget exhausted (§5.5: execution is metered by bytecode instruction count)".into();
                return Err(error);
            }
            fuel.set(fuel.get() - 1);
        }
        Ok(())
    }

    fn run(&self, entry: usize, args: &[Value]) -> Result<Value, InterpError> {
        let mut frames = vec![self.frame(entry, args.to_vec(), Span::new(0, 0), None)?];
        loop {
            let frame = frames.last_mut().expect("active VM frame");
            let function = &self.program.functions[frame.function];
            let instruction = &function.code[frame.pc];
            let span = instruction.span;
            self.safepoint(span)?;
            frame.pc += 1;
            let returned = match &instruction.code {
                Code::Op(Op::Call {
                    dst,
                    function: id,
                    args,
                }) => {
                    let values = args.iter().map(|r| frame.registers[*r].clone()).collect();
                    if frames.len() >= self.call_depth_limit {
                        return Err(InterpError::call_depth(self.call_depth_limit, span));
                    }
                    frames.push(self.frame(*id, values, span, Some(*dst))?);
                    continue;
                }
                Code::Op(Op::Capability {
                    dst,
                    path,
                    member,
                    args,
                }) => {
                    let host = self.capabilities.ok_or_else(|| {
                        InterpError::new(
                            format!("unknown function `{}.{member}`", path.join(".")),
                            span,
                        )
                    })?;
                    let values: Vec<Value> =
                        args.iter().map(|r| frame.registers[*r].clone()).collect();
                    let segments: Vec<&str> = path.iter().map(String::as_str).collect();
                    frame.registers[*dst] = host
                        .call(&segments, member, &values)
                        .map_err(|message| InterpError::new(message, span))?;
                    None
                }
                Code::Op(op) => {
                    evaluate(op, &mut frame.registers, span)?;
                    None
                }
                Code::Jump(target) => {
                    frame.pc = *target;
                    None
                }
                Code::Branch {
                    cond,
                    yes,
                    no,
                    context,
                } => {
                    frame.pc = if check_bool(&frame.registers[*cond], *context, span)? {
                        *yes
                    } else {
                        *no
                    };
                    None
                }
                Code::Return(value) => Some(frame.registers[*value].clone()),
                Code::Try { value, dst, next } => {
                    let value = frame.registers[*value].clone();
                    match value {
                        Value::Enum {
                            name,
                            variant,
                            mut payload,
                        } if name == "result"
                            && payload.len() == 1
                            && (variant == "Ok" || variant == "Err") =>
                        {
                            if variant == "Ok" {
                                frame.registers[*dst] = payload.remove(0);
                                frame.pc = *next;
                                None
                            } else {
                                Some(Value::Enum {
                                    name,
                                    variant,
                                    payload,
                                })
                            }
                        }
                        other => {
                            return Err(InterpError::new(
                                format!(
                                    "the `?` operator requires `result<T, E>`, found `{}`",
                                    other.kind_name()
                                ),
                                span,
                            ));
                        }
                    }
                }
                Code::Fail(message) => return Err(InterpError::new(message.clone(), span)),
            };
            if let Some(value) = returned {
                let value = coerce_to_declared(value, &function.return_ty, frame.call_span)?;
                let destination = frame.return_to;
                frames.pop();
                match frames.last_mut() {
                    Some(caller) => {
                        caller.registers[destination.expect("callee return destination")] = value
                    }
                    None => return Ok(value),
                }
            }
        }
    }
}

struct Frame {
    function: usize,
    pc: usize,
    registers: Vec<Value>,
    call_span: Span,
    return_to: Option<Register>,
}

fn check_bool(value: &Value, context: BoolContext, span: Span) -> Result<bool, InterpError> {
    if let Value::Bool(value) = value {
        return Ok(*value);
    }
    let kind = value.kind_name();
    let message = match context {
        BoolContext::If => format!("if condition must be `bool`, found `{kind}`"),
        BoolContext::While => format!("while condition must be `bool`, found `{kind}`"),
        BoolContext::AndLeft => format!("cannot apply `&&` to `{kind}` and `bool`"),
        BoolContext::AndRight => format!("cannot apply `&&` to `bool` and `{kind}`"),
        BoolContext::OrLeft => format!("cannot apply `||` to `{kind}` and `bool`"),
        BoolContext::OrRight => format!("cannot apply `||` to `bool` and `{kind}`"),
        BoolContext::Internal => format!("branch condition must be `bool`, found `{kind}`"),
    };
    Err(InterpError::new(message, span))
}

fn resolve_path(path: &[Access], registers: &[Value]) -> Vec<PathOp> {
    path.iter()
        .map(|access| match access {
            Access::Field(name) => PathOp::Field(name.clone()),
            Access::Index(reg) => PathOp::Index(registers[*reg].clone()),
        })
        .collect()
}

fn evaluate(op: &Op, registers: &mut [Value], span: Span) -> Result<(), InterpError> {
    let (dst, value) = match op {
        Op::Fail(message) => return Err(InterpError::new(message.clone(), span)),
        Op::Constant { dst, value } => (
            *dst,
            match value {
                Literal::Int(v) => Value::Int(*v),
                Literal::Byte(v) => Value::Byte(*v),
                Literal::Float(v) => Value::Float(*v),
                Literal::Bool(v) => Value::Bool(*v),
                Literal::Str(v) => Value::Str(v.clone()),
                Literal::Void => Value::Void,
            },
        ),
        Op::Copy { dst, src } => (*dst, registers[*src].clone()),
        Op::Coerce { dst, src, ty } => {
            (*dst, coerce_to_declared(registers[*src].clone(), ty, span)?)
        }
        Op::ByteLiteral { dst, src, against } => (
            *dst,
            match (&registers[*src], &registers[*against]) {
                (Value::Int(v), Value::Byte(_)) if (0..=255).contains(v) => Value::Byte(*v as u8),
                (value, _) => value.clone(),
            },
        ),
        Op::Bool {
            dst,
            value,
            context,
        } => (
            *dst,
            Value::Bool(check_bool(&registers[*value], *context, span)?),
        ),
        Op::Unary { dst, op, value } => (*dst, apply_unary(*op, registers[*value].clone(), span)?),
        Op::Binary {
            dst,
            op,
            left,
            right,
        } => (
            *dst,
            apply_binary(
                *op,
                registers[*left].clone(),
                registers[*right].clone(),
                span,
            )?,
        ),
        Op::Field { dst, obj, name } => (*dst, read_field(&registers[*obj], name, span)?),
        Op::Index { dst, obj, index } => (
            *dst,
            read_index(&registers[*obj], &registers[*index], span)?,
        ),
        Op::ReadPath { dst, base, path } => (
            *dst,
            read_path(&registers[*base], &resolve_path(path, registers), span)?,
        ),
        Op::WritePath { base, path, value } => {
            let path = resolve_path(path, registers);
            let value = registers[*value].clone();
            write_path(&mut registers[*base], &path, value, span)?;
            return Ok(());
        }
        Op::Struct { dst, name, fields } => (
            *dst,
            Value::Struct {
                name: name.clone(),
                fields: fields
                    .iter()
                    .map(|(name, reg)| (name.clone(), registers[*reg].clone()))
                    .collect(),
            },
        ),
        Op::Enum {
            dst,
            name,
            variant,
            payload,
        } => (
            *dst,
            Value::Enum {
                name: name.clone(),
                variant: variant.clone(),
                payload: payload.iter().map(|r| registers[*r].clone()).collect(),
            },
        ),
        Op::Array { dst, items } => (
            *dst,
            Value::Array(items.iter().map(|r| registers[*r].clone()).collect()),
        ),
        Op::Map { dst, entries } => {
            let mut pairs: Vec<(Value, Value)> = Vec::with_capacity(entries.len());
            for (key, value) in entries {
                let key = registers[*key].clone();
                let value = registers[*value].clone();
                if let Some(existing) = pairs.iter_mut().find(|(k, _)| *k == key) {
                    existing.1 = value;
                } else {
                    pairs.push((key, value));
                }
            }
            (*dst, Value::Map(pairs))
        }
        Op::Stringify { dst, value } => (
            *dst,
            Value::Str(stringify(&registers[*value]).ok_or_else(|| {
                InterpError::new(
                    format!("cannot interpolate `{}`", registers[*value].kind_name()),
                    span,
                )
            })?),
        ),
        Op::Iterate { dst, collection } => (
            *dst,
            Value::Array(match &registers[*collection] {
                Value::Array(items) => items.clone(),
                Value::Map(entries) => entries.iter().map(|(k, _)| k.clone()).collect(),
                other => {
                    return Err(InterpError::new(
                        format!("cannot iterate `{}`", other.kind_name()),
                        span,
                    ));
                }
            }),
        ),
        Op::IsVariant {
            dst,
            value,
            variant,
        } => {
            let Value::Enum {
                variant: actual, ..
            } = &registers[*value]
            else {
                return Err(InterpError::new(
                    format!(
                        "match scrutinee must be an enum value, found `{}`",
                        registers[*value].kind_name()
                    ),
                    span,
                ));
            };
            (
                *dst,
                Value::Bool(variant.as_ref().is_none_or(|wanted| wanted == actual)),
            )
        }
        Op::Payload { dst, value, index } => {
            let Value::Enum { payload, .. } = &registers[*value] else {
                return Err(InterpError::new("invalid enum payload", span));
            };
            (
                *dst,
                payload
                    .get(*index)
                    .cloned()
                    .ok_or_else(|| InterpError::new("missing enum payload", span))?,
            )
        }
        Op::Call { .. } | Op::Capability { .. } => {
            unreachable!("calls execute in the frame dispatcher")
        }
    };
    registers[dst] = value;
    Ok(())
}
