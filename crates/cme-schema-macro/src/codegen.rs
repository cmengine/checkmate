//! The bindings generator: schema in, Rust source out (WHITEPAPER §9.6).
//!
//! Per schema file the generator emits one module named after the
//! namespace containing:
//!
//! 1. schema metadata and the runtime `SchemaFile` descriptor — the same
//!    contract the script toolchain checks against;
//! 2. the §9.3 boundary types as native Rust types with `Value`
//!    pack/unpack;
//! 3. one trait per capability whose methods carry the schema signatures —
//!    implementing the trait IS the compile-time verification of the
//!    host's capability implementation;
//! 4. one proxy per interface for typed host → script calls;
//! 5. registration helpers wiring traits to `CapabilityProvider`.
//!
//! All generated paths are fully qualified (`::std::…`, `::core::…`, and
//! the configured API crate), so the output is hygiene-safe under any
//! host prelude.

use std::fmt::Write as _;

use cme_core::ast::{PrimitiveType, Type};
use cme_core::schema::{
    ContractKind, MemberRequirement, SchemaContract, SchemaEnum, SchemaFile, SchemaStruct, Version,
};

use crate::input::CompileErrorMessage;

/// Generates the bindings source for one schema, or the errors that stop
/// generation (unresolved type references, unusable names).
pub fn generate(schema: &SchemaFile, api_crate: &str) -> Result<String, Vec<CompileErrorMessage>> {
    let mut errors = Vec::new();

    // The namespace becomes a Rust module name: it must be a plain,
    // non-keyword identifier.
    if !is_plain_ident(&schema.namespace) {
        errors.push(CompileErrorMessage::new(format!(
            "schema namespace `{}` is not usable as a Rust module name",
            schema.namespace
        )));
    }

    // Every named type inside every signature must resolve to a declared
    // type of this schema (or the option/result builtins).
    validate_type_references(schema, &mut errors);

    if !errors.is_empty() {
        return Err(errors);
    }

    Ok(render(schema, api_crate))
}

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

