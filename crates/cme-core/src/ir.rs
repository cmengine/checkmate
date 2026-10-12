//! Typed, register-based control-flow IR shared by execution backends.
//!
//! Functions contain basic blocks, resolved function IDs and lexical register
//! slots. There are no executable AST nodes or runtime name lookups. Every
//! operation and terminator retains its source span. Register types come from
//! the checker; `Type::Infer` is reserved for existing unchecked host calls and
//! unconstrained phantom generic arguments, not inferred local declarations.

use crate::ast::{BinaryOp, Param, Span, Type, UnaryOp};

pub type Register = usize;
pub type FunctionId = usize;
pub type BlockId = usize;

#[derive(Debug, Clone, Default)]
pub struct Program {
    pub functions: Vec<Function>,
}

#[derive(Debug, Clone)]
pub struct Function {
    pub name: String,
    pub target: Option<String>,
    /// Parameters occupy the first registers, in declaration order.
    pub params: Vec<Param>,
    pub return_ty: Type,
    pub registers: Vec<Type>,
    /// Block zero is the function entry.
    pub blocks: Vec<Block>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct Block {
    pub instructions: Vec<Instruction>,
    pub exit: Exit,
}

#[derive(Debug, Clone)]
pub struct Instruction {
    pub op: Op,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct Exit {
    pub kind: ExitKind,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum ExitKind {
    Jump(BlockId),
    Branch {
        cond: Register,
        yes: BlockId,
        no: BlockId,
        context: BoolContext,
    },
    Return(Register),
    /// `result?`: unwrap Ok and continue, or return Err from this function.
    Try {
        value: Register,
        dst: Register,
        next: BlockId,
    },
    Fail(String),
}

#[derive(Debug, Clone, Copy)]
pub enum BoolContext {
    If,
    While,
    AndLeft,
    AndRight,
    OrLeft,
    OrRight,
    Internal,
}

#[derive(Debug, Clone)]
pub enum Literal {
    Int(i64),
    Byte(u8),
    Float(f64),
    Bool(bool),
    Str(String),
    Void,
}

#[derive(Debug, Clone)]
pub enum Access {
    Field(String),
    Index(Register),
}

#[derive(Debug, Clone)]
pub enum Op {
    /// A compile-time-only operation reached by runtime execution.
    Fail(String),
    Constant {
        dst: Register,
        value: Literal,
    },
    Copy {
        dst: Register,
        src: Register,
    },
    Coerce {
        dst: Register,
        src: Register,
        ty: Type,
    },
    /// A direct integer literal opposite a runtime byte operand stays byte
    /// arithmetic. Parenthesized/computed integers do not crystallize here.
    ByteLiteral {
        dst: Register,
        src: Register,
        against: Register,
    },
    Bool {
        dst: Register,
        value: Register,
        context: BoolContext,
    },
    Unary {
        dst: Register,
        op: UnaryOp,
        value: Register,
    },
    Binary {
        dst: Register,
        op: BinaryOp,
        left: Register,
        right: Register,
    },
    Field {
        dst: Register,
        obj: Register,
        name: String,
    },
    Index {
        dst: Register,
        obj: Register,
        index: Register,
    },
    ReadPath {
        dst: Register,
        base: Register,
        path: Vec<Access>,
    },
    WritePath {
        base: Register,
        path: Vec<Access>,
        value: Register,
    },
    Struct {
        dst: Register,
        name: String,
        fields: Vec<(String, Register)>,
    },
    Enum {
        dst: Register,
        name: String,
        variant: String,
        payload: Vec<Register>,
    },
    Array {
        dst: Register,
        items: Vec<Register>,
    },
    Map {
        dst: Register,
        entries: Vec<(Register, Register)>,
    },
    Stringify {
        dst: Register,
        value: Register,
    },
    /// Snapshot an array's elements or a map's insertion-ordered keys.
    Iterate {
        dst: Register,
        collection: Register,
    },
    IsVariant {
        dst: Register,
        value: Register,
        variant: Option<String>,
    },
    Payload {
        dst: Register,
        value: Register,
        index: usize,
    },
    Call {
        dst: Register,
        function: FunctionId,
        args: Vec<Register>,
    },
    Capability {
        dst: Register,
        path: Vec<String>,
        member: String,
        args: Vec<Register>,
    },
}

impl Op {
    /// All referenced registers, including destinations; used by bytecode
    /// verification before an externally constructed IR can execute.
    pub fn registers(&self) -> Vec<Register> {
        match self {
            Self::Fail(_) => vec![],
            Self::Constant { dst, .. } => vec![*dst],
            Self::Copy { dst, src } | Self::Coerce { dst, src, .. } => vec![*dst, *src],
            Self::ByteLiteral { dst, src, against } => vec![*dst, *src, *against],
            Self::Bool { dst, value, .. }
            | Self::Unary { dst, value, .. }
            | Self::Stringify { dst, value }
            | Self::IsVariant { dst, value, .. }
            | Self::Payload { dst, value, .. } => vec![*dst, *value],
            Self::Binary {
                dst, left, right, ..
            } => vec![*dst, *left, *right],
            Self::Field { dst, obj, .. } => vec![*dst, *obj],
            Self::Index { dst, obj, index } => vec![*dst, *obj, *index],
            Self::ReadPath { dst, base, path } => {
                let mut regs = vec![*dst, *base];
                regs.extend(path.iter().filter_map(|a| match a {
                    Access::Index(r) => Some(*r),
                    _ => None,
                }));
                regs
            }
            Self::WritePath { base, path, value } => {
                let mut regs = vec![*base, *value];
                regs.extend(path.iter().filter_map(|a| match a {
                    Access::Index(r) => Some(*r),
                    _ => None,
                }));
                regs
            }
            Self::Struct { dst, fields, .. } => std::iter::once(*dst)
                .chain(fields.iter().map(|(_, r)| *r))
                .collect(),
            Self::Enum { dst, payload, .. } => std::iter::once(*dst)
                .chain(payload.iter().copied())
                .collect(),
            Self::Array { dst, items } => {
                std::iter::once(*dst).chain(items.iter().copied()).collect()
            }
            Self::Map { dst, entries } => std::iter::once(*dst)
                .chain(entries.iter().flat_map(|(k, v)| [*k, *v]))
                .collect(),
            Self::Iterate { dst, collection } => vec![*dst, *collection],
            Self::Call { dst, args, .. } | Self::Capability { dst, args, .. } => {
                std::iter::once(*dst).chain(args.iter().copied()).collect()
            }
        }
    }
}
