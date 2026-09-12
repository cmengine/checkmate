//! The static type checker for the full language surface. Normative sources:
//! WHITEPAPER §2.6–§2.16 for declarations, structs, enums, generics, option /
//! result, control flow, pattern matching, and `infer` crystallization;
//! §10.4 for impl blocks; §11 for arrays and maps; Appendix A (§A.4–§A.7)
//! for operand typing, string concatenation, and compound assignment.
//!
//! The checker is two-pass: all top-level declarations — functions, types,
//! AND impl blocks — are collected first (so forward references, recursion,
//! and mutually-referencing types resolve), then bodies and members are
//! checked against per-function scopes. It runs on any input: [`Invalid`] nodes and
//! untypable regions are skipped without cascading — a declaration with a
//! broken initializer still declares its name, and nothing inside a broken
//! subtree is reported twice.
//!
//! Types resolve to [`Ty`]: the four scalars, `void`, and structural
//! references into a registry of the program's struct and enum
//! declarations. The built-in generic enums `option<T>` and `result<T, E>`
//! are registered before user declarations, and their constructor names
//! (`Ok`, `Err`, `Some`, `None`) plus the type names `option`, `result`,
//! and `map` are reserved.
//!
//! Constructor typing is bidirectional where the whitepaper requires it: a
//! payload-free or one-sided constructor (`None()`, `Ok(v)`, `Err(e)`)
//! crystallizes its missing type argument from the expected type of its
//! context — the declared type of a variable, a function's return type, a
//! parameter, a field, or a collection element (§2.8, §2.16).
//!
//! ```
//! let outcome = cme_compiler::parse_source("int f() {\nint hp = 100\nhp += 5\nreturn hp\n}\n");
//! assert!(cme_compiler::check::check(&outcome.statements).is_empty());
//! ```

use std::collections::{HashMap, HashSet};

use crate::diagnostics::Diagnostic;
use crate::schema::SchemaContext;
use cme_core::Span;
use cme_core::ast::{
    BinaryOp, Block, CallArg, CompoundOp, Expr, ExprKind, FieldDef, LValue, Param, Pattern,
    PrimitiveType, Stmt, StmtKind, Type, UnaryOp, VariantDecl,
};
use cme_core::schema::{MemberRequirement, SchemaMember};

/// A registered struct declaration (§2.6, §2.9).
#[derive(Clone)]
struct StructDef {
    name: String,
    params: Vec<String>,
    fields: Vec<FieldDef>,
}

/// A registered enum declaration (§2.7, §2.9).
#[derive(Clone)]
struct EnumDef {
    name: String,
    params: Vec<String>,
    variants: Vec<VariantDecl>,
}

/// One registered type declaration, in declaration order.
#[derive(Clone)]
enum TypeDef {
    Struct(StructDef),
    Enum(EnumDef),
}

impl TypeDef {
    fn name_str(&self) -> &str {
        match self {
            TypeDef::Struct(def) => &def.name,
            TypeDef::Enum(def) => &def.name,
        }
    }

    fn params(&self) -> &[String] {
        match self {
            TypeDef::Struct(def) => &def.params,
            TypeDef::Enum(def) => &def.params,
        }
    }
}

/// A function signature collected in the first pass, with its parameter and
/// return types already resolved. `poisoned` marks a signature the checker
/// cannot use (unresolvable types, or an `infer`/`void` placement — both
/// already rejected at parse level); poisoned functions stay in the table
/// so calls to them resolve silently instead of reporting phantom errors.
#[derive(Clone)]
struct FnSig {
    params: Vec<(String, Ty)>,
    return_ty: Ty,
    poisoned: bool,
}

/// The display name of a declared (AST) type, for schema diagnostics that
/// quote the contract's own spelling.
fn type_display(ty: &Type) -> String {
    match ty {
        Type::Infer => "infer".into(),
        Type::Void => "void".into(),
        Type::Prim(PrimitiveType::Int) => "int".into(),
        Type::Prim(PrimitiveType::Float) => "float".into(),
        Type::Prim(PrimitiveType::Bool) => "bool".into(),
        Type::Prim(PrimitiveType::Str) => "str".into(),
        Type::Array(elem) => format!("{}[]", type_display(elem)),
        Type::Map { key, value } => {
            format!("map<{}, {}>", type_display(key), type_display(value))
        }
        Type::Named { name, args } => {
            if args.is_empty() {
                name.clone()
            } else {
                let inner: Vec<String> = args.iter().map(type_display).collect();
                format!("{name}<{}>", inner.join(", "))
            }
        }
    }
}

/// The resolved type of an expression or declaration.
#[derive(Debug, Clone, PartialEq)]
enum Ty {
    Int,
    Float,
    Bool,
    Str,
    Void,
    /// Already reported or unrecoverable; every check against it passes
    /// silently so recovery diagnostics never cascade.
    Poison,
    /// The initializer could not crystallize a type on its own (an empty
    /// collection, a bare `None()`): like `Poison` everywhere, except the
    /// `infer` declaration path turns it into the §2.16 message.
    Ambiguous,
    Struct(usize, Vec<Ty>),
    Enum(usize, Vec<Ty>),
    Array(Box<Ty>),
    Map(Box<Ty>, Box<Ty>),
}

impl Ty {
    fn is_poison(&self) -> bool {
        matches!(self, Ty::Poison | Ty::Ambiguous)
    }

    /// True when any type argument inside is poison.
    fn has_poison(&self) -> bool {
        match self {
            Ty::Poison | Ty::Ambiguous => true,
            Ty::Struct(_, args) | Ty::Enum(_, args) => args.iter().any(Ty::has_poison),
            Ty::Array(elem) => elem.has_poison(),
            Ty::Map(key, value) => key.has_poison() || value.has_poison(),
            _ => false,
        }
    }

    /// The canonical type name for diagnostics (§A.6 display rules).
    fn name(&self, registry: &[TypeDef]) -> String {
        match self {
            Ty::Int => "int".into(),
            Ty::Float => "float".into(),
            Ty::Bool => "bool".into(),
            Ty::Str => "str".into(),
            Ty::Void => "void".into(),
            Ty::Poison => "poison".into(),
            Ty::Ambiguous => "ambiguous".into(),
            Ty::Struct(idx, args) | Ty::Enum(idx, args) => {
                let def_name = registry
                    .get(*idx)
                    .map(|def| def.name_str().to_string())
                    .unwrap_or_default();
                if args.is_empty() {
                    def_name
                } else {
                    let inner: Vec<String> = args.iter().map(|arg| arg.name(registry)).collect();
                    format!("{def_name}<{}>", inner.join(", "))
                }
            }
            Ty::Array(elem) => format!("{}[]", elem.name(registry)),
            Ty::Map(key, value) => {
                format!("map<{}, {}>", key.name(registry), value.name(registry))
            }
        }
    }
}

/// The constructor names of the built-in enums (§2.8), which no user
/// declaration may take.
const BUILTIN_CONSTRUCTORS: [&str; 4] = ["Ok", "Err", "Some", "None"];

/// Type names the language owns (§2.8, §11); user declarations may not
/// take them.
const RESERVED_TYPE_NAMES: [&str; 3] = ["option", "result", "map"];

/// Type-checks a whole program. Returns every violation found; the list is
/// empty exactly when the program satisfies §2.6–§2.16, §11, and §A.4–§A.7.
/// No schema contract is active: host-rooted imports, impl targets, and
/// path calls keep their host-style acceptance (the pre-schema behavior).
pub fn check(statements: &[Stmt]) -> Vec<Diagnostic> {
    check_with_schema(statements, None)
}

/// Type-checks a whole program against an ACTIVE schema contract
/// (WHITEPAPER §9). The schema enforces, at compile time:
///
/// - §2.5 boundary capitalization — a capitalized top-level declaration
///   must belong to the schema contract; script-internal ones are
///   camelCase;
/// - §2.3/§7.2 capability gating — host imports resolve against granted
///   namespaces, and capability calls are type-checked against the schema
///   member signatures;
/// - §9.5 version gating — members introduced after the program's target
///   version (a mod manifest's `[schemas]`, or the schema's own version
///   for loose sources) are hidden;
/// - §9.4 `requires` — capabilities call only when their prerequisite
///   interface is fully implemented, and implementing an interface pulls
///   in its own prerequisites;
/// - §10.4/§9.1 interface completeness — `impl <interface>` blocks must
///   implement every required visible member with the exact signature.
pub fn check_with_schema(statements: &[Stmt], schema: Option<&SchemaContext>) -> Vec<Diagnostic> {
    let mut checker = Checker::new_with_schema(schema);

    // Pass 1: register every top-level declaration. Forward references and
    // recursion resolve because every signature and type is registered
    // before any body or member is visited. Only the first registration of
    // a name wins: later duplicates are reported here and skipped
    // everywhere else, exactly like the calls that resolve to the first
    // registration.
    let mut function_bodies: Vec<usize> = Vec::new();
    let mut type_members: Vec<usize> = Vec::new();
    let mut impl_members: Vec<usize> = Vec::new();
    for (index, statement) in statements.iter().enumerate() {
        match &statement.kind {
            StmtKind::FuncDecl {
                name,
                params,
                return_ty,
                ..
            } => {
                checker.check_boundary_capitalization(name, statement.span);
                if checker.declaration_name_conflicts(name, statement.span) {
                    continue;
                }
                checker.check_duplicate_params(params, statement.span);
                checker.register_function(name, params, return_ty, statement.span);
                function_bodies.push(index);
            }
            StmtKind::StructDecl {
                name,
                type_params,
                fields,
            } => {
                checker.check_boundary_capitalization(name, statement.span);
                if checker.declaration_name_conflicts(name, statement.span) {
                    continue;
                }
                checker.register_struct(name, type_params, fields, statement.span);
                type_members.push(index);
            }
            StmtKind::EnumDecl {
                name,
                type_params,
                variants,
            } => {
                checker.check_boundary_capitalization(name, statement.span);
                if checker.declaration_name_conflicts(name, statement.span) {
                    continue;
                }
                checker.register_enum(name, type_params, variants, statement.span);
                type_members.push(index);
            }
            StmtKind::ImplDecl { target, members } => {
                checker.register_impl(target, members, statement.span);
                impl_members.push(index);
            }
            // Resolution against the mod tree belongs to the mod loader
            // (§10.3); `self` imports resolve there. Host-rooted imports
            // resolve against the schema contract when one is active
            // (§2.3, §7.2), so every import is collected here.
            StmtKind::Import { path } => {
                checker.imports.push((path.clone(), statement.span));
            }
            // Already reported at parse level; never cascaded here.
            StmtKind::Invalid { .. } => {}
            _ => checker.report(
                "only function, type, and impl declarations are allowed at top level",
                statement.span,
            ),
        }
    }

    // Pass 2: bodies and members, each against its own signature and fresh
    // scopes. A duplicate's body is not checked: calls resolve to the first
    // registration, so a dead definition's errors would only be noise on
    // top of the duplicate report.
    for index in function_bodies {
        if let StmtKind::FuncDecl {
            name, params, body, ..
        } = &statements[index].kind
        {
            checker.check_function(name, params, body);
        }
    }
    for index in type_members {
        checker.check_type_members(&statements[index]);
    }
    // Impl member bodies check like function bodies, under the target's
    // qualified name (§10.4). Members of an unregistered (reported) target
    // are skipped: nothing can call them, so their errors would only pile
    // on top of the target report.
    for index in impl_members {
        if let StmtKind::ImplDecl { target, members } = &statements[index].kind {
            checker.check_impl_members(target, members);
        }
    }

    // Pass 3 (schema active): the §9 contract checks that need the FULL
    // picture — every impl block registered, every signature resolved.
    checker.check_schema_boundaries();

    checker.diagnostics
}

/// The static type checker. `schema` carries the ACTIVE host contract
/// (WHITEPAPER §9); `None` keeps the host-style acceptance of imports,
/// impl targets, and path calls that predates the schema system.
struct Checker<'a> {
    diagnostics: Vec<Diagnostic>,
    functions: HashMap<String, FnSig>,
    /// The registry of type declarations, in registration order. Indices 0
    /// and 1 are the built-in `option` and `result`.
    types: Vec<TypeDef>,
    type_index: HashMap<String, usize>,
    /// Impl member signatures (§10.4), keyed by the joined target path
    /// (`counter`, `engine.gamemode`) then by member name. Blocks for the
    /// same target union here; a member implemented twice is rejected at
    /// registration.
    impls: HashMap<String, HashMap<String, FnSig>>,
    /// The declaration span of the FIRST impl block per target path, for
    /// schema diagnostics that point at the `impl` site (§10.4).
    impl_spans: HashMap<String, Span>,
    /// The span of each implemented impl member, for exact signature
    /// diagnostics.
    impl_member_spans: HashMap<String, HashMap<String, Span>>,
    /// Every host-rooted or self-rooted import, in declaration order
    /// (§2.3). Imports resolve against the schema contract when one is
    /// active.
    imports: Vec<(Vec<String>, Span)>,
    /// The active schema contract, when the host granted one.
    schema: Option<&'a SchemaContext>,
    /// Scope stack for the function currently being checked. Index 0 holds
    /// the parameters together with the body's top-level statements
    /// (redeclaring a parameter there is a duplicate, not a shadow).
    scopes: Vec<HashMap<String, Ty>>,
    /// The function whose body is being checked, for `return` and `?`
    /// validation.
    current_fn: Option<(String, Ty)>,
}

impl<'a> Checker<'a> {
    /// A checker with the schema types pre-registered (§9.3): every struct
    /// and enum of every GRANTED namespace joins the type registry before
    /// any script declaration, so script signatures, capability calls, and
    /// impl members resolve against the boundary types exactly like
    /// script-local ones. Type registration is poison-free by
    /// construction — the schema parser already validated the shapes.
    fn new_with_schema(schema: Option<&'a SchemaContext>) -> Self {
        let mut checker = Self {
            diagnostics: Vec::new(),
            functions: HashMap::new(),
            types: Vec::new(),
            type_index: HashMap::new(),
            impls: HashMap::new(),
            impl_spans: HashMap::new(),
            impl_member_spans: HashMap::new(),
            imports: Vec::new(),
            schema,
            scopes: Vec::new(),
            current_fn: None,
        };
        checker.register_builtin(
            "option",
            &["T"],
            &[("Some", &[("T", "value")]), ("None", &[])],
        );
        checker.register_builtin(
            "result",
            &["T", "E"],
            &[("Ok", &[("T", "value")]), ("Err", &[("E", "error")])],
        );
        if let Some(schema) = schema {
            checker.register_schema_types(schema);
        }
        checker
    }

    /// Registers the §9.3 boundary types of every granted namespace.
    fn register_schema_types(&mut self, schema: &SchemaContext) {
        for file in schema.set.namespaces() {
            if schema.target(&file.namespace).is_none() {
                continue;
            }
            for item in &file.items {
                match item {
                    cme_core::schema::SchemaItem::Struct(decl) => {
                        let idx = self.types.len();
                        self.types.push(TypeDef::Struct(StructDef {
                            name: decl.name.clone(),
                            params: Vec::new(),
                            fields: decl.fields.clone(),
                        }));
                        self.type_index.insert(decl.name.clone(), idx);
                    }
                    cme_core::schema::SchemaItem::Enum(decl) => {
                        let idx = self.types.len();
                        self.types.push(TypeDef::Enum(EnumDef {
                            name: decl.name.clone(),
                            params: Vec::new(),
                            variants: decl.variants.clone(),
                        }));
                        self.type_index.insert(decl.name.clone(), idx);
                    }
                    cme_core::schema::SchemaItem::Contract(_) => {}
                }
            }
        }
    }

    /// Registers one built-in generic enum (§2.8).
    fn register_builtin(
        &mut self,
        name: &str,
        params: &[&str],
        variants: &[(&str, &[(&str, &str)])],
    ) {
        let def = EnumDef {
            name: name.to_string(),
            params: params.iter().map(|p| p.to_string()).collect(),
            variants: variants
                .iter()
                .map(|(variant, fields)| VariantDecl {
                    name: variant.to_string(),
                    fields: fields
                        .iter()
                        .map(|(ty, fname)| FieldDef {
                            ty: Type::Named {
                                name: ty.to_string(),
                                args: Vec::new(),
                            },
                            name: fname.to_string(),
                        })
                        .collect(),
                })
                .collect(),
        };
        let idx = self.types.len();
        self.types.push(TypeDef::Enum(def));
        self.type_index.insert(name.to_string(), idx);
    }

    /// §2.5 boundary capitalization, enforced against the active schema
    /// contract: a capitalized top-level declaration is a boundary
    /// declaration, and boundary declarations belong to the schema (a
    /// script's interface functions live inside `impl` blocks). With no
    /// schema active the rule has nothing to check against — the
    /// pre-schema behavior is kept.
    fn check_boundary_capitalization(&mut self, name: &str, span: Span) {
        if self.schema.is_none() {
            return;
        }
        let mut chars = name.chars();
        let is_capitalized = matches!(chars.next(), Some(first) if first.is_ascii_uppercase());
        if is_capitalized {
            self.report(
                format!(
                    "top-level `{name}` is capitalized: boundary declarations belong to the \
                     schema contract; script-internal declarations are camelCase (§2.5)"
                ),
                span,
            );
        }
    }

    // -----------------------------------------------------------------
    // §9 schema contract enforcement
    // -----------------------------------------------------------------

    /// Pass 3: the schema checks that need every registration in place —
    /// import resolution (§2.3/§7.2), interface implementation against the
    /// contract (§9.1/§10.4), and `requires` edges (§9.4).
    fn check_schema_boundaries(&mut self) {
        if self.schema.is_none() {
            return;
        }
        self.check_schema_imports();
        self.check_schema_impls();
    }

