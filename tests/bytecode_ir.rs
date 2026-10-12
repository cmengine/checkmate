//! The public in-memory IR must reject invalid references before execution.
use cme_compiler::check::prepare_typed;
use cme_core::ast::{BinaryOp, PrimitiveType, Type, UnaryOp};
use cme_core::ir::{self, Access, BoolContext, ExitKind, Literal, Op};
use cme_interp::Value;
use cme_interp::bytecode::{Bytecode, Vm};

fn program() -> ir::Program {
    let parsed = cme_compiler::parse_source("int main() {\nreturn 42\n}\n");
    cme_compiler::lower::lower(&prepare_typed(&parsed.statements, &[], None), &[]).unwrap()
}

fn reject_op(op: Op) {
    let mut program = program();
    program.functions[0].blocks[0].instructions[0].op = op;
    assert!(Bytecode::compile(&program).is_err());
}

macro_rules! invalid_op {
    ($name:ident, $op:expr) => {
        #[test]
        fn $name() {
            reject_op($op);
        }
    };
}
invalid_op!(
    constant_destination,
    Op::Constant {
        dst: usize::MAX,
        value: Literal::Int(1)
    }
);
invalid_op!(
    copy_source,
    Op::Copy {
        dst: 0,
        src: usize::MAX
    }
);
invalid_op!(
    copy_destination,
    Op::Copy {
        dst: usize::MAX,
        src: 0
    }
);
invalid_op!(
    coerce_source,
    Op::Coerce {
        dst: 0,
        src: usize::MAX,
        ty: Type::Prim(PrimitiveType::Byte)
    }
);
invalid_op!(
    coerce_destination,
    Op::Coerce {
        dst: usize::MAX,
        src: 0,
        ty: Type::Prim(PrimitiveType::Int)
    }
);
invalid_op!(
    byte_literal_source,
    Op::ByteLiteral {
        dst: 0,
        src: usize::MAX,
        against: 0
    }
);
invalid_op!(
    byte_literal_against,
    Op::ByteLiteral {
        dst: 0,
        src: 0,
        against: usize::MAX
    }
);
invalid_op!(
    byte_literal_destination,
    Op::ByteLiteral {
        dst: usize::MAX,
        src: 0,
        against: 0
    }
);
invalid_op!(
    bool_source,
    Op::Bool {
        dst: 0,
        value: usize::MAX,
        context: BoolContext::If
    }
);
invalid_op!(
    bool_destination,
    Op::Bool {
        dst: usize::MAX,
        value: 0,
        context: BoolContext::If
    }
);
invalid_op!(
    unary_source,
    Op::Unary {
        dst: 0,
        value: usize::MAX,
        op: UnaryOp::Neg
    }
);
invalid_op!(
    unary_destination,
    Op::Unary {
        dst: usize::MAX,
        value: 0,
        op: UnaryOp::Neg
    }
);
invalid_op!(
    binary_left,
    Op::Binary {
        dst: 0,
        left: usize::MAX,
        right: 0,
        op: BinaryOp::Add
    }
);
invalid_op!(
    binary_right,
    Op::Binary {
        dst: 0,
        left: 0,
        right: usize::MAX,
        op: BinaryOp::Add
    }
);
invalid_op!(
    binary_destination,
    Op::Binary {
        dst: usize::MAX,
        left: 0,
        right: 0,
        op: BinaryOp::Add
    }
);
invalid_op!(
    field_object,
    Op::Field {
        dst: 0,
        obj: usize::MAX,
        name: "x".into()
    }
);
invalid_op!(
    field_destination,
    Op::Field {
        dst: usize::MAX,
        obj: 0,
        name: "x".into()
    }
);
invalid_op!(
    index_object,
    Op::Index {
        dst: 0,
        obj: usize::MAX,
        index: 0
    }
);
invalid_op!(
    index_index,
    Op::Index {
        dst: 0,
        obj: 0,
        index: usize::MAX
    }
);
invalid_op!(
    index_destination,
    Op::Index {
        dst: usize::MAX,
        obj: 0,
        index: 0
    }
);
invalid_op!(
    read_path_base,
    Op::ReadPath {
        dst: 0,
        base: usize::MAX,
        path: vec![]
    }
);
invalid_op!(
    read_path_nested_index,
    Op::ReadPath {
        dst: 0,
        base: 0,
        path: vec![Access::Field("x".into()), Access::Index(usize::MAX)]
    }
);
invalid_op!(
    read_path_destination,
    Op::ReadPath {
        dst: usize::MAX,
        base: 0,
        path: vec![]
    }
);
invalid_op!(
    write_path_base,
    Op::WritePath {
        base: usize::MAX,
        value: 0,
        path: vec![]
    }
);
invalid_op!(
    write_path_value,
    Op::WritePath {
        base: 0,
        value: usize::MAX,
        path: vec![]
    }
);
invalid_op!(
    write_path_nested_index,
    Op::WritePath {
        base: 0,
        value: 0,
        path: vec![Access::Index(usize::MAX)]
    }
);
invalid_op!(
    struct_field,
    Op::Struct {
        dst: 0,
        name: "S".into(),
        fields: vec![("x".into(), usize::MAX)]
    }
);
invalid_op!(
    struct_destination,
    Op::Struct {
        dst: usize::MAX,
        name: "S".into(),
        fields: vec![]
    }
);
invalid_op!(
    enum_payload,
    Op::Enum {
        dst: 0,
        name: "E".into(),
        variant: "V".into(),
        payload: vec![usize::MAX]
    }
);
invalid_op!(
    enum_destination,
    Op::Enum {
        dst: usize::MAX,
        name: "E".into(),
        variant: "V".into(),
        payload: vec![]
    }
);
invalid_op!(
    array_item,
    Op::Array {
        dst: 0,
        items: vec![usize::MAX]
    }
);
invalid_op!(
    array_destination,
    Op::Array {
        dst: usize::MAX,
        items: vec![]
    }
);
invalid_op!(
    map_key,
    Op::Map {
        dst: 0,
        entries: vec![(usize::MAX, 0)]
    }
);
invalid_op!(
    map_value,
    Op::Map {
        dst: 0,
        entries: vec![(0, usize::MAX)]
    }
);
invalid_op!(
    map_destination,
    Op::Map {
        dst: usize::MAX,
        entries: vec![]
    }
);
invalid_op!(
    stringify_value,
    Op::Stringify {
        dst: 0,
        value: usize::MAX
    }
);
invalid_op!(
    stringify_destination,
    Op::Stringify {
        dst: usize::MAX,
        value: 0
    }
);
invalid_op!(
    iterate_collection,
    Op::Iterate {
        dst: 0,
        collection: usize::MAX
    }
);
invalid_op!(
    iterate_destination,
    Op::Iterate {
        dst: usize::MAX,
        collection: 0
    }
);
invalid_op!(
    variant_value,
    Op::IsVariant {
        dst: 0,
        value: usize::MAX,
        variant: None
    }
);
invalid_op!(
    variant_destination,
    Op::IsVariant {
        dst: usize::MAX,
        value: 0,
        variant: None
    }
);
invalid_op!(
    payload_value,
    Op::Payload {
        dst: 0,
        value: usize::MAX,
        index: 0
    }
);
invalid_op!(
    payload_destination,
    Op::Payload {
        dst: usize::MAX,
        value: 0,
        index: 0
    }
);
invalid_op!(
    call_destination,
    Op::Call {
        dst: usize::MAX,
        function: 0,
        args: vec![]
    }
);
invalid_op!(
    call_argument,
    Op::Call {
        dst: 0,
        function: 0,
        args: vec![usize::MAX]
    }
);
invalid_op!(
    call_function,
    Op::Call {
        dst: 0,
        function: usize::MAX,
        args: vec![]
    }
);
invalid_op!(
    call_arity,
    Op::Call {
        dst: 0,
        function: 0,
        args: vec![0]
    }
);
invalid_op!(
    capability_destination,
    Op::Capability {
        dst: usize::MAX,
        path: vec!["host".into()],
        member: "F".into(),
        args: vec![]
    }
);
invalid_op!(
    capability_argument,
    Op::Capability {
        dst: 0,
        path: vec!["host".into()],
        member: "F".into(),
        args: vec![usize::MAX]
    }
);
invalid_op!(
    capability_empty_path,
    Op::Capability {
        dst: 0,
        path: vec![],
        member: "F".into(),
        args: vec![]
    }
);
invalid_op!(
    capability_empty_member,
    Op::Capability {
        dst: 0,
        path: vec!["host".into()],
        member: "".into(),
        args: vec![]
    }
);

