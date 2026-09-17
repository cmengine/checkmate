//! The §9 schema data model: the host contract a `.cm` schema file declares.
//!
//! A schema file (WHITEPAPER §9.1) defines ONE top-level namespace root.
//! `since X.Y.Z` versions the BLOCK (§9.5) and applies to every member
//! inside; the same contract may be declared in several blocks, whose
//! members union:
//!
//! ```text
//! schema engine 1.4.0
//!
//! since 1.0.0 capability graphics {
//!     TextureHandle LoadTexture(str path)
//! }
//!
//! since 1.0.0 interface gamemode requires core {
//!     GameState InitGame(GameConfig config)
//! }
//! ```
//!
//! This module owns the plain data shapes — [`Version`], [`SchemaMember`],
//! [`SchemaContract`], and [`SchemaFile`] — shared by the schema parser
//! (`cme-compiler`), the script-side checker, the host APIs, and the
//! generated bindings. Recognition and parsing stay in `cme-compiler`
//! (the same split the AST follows). Like the AST, everything carries its
//! source [`Span`] so downstream consumers anchor diagnostics exactly.
//!
//! Types reuse [`cme_core::ast::Type`] and the AST declaration pieces
//! ([`FieldDef`], [`VariantDecl`], [`Param`]): a schema's structs and enums
//! are the shared data-interchange layouts across the FFI boundary (§9.3)
//! and are spelled exactly like their Checkmate counterparts.
//!
//! Deliberately out of scope for this model (per the repository's schema
//! milestone): `suspend` members (the async/continuation system, §4) are
//! represented — a parser that sees one must reject it — but carry no
//! semantics here yet.

use std::fmt;

use crate::ast::{FieldDef, Param, Span, Type, VariantDecl};

/// A `X.Y.Z` schema version (§9.5): `since` tags, the schema header, and a
/// mod manifest's `[schemas]` target versions all use this shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl Version {
    /// The version every untagged BLOCK implicitly introduces (§9.5:
    /// members of a block without `since` have existed since the
    /// beginning, so every target version sees them).
    pub const ZERO: Version = Version {
        major: 0,
        minor: 0,
        patch: 0,
    };

    pub fn new(major: u32, minor: u32, patch: u32) -> Version {
        Version {
            major,
            minor,
            patch,
        }
    }

    /// Parses `X.Y.Z` with all-numeric components — the shape the manifest
    /// reader and the schema parser both accept.
    pub fn parse(text: &str) -> Option<Version> {
        let parts: Vec<&str> = text.split('.').collect();
        if parts.len() != 3 {
            return None;
        }
        let mut numbers = [0u32; 3];
        for (slot, part) in numbers.iter_mut().zip(&parts) {
            if part.is_empty() || !part.bytes().all(|byte| byte.is_ascii_digit()) {
                return None;
            }
            // u32::from_str rejects leading zeros? No — it accepts them;
            // `X.Y.Z` versions here allow them too (`01.0.0` stays 1.0.0).
            *slot = part.parse().ok()?;
        }
        Some(Version::new(numbers[0], numbers[1], numbers[2]))
    }
}

impl fmt::Display for Version {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// Whether a member was declared `optional` (§9.5): interface functions a
/// mod may leave unimplemented without breaking compilation. Capabilities
/// do not use the flag (a capability member the script calls must exist).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemberRequirement {
    Required,
    Optional,
}

/// One capability or interface member (§9.1): the boundary function's
/// signature plus its contract metadata. `since` is the version that
/// introduced the member (§9.5); members the mod's target version predates
/// are hidden from the script entirely.
#[derive(Debug, Clone, PartialEq)]
pub struct SchemaMember {
    /// PascalCase member name — the boundary function (§2.5, §9.3).
    pub name: String,
    pub params: Vec<Param>,
    pub return_ty: Type,
    pub since: Version,
    pub requirement: MemberRequirement,
    /// The member's full source span (name through the parameter list).
    pub span: Span,
}

impl SchemaMember {
    /// True when this member is visible under `target` (§9.5: the compiler
    /// hides members introduced in versions newer than the target).
    pub fn visible_at(&self, target: Version) -> bool {
        self.since <= target
    }
}

/// Which side of the boundary a declared contract serves (§9.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContractKind {
    /// `capability` — functions the HOST provides; scripts import and call
    /// them (§7.2 capability gating).
    Capability,
    /// `interface` — functions the SCRIPT implements via `impl` blocks;
    /// the host calls in through them (§9.4, §10.4).
    Interface,
}

impl ContractKind {
    /// The keyword that declares this kind, for diagnostics.
    pub fn keyword(&self) -> &'static str {
        match self {
            ContractKind::Capability => "capability",
            ContractKind::Interface => "interface",
        }
    }
}