fn validate_type_references(schema: &SchemaFile, errors: &mut Vec<CompileErrorMessage>) {
    let walk = |ty: &Type, context: &str, errors: &mut Vec<CompileErrorMessage>| {
        walk_type(ty, context, errors);
    };
    for item in &schema.items {
        match item {
            cme_core::schema::SchemaItem::Struct(decl) => {
                for field in &decl.fields {
                    walk(&field.ty, &format!("struct `{}`", decl.name), errors);
                }
            }
            cme_core::schema::SchemaItem::Enum(decl) => {
                for variant in &decl.variants {
                    for field in &variant.fields {
                        walk(
                            &field.ty,
                            &format!("enum `{}` variant `{}`", decl.name, variant.name),
                            errors,
                        );
                    }
                }
            }
            cme_core::schema::SchemaItem::Contract(contract) => {
                for member in &contract.members {
                    for param in &member.params {
                        walk(
                            &param.ty,
                            &format!("member `{}.{}`", contract.name, member.name),
                            errors,
                        );
                    }
                    walk(
                        &member.return_ty,
                        &format!("member `{}.{}`", contract.name, member.name),
                        errors,
                    );
                }
            }
        }
    }

    fn walk_type(ty: &Type, context: &str, errors: &mut Vec<CompileErrorMessage>) {
        match ty {
            Type::Named { name, args } => {
                match name.as_str() {
                    "option" if args.len() == 1 => {}
                    "result" if args.len() == 2 => {}
                    "option" | "result" => errors.push(CompileErrorMessage::new(format!(
                        "wrong number of type arguments for `{name}` in {context}"
                    ))),
                    _ => {}
                }
                for arg in args {
                    walk_type(arg, context, errors);
                }
            }
            Type::Array(elem) => walk_type(elem, context, errors),
            Type::Map { key, value } => {
                walk_type(key, context, errors);
                walk_type(value, context, errors);
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

fn render(schema: &SchemaFile, api: &str) -> String {
    let generator = Generator { schema, api };
    let namespace = &schema.namespace;
    let mut out = String::new();
    let _ = writeln!(
        out,
        "/// Host bindings for the `{namespace}` schema (WHITEPAPER §9.6).\n\
         ///\n\
         /// Generated by `cme_schema_bindings!` from the schema file — the\n\
         /// single source of truth for the script toolchain AND these host\n\
         /// bindings. Capability traits verify the host's implementation at\n\
         /// compile time; proxies verify the host's interface calls. DO NOT EDIT.\n\
         #[allow(clippy::all)]\n\
         #[allow(dead_code)]\n\
         pub mod {namespace} {{\n\
         \x20   #[allow(unused_imports)]\n\
         \x20   use super::*;\n"
    );
    generator.render_metadata(&mut out);
    generator.render_convert_module(&mut out);
    generator.render_types(&mut out);
    // The shared conversion-error constructor for the interface proxies.
    let _ = writeln!(
        out,
        r#"    fn __conv_err(message: ::std::string::String) -> {api}::ExecutionError {{
        {api}::ExecutionError {{
            kind: {api}::ErrorKind::Runtime,
            message,
            line: 0,
            column: 0,
            file: ::core::option::Option::None,
            span: ::core::option::Option::None,
        }}
    }}
"#,
        api = generator.api,
    );
    for item in &schema.items {
        if let cme_core::schema::SchemaItem::Contract(contract) = item {
            match contract.kind {
                ContractKind::Capability => generator.render_capability(contract, &mut out),
                ContractKind::Interface => generator.render_interface(contract, &mut out),
            }
        }
    }
    generator.render_descriptor(&mut out);
    let _ = writeln!(out, "}}");
    out
}

struct Generator<'s> {
    schema: &'s SchemaFile,
    api: &'s str,
}

impl<'s> Generator<'s> {
    // ------------------------------------------------------------------
    // Naming
    // ------------------------------------------------------------------

    /// `LoadTexture` → `load_texture`, `DrawHUD` → `draw_hud`,
    /// `authHeader` → `auth_header`.
    fn snake(name: &str) -> String {
        let chars: Vec<char> = name.chars().collect();
        let mut out = String::new();
        for (index, ch) in chars.iter().enumerate() {
            if ch.is_ascii_uppercase() {
                let previous_upper = index > 0 && chars[index - 1].is_ascii_uppercase();
                let next_lower = index + 1 < chars.len() && chars[index + 1].is_ascii_lowercase();
                if (index > 0 && !previous_upper || (previous_upper && next_lower))
                    && !out.ends_with('_')
                {
                    out.push('_');
                }
                out.push(ch.to_ascii_lowercase());
            } else {
                out.push(*ch);
            }
        }
        out
    }

    /// A snake_case identifier with keyword escaping — usable for members,
    /// parameters, fields, and function names.
    fn snake_ident(&self, name: &str) -> String {
        let snake = Self::snake(name);
        match snake.as_str() {
            "self" | "Self" | "crate" | "super" => format!("{snake}_"),
            _ if is_rust_keyword(&snake) => format!("r#{snake}"),
            _ => snake,
        }
    }

    /// `engine` + `graphics` → `EngineGraphics`.
    fn pascal(name: &str) -> String {
        let mut out = String::new();
        let mut upper_next = true;
        for ch in name.chars() {
            if ch == '_' || ch == '.' {
                upper_next = true;
            } else if upper_next {
                out.push(ch.to_ascii_uppercase());
                upper_next = false;
            } else {
                out.push(ch);
            }
        }
        out
    }

    fn capability_trait_name(&self, capability: &str) -> String {
        format!(
            "{}{}Capability",
            Self::pascal(&self.schema.namespace),
            Self::pascal(capability)
        )
    }

    fn proxy_name(&self, interface: &str) -> String {
        format!(
            "{}{}Proxy",
            Self::pascal(&self.schema.namespace),
            Self::pascal(interface)
        )
    }

    fn register_fn_name(&self, contract: &str) -> String {
        format!(
            "register_{}_{}",
            Self::snake(&self.schema.namespace),
            Self::snake(contract)
        )
    }

    // ------------------------------------------------------------------
    // Schema type → Rust type
    // ------------------------------------------------------------------

    fn rust_type(&self, ty: &Type) -> String {
        match ty {
            Type::Infer | Type::Void => "()".to_string(),
            Type::Prim(PrimitiveType::Int) => "i64".to_string(),
            Type::Prim(PrimitiveType::Float) => "f64".to_string(),
            Type::Prim(PrimitiveType::Bool) => "bool".to_string(),
            Type::Prim(PrimitiveType::Str) => "::std::string::String".to_string(),
            Type::Array(elem) => format!("::std::vec::Vec<{}>", self.rust_type(elem)),
            Type::Map { key, value } => format!(
                "::std::vec::Vec<({}, {})>",
                self.rust_type(key),
                self.rust_type(value)
            ),
            Type::Named { name, args } => match name.as_str() {
                "option" => format!("::core::option::Option<{}>", self.rust_type(&args[0])),
                "result" => format!(
                    "::core::result::Result<{}, {}>",
                    self.rust_type(&args[0]),
                    self.rust_type(&args[1])
                ),
                // Types live in the SAME generated module; a bare name is
                // correct and avoids relying on the module being re-globbed.
                other => self.rust_ident(other),
            },
        }
    }

    /// A schema-declared type name as a Rust identifier (keywords escaped).
    fn rust_ident(&self, name: &str) -> String {
        if is_rust_keyword(name) {
            format!("r#{name}")
        } else {
            name.to_string()
        }
    }

    // ------------------------------------------------------------------
    // Value conversions
    // ------------------------------------------------------------------

    /// Converts `value_name` (a `&T` expression) into a `Value`.
    fn to_value_expr(&self, ty: &Type, value_name: &str) -> String {
        match ty {
            Type::Prim(PrimitiveType::Int) => format!("__convert::int_to({value_name})"),
            Type::Prim(PrimitiveType::Float) => format!("__convert::float_to({value_name})"),
            Type::Prim(PrimitiveType::Bool) => format!("__convert::bool_to({value_name})"),
            Type::Prim(PrimitiveType::Str) => format!("__convert::string_to({value_name})"),
            Type::Array(elem) => format!(
                "__convert::vec_to({value_name}, |__v| {})",
                self.to_value_expr(elem, "__v")
            ),
            Type::Map { key, value } => format!(
                "__convert::map_to({value_name}, |__k| {}, |__v| {})",
                self.to_value_expr(key, "__k"),
                self.to_value_expr(value, "__v")
            ),
            Type::Named { name, args } => match name.as_str() {
                "option" => format!(
                    "__convert::option_to({value_name}, |__v| {})",
                    self.to_value_expr(&args[0], "__v")
                ),
                "result" => format!(
                    "__convert::result_to({value_name}, |__v| {}, |__v| {})",
                    self.to_value_expr(&args[0], "__v"),
                    self.to_value_expr(&args[1], "__v")
                ),
                other => format!("{}::to_value({value_name})", self.rust_ident(other)),
            },
            _ => "::core::unreachable!()".to_string(),
        }
    }

    /// Converts `value_name` (a `&Value` expression) into
    /// `Result<T, String>` — the caller decides how the error surfaces, so
    /// NO `?` here (closures cannot carry one).
    fn value_from_expr(&self, ty: &Type, value_name: &str) -> String {
        match ty {
            Type::Prim(PrimitiveType::Int) => format!("__convert::int_from({value_name})"),
            Type::Prim(PrimitiveType::Float) => format!("__convert::float_from({value_name})"),
            Type::Prim(PrimitiveType::Bool) => format!("__convert::bool_from({value_name})"),
            Type::Prim(PrimitiveType::Str) => format!("__convert::string_from({value_name})"),
            Type::Array(elem) => format!(
                "__convert::vec_from({value_name}, |__v| {})",
                self.value_from_expr(elem, "__v")
            ),
            Type::Map { key, value } => format!(
                "__convert::map_from({value_name}, |__k| {}, |__v| {})",
                self.value_from_expr(key, "__k"),
                self.value_from_expr(value, "__v")
            ),
            Type::Named { name, args } => match name.as_str() {
                "option" => format!(
                    "__convert::option_from({value_name}, |__v| {})",
                    self.value_from_expr(&args[0], "__v")
                ),
                "result" => format!(
                    "__convert::result_from({value_name}, |__v| {}, |__v| {})",
                    self.value_from_expr(&args[0], "__v"),
                    self.value_from_expr(&args[1], "__v")
                ),
                other => format!("{}::from_value({value_name})", self.rust_ident(other)),
            },
            _ => "::core::unreachable!()".to_string(),
        }
    }

    /// The descriptor expression for a type (for the `schema()` builder).
    fn type_descriptor(&self, ty: &Type) -> String {
        let api = self.api;
        match ty {
            Type::Prim(PrimitiveType::Int) => {
                format!("{api}::Type::Prim({api}::PrimitiveType::Int)")
            }
            Type::Prim(PrimitiveType::Float) => {
                format!("{api}::Type::Prim({api}::PrimitiveType::Float)")
            }
            Type::Prim(PrimitiveType::Bool) => {
                format!("{api}::Type::Prim({api}::PrimitiveType::Bool)")
            }
            Type::Prim(PrimitiveType::Str) => {
                format!("{api}::Type::Prim({api}::PrimitiveType::Str)")
            }
            Type::Array(elem) => format!(
                "{api}::Type::Array(::std::boxed::Box::new({}))",
                self.type_descriptor(elem)
            ),
            Type::Map { key, value } => format!(
                "{api}::Type::Map {{ key: ::std::boxed::Box::new({}), value: ::std::boxed::Box::new({}) }}",
                self.type_descriptor(key),
                self.type_descriptor(value)
            ),
            Type::Named { name, args } => {
                let args: Vec<String> = args.iter().map(|arg| self.type_descriptor(arg)).collect();
                format!(
                    "{api}::Type::Named {{ name: {name:?}.to_string(), args: ::std::vec![{}] }}",
                    args.join(", ")
                )
            }
            Type::Void => format!("{api}::Type::Void"),
            Type::Infer => "::core::unreachable!()".to_string(),
        }
    }

    // ------------------------------------------------------------------
    // Sections
    // ------------------------------------------------------------------

    fn render_metadata(&self, out: &mut String) {
        let api = self.api;
        let namespace = &self.schema.namespace;
        let version = &self.schema.version;
        let _ = writeln!(
            out,
            "    /// The schema version this contract declares (§9.5).\n\
             \x20   pub const SCHEMA_VERSION: {api}::Version = {api}::Version {{ major: {major}u32, minor: {minor}u32, patch: {patch}u32 }};\n\
             \x20   /// The namespace root (`{namespace}.capability.Member` paths, §9.1).\n\
             \x20   pub const NAMESPACE: &str = {namespace:?};\n",
            major = version.major,
            minor = version.minor,
            patch = version.patch,
            namespace = namespace,
            api = api,
        );
    }

    fn render_convert_module(&self, out: &mut String) {
        let api = self.api;
        let _ = write!(
            out,
            r#"    /// Conversion helpers between host data and script values. Every
    /// conversion is shape-checked and reports a plain message on mismatch.
    #[doc(hidden)]
    pub mod __convert {{
        #[allow(unused_imports)]
        use super::*;

        pub fn kind_of(value: &{API}::Value) -> &'static str {{
            match value {{
                {API}::Value::Int(_) => "int",
                {API}::Value::Float(_) => "float",
                {API}::Value::Str(_) => "str",
                {API}::Value::Bool(_) => "bool",
                {API}::Value::Void => "void",
                {API}::Value::Struct {{ .. }} => "struct",
                {API}::Value::Enum {{ .. }} => "enum",
                {API}::Value::Array(_) => "array",
                {API}::Value::Map(_) => "map",
            }}
        }}

        pub fn int_to(value: &i64) -> {API}::Value {{ {API}::Value::Int(*value) }}
        pub fn float_to(value: &f64) -> {API}::Value {{ {API}::Value::Float(*value) }}
        pub fn bool_to(value: &bool) -> {API}::Value {{ {API}::Value::Bool(*value) }}
        pub fn string_to(value: &::std::string::String) -> {API}::Value {{
            {API}::Value::Str(value.clone())
        }}

        pub fn int_from(value: &{API}::Value) -> ::core::result::Result<i64, ::std::string::String> {{
            match value {{
                {API}::Value::Int(inner) => ::core::result::Result::Ok(*inner),
                other => ::core::result::Result::Err(::std::format!("expected `int`, found `{{}}`", kind_of(other))),
            }}
        }}
        pub fn float_from(value: &{API}::Value) -> ::core::result::Result<f64, ::std::string::String> {{
            match value {{
                {API}::Value::Float(inner) => ::core::result::Result::Ok(*inner),
                other => ::core::result::Result::Err(::std::format!("expected `float`, found `{{}}`", kind_of(other))),
            }}
        }}
        pub fn bool_from(value: &{API}::Value) -> ::core::result::Result<bool, ::std::string::String> {{
            match value {{
                {API}::Value::Bool(inner) => ::core::result::Result::Ok(*inner),
                other => ::core::result::Result::Err(::std::format!("expected `bool`, found `{{}}`", kind_of(other))),
            }}
        }}
        pub fn string_from(value: &{API}::Value) -> ::core::result::Result<::std::string::String, ::std::string::String> {{
            match value {{
                {API}::Value::Str(inner) => ::core::result::Result::Ok(inner.clone()),
                other => ::core::result::Result::Err(::std::format!("expected `str`, found `{{}}`", kind_of(other))),
            }}
        }}

        pub fn vec_to<T, F: ::core::ops::Fn(&T) -> {API}::Value>(
            items: &::std::vec::Vec<T>,
            inner: F,
        ) -> {API}::Value {{
            {API}::Value::Array(items.iter().map(|item| inner(item)).collect())
        }}
        pub fn vec_from<T, F: ::core::ops::Fn(&{API}::Value) -> ::core::result::Result<T, ::std::string::String>>(
            value: &{API}::Value,
            inner: F,
        ) -> ::core::result::Result<::std::vec::Vec<T>, ::std::string::String> {{
            match value {{
                {API}::Value::Array(items) => items.iter().map(inner).collect(),
                other => ::core::result::Result::Err(::std::format!("expected an array, found `{{}}`", kind_of(other))),
            }}
        }}

        pub fn map_to<K, V, FK: ::core::ops::Fn(&K) -> {API}::Value, FV: ::core::ops::Fn(&V) -> {API}::Value>(
            entries: &::std::vec::Vec<(K, V)>,
            key: FK,
            value: FV,
        ) -> {API}::Value {{
            {API}::Value::Map(entries.iter().map(|(k, v)| (key(k), value(v))).collect())
        }}
        pub fn map_from<K, V, FK, FV>(
            value: &{API}::Value,
            key: FK,
            val: FV,
        ) -> ::core::result::Result<::std::vec::Vec<(K, V)>, ::std::string::String>
        where
            FK: ::core::ops::Fn(&{API}::Value) -> ::core::result::Result<K, ::std::string::String>,
            FV: ::core::ops::Fn(&{API}::Value) -> ::core::result::Result<V, ::std::string::String>,
        {{
            match value {{
                {API}::Value::Map(entries) => entries
                    .iter()
                    .map(|(k, v)| ::core::result::Result::Ok((key(k)?, val(v)?)))
                    .collect(),
                other => ::core::result::Result::Err(::std::format!("expected a map, found `{{}}`", kind_of(other))),
            }}
        }}

        pub fn option_to<T, F: ::core::ops::Fn(&T) -> {API}::Value>(
            value: &::core::option::Option<T>,
            inner: F,
        ) -> {API}::Value {{
            match value {{
                ::core::option::Option::Some(some) => {API}::Value::Enum {{
                    name: "option".to_string(),
                    variant: "Some".to_string(),
                    payload: ::std::vec![inner(some)],
                }},
                ::core::option::Option::None => {API}::Value::Enum {{
                    name: "option".to_string(),
                    variant: "None".to_string(),
                    payload: ::std::vec![],
                }},
            }}
        }}
        pub fn option_from<T, F: ::core::ops::Fn(&{API}::Value) -> ::core::result::Result<T, ::std::string::String>>(
            value: &{API}::Value,
            inner: F,
        ) -> ::core::result::Result<::core::option::Option<T>, ::std::string::String> {{
            match value {{
                {API}::Value::Enum {{ name, variant, payload }} if name == "option" && variant == "Some" => {{
                    if payload.len() != 1 {{
                        return ::core::result::Result::Err(::std::format!(
                            "`option.Some` carries 1 payload value, found {{}}",
                            payload.len()
                        ));
                    }}
                    inner(&payload[0]).map(::core::option::Option::Some)
                }}
                {API}::Value::Enum {{ name, variant, .. }} if name == "option" && variant == "None" => {{
                    ::core::result::Result::Ok(::core::option::Option::None)
                }}
                other => ::core::result::Result::Err(::std::format!("expected `option`, found `{{}}`", kind_of(other))),
            }}
        }}

        pub fn result_to<T, E, FT: ::core::ops::Fn(&T) -> {API}::Value, FE: ::core::ops::Fn(&E) -> {API}::Value>(
            value: &::core::result::Result<T, E>,
            ok: FT,
            err: FE,
        ) -> {API}::Value {{
            match value {{
                ::core::result::Result::Ok(good) => {API}::Value::Enum {{
                    name: "result".to_string(),
                    variant: "Ok".to_string(),
                    payload: ::std::vec![ok(good)],
                }},
                ::core::result::Result::Err(bad) => {API}::Value::Enum {{
                    name: "result".to_string(),
                    variant: "Err".to_string(),
                    payload: ::std::vec![err(bad)],
                }},
            }}
        }}
        pub fn result_from<T, E, FT, FE>(
            value: &{API}::Value,
            ok: FT,
            err: FE,
        ) -> ::core::result::Result<::core::result::Result<T, E>, ::std::string::String>
        where
            FT: ::core::ops::Fn(&{API}::Value) -> ::core::result::Result<T, ::std::string::String>,
            FE: ::core::ops::Fn(&{API}::Value) -> ::core::result::Result<E, ::std::string::String>,
        {{
            match value {{
                {API}::Value::Enum {{ name, variant, payload }} if name == "result" && variant == "Ok" => {{
                    if payload.len() != 1 {{
                        return ::core::result::Result::Err(::std::format!(
                            "`result.Ok` carries 1 payload value, found {{}}",
                            payload.len()
                        ));
                    }}
                    ok(&payload[0]).map(::core::result::Result::Ok)
                }}
                {API}::Value::Enum {{ name, variant, payload }} if name == "result" && variant == "Err" => {{
                    if payload.len() != 1 {{
                        return ::core::result::Result::Err(::std::format!(
                            "`result.Err` carries 1 payload value, found {{}}",
                            payload.len()
                        ));
                    }}
                    err(&payload[0]).map(::core::result::Result::Err)
                }}
                other => ::core::result::Result::Err(::std::format!("expected `result`, found `{{}}`", kind_of(other))),
            }}
        }}

        /// A struct field by name: the boundary contract unpacks by NAME,
        /// so neither side may assume the other's field ordering.
        pub fn field<'v>(
            fields: &'v ::std::vec::Vec<(::std::string::String, {API}::Value)>,
            name: &str,
        ) -> ::core::result::Result<&'v {API}::Value, ::std::string::String> {{
            fields
                .iter()
                .find(|(field, _)| field == name)
                .map(|(_, value)| value)
                .ok_or_else(|| ::std::format!("missing field `{{name}}`"))
        }}

        /// A positional call argument, for the bridge dispatch.
        pub fn expect_arg<'v>(
            args: &'v [{API}::Value],
            index: usize,
            member: &str,
        ) -> ::core::result::Result<&'v {API}::Value, ::std::string::String> {{
            if index >= args.len() {{
                return ::core::result::Result::Err(::std::format!(
                    "`{{}}` expects an argument at position {{}}, found {{}}",
                    member,
                    index,
                    args.len()
                ));
            }}
            ::core::result::Result::Ok(&args[index])
        }}
    }}