macro_rules! invalid_exit {
    ($name:ident, $exit:expr) => {
        #[test]
        fn $name() {
            let mut program = program();
            program.functions[0].blocks[0].exit.kind = $exit;
            assert!(Bytecode::compile(&program).is_err());
        }
    };
}
invalid_exit!(jump_target, ExitKind::Jump(usize::MAX));
invalid_exit!(
    branch_condition,
    ExitKind::Branch {
        cond: usize::MAX,
        yes: 0,
        no: 0,
        context: BoolContext::If
    }
);
invalid_exit!(
    branch_yes,
    ExitKind::Branch {
        cond: 0,
        yes: usize::MAX,
        no: 0,
        context: BoolContext::If
    }
);
invalid_exit!(
    branch_no,
    ExitKind::Branch {
        cond: 0,
        yes: 0,
        no: usize::MAX,
        context: BoolContext::If
    }
);
invalid_exit!(return_source, ExitKind::Return(usize::MAX));
invalid_exit!(
    try_source,
    ExitKind::Try {
        value: usize::MAX,
        dst: 0,
        next: 0
    }
);
invalid_exit!(
    try_destination,
    ExitKind::Try {
        value: 0,
        dst: usize::MAX,
        next: 0
    }
);
invalid_exit!(
    try_target,
    ExitKind::Try {
        value: 0,
        dst: 0,
        next: usize::MAX
    }
);

#[test]
fn duplicate_entries_are_rejected() {
    let mut program = program();
    program.functions.push(program.functions[0].clone());
    assert!(Bytecode::compile(&program).is_err());
}
#[test]
fn duplicate_members_are_rejected() {
    let mut program = program();
    program.functions[0].target = Some("example.Interface".into());
    program.functions.push(program.functions[0].clone());
    assert!(Bytecode::compile(&program).is_err());
}
#[test]
fn entries_and_members_have_independent_namespaces() {
    let mut program = program();
    let mut member = program.functions[0].clone();
    member.target = Some("example.Interface".into());
    program.functions.push(member);
    let code = Bytecode::compile(&program).unwrap();
    assert_eq!(Vm::new(&code).invoke("main", &[]).unwrap(), Value::Int(42));
    assert_eq!(
        Vm::new(&code)
            .invoke_member("example.Interface", "main", &[])
            .unwrap(),
        Value::Int(42)
    );
}
#[test]
fn empty_program_has_clean_unknown_entry_errors() {
    let code = Bytecode::compile(&ir::Program::default()).unwrap();
    assert!(Vm::new(&code).invoke("main", &[]).is_err());
    assert!(
        Vm::new(&code)
            .invoke_member("example.Interface", "main", &[])
            .is_err()
    );
}
#[test]
fn bytecode_is_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Bytecode>();
}