/// A `capability` or `interface` declaration (§9.1) with its `requires`
/// edge, if any (§9.4). The requires path is the declared segment list:
/// one segment names an interface of the SAME namespace, two segments
/// (`ui.widgets`) name one of another namespace (§9.2).
#[derive(Debug, Clone, PartialEq)]
pub struct SchemaContract {
    pub kind: ContractKind,
    /// PascalCase contract name (§2.5, §9.3).
    pub name: String,
    /// The `requires` path segments, when declared (§9.4).
    pub requires: Option<RequiresPath>,
    pub members: Vec<SchemaMember>,
    /// Span of the declaration head (`capability graphics`).
    pub span: Span,
}

/// A `requires` dependency path (§9.4) with its source span.
#[derive(Debug, Clone, PartialEq)]
pub struct RequiresPath {
    pub segments: Vec<String>,
    pub span: Span,
}

impl RequiresPath {
    /// The fully-qualified path this requirement names: a single segment is
    /// relative to the declaring schema's own namespace (§9.2).
    pub fn qualified(&self, namespace: &str) -> String {
        if self.segments.len() == 1 {
            format!("{namespace}.{}", self.segments[0])
        } else {
            self.segments.join(".")
        }
    }
}

/// A `struct` declared inside a schema file (§9.3): one of the shared
/// data-interchange layouts across the FFI boundary. Spelled exactly like
/// a Checkmate struct (§2.6) — newline-delimited fields — but always
/// PascalCase and never generic.
#[derive(Debug, Clone, PartialEq)]
pub struct SchemaStruct {
    pub name: String,
    pub fields: Vec<FieldDef>,
    pub span: Span,
}

/// An `enum` declared inside a schema file (§9.3): a tagged union whose
/// variants may carry typed payloads (§2.7 shape, PascalCase name).
#[derive(Debug, Clone, PartialEq)]
pub struct SchemaEnum {
    pub name: String,
    pub variants: Vec<VariantDecl>,
    pub span: Span,
}

/// One top-level declaration of a schema file, in source order.
#[derive(Debug, Clone, PartialEq)]
pub enum SchemaItem {
    Struct(SchemaStruct),
    Enum(SchemaEnum),
    Contract(SchemaContract),
}

impl SchemaItem {
    /// The declared name, for duplicate detection and indexes.
    pub fn name(&self) -> &str {
        match self {
            SchemaItem::Struct(decl) => &decl.name,
            SchemaItem::Enum(decl) => &decl.name,
            SchemaItem::Contract(decl) => &decl.name,
        }
    }

    pub fn span(&self) -> Span {
        match self {
            SchemaItem::Struct(decl) => decl.span,
            SchemaItem::Enum(decl) => decl.span,
            SchemaItem::Contract(decl) => decl.span,
        }
    }
}

/// One parsed schema file (§9.1, §9.2): exactly one namespace root, its
/// declared version, and every top-level declaration in source order.
#[derive(Debug, Clone, PartialEq)]
pub struct SchemaFile {
    /// The namespace root (`schema engine …`) — the first segment of every
    /// path into this schema (`engine.graphics.LoadTexture`, §2.3).
    pub namespace: String,
    pub version: Version,
    pub items: Vec<SchemaItem>,
    /// Span of the whole file declaration (header through last item).
    pub span: Span,
}

impl SchemaFile {
    /// The schema's struct declarations.
    pub fn structs(&self) -> impl Iterator<Item = &SchemaStruct> {
        self.items.iter().filter_map(|item| match item {
            SchemaItem::Struct(decl) => Some(decl),
            _ => None,
        })
    }

    /// The schema's enum declarations.
    pub fn enums(&self) -> impl Iterator<Item = &SchemaEnum> {
        self.items.iter().filter_map(|item| match item {
            SchemaItem::Enum(decl) => Some(decl),
            _ => None,
        })
    }

    /// The schema's capability and interface declarations, in order.
    pub fn contracts(&self) -> impl Iterator<Item = &SchemaContract> {
        self.items.iter().filter_map(|item| match item {
            SchemaItem::Contract(decl) => Some(decl),
            _ => None,
        })
    }

    /// Looks up a capability by name (§9.1).
    pub fn capability(&self, name: &str) -> Option<&SchemaContract> {
        self.contracts()
            .find(|decl| decl.kind == ContractKind::Capability && decl.name == name)
    }

    /// Looks up an interface by name (§9.1).
    pub fn interface(&self, name: &str) -> Option<&SchemaContract> {
        self.contracts()
            .find(|decl| decl.kind == ContractKind::Interface && decl.name == name)
    }