    /// Validates every host-rooted import against the granted namespaces
    /// (§2.3: imports grant capability namespaces; §7.2: host access is
    /// exclusively through them).
    fn check_schema_imports(&mut self) {
        let imports = std::mem::take(&mut self.imports);
        for (path, span) in &imports {
            if path.first().map(String::as_str) == Some("self") {
                // §10.3: resolved by the mod loader, already checked there.
                continue;
            }
            let namespace = &path[0];
            let Some(schema) = self.schema else {
                return;
            };
            if !schema.grants(namespace) {
                if schema.set.namespace(namespace).is_some() {
                    self.report(
                        format!(
                            "schema namespace `{namespace}` is not granted to this program: \
                             a mod declares its target versions in `[schemas]` (§9.5, §10.2), \
                             and the host must register the schema file"
                        ),
                        *span,
                    );
                } else {
                    self.report(
                        format!(
                            "unknown schema namespace `{namespace}`: no registered schema \
                             declares it (§9.2)"
                        ),
                        *span,
                    );
                }
                continue;
            }
            match path.len() {
                1 => {}
                2 => {
                    let Some(file) = schema.set.namespace(namespace) else {
                        continue;
                    };
                    if let Some(capability) = file.capability(&path[1]) {
                        // §9.4 rule 2: importing a capability whose
                        // prerequisite interface is not fully implemented is
                        // already a contract violation — the whitepaper's
                        // "cannot import or call" — so the gate fires here,
                        // before any call site exists. A namespace-only
                        // import (`import engine`) stays legal: the call
                        // gate catches actual uses of the capability.
                        if let Some(requires) = &capability.requires {
                            let prerequisite = requires.qualified(namespace);
                            if !self.interface_satisfied(&prerequisite) {
                                self.report(
                                    format!(
                                        "importing `{}` requires `{prerequisite}`: the program \
                                         must fully implement `{prerequisite}` before importing \
                                         or calling this capability (§9.4)",
                                        path.join(".")
                                    ),
                                    *span,
                                );
                            }
                        }
                    } else if file.interface(&path[1]).is_some() {
                        self.report(
                            format!(
                                "`{}` is an interface: implement it with `impl {}` — \
                                 interfaces are called by the host, not imported (§9.1)",
                                path.join("."),
                                path.join(".")
                            ),
                            *span,
                        );
                    } else {
                        self.report(
                            format!("`{namespace}` has no capability `{}` (§9.1)", path[1]),
                            *span,
                        );
                    }
                }
                _ => {
                    self.report(
                        format!(
                            "import paths name a namespace or `namespace.capability`, \
                             not `{}` (§2.3)",
                            path.join(".")
                        ),
                        *span,
                    );
                }
            }
        }
        self.imports = imports;
    }

    /// Validates every impl target that names a schema contract (§9.1,
    /// §10.4): members must exist in the interface with the exact schema
    /// signature, every required visible member must be implemented, and
    /// `requires` edges must be satisfied (§9.4).
    fn check_schema_impls(&mut self) {
        let targets: Vec<(String, Span)> = self
            .impl_spans
            .iter()
            .map(|(target, span)| (target.clone(), *span))
            .collect();
        for (target, span) in targets {
            let Some((namespace, contract_name)) = target.split_once('.') else {
                continue;
            };
            let Some(schema) = self.schema else {
                return;
            };
            let Some(target_version) = schema.target(namespace) else {
                // §9.5/§7.2: an impl against a namespace the program was
                // never granted is outside the contract. Silently skipping
                // it would ship an interface the schema never validated —
                // no completeness, no signature checks — so both the
                // ungranted and the unknown case are reported. An impl is
                // not an import: the import diagnostics above do not fire
                // for impl-only programs.
                if schema.set.namespace(namespace).is_some() {
                    self.report(
                        format!(
                            "schema namespace `{namespace}` is not granted to this program: \
                             `{target}` cannot be implemented (§9.5, §7.2)"
                        ),
                        span,
                    );
                } else {
                    self.report(
                        format!(
                            "unknown schema namespace `{namespace}`: no registered schema \
                             declares it, so `{target}` cannot be implemented (§9.2)"
                        ),
                        span,
                    );
                }
                continue;
            };
            let Some(file) = schema.set.namespace(namespace) else {
                continue;
            };
            if let Some(capability) = file.capability(contract_name) {
                self.report(
                    format!(
                        "cannot implement capability `{namespace}.{}`: capabilities are \
                         provided by the host; scripts implement interfaces (§9.1)",
                        capability.name
                    ),
                    span,
                );
                continue;
            }
            let Some(contract) = file.interface(contract_name) else {
                self.report(
                    format!(
                        "unknown interface `{target}`: `{namespace}.{contract_name}` is not \
                         declared by the registered schema (§9.1)"
                    ),
                    span,
                );
                continue;
            };

            // Implemented-member validation: existence (and visibility,
            // §9.5) plus the exact signature (§10.4).
            let implemented = self.impls.get(&target).cloned().unwrap_or_default();
            let member_spans = self
                .impl_member_spans
                .get(&target)
                .cloned()
                .unwrap_or_default();
            for (name, sig) in &implemented {
                let Some(member) = contract.members.iter().find(|m| &m.name == name) else {
                    self.report(
                        format!(
                            "`{target}.{name}` is not a member of the schema interface: \
                             expected one of {} (§9.1)",
                            contract
                                .members
                                .iter()
                                .map(|m| m.name.as_str())
                                .collect::<Vec<_>>()
                                .join(", ")
                        ),
                        member_spans.get(name).copied().unwrap_or(span),
                    );
                    continue;
                };
                if !member.visible_at(target_version) {
                    self.report(
                        format!(
                            "`{target}.{name}` was introduced in schema version {}, but this \
                             program targets {} — the member is hidden (§9.5)",
                            member.since, target_version
                        ),
                        member_spans.get(name).copied().unwrap_or(span),
                    );
                    continue;
                }
                self.check_impl_signature(&target, member, sig, member_spans.get(name).copied());
            }

            // Completeness: every required visible member present (§10.4,
            // §9.5 — optional members may be skipped).
            let missing: Vec<String> = contract
                .members
                .iter()
                .filter(|member| {
                    member.requirement == MemberRequirement::Required
                        && member.visible_at(target_version)
                        && !implemented.contains_key(&member.name)
                })
                .map(|member| {
                    format!(
                        "{} {}({})",
                        type_display(&member.return_ty),
                        member.name,
                        member
                            .params
                            .iter()
                            .map(|param| format!("{} {}", type_display(&param.ty), param.name))
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                })
                .collect();
            if !missing.is_empty() {
                self.report(
                    format!(
                        "interface `{target}` is not fully implemented: missing {} (§10.4) — \
                         `optional` members may be skipped, required ones may not",
                        missing.join(", ")
                    ),
                    span,
                );
            }

            // §9.4 rule 1: implementing an interface requires fully
            // implementing its prerequisite. The diagnostic anchors at the
            // IMPL site (the program text the host renders); the schema's
            // own `requires` span belongs to the schema file's coordinates.
            if let Some(requires) = &contract.requires {
                let qualified = requires.qualified(namespace);
                if !self.interface_satisfied(&qualified) {
                    self.report(
                        format!(
                            "`{target}` requires `{qualified}`: a mod cannot implement an \
                             interface without fully implementing its prerequisite (§9.4)"
                        ),
                        span,
                    );
                }
            }
        }
    }

    /// The §10.4 exactness rule: an implemented member must match the
    /// schema signature — parameter count, parameter types, and return
    /// type. Parameter names are the implementation's own business (the
    /// host calls members positionally through generated proxies).
    fn check_impl_signature(
        &mut self,
        target: &str,
        member: &SchemaMember,
        sig: &FnSig,
        span: Option<Span>,
    ) {
        let span = span.unwrap_or(Span::new(0, 0));
        if sig.poisoned || sig.params.iter().any(|(_, ty)| ty.is_poison()) {
            return; // already reported
        }
        let schema_sig = self.schema_member_sig(member, span);
        if schema_sig.poisoned {
            return;
        }
        let mut problems: Vec<String> = Vec::new();
        if sig.params.len() != schema_sig.params.len() {
            problems.push(format!(
                "takes {} parameter(s), the schema declares {}",
                sig.params.len(),
                schema_sig.params.len()
            ));
        } else {
            for (index, ((_, actual), (_, expected))) in
                sig.params.iter().zip(&schema_sig.params).enumerate()
            {
                if actual != expected {
                    problems.push(format!(
                        "parameter {} is `{}`, the schema declares `{}`",
                        index + 1,
                        actual.name(&self.types),
                        expected.name(&self.types)
                    ));
                }
            }
        }
        if sig.return_ty != schema_sig.return_ty {
            problems.push(format!(
                "returns `{}`, the schema declares `{}`",
                sig.return_ty.name(&self.types),
                schema_sig.return_ty.name(&self.types)
            ));
        }
        if !problems.is_empty() {
            self.report(
                format!(
                    "`{target}.{}` does not match the schema signature: {} (§10.4)",
                    member.name,
                    problems.join("; ")
                ),
                span,
            );
        }
    }

    /// Whether `qualified` (`engine.auth`) is fully implemented: every
    /// required member visible at the interface's target version has an
    /// impl entry (§9.4). Ungranted or unknown interfaces are never
    /// satisfied.
    fn interface_satisfied(&self, qualified: &str) -> bool {
        let Some(members) = self.interface_members_visible(qualified) else {
            return false;
        };
        let Some(registry) = self.impls.get(qualified) else {
            return false;
        };
        members
            .iter()
            .filter(|member| member.requirement == MemberRequirement::Required)
            .all(|member| registry.contains_key(&member.name))
    }

    /// The visible (§9.5) members of `namespace.interface`, or `None` when
    /// the path does not name an interface of a granted namespace.
    fn interface_members_visible(&self, qualified: &str) -> Option<Vec<&'a SchemaMember>> {
        let schema = self.schema?;
        let (namespace, interface) = qualified.split_once('.')?;
        let target = schema.target(namespace)?;
        let file = schema.set.namespace(namespace)?;
        let contract = file.interface(interface)?;
        Some(
            contract
                .members
                .iter()
                .filter(|member| member.visible_at(target))
                .collect(),
        )
    }

    /// Resolves a schema member's signature against the registry (schema
    /// types registered first, so every reference resolves).
    fn schema_member_sig(&mut self, member: &SchemaMember, span: Span) -> FnSig {
        let params: Vec<(String, Ty)> = member
            .params
            .iter()
            .map(|param| (param.name.clone(), self.resolve_type(&param.ty, span)))
            .collect();
        let return_ty = self.resolve_type(&member.return_ty, span);
        let poisoned = params.iter().any(|(_, ty)| ty.is_poison()) || return_ty.is_poison();
        FnSig {
            params,
            return_ty,
            poisoned,
        }
    }

    fn report(&mut self, message: impl Into<String>, span: Span) {
        self.diagnostics.push(Diagnostic::type_error(message, span));
    }

    /// The display name of a registry type (for messages).
    /// True when the name collides with an existing function, type, or
    /// reserved builtin name; the collision is reported here.
    fn declaration_name_conflicts(&mut self, name: &str, span: Span) -> bool {
        if BUILTIN_CONSTRUCTORS.contains(&name) || RESERVED_TYPE_NAMES.contains(&name) {
            self.report(format!("`{name}` is a reserved builtin name"), span);
            return true;
        }
        if self.functions.contains_key(name) {
            self.report(format!("duplicate function `{name}`"), span);
            return true;
        }
        if self.type_index.contains_key(name) {
            self.report(format!("duplicate type `{name}`"), span);
            return true;
        }
        false
    }

    fn check_duplicate_params(&mut self, params: &[Param], span: Span) {
        let mut seen: Vec<&str> = Vec::new();
        for param in params {
            if seen.contains(&param.name.as_str()) {
                self.report(format!("duplicate parameter `{}`", param.name), span);
            } else {
                seen.push(&param.name);
            }
        }
    }

    fn register_function(&mut self, name: &str, params: &[Param], return_ty: &Type, span: Span) {
        let resolved: Vec<(String, Ty)> = params
            .iter()
            .map(|param| (param.name.clone(), self.resolve_type(&param.ty, span)))
            .collect();
        let ret = self.resolve_type(return_ty, span);
        let poisoned = resolved.iter().any(|(_, ty)| ty.is_poison()) || ret.is_poison();
        self.functions.insert(
            name.to_string(),
            FnSig {
                params: resolved,
                return_ty: ret,
                poisoned,
            },
        );
    }

    fn register_struct(
        &mut self,
        name: &str,
        type_params: &[String],
        fields: &[FieldDef],
        span: Span,
    ) {
        self.check_duplicate_type_params(name, type_params, span);
        let mut seen: Vec<&str> = Vec::new();
        for field in fields {
            if seen.contains(&field.name.as_str()) {
                self.report(
                    format!("duplicate field `{}` in struct `{name}`", field.name),
                    span,
                );
            } else {
                seen.push(&field.name);
            }
        }
        let idx = self.types.len();
        self.types.push(TypeDef::Struct(StructDef {
            name: name.to_string(),
            params: type_params.to_vec(),
            fields: fields.to_vec(),
        }));
        self.type_index.insert(name.to_string(), idx);
    }

    fn register_enum(
        &mut self,
        name: &str,
        type_params: &[String],
        variants: &[VariantDecl],
        span: Span,
    ) {
        self.check_duplicate_type_params(name, type_params, span);
        let mut seen: Vec<&str> = Vec::new();
        for variant in variants {
            if seen.contains(&variant.name.as_str()) {
                self.report(
                    format!("duplicate variant `{}` in enum `{name}`", variant.name),
                    span,
                );
            } else {
                seen.push(&variant.name);
                let mut bound: Vec<&str> = Vec::new();
                for field in &variant.fields {
                    if bound.contains(&field.name.as_str()) {
                        self.report(
                            format!(
                                "duplicate payload name `{}` in variant `{}` of enum `{name}`",
                                field.name, variant.name
                            ),
                            span,
                        );
                    } else {
                        bound.push(&field.name);
                    }
                }
            }
        }
        let idx = self.types.len();
        self.types.push(TypeDef::Enum(EnumDef {
            name: name.to_string(),
            params: type_params.to_vec(),
            variants: variants.to_vec(),
        }));
        self.type_index.insert(name.to_string(), idx);
    }

    fn check_duplicate_type_params(&mut self, name: &str, params: &[String], span: Span) {
        let mut seen: Vec<&str> = Vec::new();
        for param in params {
            if seen.contains(&param.as_str()) {
                self.report(
                    format!("duplicate type parameter `{param}` in `{name}`"),
                    span,
                );
            } else {
                seen.push(param);
            }
        }
    }

    /// Registers one impl block (§10.4). A single-segment target must be a
    /// locally declared, non-generic, non-builtin struct or enum; a dotted
    /// path is a host-style namespace target — the members still check and
    /// run, while interface completeness against a schema stays host work.
    /// Members register under the joined target path; a member implemented
    /// twice (across any blocks for the target) is rejected here, and so is
    /// a member colliding with a variant of the same enum (variant
    /// resolution always wins, so the member could never be called).
    fn register_impl(&mut self, target: &[String], members: &[Stmt], span: Span) {
        let joined = target.join(".");
        // The impl site span feeds the §9 checks (unknown interface,
        // completeness, requires) even when this registration rejects the
        // block — the contract errors belong at the `impl` keyword.
        self.impl_spans.entry(joined.clone()).or_insert(span);
        if target.len() == 1 {
            let name = &target[0];
            if RESERVED_TYPE_NAMES.contains(&name.as_str()) {
                self.report(format!("cannot implement the builtin type `{name}`"), span);
                return;
            }
            match self.type_index.get(name) {
                Some(&idx) => {
                    if !self.types[idx].params().is_empty() {
                        self.report(
                            format!(
                                "impl blocks on generic types are not supported yet; \
                                 `{name}` declares type parameters"
                            ),
                            span,
                        );
                        return;
                    }
                }
                None => {
                    if self.functions.contains_key(name) {
                        self.report(
                            format!("impl target must be a struct or enum type, but `{name}` is a function"),
                            span,
                        );
                    } else {
                        self.report(format!("unknown impl target `{name}`"), span);
                    }
                    return;
                }
            }
        } else if self.type_index.contains_key(&target[0]) {
            // Dotted targets are host-style namespaces; piggybacking on a
            // local type name would tangle the two resolution spaces.
            self.report(
                format!(
                    "impl target `{joined}` must not extend the local type `{}`",
                    target[0]
                ),
                span,
            );
            return;
        }

        // Collect this block's signatures first, then merge: the borrow of
        // `self.impls` must not outlive the `self.report`/`resolve_type`
        // calls inside the loop.
        let mut fresh: Vec<(String, FnSig)> = Vec::new();
        for member in members {
            let StmtKind::FuncDecl {
                name,
                params,
                return_ty,
                ..
            } = &member.kind
            else {
                // The parser already enforces function members; only a
                // hand-built tree reaches this arm.
                self.report("impl members must be function declarations", member.span);
                continue;
            };
            let taken =
                |fresh: &Vec<(String, FnSig)>| fresh.iter().any(|(existing, _)| existing == name);
            if self
                .impls
                .get(&joined)
                .is_some_and(|registry| registry.contains_key(name))
                || taken(&fresh)
            {
                self.report(
                    format!("duplicate impl member `{joined}.{name}`"),
                    member.span,
                );
                continue;
            }
            if target.len() == 1
                && let Some(&idx) = self.type_index.get(&target[0])
                && !self.types[idx].is_struct()
                && self
                    .enum_def(idx)
                    .variants
                    .iter()
                    .any(|variant| variant.name == *name)
            {
                self.report(
                    format!(
                        "impl member `{joined}.{name}` collides with a variant of `{}`",
                        target[0]
                    ),
                    member.span,
                );
                continue;
            }
            self.check_duplicate_params(params, member.span);
            let resolved: Vec<(String, Ty)> = params
                .iter()
                .map(|param| {
                    (
                        param.name.clone(),
                        self.resolve_type(&param.ty, member.span),
                    )
                })
                .collect();
            let ret = self.resolve_type(return_ty, member.span);
            let poisoned = resolved.iter().any(|(_, ty)| ty.is_poison()) || ret.is_poison();
            self.impl_member_spans
                .entry(joined.clone())
                .or_default()
                .insert(name.clone(), member.span);
            fresh.push((
                name.clone(),
                FnSig {
                    params: resolved,
                    return_ty: ret,
                    poisoned,
                },
            ));
        }
        self.impls.entry(joined).or_default().extend(fresh);
    }

    /// Pass-2 validation of impl member bodies (§10.4): each member checks
    /// like a function body, scoped under its qualified display name.
    /// Members rejected at registration are skipped — nothing can call
    /// them, so their body errors would only pile on the registration
    /// report.
    fn check_impl_members(&mut self, target: &[String], members: &[Stmt]) {
        let joined = target.join(".");
        if !self.impls.contains_key(&joined) {
            return;
        }
        for member in members {
            let StmtKind::FuncDecl { name, body, .. } = &member.kind else {
                continue;
            };
            let Some(sig) = self
                .impls
                .get(&joined)
                .and_then(|registry| registry.get(name))
                .cloned()
            else {
                continue;
            };
            let display = format!("{joined}.{name}");
            self.check_function_body(&display, &sig, body);
            self.check_impl_member_lost_mutations(name, &sig, body);
        }
    }

