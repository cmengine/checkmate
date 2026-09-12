; ============================================================================
; Tree-sitter highlights for Checkmate (CME)
; ============================================================================
; This file is the canonical highlight query. The Zed extension copies it to
; editors/zed/languages/checkmate/highlights.scm; other editors (Neovim,
; Helix) can consume it from the grammar repository directly.

; --------------------------------------------------------------------------
; Comments
; --------------------------------------------------------------------------

(line_comment) @comment
(block_comment) @comment

; --------------------------------------------------------------------------
; Literals
; --------------------------------------------------------------------------

(string) @string
(pattern_iliteral) @string.special
(int_literal) @number
(float_literal) @number
(bool_literal) @boolean
(wildcard) @constant.builtin
(char_literal) @string
(version) @number

; Interpolated strings: the text chunks are string content, islands hold
; real Checkmate expressions (highlighted by their own rules).
(interp_chunk) @string

; --------------------------------------------------------------------------
; Keywords — core language
; --------------------------------------------------------------------------

[
  "import"
  "struct"
  "enum"
  "impl"
  "return"
  "if"
  "else"
  "while"
  "for"
  "in"
  "infer"
] @keyword

; --------------------------------------------------------------------------
; Keywords — megaprogramming (§8) and schema system (§9)
; --------------------------------------------------------------------------

[
  "grammar"
  "magic"
  "rule"
  "extends"
  "skip"
  "comment"
  "string"
  "island"
  "multiline"
  "until"
  "lineRest"
  "soft"
  "optional"
  "each"
  "sep"
  "trailing"
  "oneof"
  "peek"
  "not"
  "indent"
  "verbatim"
  "raw"
  "label"
  "where"

  "as"
  "with"
  "context"
  "some"
  "all"
  "when"
  "schema"
  "capability"
  "interface"
  "since"
  "requires"
  "suspend"
] @keyword

; Line-mode machinery wrapped in dedicated nodes.
[
  (pattern_eol)
  (pattern_line)
  (pattern_eof)
] @keyword

["any" "scan"] @function.builtin

; Inert editor metadata: #complete(...), #hover("..."), #token("...")
(pattern_annotation
  "#" @punctuation.special
  kind: _ @attribute)

; --------------------------------------------------------------------------
; Types
; --------------------------------------------------------------------------

(primitive_type) @type.builtin
(type_identifier) @type

(struct_declaration
  name: (identifier) @type)

(enum_declaration
  name: (identifier) @type)

(enum_variant
  name: (identifier) @constructor)

(type_parameters
  (identifier) @type)

; --------------------------------------------------------------------------
; Functions and calls
; --------------------------------------------------------------------------

(function_declaration
  name: (identifier) @function)

; Any call target: f(x), obj.method(x), @compileTime(x), $splice(args)
(call_expression
  function: (identifier) @function.call)

(call_expression
  function: (field_expression
    property: (identifier) @function.call))

(compile_time_call
  function: (dotted_path) @function.call)

"@" @operator

; Named arguments at call sites: `f(name: value)`
(named_argument
  name: (identifier) @variable.parameter)

; --------------------------------------------------------------------------
; Fields, parameters, variables
; --------------------------------------------------------------------------

(field_declaration
  name: (identifier) @property)

(field_expression
  property: (identifier) @property)

(parameter
  name: (identifier) @variable.parameter)

(schema_member
  name: (identifier) @function)

(context_parameter
  name: (identifier) @variable.parameter)

(context_binding_field
  name: (identifier) @property)

(match_pattern
  name: (identifier) @constructor)

(match_pattern
  (pattern_payload
    name: (identifier) @variable))

; --------------------------------------------------------------------------
; Declarations — megaprogramming
; --------------------------------------------------------------------------

(grammar_declaration
  name: (identifier) @type)

(grammar_declaration
  parent: (identifier) @type)

(rule_declaration
  name: (identifier) @function)

(magic_declaration
  name: (dotted_path) @function)

(magic_invocation
  macro: (dotted_path) @function.call)

(pattern_rule_ref
  rule: (dotted_path) @function.call)

(pattern_rule_ref
  rule: (recur) @function.builtin)

(oneof_branch
  tag: (identifier) @label)

; Pattern fragments: $str, $word, $tag, $tt<...>, $raw<...>, i$tag, ...
(pattern_fragment
  (fragment_token) @variable.special)

; --------------------------------------------------------------------------
; Imports and paths
; --------------------------------------------------------------------------

(import_statement
  path: (dotted_path) @module)

(impl_declaration
  target: (dotted_path) @module)

(requires_clause
  path: (dotted_path) @module)

; --------------------------------------------------------------------------
; Operators and punctuation
; --------------------------------------------------------------------------

[
  "||" "&&" "==" "!=" "<" "<=" ">" ">="
  "+" "-" "*" "/" "%"
  "=" "+=" "-=" "*=" "/=" "%="
  "!"
] @operator

[
  "(" ")" "[" "]" "{" "}"
] @punctuation.bracket

[
  "," "." ":" "=>" "?" "#"
] @punctuation.delimiter

"?" @operator

; --------------------------------------------------------------------------
; Magic regions: foreign-language content inside magic(name) { ... } and
; heredocs. Rendered as embedded/special strings by themes.
; --------------------------------------------------------------------------

(region_text) @embedded
(region_slash) @embedded
(region_string) @string.special
(region_backtick_string) @string.special
(heredoc_region) @string.special
