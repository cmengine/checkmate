; Code outline / project search structure for Checkmate.

; Top-level and impl-member functions
(function_declaration
  name: (identifier) @name) @item

; Structs and enums
(struct_declaration
  name: (identifier) @name) @item

(enum_declaration
  name: (identifier) @name) @item

; Impl blocks: the target path is the outline entry, e.g. engine.gamemode
(impl_declaration
  target: (dotted_path) @name) @item

; Imports as navigable items
(import_statement
  path: (dotted_path) @name) @item

; Megaprogramming declarations (§8)
(grammar_declaration
  name: (identifier) @name) @item

(magic_declaration
  name: (dotted_path) @name) @item

(rule_declaration
  name: (identifier) @name) @item

; Schema system declarations (§9)
(schema_declaration
  name: (identifier) @name) @item

(capability_declaration
  name: (identifier) @name) @item

(interface_declaration
  name: (identifier) @name) @item

(schema_member
  name: (identifier) @name) @item

; Enum variants (one outline level inside the enum)
(enum_variant
  name: (identifier) @name) @item
