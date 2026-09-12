; Auto-indentation for Checkmate.
;
; Statements are newline-delimited, so most indentation follows blocks and
; parenthesized groups. `config.toml` additionally carries
; increase/decrease_indent_patterns for lines ending in `{`/`(`.

; Braced blocks: functions, ifs, while/for bodies, struct/enum/impl bodies,
; grammar/magic declaration bodies.
(block) @indent

; The closing brace outdents back to the block's own level.
(block
  "}" @end) @indent

; Match bodies: arms live between the braces of the match expression.
(match_expression
  "{" @start
  "}" @end) @indent

; Parenthesized groups: conditions, call arguments, parameter lists, wrapped
; expressions (newlines are insignificant inside parentheses, §A.8).
(arguments
  "(" @start
  ")" @end) @indent

(parameter_list
  "(" @start
  ")" @end) @indent

(parenthesized_expression
  "(" @start
  ")" @end) @indent

; Array literals: elements are newline- or comma-delimited (§11.1).
(array_literal
  "[" @start
  "]" @end) @indent

; Map literals (CMON-style entries).
(map_literal
  "{" @start
  "}" @end) @indent

; Match-arm payloads `Damage(int amount)`.
(pattern_payload) @indent

; Megaprogramming pattern/template bodies (§8): soft/optional/each/oneof/
; peek/not/indent/raw/label bodies are brace-delimited.
[
  (pattern_soft)
  (pattern_optional)
  (pattern_each)
  (pattern_oneof)
  (pattern_peek)
  (pattern_not)
  (pattern_group)
  (pattern_raw)
  (pattern_label)
] @indent

; Rule context declarations: `rule r(context { ... })`.
(rule_context
  "{" @start
  "}" @end) @indent

; Context bindings: `with context { parent: sel }`.
(context_binding
  "{" @start
  "}" @end) @indent