"#,
            API = api,
        );
    }

    fn render_types(&self, out: &mut String) {
        for item in &self.schema.items {
            match item {
                cme_core::schema::SchemaItem::Struct(decl) => self.render_struct(decl, out),
                cme_core::schema::SchemaItem::Enum(decl) => self.render_enum(decl, out),
                cme_core::schema::SchemaItem::Contract(_) => {}
            }
        }
    }

    fn render_struct(&self, decl: &SchemaStruct, out: &mut String) {
        let name = &decl.name;
        let mut fields = String::new();
        let mut pack = String::new();
        let mut unpack = String::new();
        for field in &decl.fields {
            let field_ident = self.snake_ident(&field.name);
            let _ = writeln!(
                fields,
                "    pub {}: {},",
                field_ident,
                self.rust_type(&field.ty)
            );
            let _ = writeln!(
                pack,
                "                ({field_name:?}.to_string(), {}),",
                self.to_value_expr(&field.ty, &format!("&self.{field_ident}")),
                field_name = field.name,
            );
            let _ = writeln!(
                unpack,
                "                {field_ident}: {}?,",
                self.value_from_expr(
                    &field.ty,
                    &format!("__convert::field(fields, {:?})?", field.name)
                )
            );
        }

        let api = self.api;
        let _ = write!(
            out,
            "    /// Schema boundary type `{name}` (§9.3): the shared FFI layout.\n\
             \x20   #[derive(::core::fmt::Debug, ::core::clone::Clone, ::core::cmp::PartialEq)]\n\
             \x20   pub struct {name} {{\n{fields}    }}\n\n\
             \x20   impl {name} {{\n\
             \x20       /// Packs into a script value (the §9.3 boundary layout).\n\
             \x20       pub fn to_value(&self) -> {api}::Value {{\n\
             \x20           {api}::Value::Struct {{\n\
             \x20               name: {name:?}.to_string(),\n\
             \x20               fields: ::std::vec![\n{pack}                ],\n\
             \x20           }}\n\
             \x20       }}\n\n\
             \x20       /// Unpacks a script value; fields are matched by name.\n\
             \x20       pub fn from_value(value: &{api}::Value) -> ::core::result::Result<Self, ::std::string::String> {{\n\
             \x20           match value {{\n\
             \x20               {api}::Value::Struct {{ name, fields }} if name == {name:?} => {{\n\
             \x20                   ::core::result::Result::Ok(Self {{\n{unpack}                }})\n\
             \x20               }}\n\
             \x20               other => ::core::result::Result::Err(::std::format!(\n\
             \x20                   \"expected struct `{name}`, found `{{}}`\",\n\
             \x20                   __convert::kind_of(other)\n\
             \x20               )),\n\
             \x20           }}\n\
             \x20       }}\n\
             \x20   }}\n\n"
        );
    }

    fn render_enum(&self, decl: &SchemaEnum, out: &mut String) {
        let name = &decl.name;
        let api = self.api;
        let mut variants = String::new();
        let mut pack_arms = String::new();
        let mut unpack_arms = String::new();

        for variant in &decl.variants {
            let variant_name = &variant.name;
            if variant.fields.is_empty() {
                let _ = writeln!(variants, "    {variant_name},");
                let _ = writeln!(
                    pack_arms,
                    "            Self::{variant_name} => {api}::Value::Enum {{ name: {name:?}.to_string(), variant: {variant_name:?}.to_string(), payload: ::std::vec![] }},"
                );
                let _ = writeln!(
                    unpack_arms,
                    "                {variant_name:?} => {{
                    if !payload.is_empty() {{
                        return ::core::result::Result::Err(::std::format!(
                            \"{name}.{variant_name} carries no payload, found {{}} value(s)\",
                            payload.len()
                        ));
                    }}
                    ::core::result::Result::Ok(Self::{variant_name})
                }}"
                );
            } else {
                let mut fields = String::new();
                let mut field_names = Vec::new();
                let mut payload = String::new();
                let mut unpack_fields = String::new();
                for (field_index, field) in variant.fields.iter().enumerate() {
                    let field_ident = self.snake_ident(&field.name);
                    field_names.push(field_ident.clone());
                    let _ = writeln!(
                        fields,
                        "        {}: {},",
                        field_ident,
                        self.rust_type(&field.ty)
                    );
                    let _ = writeln!(
                        payload,
                        "                    {},",
                        self.to_value_expr(&field.ty, &format!("&{field_ident}"))
                    );
                    let _ = writeln!(
                        unpack_fields,
                        "                    {field_ident}: {}?,",
                        self.value_from_expr(&field.ty, &format!("&payload[{field_index}]"))
                    );
                }
                let _ = writeln!(variants, "    {variant_name} {{\n{fields}    }},");
                let _ = writeln!(
                    pack_arms,
                    "            Self::{variant_name} {{ {} }} => {api}::Value::Enum {{\n                name: {name:?}.to_string(),\n                variant: {variant_name:?}.to_string(),\n                payload: ::std::vec![\n{payload}                ],\n            }},",
                    field_names.join(", "),
                );
                let _ = writeln!(
                    unpack_arms,
                    "                {variant_name:?} => {{
                    if payload.len() != {expected} {{
                        return ::core::result::Result::Err(::std::format!(
                            \"{name}.{variant_name} carries {expected} payload value(s), found {{}}\",
                            payload.len()
                        ));
                    }}
                    ::core::result::Result::Ok(Self::{variant_name} {{
{unpack_fields}                    }})
                }}",
                    expected = variant.fields.len(),
                );
            }
        }

        let _ = write!(
            out,
            "    /// Schema boundary enum `{name}` (§9.3): variants cross the\n\
             \x20   /// boundary as `{name}.Variant` values with typed payloads.\n\
             \x20   #[derive(::core::fmt::Debug, ::core::clone::Clone, ::core::cmp::PartialEq)]\n\
             \x20   pub enum {name} {{\n{variants}    }}\n\n\
             \x20   impl {name} {{\n\
             \x20       pub fn to_value(&self) -> {api}::Value {{\n\
             \x20           match self {{\n{pack_arms}            }}\n\
             \x20       }}\n\n\
             \x20       pub fn from_value(value: &{api}::Value) -> ::core::result::Result<Self, ::std::string::String> {{\n\
             \x20           match value {{\n\
             \x20               {api}::Value::Enum {{ name, variant, payload }} if name == {name:?} => match variant.as_str() {{\n{unpack_arms}                    other => ::core::result::Result::Err(::std::format!(\n\
             \x20                       \"unknown variant `{{}}` of enum `{name}`\",\n\
             \x20                       other\n\
             \x20                   )),\n\
             \x20               }},\n\
             \x20               other => ::core::result::Result::Err(::std::format!(\n\
             \x20                   \"expected enum `{name}`, found `{{}}`\",\n\
             \x20                   __convert::kind_of(other)\n\
             \x20               )),\n\
             \x20           }}\n\
             \x20       }}\n\
             \x20   }}\n\n"
        );
    }

    fn render_capability(&self, contract: &SchemaContract, out: &mut String) {
        let qualified = format!("{}.{}", self.schema.namespace, contract.name);
        let trait_name = self.capability_trait_name(&contract.name);
        let register_name = self.register_fn_name(&contract.name);
        let api = self.api;

        // The trait: schema signatures as Rust methods (§9.6).
        let mut methods = String::new();
        for member in &contract.members {
            let member_ident = self.snake_ident(&member.name);
            let params: Vec<String> = member
                .params
                .iter()
                .map(|param| {
                    format!(
                        "{}: {}",
                        self.snake_ident(&param.name),
                        self.rust_type(&param.ty)
                    )
                })
                .collect();
            let ret = match &member.return_ty {
                Type::Void => String::new(),
                ty => format!(" -> {}", self.rust_type(ty)),
            };
            let _ = writeln!(
                methods,
                "    /// Schema member `{qualified}.{member_name}` (since {since}) — arity and types are\n\
                 \x20   /// compile-time checked against this signature.\n\
                 \x20   fn {member_ident}(&self{params}){ret};",
                member_name = member.name,
                since = format_version(&member.since),
                params = if params.is_empty() {
                    String::new()
                } else {
                    format!(", {}", params.join(", "))
                },
            );
        }

        // The bridge: CapabilityProvider dispatch, per-member unpack/pack.
        let mut arms = String::new();
        for member in &contract.members {
            let member_ident = self.snake_ident(&member.name);
            let mut unpack = String::new();
            let mut call_args = Vec::new();
            for (index, param) in member.params.iter().enumerate() {
                let param_ident = self.snake_ident(&param.name);
                let _ = writeln!(
                    unpack,
                    "                let {param_ident} = {}?;",
                    self.value_from_expr(
                        &param.ty,
                        &format!(
                            "__convert::expect_arg(args, {index}, {member_name:?})?",
                            member_name = member.name
                        )
                    )
                );
                call_args.push(param_ident);
            }
            let (call, pack) = match &member.return_ty {
                Type::Void => (
                    format!(
                        "                self.provider.{}({});",
                        member_ident,
                        call_args.join(", ")
                    ),
                    format!("                ::core::result::Result::Ok({api}::Value::Void)"),
                ),
                ty => (
                    format!(
                        "                let __ret = self.provider.{}({});",
                        member_ident,
                        call_args.join(", ")
                    ),
                    format!(
                        "                ::core::result::Result::Ok({})",
                        self.to_value_expr(ty, "&__ret")
                    ),
                ),
            };
            let _ = writeln!(
                arms,
                "            {member_name:?} => {{\n{unpack}{call}\n{pack}\n            }}",
                member_name = member.name,
            );
        }

        let _ = write!(
            out,
            r#"    /// The host side of capability `{qualified}` (§9.1): implementing this
    /// trait IS the compile-time verification of the host's implementation
    /// (§9.6) — a missing member or a wrong signature is a compile error
    /// here, never a runtime surprise.
    pub trait {trait_name}: ::core::marker::Send + ::core::marker::Sync {{
{methods}    }}

    /// Dispatches [`{api}::CapabilityProvider`] to a [`{trait_name}`] impl,
    /// unpacking schema-typed arguments in declaration order.
    pub struct {trait_name}Bridge {{
        pub provider: ::std::sync::Arc<dyn {trait_name}>,
    }}

    impl ::core::fmt::Debug for {trait_name}Bridge {{
        fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {{
            f.write_str("{trait_name}Bridge")
        }}
    }}

    impl {api}::CapabilityProvider for {trait_name}Bridge {{
        fn call(&self, member: &str, args: &[{api}::Value]) -> ::core::result::Result<{api}::Value, ::std::string::String> {{
            match member {{
{arms}                other => ::core::result::Result::Err(::std::format!(
                    "capability `{qualified}` has no member `{{}}`",
                    other
                )),
            }}
        }}
    }}

    /// Registers the `{qualified}` provider (§13.1) with the engine.
    pub fn {register_name}(
        engine: &mut {api}::Engine,
        provider: ::std::sync::Arc<dyn {trait_name}>,
    ) -> ::core::result::Result<(), ::std::string::String> {{
        engine.register_capability(
            {qualified:?},
            ::std::sync::Arc::new({trait_name}Bridge {{ provider }}),
        )
    }}

"#,
        );
    }

    fn render_interface(&self, contract: &SchemaContract, out: &mut String) {
        let qualified = format!("{}.{}", self.schema.namespace, contract.name);
        let proxy = self.proxy_name(&contract.name);
        let api = self.api;

        let mut methods = String::new();
        for member in &contract.members {
            let member_ident = self.snake_ident(&member.name);
            let params: Vec<String> = member
                .params
                .iter()
                .map(|param| {
                    format!(
                        "{}: {}",
                        self.snake_ident(&param.name),
                        self.rust_type(&param.ty)
                    )
                })
                .collect();
            let args: Vec<String> = member
                .params
                .iter()
                .map(|param| {
                    self.to_value_expr(&param.ty, &format!("&{}", self.snake_ident(&param.name)))
                })
                .collect();

            let args = args.join(", ");
            let member_name = member.name.as_str();
            let (signature_ret, body) = match &member.return_ty {
                Type::Void => (
                    format!("::core::result::Result<(), {api}::ExecutionError>"),
                    format!(
                        "            let __result = self.context.invoke_member(Self::TARGET, {member_name:?}, &[{args}])?;\n            let _ = __result;\n            ::core::result::Result::Ok(())"
                    ),
                ),
                ty => {
                    // The unpack is exact: `result<T, E>` returns keep the
                    // script's Err payload as a typed VALUE (the proxy's
                    // Rust return is Result<Result<T, E>, ExecutionError>).
                    let finish = "            ::core::result::Result::Ok(__value)".to_string();
                    let mapped = self.value_from_expr(ty, "&__result");
                    (
                        format!(
                            "::core::result::Result<{}, {api}::ExecutionError>",
                            self.rust_type(ty)
                        ),
                        format!(
                            "            let __result = self.context.invoke_member(Self::TARGET, {member_name:?}, &[{args}])?;\n            let __value = {mapped}.map_err(__conv_err)?;\n{finish}"
                        ),
                    )
                }
            };
            let _ = writeln!(
                methods,
                "    /// Calls the script's implementation of `{qualified}.{member_name}` (since {since}). Arity\n\
                 \x20   /// and types are checked at host compile time; the result unpacks by schema type.\n\
                 \x20   pub fn {member_ident}(&self{params}) -> {signature_ret} {{\n\
                 \x20       {body}\n\
                 \x20   }}\n",
                since = format_version(&member.since),
                params = if params.is_empty() {
                    String::new()
                } else {
                    format!(", {}", params.join(", "))
                },
            );
        }

        let _ = write!(
            out,
            r#"    /// A typed handle for calling INTO the script's implementation of
    /// interface `{qualified}` (§9.6, §10.4). Construction fails when the
    /// loaded program does not implement the interface.
    pub struct {proxy}<'a, 'p> {{
        context: &'a {api}::Context<'p>,
    }}

    impl<'a, 'p> {proxy}<'a, 'p> {{
        /// The schema interface path the proxy calls through.
        pub const TARGET: &'static str = {qualified:?};

        fn __conv_err(message: ::std::string::String) -> {api}::ExecutionError {{
            {api}::ExecutionError {{
                kind: {api}::ErrorKind::Runtime,
                message,
                line: 0,
                column: 0,
                file: ::core::option::Option::None,
                span: ::core::option::Option::None,
            }}
        }}

        /// Creates the proxy; fails when the program does not implement
        /// `{qualified}` (completeness was checked at script compile time;
        /// this is the host-side guard).
        pub fn new(context: &'a {api}::Context<'p>) -> ::core::result::Result<Self, {api}::ExecutionError> {{
            if !context.has_interface(Self::TARGET) {{
                return ::core::result::Result::Err(Self::__conv_err(::std::format!(
                    "the loaded program does not implement `{qualified}`"
                )));
            }}
            ::core::result::Result::Ok(Self {{ context }})
        }}

{methods}    }}
"#
        );
    }

    fn render_descriptor(&self, out: &mut String) {
        let api = self.api;
        let version = &self.schema.version;
        let mut items = String::new();
        for item in &self.schema.items {
            match item {
                cme_core::schema::SchemaItem::Struct(decl) => {
                    let mut fields = String::new();
                    for field in &decl.fields {
                        let _ = writeln!(
                            fields,
                            "                    {api}::FieldDef {{ ty: {}, name: {:?}.to_string() }},",
                            self.type_descriptor(&field.ty),
                            field.name,
                        );
                    }
                    let _ = write!(
                        items,
                        "            {api}::SchemaItem::Struct({api}::SchemaStruct {{\n                name: {name:?}.to_string(),\n                fields: ::std::vec![\n{fields}                ],\n                span: {api}::Span::new(0, 0),\n            }}),\n",
                        name = decl.name,
                    );
                }
                cme_core::schema::SchemaItem::Enum(decl) => {
                    let mut variants = String::new();
                    for variant in &decl.variants {
                        let mut fields = String::new();
                        for field in &variant.fields {
                            let _ = writeln!(
                                fields,
                                "                        {api}::FieldDef {{ ty: {}, name: {:?}.to_string() }},",
                                self.type_descriptor(&field.ty),
                                field.name,
                            );
                        }
                        let _ = write!(
                            variants,
                            "                {api}::VariantDecl {{\n                    name: {name:?}.to_string(),\n                    fields: ::std::vec![\n{fields}                    ],\n                }},\n",
                            name = variant.name,
                        );
                    }
                    let _ = write!(
                        items,
                        "            {api}::SchemaItem::Enum({api}::SchemaEnum {{\n                name: {name:?}.to_string(),\n                variants: ::std::vec![\n{variants}                ],\n                span: {api}::Span::new(0, 0),\n            }}),\n",
                        name = decl.name,
                    );
                }
                cme_core::schema::SchemaItem::Contract(contract) => {
                    let mut members = String::new();
                    for member in &contract.members {
                        let mut params = String::new();
                        for param in &member.params {
                            let _ = writeln!(
                                params,
                                "                    {api}::Param {{ ty: {}, name: {:?}.to_string() }},",
                                self.type_descriptor(&param.ty),
                                param.name,
                            );
                        }
                        let requirement = match member.requirement {
                            MemberRequirement::Required => {
                                format!("{api}::MemberRequirement::Required")
                            }
                            MemberRequirement::Optional => {
                                format!("{api}::MemberRequirement::Optional")
                            }
                        };
                        let _ = write!(
                            members,
                            "                {api}::SchemaMember {{\n                    name: {name:?}.to_string(),\n                    params: ::std::vec![\n{params}                    ],\n                    return_ty: {},\n                    since: {api}::Version::new({}, {}, {}),\n                    requirement: {requirement},\n                    span: {api}::Span::new(0, 0),\n                }},\n",
                            self.type_descriptor(&member.return_ty),
                            member.since.major,
                            member.since.minor,
                            member.since.patch,
                            name = member.name,
                        );
                    }
                    let requires = match &contract.requires {
                        Some(requires) => format!(
                            "::core::option::Option::Some({api}::RequiresPath {{ segments: ::std::vec![{}], span: {api}::Span::new(0, 0) }})",
                            requires
                                .segments
                                .iter()
                                .map(|segment| format!("{segment:?}.to_string()"))
                                .collect::<Vec<_>>()
                                .join(", ")
                        ),
                        None => "::core::option::Option::None".to_string(),
                    };
                    let kind = match contract.kind {
                        ContractKind::Capability => format!("{api}::ContractKind::Capability"),
                        ContractKind::Interface => format!("{api}::ContractKind::Interface"),
                    };
                    let _ = write!(
                        items,
                        "            {api}::SchemaItem::Contract({api}::SchemaContract {{\n                kind: {kind},\n                name: {name:?}.to_string(),\n                requires: {requires},\n                members: ::std::vec![\n{members}                ],\n                span: {api}::Span::new(0, 0),\n            }}),\n",
                        name = contract.name,
                    );
                }
            }
        }

        let _ = write!(
            out,
            r#"    /// The schema contract as a runtime descriptor: the exact
    /// [`{api}::SchemaFile`] these bindings were generated from, without
    /// re-parsing. `register_schema` feeds it to the engine, so script-side
    /// checking and host-side bindings provably share one source of truth.
    pub fn schema() -> {api}::SchemaFile {{
        {api}::SchemaFile {{
            namespace: {namespace:?}.to_string(),
            version: {api}::Version::new({major}, {minor}, {patch}),
            items: ::std::vec![
{items}            ],
            span: {api}::Span::new(0, 0),
        }}
    }}

    /// Registers the schema contract with the engine (§9.2).
    pub fn register_schema(engine: &mut {api}::Engine) -> ::core::result::Result<(), {api}::SchemaError> {{
        engine.register_schema(schema())
    }}
"#,
            namespace = self.schema.namespace,
            major = version.major,
            minor = version.minor,
            patch = version.patch,
        );
    }
}

fn format_version(version: &Version) -> String {
    format!("{}.{}.{}", version.major, version.minor, version.patch)
}

/// A plain Rust identifier usable as a module name (no keywords).
fn is_plain_ident(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) if first.is_ascii_alphabetic() || first == '_' => {}
        _ => return false,
    }
    name.chars()
        .all(|rest| rest.is_ascii_alphanumeric() || rest == '_')
        && !is_rust_keyword(name)
}

/// The Rust keywords that need `r#` (a few get a suffix instead, handled
/// by the caller).
fn is_rust_keyword(name: &str) -> bool {
    const KEYWORDS: [&str; 49] = [
        "as", "async", "await", "become", "box", "break", "const", "continue", "do", "dyn", "else",
        "enum", "extern", "false", "final", "fn", "for", "if", "impl", "in", "let", "loop",
        "macro", "match", "mod", "move", "mut", "override", "priv", "pub", "ref", "return",
        "static", "struct", "trait", "true", "try", "type", "typeof", "union", "unsafe", "unsized",
        "use", "virtual", "where", "while", "yield", "abstract", "gen",
    ];
    KEYWORDS.contains(&name)
}