    /// The §10.4 TODO the whitepaper itself carries: an impl member body
    /// that mutates a value-copied parameter and drops the change —
    /// "`cme` has to error here". The host caller's state silently does
    /// not update (§2.13 value semantics), and unlike a script-internal
    /// call site the loss is invisible at every caller.
    ///
    /// Scope, deliberately precise: the member returns `void` — nothing a
    /// void member computes can escape, so ANY mutation of a structured
    /// parameter (struct, enum, array, map) is definitionally lost. A
    /// non-void member's mutation may feed its result instead
    /// (`c.value -= 1; return c.value + counter.sumTo(c)` reads the local
    /// clone deliberately), and primitives plus `str` have no interior to
    /// mutate — a lost rebinding there is an ordinary dead store, not
    /// silent state loss. The conservative cut keeps the diagnostic free
    /// of false positives while pinning the exact shape the whitepaper
    /// annotates.
    fn check_impl_member_lost_mutations(&mut self, member: &str, sig: &FnSig, body: &Block) {
        if sig.return_ty != Ty::Void {
            return;
        }
        let reportable: Vec<&str> = sig
            .params
            .iter()
            .filter(|(_, ty)| {
                matches!(
                    ty,
                    Ty::Struct(_, _) | Ty::Enum(_, _) | Ty::Array(_) | Ty::Map(_, _)
                )
            })
            .map(|(name, _)| name.as_str())
            .collect();
        if reportable.is_empty() {
            return;
        }

        let mut returned: Vec<String> = Vec::new();
        let mut mutations: Vec<(String, Span)> = Vec::new();
        Self::collect_impl_member_mutation_facts(
            &body.stmts,
            &reportable,
            &mut returned,
            &mut mutations,
        );

        for (name, span) in mutations {
            if !returned.contains(&name) {
                self.report(
                    format!(
                        "impl member `{member}` mutates parameter `{name}`, but the change is \
                         lost when the call returns (§2.13 value semantics); return the updated \
                         state instead of mutating in place"
                    ),
                    span,
                );
            }
        }
    }

    /// One recursive walk collecting both facts the lost-mutation rule
    /// needs: the first mutation site per reportable parameter root, and
    /// every reportable parameter the body returns bare (or wrapped).
    fn collect_impl_member_mutation_facts(
        statements: &[Stmt],
        reportable: &[&str],
        returned: &mut Vec<String>,
        mutations: &mut Vec<(String, Span)>,
    ) {
        for statement in statements {
            match &statement.kind {
                StmtKind::Assign { target, .. } | StmtKind::CompoundAssign { target, .. } => {
                    if let Some(root) = Self::lvalue_root(target)
                        && reportable.contains(&root)
                        && !mutations.iter().any(|(name, _)| name == root)
                    {
                        mutations.push((root.to_string(), statement.span));
                    }
                }
                StmtKind::Return { value: Some(expr) } => {
                    if let Some(name) = Self::returned_param(expr, reportable)
                        && !returned.iter().any(|n| n == name)
                    {
                        returned.push(name.to_string());
                    }
                }
                StmtKind::If {
                    then_branch,
                    else_branch,
                    ..
                } => {
                    Self::collect_impl_member_mutation_facts(
                        &then_branch.stmts,
                        reportable,
                        returned,
                        mutations,
                    );
                    if let Some(else_stmt) = else_branch {
                        Self::collect_impl_member_mutation_facts(
                            std::slice::from_ref(else_stmt),
                            reportable,
                            returned,
                            mutations,
                        );
                    }
                }
                StmtKind::While { body, .. } | StmtKind::For { body, .. } => {
                    Self::collect_impl_member_mutation_facts(
                        &body.stmts,
                        reportable,
                        returned,
                        mutations,
                    );
                }
                StmtKind::Match { arms, .. } => {
                    for arm in arms {
                        Self::collect_impl_member_mutation_facts(
                            &arm.body.stmts,
                            reportable,
                            returned,
                            mutations,
                        );
                    }
                }
                StmtKind::Block(block) => {
                    Self::collect_impl_member_mutation_facts(
                        &block.stmts,
                        reportable,
                        returned,
                        mutations,
                    );
                }
                _ => {}
            }
        }
    }

    /// The root name of an assignment target chain (`state.score[0]` →
    /// `state`).
    fn lvalue_root(lvalue: &LValue) -> Option<&str> {
        match lvalue {
            LValue::Var { name } => Some(name),
            LValue::Field { base, .. } | LValue::Index { base, .. } => Self::lvalue_root(base),
        }
    }