    /// Looks up a declared type (struct or enum) by name (§9.3).
    pub fn declared_type(&self, name: &str) -> Option<&SchemaItem> {
        self.items.iter().find(|item| {
            matches!(item, SchemaItem::Struct(_) | SchemaItem::Enum(_)) && item.name() == name
        })
    }
}

/// §2.5 boundary capitalization: `PascalCase` — the shape every schema
/// declaration (types, capabilities, interfaces, members) must have.
/// A single leading uppercase run followed by lowercase/digits is enough
/// here: `LoadTexture`, `HUD`, `Vec2` all qualify.
pub fn is_pascal_case(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) if first.is_ascii_uppercase() => {}
        _ => return false,
    }
    chars.all(|rest| rest.is_ascii_alphanumeric() || rest == '_')
}

/// §2.5 boundary capitalization: `camelCase` — the shape script-internal
/// declarations take. Used for schema parameter and field names.
pub fn is_camel_case(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) if first.is_ascii_lowercase() || first == '_' => {}
        _ => return false,
    }
    chars.all(|rest| rest.is_ascii_alphanumeric() || rest == '_')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_parse_and_order() {
        assert_eq!(Version::parse("1.4.0"), Some(Version::new(1, 4, 0)));
        assert_eq!(Version::parse("1.4"), None);
        assert_eq!(Version::parse("1.x.0"), None);
        assert_eq!(Version::parse("1..0"), None);
        assert_eq!(Version::parse(""), None);
        assert!(Version::new(1, 4, 0) > Version::new(1, 3, 9));
        assert!(Version::new(1, 0, 0) <= Version::new(1, 0, 0));
        assert_eq!(Version::new(1, 4, 0).to_string(), "1.4.0");
    }

    #[test]
    fn member_visibility_follows_since() {
        let member = SchemaMember {
            name: "InvalidateSession".to_string(),
            params: vec![],
            return_ty: Type::Void,
            since: Version::new(1, 4, 0),
            requirement: MemberRequirement::Optional,
            span: Span::new(0, 0),
        };
        assert!(!member.visible_at(Version::new(1, 2, 0)));
        assert!(member.visible_at(Version::new(1, 4, 0)));
        assert!(member.visible_at(Version::new(2, 0, 0)));
    }

    #[test]
    fn requires_paths_qualify_relative_segments() {
        let relative = RequiresPath {
            segments: vec!["core".to_string()],
            span: Span::new(0, 0),
        };
        assert_eq!(relative.qualified("engine"), "engine.core");
        let qualified = RequiresPath {
            segments: vec!["ui".to_string(), "widgets".to_string()],
            span: Span::new(0, 0),
        };
        assert_eq!(qualified.qualified("engine"), "ui.widgets");
    }

    #[test]
    fn capitalization_shapes() {
        assert!(is_pascal_case("LoadTexture"));
        assert!(is_pascal_case("HUD"));
        assert!(is_pascal_case("Vec2"));
        assert!(!is_pascal_case("loadTexture"));
        assert!(!is_pascal_case(""));
        assert!(!is_pascal_case("_Load"));

        assert!(is_camel_case("loadTexture"));
        assert!(is_camel_case("_private"));
        assert!(!is_camel_case("LoadTexture"));
        assert!(!is_camel_case(""));
    }

    #[test]
    fn schema_file_lookups() {
        let file = SchemaFile {
            namespace: "engine".to_string(),
            version: Version::new(1, 4, 0),
            span: Span::new(0, 0),
            items: vec![
                SchemaItem::Contract(SchemaContract {
                    kind: ContractKind::Capability,
                    name: "graphics".to_string(),
                    requires: None,
                    members: vec![],
                    span: Span::new(0, 0),
                }),
                SchemaItem::Contract(SchemaContract {
                    kind: ContractKind::Interface,
                    name: "gamemode".to_string(),
                    requires: None,
                    members: vec![],
                    span: Span::new(0, 0),
                }),
                SchemaItem::Struct(SchemaStruct {
                    name: "TextureHandle".to_string(),
                    fields: vec![FieldDef {
                        ty: Type::Prim(crate::ast::PrimitiveType::Int),
                        name: "id".to_string(),
                    }],
                    span: Span::new(0, 0),
                }),
            ],
        };
        assert!(file.capability("graphics").is_some());
        assert!(file.capability("gamemode").is_none());
        assert!(file.interface("gamemode").is_some());
        assert!(file.interface("graphics").is_none());
        assert!(file.declared_type("TextureHandle").is_some());
        assert!(file.declared_type("graphics").is_none());
        assert_eq!(file.structs().count(), 1);
        assert_eq!(file.enums().count(), 0);
    }
}