    /// Whether a returned expression hands a reportable parameter back to
    /// the caller: a bare `return state`, or the parameter as the sole
    /// payload of a constructor call (`return Ok(state)`,
    /// `return Some(state)`).
    fn returned_param<'b>(expr: &'b Expr, reportable: &[&'b str]) -> Option<&'b str> {
        match &expr.kind {
            ExprKind::Ident(name) if reportable.contains(&name.as_str()) => Some(name.as_str()),
            // `Ok(state)` / `Err(state)` / `Some(state)`: the built-in
            // constructors parse as plain calls (the checker disambiguates
            // them from user functions), and they carry the parameter back
            // to the caller just like a bare return.
            ExprKind::Call { name, args }
                if BUILTIN_CONSTRUCTORS.contains(&name.as_str()) && args.len() == 1 =>
            {
                match &args[0] {
                    CallArg::Positional(value) => match &value.kind {
                        ExprKind::Ident(pname) if reportable.contains(&pname.as_str()) => {
                            Some(pname.as_str())
                        }
                        _ => None,
                    },
                    CallArg::Named { .. } => None,
                }
            }
            _ => None,
        }
    }

    /// Pass-2 validation of a type declaration's member types: every type
    /// reference must resolve, and type parameters must be used bare.
    fn check_type_members(&mut self, statement: &Stmt) {
        let span = statement.span;
        match &statement.kind {
            StmtKind::StructDecl {
                type_params,
                fields,
                ..
            } => {
                for field in fields {
                    self.validate_decl_type(&field.ty, type_params, span);
                }
            }
            StmtKind::EnumDecl {
                type_params,
                variants,
                ..
            } => {
                for variant in variants {
                    for field in &variant.fields {
                        self.validate_decl_type(&field.ty, type_params, span);
                    }
                }
            }
            _ => {}
        }
    }

    fn validate_decl_type(&mut self, ty: &Type, params: &[String], span: Span) {
        match ty {
            Type::Array(elem) => self.validate_decl_type(elem, params, span),
            Type::Map { key, value } => {
                self.validate_decl_type(key, params, span);
                self.validate_decl_type(value, params, span);
            }
            Type::Named { name, args } => {
                // §8.5 (text-level deviation): `code` — the compile-time
                // syntax-fragment type — degenerates to its textual payload
                // at runtime, so the checker treats it as `str`.
                if name == "code" {
                    if !args.is_empty() {
                        self.report("`code` takes no type arguments".to_string(), span);
                        return;
                    }
                    return;
                }
                if params.iter().any(|p| p == name) {
                    if !args.is_empty() {
                        self.report(
                            format!("type parameter `{name}` cannot take arguments"),
                            span,
                        );
                    }
                    return;
                }
                match self.type_index.get(name) {
                    None => self.report(format!("unknown type `{name}`"), span),
                    Some(&idx) => {
                        let expected = self.types[idx].params().len();
                        if args.len() != expected {
                            self.report(
                                format!(
                                    "wrong number of type arguments for `{name}`: expected {expected}, found {}",
                                    args.len()
                                ),
                                span,
                            );
                        }
                    }
                }
                for arg in args {
                    self.validate_decl_type(arg, params, span);
                }
            }
            _ => {}
        }
    }

    /// Resolves a declared type at a use site (no type parameters in
    /// scope).
    fn resolve_type(&mut self, ty: &Type, span: Span) -> Ty {
        self.subst_type(ty, &[], &[], span)
    }

    /// Resolves a declared type with `params` substituted by `args` (§2.9).
    /// An unbound parameter resolves to `Poison` — the enclosing
    /// declaration's members are validated separately.
    fn subst_type(&mut self, ty: &Type, params: &[String], args: &[Ty], span: Span) -> Ty {
        match ty {
            Type::Infer => Ty::Poison,
            Type::Void => Ty::Void,
            Type::Prim(PrimitiveType::Int) => Ty::Int,
            Type::Prim(PrimitiveType::Float) => Ty::Float,
            Type::Prim(PrimitiveType::Bool) => Ty::Bool,
            Type::Prim(PrimitiveType::Str) => Ty::Str,
            Type::Array(elem) => Ty::Array(Box::new(self.subst_type(elem, params, args, span))),
            Type::Map { key, value } => Ty::Map(
                Box::new(self.subst_type(key, params, args, span)),
                Box::new(self.subst_type(value, params, args, span)),
            ),
            Type::Named {
                name,
                args: type_args,
            } => {
                // §8.5 (text-level deviation): the compile-time `code` type
                // is a checked alias of `str` — a `code` value IS its text.
                if name == "code" {
                    if !type_args.is_empty() {
                        self.report("`code` takes no type arguments".to_string(), span);
                        return Ty::Poison;
                    }
                    return Ty::Str;
                }
                if let Some(pos) = params.iter().position(|p| p == name) {
                    if !type_args.is_empty() {
                        self.report(
                            format!("type parameter `{name}` cannot take arguments"),
                            span,
                        );
                        return Ty::Poison;
                    }
                    return args.get(pos).cloned().unwrap_or(Ty::Poison);
                }
                let Some(&idx) = self.type_index.get(name) else {
                    self.report(format!("unknown type `{name}`"), span);
                    return Ty::Poison;
                };
                let expected = self.types[idx].params().len();
                if type_args.len() != expected {
                    self.report(
                        format!(
                            "wrong number of type arguments for `{name}`: expected {expected}, found {}",
                            type_args.len()
                        ),
                        span,
                    );
                    return Ty::Poison;
                }
                let resolved: Vec<Ty> = type_args
                    .iter()
                    .map(|arg| self.subst_type(arg, params, args, span))
                    .collect();
                if self.types[idx].is_struct() {
                    Ty::Struct(idx, resolved)
                } else {
                    Ty::Enum(idx, resolved)
                }
            }
        }
    }

    fn check_function(&mut self, name: &str, _params: &[Param], body: &Block) {
        let Some(sig) = self.functions.get(name).cloned() else {
            return;
        };
        self.check_function_body(name, &sig, body);
    }

    /// Checks one function-shaped body against a registered signature —
    /// shared by top-level functions and impl members (§10.4), which pass
    /// their qualified display name.
    fn check_function_body(&mut self, name: &str, sig: &FnSig, body: &Block) {
        self.current_fn = Some((name.to_string(), sig.return_ty.clone()));

        let mut scope = HashMap::new();
        for (param_name, ty) in &sig.params {
            scope.insert(param_name.clone(), ty.clone());
        }
        self.scopes.push(scope);

        for statement in &body.stmts {
            self.check_stmt(statement);
        }

        // §2.11 + the owner ruling: a non-void function must not be able
        // to fall off the end. `while` and `for` never count, `if` without
        // `else` does not count, `if`/`else` counts only when both branches
        // transfer control, and a `match` counts only when every arm does.
        if sig.return_ty != Ty::Void && !block_returns(body) {
            self.report(
                format!("missing return in non-void function `{name}`"),
                body.span,
            );
        }

        self.scopes.pop();
        self.current_fn = None;
    }

    fn check_block(&mut self, block: &Block) {
        self.scopes.push(HashMap::new());
        for statement in &block.stmts {
            self.check_stmt(statement);
        }
        self.scopes.pop();
    }

    /// Declares `name` in the innermost scope, reporting redeclaration in
    /// the same scope and shadowing of an enclosing scope (owner ruling:
    /// shadowing is forbidden). A declaration enters scope *after* its
    /// own initializer, so callers type the initializer first.
    fn declare(&mut self, name: &str, ty: Ty, span: Span) {
        if let Some(index) = self
            .scopes
            .iter()
            .rposition(|scope| scope.contains_key(name))
        {
            if index + 1 == self.scopes.len() {
                self.report(format!("duplicate declaration of `{name}`"), span);
            } else {
                self.report(
                    format!("declaration of `{name}` shadows a declaration in an enclosing scope"),
                    span,
                );
            }
        }
        if let Some(scope) = self.scopes.last_mut() {
            scope.insert(name.to_string(), ty);
        }
    }

    fn lookup(&self, name: &str) -> Option<Ty> {
        self.scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name).cloned())
    }

    fn check_stmt(&mut self, stmt: &Stmt) {
        match &stmt.kind {
            StmtKind::Invalid { .. } => {}
            // Nested declarations parse (tolerance) but are illegal: only
            // the top level declares functions and types. The body of the
            // illegal declaration is skipped, and calls to its name stay
            // unknown (it never registers).
            StmtKind::FuncDecl { .. } => {
                self.report(
                    "function declarations are only allowed at top level",
                    stmt.span,
                );
            }
            StmtKind::StructDecl { .. } | StmtKind::EnumDecl { .. } => {
                self.report("type declarations are only allowed at top level", stmt.span);
            }
            // Top-level impl blocks are consumed by the registration pass
            // (§10.4); one nested in a body is illegal for the same reason
            // as a nested function.
            StmtKind::ImplDecl { .. } => {
                self.report("impl blocks are only allowed at top level", stmt.span);
            }
            // Imports are top-level declarations (§2.3); a nested one cannot
            // take part in mod resolution.
            StmtKind::Import { .. } => {
                self.report("imports are only allowed at top level", stmt.span);
            }
            StmtKind::VarDecl { ty, name, expr } => {
                // §2.16: the initializer is typed before the name exists,
                // so `int a = a` reports the unknown name. Where the
                // declared type is known it threads into the initializer
                // so bare constructors crystallize (§2.8).
                match ty {
                    Type::Infer => {
                        let init = self.type_expr(expr, None);
                        match init {
                            Ty::Void => {
                                self.report(
                                    format!("cannot infer type for '{name}'; void initializer"),
                                    stmt.span,
                                );
                                self.declare(name, Ty::Poison, stmt.span);
                            }
                            Ty::Ambiguous => {
                                self.report(
                                    format!(
                                        "cannot infer type for '{name}'; ambiguous initializer"
                                    ),
                                    stmt.span,
                                );
                                self.declare(name, Ty::Poison, stmt.span);
                            }
                            other => self.declare(name, other, stmt.span),
                        }
                    }
                    // §2.4: void returns no value; parse rejects this from
                    // source, hand-built trees get the same message here.
                    Type::Void => {
                        self.type_expr(expr, None);
                        self.report("`void` is only valid as a function return type", stmt.span);
                        self.declare(name, Ty::Poison, stmt.span);
                    }
                    _ => {
                        let declared = self.resolve_type(ty, stmt.span);
                        let expected = if declared.is_poison() {
                            None
                        } else {
                            Some(declared.clone())
                        };
                        let init = self.type_expr(expr, expected.as_ref());
                        if !init.is_poison() && !declared.is_poison() && init != declared {
                            self.report(
                                format!(
                                    "type mismatch in declaration of `{name}`: expected `{}`, found `{}`",
                                    declared.name(&self.types),
                                    init.name(&self.types)
                                ),
                                stmt.span,
                            );
                        }
                        // The declaration survives a broken initializer with
                        // its declared type — that is the recovery design.
                        self.declare(name, declared, stmt.span);
                    }
                }
            }
            StmtKind::Assign { target, expr } => {
                let target_ty = self.resolve_lvalue(target, stmt.span);
                let expected = if target_ty.is_poison() {
                    None
                } else {
                    Some(target_ty.clone())
                };
                let rhs = self.type_expr(expr, expected.as_ref());
                if !target_ty.is_poison() && !rhs.is_poison() && rhs != target_ty {
                    self.report(
                        format!(
                            "type mismatch in assignment to `{}`: expected `{}`, found `{}`",
                            lvalue_name(target),
                            target_ty.name(&self.types),
                            rhs.name(&self.types)
                        ),
                        stmt.span,
                    );
                }
            }
            StmtKind::CompoundAssign { target, op, expr } => {
                // §A.7: `x op= e` is exactly `x = x op e`, so the operator
                // rules of §A.4/§A.6 apply with the target as the left
                // operand and the result must equal the target's type.
                let target_ty = self.resolve_lvalue(target, stmt.span);
                let rhs = self.type_expr(expr, None);
                if !target_ty.is_poison() && !rhs.is_poison() {
                    let matches = binary_result(compound_to_binary(*op), &target_ty, &rhs)
                        .is_some_and(|result| result == target_ty);
                    if !matches {
                        self.report(
                            format!(
                                "cannot apply `{}` to `{}` and `{}`",
                                compound_op_symbol(*op),
                                target_ty.name(&self.types),
                                rhs.name(&self.types)
                            ),
                            stmt.span,
                        );
                    }
                }
            }
            StmtKind::Expression { expr } => match &expr.kind {
                ExprKind::Call { .. }
                | ExprKind::VariantCall { .. }
                | ExprKind::PathCall { .. } => {
                    self.type_expr(expr, None);
                }
                ExprKind::Invalid { .. } => {}
                // Owner ruling: only call-shaped expression statements.
                _ => self.report("expression statements must be function calls", stmt.span),
            },
            StmtKind::If {
                cond,
                then_branch,
                else_branch,
            } => {
                let cond_ty = self.type_expr(cond, None);
                if !cond_ty.is_poison() && cond_ty != Ty::Bool {
                    self.report(
                        format!(
                            "if condition must be `bool`, found `{}`",
                            cond_ty.name(&self.types)
                        ),
                        cond.span,
                    );
                }
                self.check_block(then_branch);
                if let Some(else_stmt) = else_branch {
                    self.check_stmt(else_stmt);
                }
            }
            StmtKind::While { cond, body } => {
                let cond_ty = self.type_expr(cond, None);
                if !cond_ty.is_poison() && cond_ty != Ty::Bool {
                    self.report(
                        format!(
                            "while condition must be `bool`, found `{}`",
                            cond_ty.name(&self.types)
                        ),
                        cond.span,
                    );
                }
                self.check_block(body);
            }
            StmtKind::For {
                elem_ty,
                elem_name,
                iterable,
                body,
            } => self.check_for(elem_ty, elem_name, iterable, body, stmt.span),
            StmtKind::Match { scrutinee, arms } => {
                self.check_match_statement(scrutinee, arms, stmt.span)
            }
            StmtKind::Return { value } => self.check_return(stmt, value.as_ref()),
            StmtKind::Block(block) => self.check_block(block),
        }
    }

    fn check_for(
        &mut self,
        elem_ty: &Type,
        elem_name: &str,
        iterable: &Expr,
        body: &Block,
        span: Span,
    ) {
        let declared = self.resolve_type(elem_ty, span);
        let iter_ty = self.type_expr(iterable, None);
        // §1.4.10 (plan): iterating a map yields its keys — the compile-time
        // helpers walk capture record fields this way.
        let element = match iter_ty {
            Ty::Array(elem) => {
                if !declared.is_poison() && declared != *elem {
                    self.report(
                        format!(
                            "wrong element type in for loop: expected `{}`, found `{}`",
                            declared.name(&self.types),
                            elem.name(&self.types)
                        ),
                        span,
                    );
                }
                *elem
            }
            Ty::Map(key, _) => *key,
            Ty::Poison | Ty::Ambiguous => Ty::Poison,
            other => {
                self.report(
                    format!("cannot iterate `{}`", other.name(&self.types)),
                    iterable.span,
                );
                Ty::Poison
            }
        };

        // The loop variable binds per-iteration in the body's scope — and
        // honors the owner ruling `declare` enforces everywhere else:
        // shadowing an enclosing declaration is forbidden. The binding
        // survives so the loop body still checks.
        if self
            .scopes
            .iter()
            .any(|scope| scope.contains_key(elem_name))
        {
            self.report(
                format!("declaration of `{elem_name}` shadows a declaration in an enclosing scope"),
                span,
            );
        }
        let mut scope = HashMap::new();
        scope.insert(
            elem_name.to_string(),
            if element.is_poison() {
                declared
            } else {
                element
            },
        );
        self.scopes.push(scope);
        for statement in &body.stmts {
            self.check_stmt(statement);
        }
        self.scopes.pop();
    }

    fn check_match_statement(
        &mut self,
        scrutinee: &Expr,
        arms: &[cme_core::ast::MatchArmStmt],
        span: Span,
    ) {
        let scr_ty = self.type_expr(scrutinee, None);
        let (idx, enum_args) = match self.matchable_enum(&scr_ty, scrutinee.span) {
            Some(pair) => pair,
            None => return,
        };
        let def = self.enum_def(idx);
        let mut covered: HashSet<String> = HashSet::new();
        let mut wildcard = false;
        for arm in arms {
            match &arm.pattern {
                Pattern::Wildcard => wildcard = true,
                Pattern::Variant { variant, bindings } => {
                    if covered.contains(variant) {
                        self.report(
                            format!("duplicate arm for variant `{variant}` in `{}`", def.name),
                            span,
                        );
                        continue;
                    }
                    covered.insert(variant.clone());
                    let pattern_scope =
                        self.check_pattern(variant, bindings, idx, &enum_args, &def, span);
                    self.scopes.push(pattern_scope);
                    for statement in &arm.body.stmts {
                        self.check_stmt(statement);
                    }
                    self.scopes.pop();
                }
            }
        }
        self.check_exhaustiveness(&def, &covered, wildcard, span);
    }

    /// True (with the registry index and type arguments) when the
    /// scrutinee type is an enum; reports and returns `None` otherwise.
    fn matchable_enum(&mut self, ty: &Ty, span: Span) -> Option<(usize, Vec<Ty>)> {
        match ty {
            Ty::Enum(idx, args) => Some((*idx, args.clone())),
            Ty::Poison | Ty::Ambiguous => None,
            other => {
                self.report(
                    format!(
                        "match scrutinee must be an enum type, found `{}`",
                        other.name(&self.types)
                    ),
                    span,
                );
                None
            }
        }
    }

    /// Validates one pattern against the scrutinee's enum and returns the
    /// bindings scope (§2.15): payload types are the variant's declared
    /// types with the scrutinee's type arguments substituted.
    fn check_pattern(
        &mut self,
        variant: &str,
        bindings: &[FieldDef],
        _idx: usize,
        enum_args: &[Ty],
        def: &EnumDef,
        span: Span,
    ) -> HashMap<String, Ty> {
        let Some(variant_def) = def.variants.iter().find(|v| v.name == variant) else {
            self.report(
                format!("unknown variant `{variant}` in `{}`", def.name),
                span,
            );
            return HashMap::new();
        };
        if bindings.len() != variant_def.fields.len() {
            self.report(
                format!(
                    "wrong number of bindings in pattern `{variant}`: expected {}, found {}",
                    variant_def.fields.len(),
                    bindings.len()
                ),
                span,
            );
            return HashMap::new();
        }
        let mut scope = HashMap::new();
        let mut seen: Vec<&str> = Vec::new();
        for (binding, field) in bindings.iter().zip(&variant_def.fields) {
            // The owner ruling applies to payload bindings like any other
            // declaration: shadowing an enclosing name is forbidden. The
            // binding still enters the arm's scope so the body keeps
            // checking without cascades.
            if self
                .scopes
                .iter()
                .any(|scope| scope.contains_key(binding.name.as_str()))
            {
                self.report(
                    format!(
                        "declaration of `{}` shadows a declaration in an enclosing scope",
                        binding.name
                    ),
                    span,
                );
            }
            let subst = self.subst_type(&field.ty, &def.params, enum_args, span);
            let declared = self.resolve_type(&binding.ty, span);
            if !declared.is_poison() && !subst.is_poison() && declared != subst {
                self.report(
                    format!(
                        "wrong type for payload `{}` in pattern `{variant}`: expected `{}`, found `{}`",
                        binding.name,
                        subst.name(&self.types),
                        declared.name(&self.types)
                    ),
                    span,
                );
            }
            if seen.contains(&binding.name.as_str()) {
                self.report(
                    format!(
                        "duplicate binding `{}` in pattern `{variant}`",
                        binding.name
                    ),
                    span,
                );
            } else {
                seen.push(&binding.name);
            }
            scope.insert(
                binding.name.clone(),
                if subst.is_poison() { declared } else { subst },
            );
        }
        scope
    }

    fn check_exhaustiveness(
        &mut self,
        def: &EnumDef,
        covered: &HashSet<String>,
        wildcard: bool,
        span: Span,
    ) {
        if wildcard {
            return;
        }
        let missing: Vec<&str> = def
            .variants
            .iter()
            .map(|v| v.name.as_str())
            .filter(|name| !covered.contains(*name))
            .collect();
        if !missing.is_empty() {
            self.report(
                format!(
                    "match on `{}` is not exhaustive: missing arms for {}",
                    def.name,
                    missing.join(", ")
                ),
                span,
            );
        }
    }

    fn check_return(&mut self, stmt: &Stmt, value: Option<&Expr>) {
        let Some((name, return_ty)) = self.current_fn.clone() else {
            return;
        };
        match return_ty {
            Ty::Void => {
                if let Some(expr) = value {
                    // Still resolve names inside the value.
                    self.type_expr(expr, None);
                    self.report(
                        format!("void function `{name}` cannot return a value"),
                        stmt.span,
                    );
                }
            }
            // An `infer` return type only reaches a hand-built tree (parse
            // rejects it); there is no expected type to compare against.
            Ty::Poison | Ty::Ambiguous => {
                if let Some(expr) = value {
                    self.type_expr(expr, None);
                }
            }
            expected => match value {
                None => self.report(
                    format!("non-void function `{name}` must return a value"),
                    stmt.span,
                ),
                Some(expr) => {
                    let actual = self.type_expr(expr, Some(&expected));
                    if !actual.is_poison() && actual != expected {
                        self.report(
                            format!(
                                "wrong return type in `{name}`: expected `{}`, found `{}`",
                                expected.name(&self.types),
                                actual.name(&self.types)
                            ),
                            stmt.span,
                        );
                    }
                }
            },
        }
    }

    /// Types an assignment target's variable/field/index chain and returns
    /// the type it denotes (§2.10, §2.13, §A.7).
    fn resolve_lvalue(&mut self, target: &LValue, span: Span) -> Ty {
        match target {
            LValue::Var { name } => match self.lookup(name) {
                Some(ty) => ty,
                None => {
                    self.report(format!("unknown name `{name}`"), span);
                    Ty::Poison
                }
            },
            LValue::Field { base, name } => {
                let base_ty = self.resolve_lvalue(base, span);
                match &base_ty {
                    Ty::Array(_) if name == "length" => {
                        self.report("cannot assign to `.length`", span);
                        Ty::Poison
                    }
                    Ty::Struct(idx, args) => {
                        let def = self.struct_def(*idx);
                        match def.fields.iter().find(|f| f.name == *name) {
                            Some(field) => self.subst_type(&field.ty, &def.params, args, span),
                            None => {
                                self.report(
                                    format!(
                                        "unknown field `{name}` on `{}`",
                                        base_ty.name(&self.types)
                                    ),
                                    span,
                                );
                                Ty::Poison
                            }
                        }
                    }
                    ty if ty.is_poison() => Ty::Poison,
                    _ => {
                        self.report(
                            format!("unknown field `{name}` on `{}`", base_ty.name(&self.types)),
                            span,
                        );
                        Ty::Poison
                    }
                }
            }
            LValue::Index { base, index } => {
                let base_ty = self.resolve_lvalue(base, span);
                let index_ty = self.type_expr(index, None);
                match &base_ty {
                    Ty::Array(elem) => {
                        if !index_ty.is_poison() && index_ty != Ty::Int {
                            self.report(
                                format!(
                                    "array index must be `int`, found `{}`",
                                    index_ty.name(&self.types)
                                ),
                                index.span,
                            );
                        }
                        (**elem).clone()
                    }
                    Ty::Map(key, value) => {
                        if !index_ty.is_poison() && index_ty != **key {
                            self.report(
                                format!(
                                    "map key must be `{}`, found `{}`",
                                    key.name(&self.types),
                                    index_ty.name(&self.types)
                                ),
                                index.span,
                            );
                        }
                        (**value).clone()
                    }
                    ty if ty.is_poison() => Ty::Poison,
                    _ => {
                        self.report(
                            format!("cannot index `{}`", base_ty.name(&self.types)),
                            span,
                        );
                        Ty::Poison
                    }
                }
            }
        }
    }

    /// The snapshot of a struct declaration from the registry.
    fn struct_def(&self, idx: usize) -> StructDef {
        match &self.types[idx] {
            TypeDef::Struct(def) => def.clone(),
            TypeDef::Enum(_) => unreachable!("index points at a struct"),
        }
    }

    /// The snapshot of an enum declaration from the registry.
    fn enum_def(&self, idx: usize) -> EnumDef {
        match &self.types[idx] {
            TypeDef::Enum(def) => def.clone(),
            TypeDef::Struct(_) => unreachable!("index points at an enum"),
        }
    }

    /// Types an expression, threading the expected type where bare
    /// constructors need it (§2.8, §2.16).
    fn type_expr(&mut self, expr: &Expr, expected: Option<&Ty>) -> Ty {
        match &expr.kind {
            ExprKind::IntLit(_) => Ty::Int,
            ExprKind::FloatLit(_) => Ty::Float,
            ExprKind::StrLit(_) => Ty::Str,
            ExprKind::BoolLit(_) => Ty::Bool,
            // Recovery placeholder: already reported at parse level.
            ExprKind::Invalid { .. } => Ty::Poison,
            ExprKind::Ident(name) => self.lookup(name).unwrap_or_else(|| {
                self.report(format!("unknown name `{name}`"), expr.span);
                Ty::Poison
            }),
            ExprKind::Paren { expr: inner } => self.type_expr(inner, expected),
            ExprKind::Call { name, args } => self.type_call(name, args, expected, expr.span),
            ExprKind::VariantCall {
                enum_name,
                variant,
                args,
            } => self.type_variant_call(enum_name, variant, args, expected, expr.span),
            ExprKind::PathCall { path, args } => {
                self.type_path_call(path, args, expected, expr.span)
            }
            ExprKind::Field { obj, name } => self.type_field(obj, name, expr.span),
            ExprKind::Index { obj, index } => self.type_index(obj, index, expr.span),
            ExprKind::Try { expr: inner } => self.type_try(inner, expr.span),
            ExprKind::Match { scrutinee, arms } => {
                self.type_match_expr(scrutinee, arms, expected, expr.span)
            }
            ExprKind::ArrayLit { elements } => self.type_array_lit(elements, expected, expr.span),
            ExprKind::MapLit { entries } => self.type_map_lit(entries, expected, expr.span),
            ExprKind::Interpolated { parts } => {
                for part in parts {
                    if let cme_core::ast::InterpPart::Expr(island) = part {
                        // §A.6: only scalars stringify.
                        let ty = self.type_expr(island, None);
                        match ty {
                            Ty::Int | Ty::Float | Ty::Bool | Ty::Str => {}
                            Ty::Poison | Ty::Ambiguous => {}
                            other => self.report(
                                format!("cannot interpolate `{}`", other.name(&self.types)),
                                island.span,
                            ),
                        }
                    }
                }
                Ty::Str
            }
            ExprKind::Unary { op, expr: inner } => {
                let operand = self.type_expr(inner, None);
                if operand.is_poison() {
                    return Ty::Poison;
                }
                let ok = match op {
                    UnaryOp::Neg => matches!(operand, Ty::Int | Ty::Float),
                    UnaryOp::Not => operand == Ty::Bool,
                };
                if !ok {
                    self.report(
                        format!(
                            "cannot apply `{}` to `{}`",
                            unary_op_symbol(*op),
                            operand.name(&self.types)
                        ),
                        expr.span,
                    );
                    return Ty::Poison;
                }
                operand
            }
            ExprKind::Binary { op, lhs, rhs } => {
                let left = self.type_expr(lhs, None);
                let right = self.type_expr(rhs, None);
                if left.is_poison() || right.is_poison() {
                    return Ty::Poison;
                }
                match binary_result(*op, &left, &right) {
                    Some(result) => result,
                    None => {
                        self.report(
                            format!(
                                "cannot apply `{}` to `{}` and `{}`",
                                binary_op_symbol(*op),
                                left.name(&self.types),
                                right.name(&self.types)
                            ),
                            expr.span,
                        );
                        Ty::Poison
                    }
                }
            }
        }
    }

    /// A call by name: a user function, a struct construction, or a
    /// built-in constructor — disambiguated by the registry (§2.6, §2.8,
    /// §2.11, §2.12).
    fn type_call(&mut self, name: &str, args: &[CallArg], expected: Option<&Ty>, span: Span) -> Ty {
        if let Some(sig) = self.functions.get(name).cloned() {
            return self.type_function_call(name, &sig, args, span);
        }
        if BUILTIN_CONSTRUCTORS.contains(&name) {
            return self.type_builtin_ctor(name, args, expected, span);
        }
        if let Some(&idx) = self.type_index.get(name) {
            if self.types[idx].is_struct() {
                let def = self.struct_def(idx);
                return self.type_struct_literal(&def, idx, args, expected, span);
            }
            // A bare enum name cannot construct: the call must name a
            // variant (§2.7).
            for arg in args {
                let expr = match arg {
                    CallArg::Positional(expr) | CallArg::Named { expr, .. } => expr,
                };
                self.type_expr(expr, None);
            }
            self.report(
                format!("enum `{name}` cannot be constructed directly; use `{name}.Variant(...)`"),
                span,
            );
            return Ty::Poison;
        }
        // Arguments are still typed so unknown names inside them are
        // reported rather than swallowed.
        for arg in args {
            let expr = match arg {
                CallArg::Positional(expr) | CallArg::Named { expr, .. } => expr,
            };
            self.type_expr(expr, None);
        }
        self.report(format!("unknown function `{name}`"), span);
        Ty::Poison
    }

    fn type_function_call(&mut self, name: &str, sig: &FnSig, args: &[CallArg], span: Span) -> Ty {
        let has_positional = args.iter().any(|arg| matches!(arg, CallArg::Positional(_)));
        let has_named = args.iter().any(|arg| matches!(arg, CallArg::Named { .. }));
        if has_positional && has_named {
            // Parse already rejects mixing (§2.12); a hand-built tree
            // reaching here is poisoned without a cascade.
            for arg in args {
                let expr = match arg {
                    CallArg::Positional(expr) | CallArg::Named { expr, .. } => expr,
                };
                self.type_expr(expr, None);
            }
            return Ty::Poison;
        }

        if has_named {
            // Named arguments bind by parameter name (§2.12).
            let named: Vec<(&str, &Expr)> = args
                .iter()
                .filter_map(|arg| match arg {
                    CallArg::Named { name, expr } => Some((name.as_str(), expr)),
                    CallArg::Positional(_) => None,
                })
                .collect();
            let mut provided: Vec<&str> = Vec::new();
            for (arg_name, value) in &named {
                if provided.contains(arg_name) {
                    self.report(
                        format!("duplicate argument `{arg_name}` in call to `{name}`"),
                        value.span,
                    );
                } else {
                    provided.push(arg_name);
                }
            }
            for (param_name, param_ty) in &sig.params {
                match named.iter().find(|(arg_name, _)| arg_name == param_name) {
                    None => self.report(
                        format!("missing argument `{param_name}` in call to `{name}`"),
                        span,
                    ),
                    Some((_, value)) => {
                        let expected = if param_ty.is_poison() || sig.poisoned {
                            None
                        } else {
                            Some(param_ty.clone())
                        };
                        let actual = self.type_expr(value, expected.as_ref());
                        if !sig.poisoned && !actual.is_poison() && actual != *param_ty {
                            self.report(
                                format!(
                                    "wrong argument type in call to `{name}`: expected `{}`, found `{}`",
                                    param_ty.name(&self.types),
                                    actual.name(&self.types)
                                ),
                                value.span,
                            );
                        }
                    }
                }
            }
            for (arg_name, value) in &named {
                if !sig.params.iter().any(|(param, _)| param == arg_name) {
                    self.report(
                        format!("unknown argument `{arg_name}` in call to `{name}`"),
                        value.span,
                    );
                }
            }
        } else {
            // Positional arguments bind in order (§2.12).
            let positional: Vec<&Expr> = args
                .iter()
                .filter_map(|arg| match arg {
                    CallArg::Positional(expr) => Some(expr),
                    CallArg::Named { .. } => None,
                })
                .collect();
            if positional.len() != sig.params.len() {
                self.report(
                    format!(
                        "wrong number of arguments to `{name}`: expected {}, found {}",
                        sig.params.len(),
                        positional.len()
                    ),
                    span,
                );
            }
            for (value, (_, param_ty)) in positional.iter().zip(&sig.params) {
                let expected = if param_ty.is_poison() || sig.poisoned {
                    None
                } else {
                    Some(param_ty.clone())
                };
                let actual = self.type_expr(value, expected.as_ref());
                if !sig.poisoned && !actual.is_poison() && actual != *param_ty {
                    self.report(
                        format!(
                            "wrong argument type in call to `{name}`: expected `{}`, found `{}`",
                            param_ty.name(&self.types),
                            actual.name(&self.types)
                        ),
                        value.span,
                    );
                }
            }
        }

        if sig.poisoned {
            Ty::Poison
        } else {
            sig.return_ty.clone()
        }
    }

    /// A struct construction: named arguments must cover the declared
    /// fields exactly, and generic arguments crystallize from the field
    /// values and the expected type (§2.6, §2.9, §2.16).
    fn type_struct_literal(
        &mut self,
        def: &StructDef,
        idx: usize,
        args: &[CallArg],
        expected: Option<&Ty>,
        span: Span,
    ) -> Ty {
        if args.iter().any(|arg| matches!(arg, CallArg::Positional(_))) {
            for arg in args {
                let expr = match arg {
                    CallArg::Positional(expr) | CallArg::Named { expr, .. } => expr,
                };
                self.type_expr(expr, None);
            }
            self.report(
                format!(
                    "construction of struct `{}` requires named arguments",
                    def.name
                ),
                span,
            );
            return Ty::Poison;
        }
        let named: Vec<(&str, &Expr)> = args
            .iter()
            .filter_map(|arg| match arg {
                CallArg::Named { name, expr } => Some((name.as_str(), expr)),
                CallArg::Positional(_) => None,
            })
            .collect();

        let mut seen: Vec<&str> = Vec::new();
        for (field_name, value) in &named {
            if seen.contains(field_name) {
                self.report(
                    format!(
                        "duplicate field `{field_name}` in construction of `{}`",
                        def.name
                    ),
                    value.span,
                );
            } else {
                seen.push(field_name);
            }
        }

        // Generic argument bindings: pre-filled from the expected type,
        // then crystallized from the field value types.
        let mut bindings: Vec<Option<Ty>> = vec![None; def.params.len()];
        if let Some(Ty::Struct(eidx, eargs)) = expected
            && *eidx == idx
        {
            for (slot, arg) in bindings.iter_mut().zip(eargs) {
                *slot = Some(arg.clone());
            }
        }

        // Phase A: type each provided value once, inferring bindings.
        let mut typed: Vec<(&str, Ty, Span)> = Vec::new();
        for (field_name, value) in &named {
            let field = def.fields.iter().find(|f| f.name == *field_name);
            let expected_for_value = field.and_then(|f| {
                let partial: Vec<Ty> = bindings
                    .iter()
                    .map(|b| b.clone().unwrap_or(Ty::Poison))
                    .collect();
                let partial_ty = self.peek_subst_type(&f.ty, &def.params, &partial);
                if partial_ty.has_poison() {
                    None
                } else {
                    Some(partial_ty)
                }
            });
            let actual = self.type_expr(value, expected_for_value.as_ref());
            if let Some(field) = field {
                self.bind_type_params(&field.ty, &actual, &def.params, &mut bindings);
            }
            typed.push((field_name, actual, value.span));
        }

        // Phase B: resolve the bindings; unbound arguments are an error.
        let mut resolved: Vec<Ty> = Vec::with_capacity(bindings.len());
        let mut unbound = false;
        for (slot, param) in bindings.iter().zip(&def.params) {
            match slot {
                Some(ty) => resolved.push(ty.clone()),
                None => {
                    self.report(
                        format!(
                            "cannot infer type argument `{param}` for struct `{}`",
                            def.name
                        ),
                        span,
                    );
                    unbound = true;
                    resolved.push(Ty::Poison);
                }
            }
        }
        if unbound {
            return Ty::Poison;
        }

        // Phase C: coverage and field types against the substituted types.
        for (field_name, actual, value_span) in &typed {
            let Some(field) = def.fields.iter().find(|f| f.name == *field_name) else {
                self.report(
                    format!(
                        "unknown field `{field_name}` in construction of `{}`",
                        def.name
                    ),
                    span,
                );
                continue;
            };
            let field_ty = self.subst_type(&field.ty, &def.params, &resolved, span);
            if !actual.is_poison() && *actual != field_ty {
                self.report(
                    format!(
                        "wrong type for field `{field_name}` in construction of `{}`: expected `{}`, found `{}`",
                        def.name,
                        field_ty.name(&self.types),
                        actual.name(&self.types)
                    ),
                    *value_span,
                );
            }
        }
        for field in &def.fields {
            if !named.iter().any(|(name, _)| *name == field.name) {
                self.report(
                    format!(
                        "missing field `{}` in construction of `{}`",
                        field.name, def.name
                    ),
                    span,
                );
            }
        }

        Ty::Struct(idx, resolved)
    }

    /// A substitution that never reports (for expected-type previews).
    fn peek_subst_type(&self, ty: &Type, params: &[String], args: &[Ty]) -> Ty {
        match ty {
            Type::Infer => Ty::Poison,
            Type::Void => Ty::Void,
            Type::Prim(PrimitiveType::Int) => Ty::Int,
            Type::Prim(PrimitiveType::Float) => Ty::Float,
            Type::Prim(PrimitiveType::Bool) => Ty::Bool,
            Type::Prim(PrimitiveType::Str) => Ty::Str,
            Type::Array(elem) => Ty::Array(Box::new(self.peek_subst_type(elem, params, args))),
            Type::Map { key, value } => Ty::Map(
                Box::new(self.peek_subst_type(key, params, args)),
                Box::new(self.peek_subst_type(value, params, args)),
            ),
            Type::Named {
                name,
                args: type_args,
            } => {
                // §8.5 (text-level deviation): `code` resolves as `str`.
                if name == "code" && type_args.is_empty() {
                    return Ty::Str;
                }
                if let Some(pos) = params.iter().position(|p| p == name) {
                    return args.get(pos).cloned().unwrap_or(Ty::Poison);
                }
                let resolved: Vec<Ty> = type_args
                    .iter()
                    .map(|arg| self.peek_subst_type(arg, params, args))
                    .collect();
                match self.type_index.get(name) {
                    Some(&idx) if self.types[idx].is_struct() => Ty::Struct(idx, resolved),
                    Some(&idx) => Ty::Enum(idx, resolved),
                    None => Ty::Poison,
                }
            }
        }
    }

    /// Crystallizes generic bindings by walking a declared field type
    /// against the actual value type (§2.9, §2.16).
    fn bind_type_params(
        &mut self,
        declared: &Type,
        actual: &Ty,
        params: &[String],
        bindings: &mut [Option<Ty>],
    ) {
        match declared {
            Type::Named { name, args } if params.iter().any(|p| p == name) => {
                if let Some(pos) = params.iter().position(|p| p == name) {
                    match (&bindings[pos], actual) {
                        (None, ty) if !ty.is_poison() => bindings[pos] = Some(ty.clone()),
                        _ => {}
                    }
                }
                let _ = args;
            }
            Type::Array(elem) => {
                if let Ty::Array(actual_elem) = actual {
                    self.bind_type_params(elem, actual_elem, params, bindings);
                }
            }
            Type::Map { key, value } => {
                if let Ty::Map(actual_key, actual_value) = actual {
                    self.bind_type_params(key, actual_key, params, bindings);
                    self.bind_type_params(value, actual_value, params, bindings);
                }
            }
            Type::Named { args, .. } => {
                // A generic field type like `A[]` nested in another generic:
                // zip the declared arguments with the actual ones.
                if let Some(actual_args) = actual.args_of() {
                    for (declared_arg, actual_arg) in args.iter().zip(actual_args) {
                        self.bind_type_params(declared_arg, &actual_arg.clone(), params, bindings);
                    }
                }
            }
            _ => {}
        }
    }

    /// A qualified enum construction `Enum.Variant(args)` (§2.7), including
    /// the qualified builtins `option.Some(...)` / `result.Ok(...)`.
    fn type_variant_call(
        &mut self,
        enum_name: &str,
        variant: &str,
        args: &[CallArg],
        expected: Option<&Ty>,
        span: Span,
    ) -> Ty {
        // Resolution order (§2.7, §10.4): an enum variant first, then an
        // impl member on the same target — `Type.name(args)` is a
        // construction when `Type` is an enum with a variant `name`, and a
        // member call otherwise.
        // §8.5 (text-level deviation): the compile-time builtin namespace —
        // `cm.parseExpr` / `cm.parseStmts` / `cm.parse` return `code` (the
        // checked alias of `str`); `cm.code.*` builders likewise. The
        // megaprogram evaluator answers these calls during expansion; at
        // runtime the helpers are dead code.
        if enum_name == "cm" {
            for arg in args {
                let expr = match arg {
                    CallArg::Positional(expr) | CallArg::Named { expr, .. } => expr,
                };
                self.type_expr(expr, None);
            }
            return Ty::Str;
        }
        if let Some(&idx) = self.type_index.get(enum_name) {
            if self.types[idx].is_struct() {
                // A struct target: structs construct by bare name (§2.6), so
                // a qualified call can only be an impl member.
                if let Some(sig) = self
                    .impls
                    .get(enum_name)
                    .and_then(|registry| registry.get(variant))
                    .cloned()
                {
                    let display = format!("{enum_name}.{variant}");
                    return self.type_function_call(&display, &sig, args, span);
                }
                for arg in args {
                    let expr = match arg {
                        CallArg::Positional(expr) | CallArg::Named { expr, .. } => expr,
                    };
                    self.type_expr(expr, None);
                }
                self.report(format!("unknown member `{variant}` in `{enum_name}`"), span);
                return Ty::Poison;
            }
            let def = self.enum_def(idx);
            if def
                .variants
                .iter()
                .any(|candidate| candidate.name == variant)
            {
                return self.type_variant_construction(&def, idx, variant, args, expected, span);
            }
            if let Some(sig) = self
                .impls
                .get(enum_name)
                .and_then(|registry| registry.get(variant))
                .cloned()
            {
                let display = format!("{enum_name}.{variant}");
                return self.type_function_call(&display, &sig, args, span);
            }
            for arg in args {
                let expr = match arg {
                    CallArg::Positional(expr) | CallArg::Named { expr, .. } => expr,
                };
                self.type_expr(expr, None);
            }
            self.report(
                format!("unknown variant `{variant}` in `{}`", def.name),
                span,
            );
            return Ty::Poison;
        }
        // Not a declared type: a single-segment host-style impl target, if
        // one carries this member.
        if let Some(sig) = self
            .impls
            .get(enum_name)
            .and_then(|registry| registry.get(variant))
            .cloned()
        {
            let display = format!("{enum_name}.{variant}");
            return self.type_function_call(&display, &sig, args, span);
        }
        for arg in args {
            let expr = match arg {
                CallArg::Positional(expr) | CallArg::Named { expr, .. } => expr,
            };
            self.type_expr(expr, None);
        }
        self.report(format!("unknown enum `{enum_name}`"), span);
        Ty::Poison
    }

    /// A call through a dotted path of three or more segments (§2.3,
    /// §10.4): the last segment names an impl member; the leading segments
    /// name its target. Host capability calls (`engine.graphics.DrawTexture`)
    /// resolve the same way once a host registers impls for the path.
    fn type_path_call(
        &mut self,
        path: &[String],
        args: &[CallArg],
        expected: Option<&Ty>,
        span: Span,
    ) -> Ty {
        let _ = expected; // Member signatures are concrete; nothing crystallizes.
        // §8.5 (text-level deviation): the compile-time builtin namespace —
        // `cm.parseExpr` / `cm.parseStmts` / `cm.parse` return `code` (the
        // checked alias of `str`); `cm.code.*` builders likewise. Arguments
        // are still type-checked. The megaprogram evaluator answers these
        // calls during expansion; at runtime the helpers are dead code.
        if path.first().map(String::as_str) == Some("cm") {
            for arg in args {
                let expr = match arg {
                    CallArg::Positional(expr) | CallArg::Named { expr, .. } => expr,
                };
                self.type_expr(expr, None);
            }
            return Ty::Str;
        }
        if path.len() >= 2 {
            let member = &path[path.len() - 1];
            let target = path[..path.len() - 1].join(".");
            if let Some(sig) = self
                .impls
                .get(&target)
                .and_then(|registry| registry.get(member))
                .cloned()
            {
                let display = format!("{target}.{member}");
                return self.type_function_call(&display, &sig, args, span);
            }
        }
        // §9.1/§7.2: a schema capability call — `engine.graphics.LoadTexture`.
        // Resolution, import gating, `requires` gating, and version gating
        // all happen against the ACTIVE contract; the call then type-checks
        // against the member's schema signature exactly like a script
        // function.
        if self.schema.is_some()
            && path.len() >= 2
            && let Some(ty) = self.type_schema_capability_call(path, args, span)
        {
            return ty;
        }
        for arg in args {
            let expr = match arg {
                CallArg::Positional(expr) | CallArg::Named { expr, .. } => expr,
            };
            self.type_expr(expr, None);
        }
        self.report(format!("unknown function `{}`", path.join(".")), span);
        Ty::Poison
    }

    /// Types a call into a schema capability member. Returns `Some` when
    /// the path names a capability of a GRANTED namespace (the call is
    /// then fully checked — errors reported, `Ty::Poison` on failure), and
    /// `None` when the path is not a capability call (letting the generic
    /// `unknown function` diagnostic handle it).
    fn type_schema_capability_call(
        &mut self,
        path: &[String],
        args: &[CallArg],
        span: Span,
    ) -> Option<Ty> {
        let schema = self.schema?;
        let namespace = &path[0];
        let target_version = schema.target(namespace)?;
        let file = schema.set.namespace(namespace)?;
        if path.len() == 2 {
            // `ns.name` — if `name` is an interface, give the pointed
            // diagnostic; otherwise this is not a capability call.
            if file.interface(&path[1]).is_some() {
                self.report(
                    format!(
                        "`{}` is an interface: the script implements it and the host calls \
                         in — scripts call capabilities (§9.1)",
                        path.join(".")
                    ),
                    span,
                );
                return Some(Ty::Poison);
            }
            return None;
        }
        if path.len() != 3 {
            return None;
        }
        let capability = file.capability(&path[1])?;
        let qualified = format!("{namespace}.{}", capability.name);

        // §2.3/§7.2: the capability must be imported (`import engine`,
        // `import engine.graphics`, or any import prefix of the call).
        let imported = self.imports.iter().any(|(import_path, _)| {
            import_path
                .iter()
                .zip(path.iter())
                .all(|(imported, called)| imported == called)
                && import_path.len() <= path.len()
                && !import_path.is_empty()
                && import_path[0] != "self"
        });
        if !imported {
            self.report(
                format!(
                    "call to `{}` requires importing the capability first: \
                     add `import {qualified}` (§2.3, §7.2)",
                    path.join(".")
                ),
                span,
            );
            // Still type-check the arguments for recovery quality.
            for arg in args {
                let expr = match arg {
                    CallArg::Positional(expr) | CallArg::Named { expr, .. } => expr,
                };
                self.type_expr(expr, None);
            }
            return Some(Ty::Poison);
        }

        // §9.4 rule 2: a capability with a `requires` edge calls only when
        // the prerequisite interface is fully implemented by this program.
        if let Some(requires) = &capability.requires {
            let prerequisite = requires.qualified(namespace);
            if !self.interface_satisfied(&prerequisite) {
                self.report(
                    format!(
                        "capability `{qualified}` requires `{prerequisite}`: the program must \
                         fully implement `{prerequisite}` before importing or calling (§9.4)"
                    ),
                    span,
                );
                for arg in args {
                    let expr = match arg {
                        CallArg::Positional(expr) | CallArg::Named { expr, .. } => expr,
                    };
                    self.type_expr(expr, None);
                }
                return Some(Ty::Poison);
            }
        }

        let member_name = &path[2];
        let Some(member) = capability.members.iter().find(|m| &m.name == member_name) else {
            self.report(
                format!(
                    "capability `{qualified}` has no member `{member_name}`: declared members \
                     are {} (§9.1)",
                    capability
                        .members
                        .iter()
                        .map(|m| m.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                span,
            );
            for arg in args {
                let expr = match arg {
                    CallArg::Positional(expr) | CallArg::Named { expr, .. } => expr,
                };
                self.type_expr(expr, None);
            }
            return Some(Ty::Poison);
        };
        if !member.visible_at(target_version) {
            self.report(
                format!(
                    "`{qualified}.{member_name}` was introduced in schema version {}, but this \
                     program targets {} — the member is hidden (§9.5)",
                    member.since, target_version
                ),
                span,
            );
            for arg in args {
                let expr = match arg {
                    CallArg::Positional(expr) | CallArg::Named { expr, .. } => expr,
                };
                self.type_expr(expr, None);
            }
            return Some(Ty::Poison);
        }
        let sig = self.schema_member_sig(member, span);
        let display = format!("{qualified}.{member_name}");
        Some(self.type_function_call(&display, &sig, args, span))
    }

    /// A bare built-in constructor: `Ok`, `Err`, `Some`, `None` (§2.8).
    fn type_builtin_ctor(
        &mut self,
        name: &str,
        args: &[CallArg],
        expected: Option<&Ty>,
        span: Span,
    ) -> Ty {
        let (enum_name, variant) = match name {
            "Ok" => ("result", "Ok"),
            "Err" => ("result", "Err"),
            "Some" => ("option", "Some"),
            _ => ("option", "None"),
        };
        let idx = self.type_index[enum_name];
        let def = self.enum_def(idx);
        self.type_variant_construction(&def, idx, variant, args, expected, span)
    }

    /// The shared typing of an enum variant construction: positional
    /// payload values, generic bindings crystallized from payloads and the
    /// expected type, and payload types checked against the substituted
    /// declarations (§2.7, §2.8, §2.9).
    fn type_variant_construction(
        &mut self,
        def: &EnumDef,
        idx: usize,
        variant: &str,
        args: &[CallArg],
        expected: Option<&Ty>,
        span: Span,
    ) -> Ty {
        if args.iter().any(|arg| matches!(arg, CallArg::Named { .. })) {
            for arg in args {
                let expr = match arg {
                    CallArg::Positional(expr) | CallArg::Named { expr, .. } => expr,
                };
                self.type_expr(expr, None);
            }
            self.report(
                format!(
                    "construction of `{}.{variant}` takes positional arguments",
                    def.name
                ),
                span,
            );
            return Ty::Poison;
        }
        let Some(variant_def) = def.variants.iter().find(|v| v.name == variant) else {
            for arg in args {
                let expr = match arg {
                    CallArg::Positional(expr) | CallArg::Named { expr, .. } => expr,
                };
                self.type_expr(expr, None);
            }
            self.report(
                format!("unknown variant `{variant}` in `{}`", def.name),
                span,
            );
            return Ty::Poison;
        };
        let positional: Vec<&Expr> = args
            .iter()
            .filter_map(|arg| match arg {
                CallArg::Positional(expr) => Some(expr),
                CallArg::Named { .. } => None,
            })
            .collect();
        if positional.len() != variant_def.fields.len() {
            self.report(
                format!(
                    "wrong number of payload values for `{}.{variant}`: expected {}, found {}",
                    def.name,
                    variant_def.fields.len(),
                    positional.len()
                ),
                span,
            );
            return Ty::Poison;
        }

        // Generic argument bindings: pre-filled from the expected type,
        // then crystallized from the payload value types.
        let mut bindings: Vec<Option<Ty>> = vec![None; def.params.len()];
        if let Some(Ty::Enum(eidx, eargs)) = expected
            && *eidx == idx
        {
            for (slot, arg) in bindings.iter_mut().zip(eargs) {
                *slot = Some(arg.clone());
            }
        }
        let mut typed: Vec<Ty> = Vec::with_capacity(positional.len());
        for (value, field) in positional.iter().zip(&variant_def.fields) {
            let expected_for_value = {
                let partial: Vec<Ty> = bindings
                    .iter()
                    .map(|b| b.clone().unwrap_or(Ty::Poison))
                    .collect();
                let partial_ty = self.peek_subst_type(&field.ty, &def.params, &partial);
                if partial_ty.has_poison() {
                    None
                } else {
                    Some(partial_ty)
                }
            };
            let actual = self.type_expr(value, expected_for_value.as_ref());
            self.bind_type_params(&field.ty, &actual, &def.params, &mut bindings);
            typed.push(actual);
        }
        let mut resolved: Vec<Ty> = Vec::with_capacity(bindings.len());
        let mut unbound = false;
        for slot in &bindings {
            match slot {
                Some(ty) => resolved.push(ty.clone()),
                None => {
                    unbound = true;
                    resolved.push(Ty::Poison);
                }
            }
        }
        if unbound {
            // The construction's type arguments cannot be determined here;
            // the context (a declaration, a parameter, a field) supplies
            // them. Without one, the type stays ambiguous (§2.16).
            return Ty::Ambiguous;
        }
        for (actual, field) in typed.iter().zip(&variant_def.fields) {
            let field_ty = self.subst_type(&field.ty, &def.params, &resolved, span);
            if !actual.is_poison() && *actual != field_ty {
                self.report(
                    format!(
                        "wrong type for payload `{}` of `{}.{variant}`: expected `{}`, found `{}`",
                        field.name,
                        def.name,
                        field_ty.name(&self.types),
                        actual.name(&self.types)
                    ),
                    span,
                );
            }
        }
        Ty::Enum(idx, resolved)
    }

    /// Field access (§2.6) and `.length` on arrays (§11).
    fn type_field(&mut self, obj: &Expr, name: &str, span: Span) -> Ty {
        let obj_ty = self.type_expr(obj, None);
        match &obj_ty {
            Ty::Array(_) if name == "length" => Ty::Int,
            Ty::Struct(idx, args) => {
                let def = self.struct_def(*idx);
                match def.fields.iter().find(|f| f.name == name) {
                    Some(field) => self.subst_type(&field.ty, &def.params, args, span),
                    None => {
                        self.report(
                            format!("unknown field `{name}` on `{}`", obj_ty.name(&self.types)),
                            span,
                        );
                        Ty::Poison
                    }
                }
            }
            ty if ty.is_poison() => Ty::Poison,
            _ => {
                self.report(
                    format!("unknown field `{name}` on `{}`", obj_ty.name(&self.types)),
                    span,
                );
                Ty::Poison
            }
        }
    }

    /// Indexing: arrays take `int` indices; maps take their key type (§11).
    fn type_index(&mut self, obj: &Expr, index: &Expr, span: Span) -> Ty {
        let obj_ty = self.type_expr(obj, None);
        let index_ty = self.type_expr(index, None);
        match &obj_ty {
            Ty::Array(elem) => {
                if !index_ty.is_poison() && index_ty != Ty::Int {
                    self.report(
                        format!(
                            "array index must be `int`, found `{}`",
                            index_ty.name(&self.types)
                        ),
                        index.span,
                    );
                }
                (**elem).clone()
            }
            Ty::Map(key, value) => {
                if !index_ty.is_poison() && index_ty != **key {
                    self.report(
                        format!(
                            "map key must be `{}`, found `{}`",
                            key.name(&self.types),
                            index_ty.name(&self.types)
                        ),
                        index.span,
                    );
                }
                (**value).clone()
            }
            ty if ty.is_poison() => Ty::Poison,
            _ => {
                self.report(format!("cannot index `{}`", obj_ty.name(&self.types)), span);
                Ty::Poison
            }
        }
    }

    /// The `?` operator (§2.8): the operand must be `result<T, E>` and the
    /// enclosing function must return `result<_, E>` with the exact same
    /// error type; the expression's type is the success type.
    fn type_try(&mut self, inner: &Expr, span: Span) -> Ty {
        let operand = self.type_expr(inner, None);
        let result_idx = self.type_index["result"];
        let match_result = match &operand {
            Ty::Enum(idx, ty_args) if *idx == result_idx && ty_args.len() == 2 => {
                Some((ty_args[0].clone(), ty_args[1].clone()))
            }
            Ty::Poison | Ty::Ambiguous => None,
            _ => {
                self.report(
                    format!(
                        "the `?` operator requires `result<T, E>`, found `{}`",
                        operand.name(&self.types)
                    ),
                    span,
                );
                None
            }
        };
        let Some((ok_ty, err_ty)) = match_result else {
            return Ty::Poison;
        };
        let Some((name, ret)) = self.current_fn.clone() else {
            return Ty::Poison;
        };
        match &ret {
            Ty::Enum(fidx, fargs) if *fidx == result_idx && fargs.len() == 2 => {
                let fn_err = fargs[1].clone();
                if err_ty.is_poison() || fn_err.is_poison() || err_ty == fn_err {
                    ok_ty
                } else {
                    self.report(
                        format!(
                            "`?` propagates error `{}` but function `{name}` returns errors of type `{}`",
                            err_ty.name(&self.types),
                            fn_err.name(&self.types)
                        ),
                        span,
                    );
                    Ty::Poison
                }
            }
            _ => {
                self.report(
                    format!(
                        "`?` requires the enclosing function `{name}` to return `result<_, E>`, found `{}`",
                        ret.name(&self.types)
                    ),
                    span,
                );
                Ty::Poison
            }
        }
    }

    /// A match in expression position (§2.15): every arm yields the same
    /// type, patterns bind their payloads, and the match is exhaustive.
    fn type_match_expr(
        &mut self,
        scrutinee: &Expr,
        arms: &[cme_core::ast::MatchArmExpr],
        expected: Option<&Ty>,
        span: Span,
    ) -> Ty {
        let scr_ty = self.type_expr(scrutinee, None);
        let Some((idx, enum_args)) = self.matchable_enum(&scr_ty, scrutinee.span) else {
            return Ty::Poison;
        };
        let def = self.enum_def(idx);
        let mut covered: HashSet<String> = HashSet::new();
        let mut wildcard = false;
        let mut result = Ty::Poison;
        for arm in arms {
            match &arm.pattern {
                Pattern::Wildcard => wildcard = true,
                Pattern::Variant { variant, bindings } => {
                    if covered.contains(variant) {
                        self.report(
                            format!("duplicate arm for variant `{variant}` in `{}`", def.name),
                            span,
                        );
                        continue;
                    }
                    covered.insert(variant.clone());
                    let pattern_scope =
                        self.check_pattern(variant, bindings, idx, &enum_args, &def, span);
                    self.scopes.push(pattern_scope);
                    let body_ty = self.type_expr(&arm.body, expected);
                    self.scopes.pop();
                    if result.is_poison() && !body_ty.is_poison() {
                        result = body_ty;
                    } else if !result.is_poison() && !body_ty.is_poison() && result != body_ty {
                        self.report(
                            format!(
                                "match arms yield different types: `{}` and `{}`",
                                result.name(&self.types),
                                body_ty.name(&self.types)
                            ),
                            arm.body.span,
                        );
                    }
                }
            }
        }
        self.check_exhaustiveness(&def, &covered, wildcard, span);
        result
    }

    /// An array literal (§11): elements unify to one type; an empty
    /// literal without an expected element type is ambiguous (§2.16).
    fn type_array_lit(&mut self, elements: &[Expr], expected: Option<&Ty>, span: Span) -> Ty {
        let elem_expected = match expected {
            Some(Ty::Array(elem)) => Some((**elem).clone()),
            _ => None,
        };
        if elements.is_empty() {
            return match elem_expected {
                Some(elem) => Ty::Array(Box::new(elem)),
                None => {
                    let _ = span;
                    Ty::Ambiguous
                }
            };
        }
        let mut elem_ty = Ty::Poison;
        for element in elements {
            let actual = self.type_expr(element, elem_expected.as_ref());
            if elem_ty.is_poison() {
                if !actual.is_poison() {
                    elem_ty = actual;
                }
            } else if !actual.is_poison() && actual != elem_ty {
                self.report(
                    format!(
                        "array elements must all have type `{}`, found `{}`",
                        elem_ty.name(&self.types),
                        actual.name(&self.types)
                    ),
                    element.span,
                );
            }
        }
        Ty::Array(Box::new(elem_ty))
    }

    /// A map literal (§11): keys and values each unify to one type; an
    /// empty literal without an expected type is ambiguous (§2.16).
    fn type_map_lit(&mut self, entries: &[(Expr, Expr)], expected: Option<&Ty>, span: Span) -> Ty {
        let (key_expected, value_expected) = match expected {
            Some(Ty::Map(key, value)) => ((**key).clone(), Some((**value).clone())),
            _ => (Ty::Poison, None),
        };
        let value_expected = value_expected.or(None);
        if entries.is_empty() {
            return match expected {
                Some(Ty::Map(key, value)) => {
                    Ty::Map(Box::new((**key).clone()), Box::new((**value).clone()))
                }
                _ => {
                    let _ = span;
                    Ty::Ambiguous
                }
            };
        }
        let mut key_ty = Ty::Poison;
        let mut value_ty = Ty::Poison;
        for (key, value) in entries {
            let key_expected_ref = if key_expected.is_poison() {
                None
            } else {
                Some(key_expected.clone())
            };
            let actual_key = self.type_expr(key, key_expected_ref.as_ref());
            if key_ty.is_poison() {
                if !actual_key.is_poison() {
                    key_ty = actual_key;
                }
            } else if !actual_key.is_poison() && actual_key != key_ty {
                self.report(
                    format!(
                        "map keys must all have type `{}`, found `{}`",
                        key_ty.name(&self.types),
                        actual_key.name(&self.types)
                    ),
                    key.span,
                );
            }
            let actual_value = self.type_expr(value, value_expected.as_ref());
            if value_ty.is_poison() {
                if !actual_value.is_poison() {
                    value_ty = actual_value;
                }
            } else if !actual_value.is_poison() && actual_value != value_ty {
                self.report(
                    format!(
                        "map values must all have type `{}`, found `{}`",
                        value_ty.name(&self.types),
                        actual_value.name(&self.types)
                    ),
                    value.span,
                );
            }
        }
        Ty::Map(Box::new(key_ty), Box::new(value_ty))
    }
}

impl Ty {
    /// The generic arguments of a registry type, for binding inference.
    fn args_of(&self) -> Option<&[Ty]> {
        match self {
            Ty::Struct(_, args) | Ty::Enum(_, args) => Some(args),
            _ => None,
        }
    }
}

impl TypeDef {
    fn is_struct(&self) -> bool {
        matches!(self, TypeDef::Struct(_))
    }
}

/// The base name of an lvalue, for assignment diagnostics.
fn lvalue_name(target: &LValue) -> String {
    match target {
        LValue::Var { name } => name.clone(),
        LValue::Field { base, name } => format!("{}.{name}", lvalue_name(base)),
        LValue::Index { base, .. } => format!("{}[...]", lvalue_name(base)),
    }
}

/// §A.4 operand typing. `None` means the operator does not accept the
/// operand types. `void` and poison are not values. `==`/`!=` accept any
/// same-type pair — struct and enum equality is structural (§A.4).
fn binary_result(op: BinaryOp, left: &Ty, right: &Ty) -> Option<Ty> {
    if left == &Ty::Void || right == &Ty::Void {
        return None;
    }
    if left.is_poison() || right.is_poison() {
        return None;
    }
    match op {
        BinaryOp::Add => match (left, right) {
            (Ty::Int, Ty::Int) => Some(Ty::Int),
            (Ty::Float, Ty::Float) => Some(Ty::Float),
            // §A.6: either side str (scalars only), the other stringifies.
            (Ty::Str, Ty::Str | Ty::Int | Ty::Float | Ty::Bool)
            | (Ty::Int | Ty::Float | Ty::Bool, Ty::Str) => Some(Ty::Str),
            _ => None,
        },
        BinaryOp::Sub | BinaryOp::Mul | BinaryOp::Div => match (left, right) {
            (Ty::Int, Ty::Int) => Some(Ty::Int),
            (Ty::Float, Ty::Float) => Some(Ty::Float),
            _ => None,
        },
        BinaryOp::Rem => match (left, right) {
            (Ty::Int, Ty::Int) => Some(Ty::Int),
            _ => None,
        },
        // §A.4: strict same-type value equality, structural for structs
        // and enums (Ty equality compares registry index and arguments).
        BinaryOp::Eq | BinaryOp::Ne => (left == right).then_some(Ty::Bool),
        BinaryOp::Lt | BinaryOp::Le | BinaryOp::Gt | BinaryOp::Ge => match (left, right) {
            (Ty::Int, Ty::Int) | (Ty::Float, Ty::Float) => Some(Ty::Bool),
            _ => None,
        },
        BinaryOp::And | BinaryOp::Or => match (left, right) {
            (Ty::Bool, Ty::Bool) => Some(Ty::Bool),
            _ => None,
        },
    }
}

/// §A.7: a compound assignment is exactly the corresponding binary
/// operator applied to the target and the right-hand side.
fn compound_to_binary(op: CompoundOp) -> BinaryOp {
    match op {
        CompoundOp::Add => BinaryOp::Add,
        CompoundOp::Sub => BinaryOp::Sub,
        CompoundOp::Mul => BinaryOp::Mul,
        CompoundOp::Div => BinaryOp::Div,
        CompoundOp::Rem => BinaryOp::Rem,
    }
}

fn compound_op_symbol(op: CompoundOp) -> &'static str {
    match op {
        CompoundOp::Add => "+=",
        CompoundOp::Sub => "-=",
        CompoundOp::Mul => "*=",
        CompoundOp::Div => "/=",
        CompoundOp::Rem => "%=",
    }
}

fn binary_op_symbol(op: BinaryOp) -> &'static str {
    match op {
        BinaryOp::Or => "||",
        BinaryOp::And => "&&",
        BinaryOp::Eq => "==",
        BinaryOp::Ne => "!=",
        BinaryOp::Lt => "<",
        BinaryOp::Le => "<=",
        BinaryOp::Gt => ">",
        BinaryOp::Ge => ">=",
        BinaryOp::Add => "+",
        BinaryOp::Sub => "-",
        BinaryOp::Mul => "*",
        BinaryOp::Div => "/",
        BinaryOp::Rem => "%",
    }
}

fn unary_op_symbol(op: UnaryOp) -> &'static str {
    match op {
        UnaryOp::Neg => "-",
        UnaryOp::Not => "!",
    }
}

/// True when executing `block` cannot fall off its end: some statement in
/// it transfers control (§2.14 plus the owner ruling). `while` and `for`
/// never count; `if` without `else` does not count; `if`/`else` counts
/// only when both branches transfer; a `match` counts only when every arm
/// transfers.
fn block_returns(block: &Block) -> bool {
    block.stmts.iter().any(stmt_transfers)
}

fn stmt_transfers(stmt: &Stmt) -> bool {
    match &stmt.kind {
        StmtKind::Return { .. } => true,
        StmtKind::Block(block) => block_returns(block),
        StmtKind::If {
            then_branch,
            else_branch,
            ..
        } => else_branch
            .as_ref()
            .is_some_and(|else_stmt| block_returns(then_branch) && stmt_transfers(else_stmt)),
        StmtKind::Match { arms, .. } => {
            !arms.is_empty() && arms.iter().all(|arm| block_returns(&arm.body))
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::check;
    use crate::diagnostics::Diagnostic;
    use crate::parse_source;
    use cme_core::Span;
    use cme_core::ast::{Block, Expr, ExprKind, Stmt, StmtKind, Type};

    const BASIC_CM: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../basic.cm"));

    fn check_source(source: &str) -> Vec<Diagnostic> {
        check(&parse_source(source).statements)
    }

    /// Parses + checks, asserting exactly one diagnostic whose message
    /// contains `substring` and whose span equals `span`.
    fn assert_error(source: &str, substring: &str, span: Span) {
        let diagnostics = check_source(source);
        assert_eq!(
            diagnostics.len(),
            1,
            "expected exactly one diagnostic: {diagnostics:#?}"
        );
        assert!(
            diagnostics[0].to_string().contains(substring),
            "message {:?} should contain {substring:?}",
            diagnostics[0].to_string()
        );
        assert_eq!(diagnostics[0].span(), span);
    }

    /// Span helper: `source[start..end]` located by substring.
    fn span_of(source: &str, needle: &str) -> Span {
        let start = source
            .find(needle)
            .unwrap_or_else(|| panic!("{needle:?} not in source"));
        Span::new(start, start + needle.len())
    }

    #[test]
    fn basic_cm_type_checks_clean() {
        let diagnostics = check_source(BASIC_CM);
        assert!(
            diagnostics.is_empty(),
            "check(parse_source(basic.cm)) must be empty: {diagnostics:#?}"
        );
    }

    /// The full pipeline the CLI runs: parse (lexer, recovery, post-parse
    /// validation) plus the type checker. A healthy program produces an
    /// empty list; a program with one defect produces exactly one
    /// diagnostic, no matter where in the tree the defect hides.
    fn full_pipeline(source: &str) -> Vec<Diagnostic> {
        let outcome = parse_source(source);
        let mut diagnostics = outcome.diagnostics;
        diagnostics.extend(check(&outcome.statements));
        diagnostics
    }

    #[test]
    fn mixed_logic_inside_a_function_is_reported_exactly_once() {
        // Regression for the silent-validator hotfix: `return a || b && c`
        // inside a function body used to produce NO diagnostic from the
        // full pipeline because the validator never descended into
        // function bodies (Appendix A §A.3 Rule 1 must fire there).
        let source = "bool f(bool a, bool b, bool c) {\nreturn a || b && c\n}\n";
        let diagnostics = full_pipeline(source);
        assert_eq!(
            diagnostics.len(),
            1,
            "expected exactly one diagnostic: {diagnostics:#?}"
        );
        assert_eq!(
            diagnostics[0].to_string(),
            "mixed && and || require parentheses"
        );
        assert_eq!(diagnostics[0].span(), span_of(source, "b && c"));
    }

    #[test]
    fn forward_references_and_recursion_resolve() {
        let source =
            "int main() {\nreturn helper(2) + main()\n}\nint helper(int n) {\nreturn n\n}\n";
        assert!(check_source(source).is_empty());
    }

    #[test]
    fn string_concatenation_stringifies_per_a6() {
        // ": " + value with value: int is legal (§A.6).
        let source = "str label(int value) {\nstr s = \": \" + value\nreturn s\n}\n";
        assert!(check_source(source).is_empty());
    }

    #[test]
    fn compound_assignment_with_stringification_is_legal() {
        // status += 100 ≡ status = status + 100 → str + int → str (§A.7, §A.6).
        let source = "int f() {\nstr status = \"start\"\nstatus += 100\nreturn 0\n}\n";
        assert!(check_source(source).is_empty());
    }

    #[test]
    fn nested_scopes_with_distinct_names_are_legal() {
        let source = "int f() {\nint x = 1\nif (true) {\nint y = 2\ny = y + x\n}\nreturn x\n}\n";
        assert!(check_source(source).is_empty());
    }

    #[test]
    fn for_loop_variable_may_not_shadow_an_enclosing_declaration() {
        // The owner ruling (shadowing is forbidden) covers the for loop's
        // element binding like any other declaration.
        let source = "int f() {\nint x = 1\nfor (int x in [1, 2]) {\nx += 1\n}\nreturn x\n}\n";
        assert_error(
            source,
            "declaration of `x` shadows a declaration in an enclosing scope",
            span_of(source, "for (int x in [1, 2]) {\nx += 1\n}"),
        );
        // A distinct element name is legal.
        let source = "int f() {\nint x = 1\nfor (int v in [1, 2]) {\nx += v\n}\nreturn x\n}\n";
        assert!(check_source(source).is_empty());
    }

    #[test]
    fn match_pattern_binding_may_not_shadow_an_enclosing_declaration() {
        let source = "enum opt2 {\nSome(int value)\nNone()\n}\nint f(opt2 e) {\nint value = 5\nmatch (e) {\nSome(int value) => { value += 1 }\nNone() => {}\n}\nreturn value\n}\n";
        assert_error(
            source,
            "declaration of `value` shadows a declaration in an enclosing scope",
            span_of(
                source,
                "match (e) {\nSome(int value) => { value += 1 }\nNone() => {}\n}",
            ),
        );
        // The same rule holds in match EXPRESSION position.
        let source = "enum opt2 {\nSome(int v)\nNone()\n}\nint f(opt2 e) {\nint v = 5\nint r = match (e) {\nSome(int v) => v\nNone() => 0\n}\nreturn r + v\n}\n";
        assert_error(
            source,
            "declaration of `v` shadows a declaration in an enclosing scope",
            span_of(source, "match (e) {\nSome(int v) => v\nNone() => 0\n}"),
        );
    }

    #[test]
    fn if_else_where_both_branches_return_does_not_need_a_tail_return() {
        let source = "int f(bool b) {\nif (b) {\nreturn 1\n} else {\nreturn 2\n}\n}\n";
        assert!(check_source(source).is_empty());
    }

    #[test]
    fn void_function_without_return_is_legal() {
        let source = "void f(int x) {\nx += 1\n}\n";
        assert!(check_source(source).is_empty());
    }

    #[test]
    fn unknown_function() {
        let source = "int f() {\nreturn boom(1)\n}\n";
        assert_error(
            source,
            "unknown function `boom`",
            span_of(source, "boom(1)"),
        );
    }

    #[test]
    fn wrong_arity() {
        let source = "int add(int a, int b) {\nreturn a + b\n}\nint f() {\nreturn add(1)\n}\n";
        assert_error(
            source,
            "wrong number of arguments to `add`: expected 2, found 1",
            span_of(source, "add(1)"),
        );
    }

    #[test]
    fn wrong_argument_type() {
        let source =
            "int add(int a, int b) {\nreturn a + b\n}\nint f() {\nreturn add(1, \"x\")\n}\n";
        assert_error(
            source,
            "wrong argument type in call to `add`: expected `int`, found `str`",
            span_of(source, "\"x\""),
        );
    }

    #[test]
    fn undeclared_variable() {
        let source = "int f() {\nreturn x\n}\n";
        assert_error(source, "unknown name `x`", span_of(source, "x"));
    }

    #[test]
    fn use_before_declaration() {
        // A declaration enters scope after its own initializer: the `a`
        // initializer sits at byte 18 and is unknown at that point.
        let source = "int f() {\nint a = a\nreturn a\n}\n";
        assert_error(source, "unknown name `a`", Span::new(18, 19));
    }

    #[test]
    fn shadowing_is_forbidden() {
        let source = "int f() {\nint x = 1\nif (true) {\nint x = 2\n}\nreturn x\n}\n";
        assert_error(
            source,
            "declaration of `x` shadows a declaration in an enclosing scope",
            span_of(source, "int x = 2"),
        );
    }

    #[test]
    fn redeclaration_in_the_same_scope_is_forbidden() {
        let source = "int f() {\nint x = 1\nint x = 2\nreturn x\n}\n";
        assert_error(
            source,
            "duplicate declaration of `x`",
            span_of(source, "int x = 2"),
        );
    }

    #[test]
    fn duplicate_function() {
        let source = "int f() {\nreturn 1\n}\nint f() {\nreturn 2\n}\n";
        assert_error(
            source,
            "duplicate function `f`",
            span_of(source, "int f() {\nreturn 2\n}"),
        );
    }

    #[test]
    fn duplicate_function_body_is_not_checked() {
        // The first registration is authoritative: the duplicate is
        // reported, but its body is skipped, so a broken second body adds
        // no cascade on top of the duplicate error.
        let source = "int f() {\nreturn 1\n}\nint f() {\nreturn \"nope\"\n}\n";
        assert_error(
            source,
            "duplicate function `f`",
            span_of(source, "int f() {\nreturn \"nope\"\n}"),
        );
    }

    #[test]
    fn calls_to_a_duplicate_function_use_the_first_signature() {
        // f is int (first registration), so `f() + 1` is int and the
        // return type-checks. Had the second (str) signature won instead,
        // f() + 1 would crystallize to str and the return would mismatch.
        let source =
            "int f() {\nreturn 1\n}\nstr f() {\nreturn \"x\"\n}\nint main() {\nreturn f() + 1\n}\n";
        assert_error(
            source,
            "duplicate function `f`",
            span_of(source, "str f() {\nreturn \"x\"\n}"),
        );
    }

    #[test]
    fn duplicate_parameter() {
        let source = "int f(int a, int a) {\nreturn a\n}\n";
        assert_error(
            source,
            "duplicate parameter `a`",
            span_of(source, "int f(int a, int a) {\nreturn a\n}"),
        );
    }

    #[test]
    fn assignment_type_mismatch() {
        let source = "int f() {\nint x = 1\nx = 2.5\nreturn x\n}\n";
        assert_error(
            source,
            "type mismatch in assignment to `x`: expected `int`, found `float`",
            span_of(source, "x = 2.5"),
        );
    }

    #[test]
    fn assignment_target_must_be_declared() {
        let source = "int f() {\nx = 1\nreturn 0\n}\n";
        assert_error(source, "unknown name `x`", span_of(source, "x = 1"));
    }

    #[test]
    fn int_plus_float_is_rejected() {
        let source = "int f() {\ninfer x = 1 + 2.5\nreturn 0\n}\n";
        assert_error(
            source,
            "cannot apply `+` to `int` and `float`",
            span_of(source, "1 + 2.5"),
        );
    }

    #[test]
    fn string_repetition_is_rejected() {
        let source = "int f() {\ninfer x = \"a\" * 3\nreturn 0\n}\n";
        assert_error(
            source,
            "cannot apply `*` to `str` and `int`",
            span_of(source, "\"a\" * 3"),
        );
    }

    #[test]
    fn if_condition_must_be_bool() {
        let source = "int f() {\nif (1) {\nreturn 1\n}\nreturn 0\n}\n";
        assert_error(
            source,
            "if condition must be `bool`, found `int`",
            span_of(source, "1"),
        );
    }

    #[test]
    fn while_condition_must_be_bool() {
        let source = "int f() {\nwhile (2.5) {\nreturn 1\n}\nreturn 0\n}\n";
        assert_error(
            source,
            "while condition must be `bool`, found `float`",
            span_of(source, "2.5"),
        );
    }

    #[test]
    fn logical_not_requires_bool() {
        let source = "int f() {\ninfer x = !5\nreturn 0\n}\n";
        assert_error(source, "cannot apply `!` to `int`", span_of(source, "!5"));
    }

    #[test]
    fn unary_minus_rejects_bool() {
        let source = "int f() {\ninfer x = -true\nreturn 0\n}\n";
        assert_error(
            source,
            "cannot apply `-` to `bool`",
            span_of(source, "-true"),
        );
    }

    #[test]
    fn cross_type_equality_is_rejected() {
        let source = "int f() {\ninfer x = 1 == \"1\"\nreturn 0\n}\n";
        assert_error(
            source,
            "cannot apply `==` to `int` and `str`",
            span_of(source, "1 == \"1\""),
        );
    }

    #[test]
    fn missing_return() {
        let source = "int f() {\nint x = 1\n}\n";
        assert_error(
            source,
            "missing return in non-void function `f`",
            span_of(source, "{\nint x = 1\n}"),
        );
    }

    #[test]
    fn missing_return_when_if_has_no_else() {
        // Ruling: if without else does not count as returning.
        let source = "int f(bool b) {\nif (b) {\nreturn 1\n}\n}\n";
        assert_error(
            source,
            "missing return in non-void function `f`",
            span_of(source, "{\nif (b) {\nreturn 1\n}\n}"),
        );
    }

    #[test]
    fn missing_return_when_only_a_while_returns() {
        // Ruling: while never counts, no while (true) special-casing.
        let source = "int f() {\nwhile (true) {\nreturn 1\n}\n}\n";
        assert_error(
            source,
            "missing return in non-void function `f`",
            span_of(source, "{\nwhile (true) {\nreturn 1\n}\n}"),
        );
    }

    #[test]
    fn value_return_in_void_function() {
        let source = "void f() {\nreturn 1\n}\n";
        assert_error(
            source,
            "void function `f` cannot return a value",
            span_of(source, "return 1"),
        );
    }

    #[test]
    fn bare_return_in_non_void_function() {
        let source = "int f() {\nreturn\n}\n";
        assert_error(
            source,
            "non-void function `f` must return a value",
            span_of(source, "return"),
        );
    }

    #[test]
    fn wrong_return_type() {
        let source = "int f() {\nreturn \"nope\"\n}\n";
        assert_error(
            source,
            "wrong return type in `f`: expected `int`, found `str`",
            span_of(source, "return \"nope\""),
        );
    }

    #[test]
    fn void_call_as_value() {
        let source = "void g() {\nreturn\n}\nint f() {\nint x = g()\nreturn x\n}\n";
        assert_error(
            source,
            "type mismatch in declaration of `x`: expected `int`, found `void`",
            span_of(source, "int x = g()"),
        );
    }

    #[test]
    fn infer_from_void_call() {
        let source = "void g() {\nreturn\n}\nint f() {\ninfer x = g()\nreturn 0\n}\n";
        assert_error(
            source,
            "cannot infer type for 'x'; void initializer",
            span_of(source, "infer x = g()"),
        );
    }

    #[test]
    fn infer_crystallizes_to_the_initializer_type() {
        // §2.16: declared type = the initializer's type; assignments must
        // then match the crystallized type.
        let source = "int f() {\ninfer x = 1\nx = 2\nreturn x\n}\n";
        assert!(check_source(source).is_empty());
        let bad = "int f() {\ninfer x = 1\nx = 2.5\nreturn x\n}\n";
        assert_error(
            bad,
            "type mismatch in assignment to `x`: expected `int`, found `float`",
            span_of(bad, "x = 2.5"),
        );
    }

    #[test]
    fn top_level_statement() {
        let source = "int x = 1\n";
        assert_error(
            source,
            "only function, type, and impl declarations are allowed at top level",
            span_of(source, "int x = 1"),
        );
    }

    #[test]
    fn nested_function_declaration_is_rejected() {
        // Functions parse at any statement position (tolerance), but the
        // subset only allows top-level declarations; the nested one errors
        // and its body is never checked.
        let source = "int f() {\nint g() {\nreturn 1\n}\nreturn 1\n}\n";
        assert_error(
            source,
            "function declarations are only allowed at top level",
            span_of(source, "int g() {\nreturn 1\n}"),
        );
    }

    #[test]
    fn nested_function_declaration_in_a_block_is_rejected() {
        // Same rule inside a nested block: parse keeps the declaration,
        // the checker reports it where it appears.
        let source = "int f(bool b) {\nif (b) {\nint g() {\nreturn 1\n}\n}\nreturn 1\n}\n";
        assert_error(
            source,
            "function declarations are only allowed at top level",
            span_of(source, "int g() {\nreturn 1\n}"),
        );
    }

    #[test]
    fn calling_a_nested_function_still_reports_unknown_function() {
        // The nested declaration is illegal and never registers, so a call
        // to it resolves to no function; both facts are reported.
        let source = "int f() {\nint g() {\nreturn 1\n}\nreturn g()\n}\n";
        let diagnostics = check_source(source);
        assert_eq!(diagnostics.len(), 2, "{diagnostics:#?}");
        assert!(
            diagnostics.iter().any(|d| {
                d.to_string() == "function declarations are only allowed at top level"
            })
        );
        assert!(
            diagnostics
                .iter()
                .any(|d| d.to_string().contains("unknown function `g`"))
        );
    }

    #[test]
    fn non_call_expression_statement() {
        // The parser only produces call expression statements; the ruling
        // is enforced at check time, so the AST is built by hand. The
        // statement must sit inside a function body, or the top-level rule
        // fires instead.
        let inner = Stmt::new(
            StmtKind::Expression {
                expr: Expr::new(ExprKind::IntLit(1), Span::new(12, 13)),
            },
            Span::new(12, 13),
        );
        let func = Stmt::new(
            StmtKind::FuncDecl {
                name: "f".to_string(),
                params: vec![],
                return_ty: Type::Void,
                body: Block {
                    span: Span::new(8, 15),
                    stmts: vec![inner],
                },
            },
            Span::new(0, 15),
        );
        let diagnostics = check(&[func]);
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(
            diagnostics[0].to_string(),
            "expression statements must be function calls"
        );
        assert_eq!(diagnostics[0].span(), Span::new(12, 13));
    }

    #[test]
    fn infer_return_type_is_rejected_at_parse_level() {
        // Part 0 pins the parse-level error; the checker stays silent on
        // the poisoned signature (no cascade).
        let outcome = parse_source("infer f() {\nreturn 1\n}\n");
        assert_eq!(outcome.diagnostics.len(), 1);
        assert_eq!(
            outcome.diagnostics[0].to_string(),
            "`infer` is only valid for local declarations"
        );
        assert_eq!(outcome.diagnostics[0].span(), Span::new(0, 5));
        assert!(check(&outcome.statements).is_empty());
    }

    #[test]
    fn surviving_declaration_with_invalid_initializer_still_declares() {
        // The recovery design: `int x = )` keeps a VarDecl with an Invalid
        // initializer; the checker must not report anything for it.
        let outcome = parse_source("int f() {\nint x = )\nreturn x\n}\n");
        assert!(!outcome.diagnostics.is_empty());
        assert!(check(&outcome.statements).is_empty());
    }

    #[test]
    fn function_and_variable_namespaces_are_separate() {
        let source = "int f() {\nreturn 1\n}\nint main() {\nint f = f()\nreturn f\n}\n";
        assert!(check_source(source).is_empty());
    }

    #[test]
    fn checker_never_panics_on_the_stress_fixture() {
        let outcome = parse_source(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../boom.cm"
        )));
        // Runs to completion on any input; output volume is not pinned.
        let _ = check(&outcome.statements);
    }

    // ------------------------------------------------------------------
    // Full-surface checks: structs, enums, generics, match, for,
    // collections, interpolation, and the ? operator.
    // ------------------------------------------------------------------

    fn check_full(source: &str) -> Vec<Diagnostic> {
        check(&parse_source(source).statements)
    }

    #[test]
    fn syntax_cm_type_checks_clean() {
        const SYNTAX_CM: &str =
            include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../syntax.cm"));
        let diagnostics = check_full(SYNTAX_CM);
        assert!(
            diagnostics.is_empty(),
            "check(parse_source(syntax.cm)) must be empty: {diagnostics:#?}"
        );
    }

    #[test]
    fn struct_literals_type_check_with_named_fields() {
        let source = "struct vec2 {\n    float x\n    float y\n}\nint main() {\nvec2 p = vec2(x: 1.0, y: 2.0)\nfloat fx = p.x\nreturn 0\n}\n";
        assert!(check_full(source).is_empty());
    }

    #[test]
    fn struct_construction_rejects_missing_and_unknown_fields() {
        let head = "struct vec2 {\n    float x\n    float y\n}\nint main() {\n";
        // Missing field.
        let source = format!("{head}vec2 p = vec2(x: 1.0)\nreturn 0\n}}\n");
        assert_eq!(check_full(&source).len(), 1);
        assert!(
            check_full(&source)[0]
                .to_string()
                .contains("missing field `y`")
        );
        // Unknown field.
        let source = format!("{head}vec2 p = vec2(x: 1.0, y: 2.0, z: 3.0)\nreturn 0\n}}\n");
        assert!(
            check_full(&source)
                .iter()
                .any(|d| d.to_string().contains("unknown field `z`"))
        );
        // Wrong field type.
        let source = format!("{head}vec2 p = vec2(x: 1, y: 2.0)\nreturn 0\n}}\n");
        assert!(
            check_full(&source)
                .iter()
                .any(|d| d.to_string().contains("wrong type for field `x`"))
        );
        // Positional construction is not a struct literal.
        let source = format!("{head}vec2 p = vec2(1.0, 2.0)\nreturn 0\n}}\n");
        assert!(
            check_full(&source)
                .iter()
                .any(|d| d.to_string().contains("requires named arguments"))
        );
    }

    #[test]
    fn generic_struct_infers_type_arguments_from_fields() {
        let source = "struct pair<A, B> {\n    A first\n    B second\n}\nstr main() {\npair<int, str> p = pair(first: 1, second: \"x\")\nreturn p.second\n}\n";
        assert!(check_full(source).is_empty());
        // A conflicting field type reports the mismatch.
        let source = "struct pair<A, B> {\n    A first\n    B second\n}\nint main() {\npair<int, int> p = pair(first: 1, second: \"x\")\nreturn 0\n}\n";
        assert!(
            check_full(source)
                .iter()
                .any(|d| d.to_string().contains("wrong type for field `second`"))
        );
    }

    #[test]
    fn unknown_field_access_is_rejected() {
        let source = "struct vec2 {\n    float x\n    float y\n}\nint main() {\nvec2 p = vec2(x: 1.0, y: 2.0)\nfloat f = p.z\nreturn 0\n}\n";
        let diagnostics = check_full(source);
        assert_eq!(diagnostics.len(), 1);
        assert!(diagnostics[0].to_string().contains("unknown field `z`"));
    }

    #[test]
    fn enum_construction_and_match_type_check() {
        let source = "enum gameEvent {\n    Damage(int amount)\n    PlayerDied()\n}\nint main() {\ngameEvent evt = gameEvent.Damage(25)\nmatch (evt) {\n    Damage(int amount) => { return amount }\n    PlayerDied() => { return 0 }\n}\n}\n";
        assert!(check_full(source).is_empty());
    }

    #[test]
    fn non_exhaustive_match_is_rejected() {
        let source = "enum gameEvent {\n    Damage(int amount)\n    Heal(int amount)\n}\nint main() {\ngameEvent evt = gameEvent.Damage(25)\nmatch (evt) {\n    Damage(int amount) => { return amount }\n}\n}\n";
        let diagnostics = check_full(source);
        assert_eq!(diagnostics.len(), 1);
        assert!(diagnostics[0].to_string().contains("not exhaustive"));
        assert!(diagnostics[0].to_string().contains("Heal"));
        // A wildcard makes it exhaustive.
        let source = "enum gameEvent {\n    Damage(int amount)\n    Heal(int amount)\n}\nint main() {\ngameEvent evt = gameEvent.Damage(25)\nmatch (evt) {\n    Damage(int amount) => { return amount }\n    _ => { return 0 }\n}\n}\n";
        assert!(check_full(source).is_empty());
    }

    #[test]
    fn match_pattern_payload_types_are_checked() {
        let source = "enum gameEvent {\n    Damage(int amount)\n    Heal(int amount)\n}\nint main() {\ngameEvent evt = gameEvent.Damage(25)\nmatch (evt) {\n    Damage(str amount) => { return 0 }\n    _ => { return 1 }\n}\n}\n";
        let diagnostics = check_full(source);
        assert_eq!(diagnostics.len(), 1);
        assert!(
            diagnostics[0]
                .to_string()
                .contains("wrong type for payload `amount`")
        );
    }

    #[test]
    fn match_scrutinee_must_be_an_enum() {
        let source = "int main() {\nint x = 1\nmatch (x) {\n    _ => { return 0 }\n}\n}\n";
        let diagnostics = check_full(source);
        assert_eq!(diagnostics.len(), 1);
        assert!(
            diagnostics[0]
                .to_string()
                .contains("match scrutinee must be an enum")
        );
    }

    #[test]
    fn option_and_result_check_clean() {
        let source = "option<int> findEven(int[] xs) {\nreturn Some(1)\n}\nresult<int, str> safeDiv(int a, int b) {\nif (b == 0) {\nreturn Err(\"zero\")\n}\nreturn Ok(a / b)\n}\nint main() {\noption<int> found = None()\nresult<int, str> r = Ok(1)\nreturn 0\n}\n";
        assert!(check_full(source).is_empty());
    }

    #[test]
    fn try_operator_checks_the_enclosing_error_type() {
        // Clean: the propagated error type matches exactly (§2.8).
        let source = "result<int, str> safeDiv(int a, int b) {\nreturn Ok(a / b)\n}\nresult<int, str> chain(int a) {\nint v = safeDiv(a, 2)?\nreturn Ok(v)\n}\n";
        assert!(check_full(source).is_empty());

        // Mismatched error types are rejected.
        let source = "result<int, str> safeDiv(int a, int b) {\nreturn Ok(a / b)\n}\nresult<int, bool> chain(int a) {\nint v = safeDiv(a, 2)?\nreturn Ok(v)\n}\n";
        let diagnostics = check_full(source);
        assert_eq!(diagnostics.len(), 1);
        assert!(diagnostics[0].to_string().contains("`?` propagates error"));

        // ? in a non-result function is rejected.
        let source = "result<int, str> safeDiv(int a, int b) {\nreturn Ok(a / b)\n}\nint chain(int a) {\nint v = safeDiv(a, 2)?\nreturn v\n}\n";
        let diagnostics = check_full(source);
        assert_eq!(diagnostics.len(), 1);
        assert!(diagnostics[0].to_string().contains("`?` requires"));

        // ? on a non-result operand is rejected.
        let source = "int main() {\nint v = 1?\nreturn v\n}\n";
        let diagnostics = check_full(source);
        assert_eq!(diagnostics.len(), 1);
        assert!(
            diagnostics[0]
                .to_string()
                .contains("requires `result<T, E>`")
        );
    }

    #[test]
    fn arrays_and_maps_check_clean() {
        let source = "int main() {\nint[] xs = [1, 2, 3]\nint first = xs[0]\nint len = xs.length\nmap<str, int> m = {\"a\": 1\n\"b\": 2\n}\nint a = m[\"a\"]\nfor (int v in xs) {\na += v\n}\nreturn a\n}\n";
        assert!(check_full(source).is_empty());
    }

    #[test]
    fn array_element_mismatch_and_bad_index_are_rejected() {
        let source = "int main() {\nint[] xs = [1, 2, \"3\"]\nreturn 0\n}\n";
        let diagnostics = check_full(source);
        assert_eq!(diagnostics.len(), 1);
        assert!(
            diagnostics[0]
                .to_string()
                .contains("array elements must all have type `int`")
        );

        let source = "int main() {\nint[] xs = [1, 2]\nint i = xs[\"0\"]\nreturn i\n}\n";
        let diagnostics = check_full(source);
        assert_eq!(diagnostics.len(), 1);
        assert!(
            diagnostics[0]
                .to_string()
                .contains("array index must be `int`")
        );

        let source = "int main() {\nint x = 1\nint i = x[0]\nreturn i\n}\n";
        let diagnostics = check_full(source);
        assert_eq!(diagnostics.len(), 1);
        assert!(diagnostics[0].to_string().contains("cannot index `int`"));
    }

    #[test]
    fn infer_rejects_ambiguous_empty_collections() {
        // §2.16: the exact whitepaper message.
        let source = "int main() {\ninfer items = []\nreturn 0\n}\n";
        let diagnostics = check_full(source);
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(
            diagnostics[0].to_string(),
            "cannot infer type for 'items'; ambiguous initializer"
        );

        let source = "int main() {\ninfer m = {}\nreturn 0\n}\n";
        let diagnostics = check_full(source);
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(
            diagnostics[0].to_string(),
            "cannot infer type for 'm'; ambiguous initializer"
        );

        // With a declared type, empty collections are fine.
        let source = "int main() {\nint[] items = []\nreturn items.length\n}\n";
        assert!(check_full(source).is_empty());
    }

    #[test]
    fn interpolated_islands_must_be_scalars() {
        let source = "struct vec2 {\n    float x\n    float y\n}\nstr main() {\nvec2 p = vec2(x: 1.0, y: 2.0)\nstr s = $\"{p}\"\nreturn s\n}\n";
        let diagnostics = check_full(source);
        assert_eq!(diagnostics.len(), 1);
        assert!(
            diagnostics[0]
                .to_string()
                .contains("cannot interpolate `vec2`")
        );

        let source = "str main() {\nint hp = 100\nstr s = $\"hp={hp}\"\nreturn s\n}\n";
        assert!(check_full(source).is_empty());
    }

    #[test]
    fn for_loop_element_type_is_checked() {
        let source = "int main() {\nint[] xs = [1, 2]\nfor (str v in xs) {\n}\nreturn 0\n}\n";
        let diagnostics = check_full(source);
        assert_eq!(diagnostics.len(), 1);
        assert!(
            diagnostics[0]
                .to_string()
                .contains("wrong element type in for loop")
        );

        let source = "int main() {\nint x = 1\nfor (int v in x) {\n}\nreturn 0\n}\n";
        let diagnostics = check_full(source);
        assert_eq!(diagnostics.len(), 1);
        assert!(diagnostics[0].to_string().contains("cannot iterate `int`"));
    }

    #[test]
    fn struct_and_enum_equality_is_structural_and_same_type() {
        let source = "struct vec2 {\n    float x\n    float y\n}\nint main() {\nvec2 a = vec2(x: 1.0, y: 2.0)\nvec2 b = vec2(x: 1.0, y: 2.0)\nbool same = a == b\nreturn 0\n}\n";
        assert!(check_full(source).is_empty());

        let source = "struct vec2 {\n    float x\n    float y\n}\nstruct other {\n    float x\n}\nint main() {\nvec2 a = vec2(x: 1.0, y: 2.0)\nother b = other(x: 1.0)\nbool same = a == b\nreturn 0\n}\n";
        let diagnostics = check_full(source);
        assert_eq!(diagnostics.len(), 1);
        assert!(
            diagnostics[0]
                .to_string()
                .contains("cannot apply `==` to `vec2` and `other`")
        );
    }

    #[test]
    fn duplicate_and_reserved_type_names_are_rejected() {
        let source = "struct vec2 {\n    float x\n}\nstruct vec2 {\n    float y\n}\n";
        let diagnostics = check_full(source);
        assert_eq!(diagnostics.len(), 1);
        assert!(diagnostics[0].to_string().contains("duplicate type `vec2`"));

        let source = "struct option {\n    float x\n}\n";
        let diagnostics = check_full(source);
        assert_eq!(diagnostics.len(), 1);
        assert!(diagnostics[0].to_string().contains("reserved builtin name"));

        let source = "int Ok(int x) {\nreturn x\n}\n";
        let diagnostics = check_full(source);
        assert_eq!(diagnostics.len(), 1);
        assert!(diagnostics[0].to_string().contains("reserved builtin name"));
    }

    #[test]
    fn unknown_types_are_rejected_at_declarations() {
        let source = "int main() {\nmissing x = 1\nreturn 0\n}\n";
        let diagnostics = check_full(source);
        assert_eq!(diagnostics.len(), 1);
        assert!(
            diagnostics[0]
                .to_string()
                .contains("unknown type `missing`")
        );

        // Struct members referencing unknown types.
        let source = "struct s {\n    missing f\n}\n";
        let diagnostics = check_full(source);
        assert_eq!(diagnostics.len(), 1);
        assert!(
            diagnostics[0]
                .to_string()
                .contains("unknown type `missing`")
        );
    }

    #[test]
    fn forward_type_references_resolve() {
        // A struct field may reference a type declared later.
        let source = "struct player {\n    vec2 position\n}\nstruct vec2 {\n    float x\n    float y\n}\nint main() {\nplayer p = player(position: vec2(x: 1.0, y: 2.0))\nreturn 0\n}\n";
        assert!(check_full(source).is_empty());
    }

    #[test]
    fn value_semantics_of_field_assignment_type_check() {
        let source = "struct player {\n    int health\n}\nplayer damage(player p, int amount) {\np.health = p.health - amount\nreturn p\n}\nint main() {\nplayer hero = player(health: 100)\nplayer hurt = damage(hero, 30)\nhero.health += 5\nreturn hurt.health\n}\n";
        assert!(check_full(source).is_empty());
    }

    #[test]
    fn match_expressions_yield_one_type() {
        let source = "enum gameEvent {\n    Damage(int amount)\n    Heal(int amount)\n}\nint main() {\ngameEvent evt = gameEvent.Damage(25)\nint v = match (evt) {\n    Damage(int amount) => amount\n    Heal(int amount) => amount\n}\nreturn v\n}\n";
        assert!(check_full(source).is_empty());

        let source = "enum gameEvent {\n    Damage(int amount)\n    Heal(int amount)\n}\nint main() {\ngameEvent evt = gameEvent.Damage(25)\nint v = match (evt) {\n    Damage(int amount) => amount\n    Heal(int amount) => \"healed\"\n}\nreturn v\n}\n";
        let diagnostics = check_full(source);
        assert_eq!(diagnostics.len(), 1);
        assert!(
            diagnostics[0]
                .to_string()
                .contains("match arms yield different types")
        );
    }

    #[test]
    fn match_with_all_arms_returning_needs_no_tail_return() {
        let source = "enum gameEvent {\n    Damage(int amount)\n    Heal(int amount)\n}\nint classify(gameEvent evt) {\nmatch (evt) {\n    Damage(int amount) => { return 1 }\n    Heal(int amount) => { return 2 }\n}\n}\n";
        assert!(check_full(source).is_empty());
    }

    // ------------------------------------------------------------------
    // §10.4 — impl blocks
    // ------------------------------------------------------------------

    #[test]
    fn impl_member_mutating_a_parameter_without_returning_it_is_reported() {
        // §10.4's own annotated TODO: `OnTick` mutates `state.score` and
        // drops the change — "cme has to error here". Under §2.13 value
        // semantics the host caller's state never updates, and the loss is
        // invisible at the call site.
        let source = "struct GameState {\n    int score\n}\nimpl engine.gamemode {\n    void OnTick(GameState state, float deltaTime) {\n        state.score = state.score + 1\n        // Changes lost here because nothing is returned\n    }\n}\n";
        assert_error(
            source,
            "impl member `OnTick` mutates parameter `state`, but the change is lost when the call returns",
            span_of(source, "state.score = state.score + 1"),
        );
    }

    #[test]
    fn impl_member_returning_the_mutated_parameter_is_clean() {
        // §2.13's reassignment idiom: the mutation is only observable when
        // the member hands the parameter back. Wrapped forms count too
        // (result-returning interface members), and mutating a primitive
        // parameter is an ordinary dead store, not silent state loss.
        let source = "struct GameState {\n    int score\n}\nimpl engine.gamemode {\n    GameState OnTick(GameState state, float deltaTime) {\n        if (deltaTime > 1.0) {\n            state.score = state.score + 1\n        }\n        return state\n    }\n}\n";
        assert!(check_full(source).is_empty());

        let wrapped = "struct GameState {\n    int score\n}\nimpl engine.gamemode {\n    result<GameState, str> OnTick(GameState state, float deltaTime) {\n        state.score += 1\n        return Ok(state)\n    }\n}\n";
        assert!(check_full(wrapped).is_empty());

        let primitive = "impl engine.gamemode {\n    void Tick(float deltaTime) {\n        deltaTime = deltaTime * 2.0\n    }\n}\n";
        assert!(check_full(primitive).is_empty());
    }

    #[test]
    fn impl_members_on_a_struct_check_clean() {
        let source = "struct counter {\n    int value\n}\nimpl counter {\n    int peek(counter c) {\n        return c.value\n    }\n}\nint main() {\ncounter c = counter(value: 41)\nreturn counter.peek(c)\n}\n";
        assert!(check_full(source).is_empty());
    }

    #[test]
    fn impl_blocks_for_the_same_target_union() {
        let source = "struct counter {\n    int value\n}\nimpl counter {\n    int peek(counter c) {\n        return c.value\n    }\n}\nimpl counter {\n    counter bump(counter c) {\n        return counter(value: counter.peek(c) + 1)\n    }\n}\nint main() {\ncounter c = counter(value: 41)\nreturn counter.bump(c).value\n}\n";
        assert!(check_full(source).is_empty());
    }

    #[test]
    fn duplicate_impl_member_across_blocks_is_reported() {
        // §10.4: a member implemented multiple times fails with an exact
        // diagnostic; the first implementation wins for calls.
        let source = "struct counter {\n    int value\n}\nimpl counter {\n    int peek(counter c) {\n        return c.value\n    }\n}\nimpl counter {\n    int peek(counter c) {\n        return 0\n    }\n}\n";
        assert_error(
            source,
            "duplicate impl member `counter.peek`",
            span_of(source, "int peek(counter c) {\n        return 0\n    }"),
        );
    }

    #[test]
    fn unknown_impl_target_is_reported() {
        let source = "impl mystery {\n    int f() {\n        return 1\n    }\n}\n";
        assert_error(
            source,
            "unknown impl target `mystery`",
            span_of(
                source,
                "impl mystery {\n    int f() {\n        return 1\n    }\n}",
            ),
        );
    }

    #[test]
    fn impl_target_that_is_a_function_is_reported() {
        let source = "int helper() {\n    return 1\n}\nimpl helper {\n    int f() {\n        return 1\n    }\n}\n";
        assert_error(
            source,
            "impl target must be a struct or enum type",
            span_of(
                source,
                "impl helper {\n    int f() {\n        return 1\n    }\n}",
            ),
        );
    }

    #[test]
    fn generic_impl_targets_are_rejected_for_now() {
        let source = "struct box<T> {\n    T item\n}\nimpl box {\n    int f() {\n        return 1\n    }\n}\n";
        assert_error(
            source,
            "impl blocks on generic types are not supported yet",
            span_of(
                source,
                "impl box {\n    int f() {\n        return 1\n    }\n}",
            ),
        );
    }

    #[test]
    fn builtin_impl_targets_are_rejected() {
        let source = "impl option {\n    int f() {\n        return 1\n    }\n}\n";
        assert_error(
            source,
            "cannot implement the builtin type `option`",
            span_of(
                source,
                "impl option {\n    int f() {\n        return 1\n    }\n}",
            ),
        );
    }

    #[test]
    fn impl_member_colliding_with_a_variant_is_reported() {
        // Variant resolution always wins for `Color.Red(...)`, so a member
        // of the same name could never be called — reject it up front.
        let source = "enum color {\n    Red()\n}\nimpl color {\n    int Red() {\n        return 1\n    }\n}\n";
        assert_error(
            source,
            "impl member `color.Red` collides with a variant",
            span_of(source, "int Red() {\n        return 1\n    }"),
        );
    }

    #[test]
    fn impl_member_bodies_are_checked_like_function_bodies() {
        // Missing return inside a member, under the qualified name. (The
        // member is non-void, so a structured-parameter mutation here may
        // feed its result — the §10.4 lost-mutation diagnostic scopes
        // itself to void members, where nothing can escape.)
        let source = "struct counter {\n    int value\n}\nimpl counter {\n    int peek(counter c) {\n        c.value += 1\n    }\n}\n";
        assert_error(
            source,
            "missing return in non-void function `counter.peek`",
            span_of(source, "{\n        c.value += 1\n    }"),
        );
    }

    #[test]
    fn impl_member_calls_check_arguments_and_return_type() {
        let source = "struct counter {\n    int value\n}\nimpl counter {\n    int peek(counter c) {\n        return c.value\n    }\n}\nint main() {\nreturn counter.peek(41)\n}\n";
        assert_error(
            source,
            "wrong argument type in call to `counter.peek`",
            span_of(source, "41"),
        );
    }

    #[test]
    fn unknown_impl_member_on_a_struct_is_reported() {
        let source = "struct counter {\n    int value\n}\nimpl counter {\n    int peek(counter c) {\n        return c.value\n    }\n}\nint main() {\nreturn counter.poke(1)\n}\n";
        assert_error(
            source,
            "unknown member `poke` in `counter`",
            span_of(source, "counter.poke(1)"),
        );
    }

    #[test]
    fn dotted_impl_targets_and_path_calls_check_clean() {
        let source = "struct GameConfig {\n    int startingScore\n}\nstruct GameState {\n    int score\n}\nimpl engine.gamemode {\n    GameState InitGame(GameConfig config) {\n        return GameState(score: config.startingScore)\n    }\n}\nint main() {\nGameConfig config = GameConfig(startingScore: 100)\nGameState state = engine.gamemode.InitGame(config)\nreturn state.score\n}\n";
        assert!(check_full(source).is_empty());
    }

    #[test]
    fn unknown_path_call_targets_are_reported() {
        let source = "int main() {\nreturn engine.graphics.DrawTexture(1)\n}\n";
        assert_error(
            source,
            "unknown function `engine.graphics.DrawTexture`",
            span_of(source, "engine.graphics.DrawTexture(1)"),
        );
    }

    #[test]
    fn path_call_argument_types_are_checked() {
        let source = "struct GameConfig {\n    int startingScore\n}\nstruct GameState {\n    int score\n}\nimpl engine.gamemode {\n    GameState InitGame(GameConfig config) {\n        return GameState(score: config.startingScore)\n    }\n}\nint main() {\nGameState state = engine.gamemode.InitGame(5)\nreturn state.score\n}\n";
        assert_error(
            source,
            "wrong argument type in call to `engine.gamemode.InitGame`",
            span_of(source, "5"),
        );
    }

    #[test]
    fn dotted_impl_target_extending_a_local_type_is_reported() {
        let source = "struct counter {\n    int value\n}\nimpl counter.utils {\n    int f() {\n        return 1\n    }\n}\n";
        assert_error(
            source,
            "impl target `counter.utils` must not extend the local type",
            span_of(
                source,
                "impl counter.utils {\n    int f() {\n        return 1\n    }\n}",
            ),
        );
    }

    #[test]
    fn enum_variants_keep_priority_over_impl_members() {
        // A different-named member on the same enum resolves fine while the
        // variant construction keeps working.
        let source = "enum color {\n    Red()\n    Blue()\n}\nimpl color {\n    bool isRed(color c) {\n        return c == color.Red()\n    }\n}\nint main() {\nbool red = color.isRed(color.Red())\nbool blue = color.isRed(color.Blue())\nif (red == blue) {\n    return 0\n}\nreturn 1\n}\n";
        assert!(check_full(source).is_empty());
    }
}
