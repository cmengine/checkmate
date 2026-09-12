/**
 * Tree-sitter grammar for Checkmate (CME).
 *
 * Covers the full implemented language surface (WHITEPAPER.md v0.6):
 *   - the core language: newline-significant statements, Appendix A operator
 *     table, structs/enums/impls, arrays/maps, match, `?`, $"..." interpolation
 *   - §8 megaprogramming: `grammar` declarations with lexical profiles and the
 *     pattern language, `magic` declarations (patterns + templates),
 *     `magic(name) { region }` invocations with brace-balanced regions, and
 *     heredoc regions (`magic(name) <<TAG ... TAG`, external scanner)
 *   - §9 schema files: `schema`, `capability`, `interface`, `since`,
 *     `requires`, `optional`, `suspend`
 *
 * Newlines are significant at statement boundaries (AGENTS.md): they separate
 * statements, struct fields, enum variants, match arms, array elements and map
 * entries, and are insignificant only inside parentheses. Binary operators
 * join across line breaks when the line ends with the operator (§A.8), and
 * `return` is the restricted production (its value never starts on the next
 * line).
 *
 * Megaprogramming keywords (each, oneof, where, optional, peek, not, until,
 * soft, indent, raw, label, sep, trailing, recur, extends, skip, comment,
 * string, island, some, all, when, ...) are contextual: they are ordinary
 * identifiers elsewhere. The `word` property keeps keyword matching
 * boundary-exact.
 */

const PREC = {
  OR: 1,
  AND: 2,
  COMPARE: 3,
  ADD: 4,
  MUL: 5,
  UNARY: 7,
  PROPAGATE: 10,
  INDEX: 11,
  POSTFIX: 12,
  DECLARATION: 1,
};

/**
 * A list whose elements are separated by commas or newlines, with an optional
 * trailing comma: array literals, map literals. Each element consumes its own
 * trailing separators, which keeps the structure conflict-free; `[\r\n]+`
 * runs are single tokens.
 */
function commaOrNewlineList($, item) {
  return seq(
    repeat($._newline),
    repeat(
      seq(
        item,
        repeat($._newline),
        optional(seq(',', repeat($._newline))),
      ),
    ),
  );
}

/**
 * A list whose elements are separated by one or more newlines (no commas):
 * block statements, struct fields, enum variants, match arms, impl members.
 * Each element consumes its own trailing newlines, so a single element may
 * sit directly between the braces (`{ return 1 }`) and blank lines are free.
 */
function newlineList($, item) {
  return seq(repeat($._newline), repeat(seq(item, repeat($._newline))));
}

module.exports = grammar({
  name: 'checkmate',

  word: ($) => $.identifier,

  extras: ($) => [/[ \t\u00A0\uFEFF]+/, $.line_comment, $.block_comment],

  externals: ($) => [$.heredoc_region],

  // `_type` vs `_expr`: `counter c = ...` (declaration) vs `counter`
  // (expression) — both readings start with an identifier; GLR keeps both
  // alive and the non-viable one dies naturally.
  // `_lvalue` vs `_expr`: `a.b = 1` (assignment target) vs `a.b`
  // (expression statement).
  conflicts: ($) => [
    [$._type, $._expr],
    [$._lvalue, $._expr],
    // `Damage(int amount)` payloads vs an arm body starting with a call.
    [$.match_pattern],
    // `$str key`: the identifier after a fragment is its capture bind or
    // the next pattern item (a rule reference).
    [$.pattern_fragment],
    // A break inside parentheses either opens an operator continuation or
    // is the trailing break before `)`; GLR keeps both readings alive.
    [$._paren_expr],
  ],

  supertypes: ($) => [$._expr, $._statement, $._type, $._pattern_item],

  rules: {
    source_file: ($) =>
      repeat(choice($._newline, $._top_level_item)),

    // ------------------------------------------------------------------
    // Lexical
    // ------------------------------------------------------------------

    _newline: ($) => token(/[\r\n]+/),

    line_comment: ($) => token(seq('//', /[^\r\n]*/)),

    block_comment: ($) => token(/\/\*([^*]|\*+[^*\/])*\*+\//),

    string: ($) => token(/"(?:\\[^\r\n]|\\|[^"\\\r\n])*"/),

    char_literal: ($) => token(/'(?:\\.|[^\\'])'/),

    int_literal: ($) => token(/\d+/),

    float_literal: ($) => token(/\d+\.\d+/),

    bool_literal: ($) => choice('true', 'false'),

    version: ($) => token(prec(2, /v?\d+\.\d+\.\d+/)),

    wildcard: ($) => token(prec(1, '_')),

    identifier: ($) => /[a-zA-Z_][a-zA-Z0-9_]*/,

    dotted_path: ($) =>
      prec.left(seq($.identifier, repeat(seq('.', $.identifier)))),

    // Interpolated strings (§2.8/§4.1): $"...{expr}..." — text runs stop at
    // `{` (island start) and `"` (string end); escape pairs are consumed
    // atomically so `\{` stays literal text.
    interp_string: ($) =>
      seq(
        '$',
        '"',
        repeat(choice($.interp_chunk, $.interpolation)),
        '"',
      ),

    interp_chunk: ($) => token(prec(1, /(?:\\[^\r\n]|[^"{\\])+/)),

    interpolation: ($) =>
      seq(
        '{',
        repeat($._newline),
        field('expression', $._expr),
        repeat($._newline),
        '}',
      ),

    // ------------------------------------------------------------------
    // Types (§2.4, §2.9, §11)
    // ------------------------------------------------------------------

    _type: ($) =>
      choice(
        $.primitive_type,
        $.generic_type,
        $.array_type,
        alias($.identifier, $.type_identifier),
        $.magic_invocation,
      ),

    primitive_type: ($) => choice('int', 'float', 'bool', 'str', 'void'),

    generic_type: ($) =>
      prec(
        1,
        seq(
          field('name', alias($.identifier, $.type_identifier)),
          $.type_arguments,
        ),
      ),

    type_arguments: ($) =>
      seq(
        '<',
        repeat($._newline),
        $._type,
        repeat(seq(',', repeat($._newline), $._type)),
        repeat($._newline),
        '>',
      ),

    array_type: ($) => prec(2, seq($._type, '[]')),

    type_parameters: ($) =>
      seq(
        '<',
        repeat($._newline),
        $.identifier,
        repeat(seq(',', repeat($._newline), $.identifier)),
        repeat($._newline),
        '>',
      ),

    // ------------------------------------------------------------------
    // Top level (§2.1–§2.3, §2.6, §2.7, §2.11, §9, §10.4, §8)
    // ------------------------------------------------------------------

    _top_level_item: ($) =>
      choice(
        $.import_statement,
        $.struct_declaration,
        $.enum_declaration,
        $.function_declaration,
        $.impl_declaration,
        $.grammar_declaration,
        $.magic_declaration,
        // magic_invocation at top level arrives wrapped in an
        // expression_statement via _statement, keeping one canonical shape.
        $.schema_declaration,
        $.capability_declaration,
        $.interface_declaration,
        $._statement,
      ),

    import_statement: ($) => seq('import', field('path', $.dotted_path)),

    struct_declaration: ($) =>
      seq(
        'struct',
        field('name', $.identifier),
        optional($.type_parameters),
        '{',
        newlineList($, $.field_declaration),
        '}',
      ),

    field_declaration: ($) =>
      seq(field('type', $._type), field('name', $.identifier)),

    enum_declaration: ($) =>
      seq(
        'enum',
        field('name', $.identifier),
        optional($.type_parameters),
        '{',
        newlineList($, $.enum_variant),
        '}',
      ),

    enum_variant: ($) =>
      seq(
        field('name', $.identifier),
        optional(field('payload', $.parameter_list)),
      ),

    function_declaration: ($) =>
      seq(
        field('return_type', $._type),
        field('name', $.identifier),
        field('parameters', $.parameter_list),
        field('body', $.block),
      ),

    parameter_list: ($) =>
      seq(
        '(',
        repeat($._newline),
        repeat(
          seq(
            $.parameter,
            repeat($._newline),
            optional(seq(',', repeat($._newline))),
          ),
        ),
        ')',
      ),

    parameter: ($) =>
      seq(field('type', $._type), field('name', $.identifier)),

    impl_declaration: ($) =>
      seq(
        'impl',
        field('target', $.dotted_path),
        '{',
        newlineList($, $.function_declaration),
        '}',
      ),

    // §9 — schema files share the .cm extension.
    schema_declaration: ($) =>
      seq('schema', field('name', $.identifier), field('version', $.version)),

    capability_declaration: ($) =>
      seq(
        'capability',
        field('name', $.identifier),
        optional(seq('requires', field('requires', $.dotted_path))),
        '{',
        newlineList($, choice($.schema_member, $.requires_clause)),
        '}',
      ),

    interface_declaration: ($) =>
      seq(
        'interface',
        field('name', $.identifier),
        optional(seq('requires', field('requires', $.dotted_path))),
        '{',
        newlineList($, choice($.schema_member, $.requires_clause)),
        '}',
      ),

    requires_clause: ($) => seq('requires', field('path', $.dotted_path)),

    schema_member: ($) =>
      seq(
        optional(seq('since', field('since', $.version))),
        optional(field('modifier', choice('suspend', 'optional'))),
        field('return_type', $._type),
        field('name', $.identifier),
        field('parameters', $.parameter_list),
      ),

    // ------------------------------------------------------------------
    // Statements (§2.10, §2.14–§2.16, §A.7)
    // ------------------------------------------------------------------

    // prec(1): `{}` in statement position is an empty block, not an empty
    // map literal; in expression position only the map reading exists.
    block: ($) => prec(1, seq('{', newlineList($, $._statement), '}')),

    _statement: ($) =>
      choice(
        prec(PREC.DECLARATION, $.variable_declaration),
        prec(PREC.DECLARATION, $.infer_declaration),
        $.assignment_statement,
        $.return_statement,
        $.if_statement,
        $.while_statement,
        $.for_statement,
        $.expression_statement,
        $.block,
      ),

    variable_declaration: ($) =>
      seq(
        field('type', $._type),
        field('name', $.identifier),
        '=',
        field('value', $._expr),
      ),

    infer_declaration: ($) =>
      seq('infer', field('name', $.identifier), '=', field('value', $._expr)),

    // Template hole: `infer $param` splices a declared name without a
    // syntactic initializer (the capture value becomes one).
    template_infer_hole: ($) =>
      seq('infer', field('name', choice($.identifier, $.splice))),

    assignment_statement: ($) =>
      seq(
        field('left', $._lvalue),
        field('operator', choice('=', '+=', '-=', '*=', '/=', '%=')),
        field('right', $._expr),
      ),

    _lvalue: ($) =>
      seq(
        $.identifier,
        repeat(
          choice(
            seq('.', $.identifier),
            seq('[', repeat($._newline), $._expr, repeat($._newline), ']'),
          ),
        ),
      ),

    // prec.right: prefer parsing a value over reducing the bare form —
    // `return` is still the restricted production because the value itself
    // cannot begin on the next line (statements separate on newlines).
    return_statement: ($) =>
      prec.right(seq('return', optional(field('value', $._expr)))),

    // `else` attaches on the same line as the closing brace (`} else {`),
    // matching every fixture and the canonical formatting style. The real
    // parser also re-attaches a newline-separated `else` by backtracking;
    // a pure-CFG grammar cannot speculatively consume the break, so that
    // spelling is left to recovery here.
    if_statement: ($) =>
      prec.right(
        seq(
          'if',
          repeat($._newline),
          field('condition', $._parenthesized),
          repeat($._newline),
          field('consequence', $.block),
          optional(field('alternative', $.else_clause)),
        ),
      ),

    else_clause: ($) => choice($.if_statement, $.block),

    while_statement: ($) =>
      seq(
        'while',
        repeat($._newline),
        field('condition', $._parenthesized),
        repeat($._newline),
        field('body', $.block),
      ),

    for_statement: ($) =>
      seq(
        'for',
        repeat($._newline),
        '(',
        repeat($._newline),
        field('element_type', $._type),
        field('element_name', $.identifier),
        'in',
        repeat($._newline),
        field('iterable', $._expr),
        repeat($._newline),
        ')',
        repeat($._newline),
        field('body', $.block),
      ),

    expression_statement: ($) => $._expr,

    // ------------------------------------------------------------------
    // Expressions (Appendix A)
    // ------------------------------------------------------------------

    _expr: ($) =>
      choice(
        $.identifier,
        $.int_literal,
        $.float_literal,
        $.bool_literal,
        $.string,
        $.interp_string,
        $.parenthesized_expression,
        $.call_expression,
        $.field_expression,
        $.index_expression,
        $.propagate_expression,
        $.unary_expression,
        $.binary_expression,
        $.match_expression,
        $.array_literal,
        $.map_literal,
        $.splice,
        $.splice_array,
        $.compile_time_call,
        $.quantifier_expression,
        $.magic_invocation,
      ),

    _parenthesized: ($) =>
      seq(
        '(',
        repeat($._newline),
        $._paren_expr,
        repeat($._newline),
        ')',
      ),

    parenthesized_expression: ($) => $._parenthesized,

    // Inside parentheses newlines are insignificant (§A.8), so a chain may
    // continue with ANY binary operator at the head of the next line:
    //
    //     int wrapped = (
    //         base
    //         + bonus
    //     )
    //
    // The continuation lives HERE, not in binary_expression, because the
    // leading break is unambiguous only at the paren-expression level (a
    // statement-level break before an operator is a compile error). Each
    // continuation operand is a full _expr, so operator precedence is kept
    // for the operand itself.
    _paren_expr: ($) =>
      seq(
        $._expr,
        repeat(
          seq(
            repeat1($._newline),
            field(
              'operator',
              choice(
                '||',
                '&&',
                '==',
                '!=',
                '<',
                '<=',
                '>',
                '>=',
                '+',
                '-',
                '*',
                '/',
                '%',
              ),
            ),
            repeat($._newline),
            $._expr,
          ),
        ),
      ),

    call_expression: ($) =>
      prec(PREC.POSTFIX, seq(field('function', $._expr), $.arguments)),

    arguments: ($) =>
      seq(
        '(',
        repeat($._newline),
        repeat(
          seq(
            $._argument,
            repeat($._newline),
            optional(seq(',', repeat($._newline))),
          ),
        ),
        ')',
      ),

    // Named arguments may be comma-separated or adjacent (§2.12: after the
    // insignificant-newline pass, the multi-line named style arrives as
    // adjacent `name: value` pairs). Template element lists may contain
    // `[each ...]` repetitions directly (§8.4 join conventions).
    _argument: ($) =>
      choice($.named_argument, $.template_each, $.template_when, $._expr),

    named_argument: ($) =>
      seq(
        field('name', $.identifier),
        ':',
        repeat($._newline),
        field('value', $._expr),
      ),

    field_expression: ($) =>
      prec(
        PREC.POSTFIX,
        seq(field('object', $._expr), '.', field('property', $.identifier)),
      ),

    index_expression: ($) =>
      prec(
        PREC.INDEX,
        seq(
          field('object', $._expr),
          '[',
          repeat($._newline),
          field('index', $._expr),
          repeat($._newline),
          ']',
        ),
      ),

    // `$base[]` — a spliced name followed by an empty index (the `listOf`
    // template emits an array type this way).
    splice_array: ($) => prec(PREC.INDEX, seq($.splice, '[]')),

    propagate_expression: ($) => prec(PREC.PROPAGATE, seq($._expr, '?')),

    unary_expression: ($) =>
      prec(
        PREC.UNARY,
        seq(
          field('operator', choice('-', '!')),
          repeat($._newline),
          field('operand', $._expr),
        ),
      ),

    // Binary operators join across a line break when the line ends with the
    // operator (§A.8). A break BEFORE an operator occurs only inside
    // parentheses (where the real pipeline strips it) — _parenthesized
    // carries a dedicated continuation for that spelling below.
    binary_expression: ($) =>
      choice(
        prec.left(
          PREC.OR,
          seq(
            field('left', $._expr),
            field('operator', '||'),
            repeat($._newline),
            field('right', $._expr),
          ),
        ),
        prec.left(
          PREC.AND,
          seq(
            field('left', $._expr),
            field('operator', '&&'),
            repeat($._newline),
            field('right', $._expr),
          ),
        ),
        prec.left(
          PREC.COMPARE,
          seq(
            field('left', $._expr),
            field('operator', choice('==', '!=', '<', '<=', '>', '>=')),
            repeat($._newline),
            field('right', $._expr),
          ),
        ),
        prec.left(
          PREC.ADD,
          seq(
            field('left', $._expr),
            field('operator', choice('+', '-')),
            repeat($._newline),
            field('right', $._expr),
          ),
        ),
        prec.left(
          PREC.MUL,
          seq(
            field('left', $._expr),
            field('operator', choice('*', '/', '%')),
            repeat($._newline),
            field('right', $._expr),
          ),
        ),
      ),

    // `some x in xs { ... }` / `all x in xs { ... }` (§8.3.4 conditions).
    quantifier_expression: ($) =>
      seq(
        field('quantifier', choice('some', 'all')),
        field('variable', $.identifier),
        'in',
        field('list', $._expr),
        '{',
        field('test', $._expr),
        '}',
      ),

    // §2.15 — match in both statement and expression position.
    match_expression: ($) =>
      seq(
        'match',
        repeat($._newline),
        field('subject', $._parenthesized),
        repeat($._newline),
        '{',
        newlineList($, $.match_arm),
        '}',
      ),

    match_arm: ($) =>
      seq(
        field('pattern', $.match_pattern),
        '=>',
        repeat($._newline),
        field('body', choice($.block, $._expr)),
      ),

    match_pattern: ($) =>
      choice(
        $.wildcard,
        seq(
          field('name', $.identifier),
          optional(
            seq(
              '(',
              repeat($._newline),
              optional($.pattern_payload),
              repeat($._newline),
              ')',
            ),
          ),
        ),
      ),

    pattern_payload: ($) =>
      seq(
        field('type', $._type),
        field('name', $.identifier),
        repeat(
          seq(
            ',',
            repeat($._newline),
            $._type,
            repeat($._newline),
            $.identifier,
          ),
        ),
      ),

    array_literal: ($) =>
      seq(
        '[',
        commaOrNewlineList($, choice($._expr, $.template_each, $.template_when)),
        ']',
      ),

    map_literal: ($) =>
      seq('{', commaOrNewlineList($, $.map_entry), '}'),

    map_entry: ($) =>
      seq(
        field('key', choice($.string, $.int_literal, $.float_literal, $.identifier)),
        ':',
        repeat($._newline),
        field('value', $._expr),
      ),

    // §8.5 — `@fn(...)` executes at compile time during expansion. prec.right
    // binds the argument list to the @-call itself.
    compile_time_call: ($) =>
      prec.right(
        seq('@', field('function', $.dotted_path), optional($.arguments)),
      ),

    // Template splice: `$cap`, `$item.field` (§8.4).
    splice: ($) =>
      prec.left(
        seq(
          '$',
          field('name', $.identifier),
          repeat(seq('.', field('field', $.identifier))),
        ),
      ),

    // ------------------------------------------------------------------
    // §8.2 — grammars
    // ------------------------------------------------------------------

    grammar_declaration: ($) =>
      seq(
        'grammar',
        field('name', $.identifier),
        optional(seq('extends', field('parent', $.identifier))),
        '{',
        repeat(choice($._newline, $.profile_entry, $.rule_declaration)),
        '}',
      ),

    // Lexical profile entries: skip set, comment/string/island forms.
    profile_entry: ($) =>
      seq(
        field('kind', choice('skip', 'comment', 'string', 'island')),
        choice(
          $.character_class,
          seq(
            '(',
            repeat(
              choice(
                $.string,
                $.char_literal,
                'until',
                'multiline',
                $.profile_entry,
              ),
            ),
            repeat($._newline),
            ')',
          ),
        ),
      ),

    rule_declaration: ($) =>
      seq(
        'rule',
        field('name', $.identifier),
        // `rule styleRule(context { selector parent = none }) { ... }`
        optional(seq('(', field('context', $.rule_context), ')')),
        '{',
        repeat(choice($._newline, $._pattern_item)),
        '}',
      ),

    rule_context: ($) =>
      seq('context', '{', newlineList($, $.context_parameter), '}'),

    context_parameter: ($) =>
      seq(
        field('type', $._type),
        field('name', $.identifier),
        optional(seq('=', field('default', $._expr))),
      ),

    // ------------------------------------------------------------------
    // §8.3 — the pattern language
    // ------------------------------------------------------------------

    _pattern_item: ($) =>
      choice(
        $.string,
        $.pattern_iliteral,
        $.pattern_class,
        $.pattern_any,
        $.pattern_scan,
        $.pattern_until,
        $.pattern_line_rest,
        $.pattern_eol,
        $.pattern_line,
        $.pattern_eof,
        $.pattern_soft,
        $.pattern_optional,
        $.pattern_each,
        $.pattern_oneof,
        $.pattern_peek,
        $.pattern_not,
        $.pattern_group,
        $.pattern_indent,
        $.pattern_raw,
        $.pattern_label,
        $.pattern_where,
        $.pattern_rule_ref,
        $.pattern_fragment,
        $.pattern_annotation,
      ),

    pattern_iliteral: ($) => token(/i"(?:\\[^\r\n]|\\|[^"\\\r\n])*"/),

    character_class: ($) => token(/\[(?:\\.|[^\\\]\r\n])*\]/),

    pattern_class: ($) =>
      seq($.character_class, optional(field('bind', $.pattern_bind))),

    pattern_any: ($) => seq('any', optional(field('bind', $.pattern_bind))),

    pattern_scan: ($) =>
      seq('scan', $.character_class, optional(field('bind', $.pattern_bind))),

    pattern_until: ($) =>
      seq(
        'until',
        choice($.string, field('stop', $._pattern_body)),
        optional(field('bind', $.pattern_bind)),
      ),

    pattern_line_rest: ($) =>
      seq('lineRest', optional(field('bind', $.pattern_bind))),

    pattern_eol: ($) => 'eol',

    pattern_line: ($) => 'line',

    pattern_eof: ($) => 'eof',

    pattern_soft: ($) => seq('soft', $._pattern_body),

    pattern_optional: ($) => seq('optional', $._pattern_body),

    pattern_each: ($) =>
      seq(
        'each',
        optional('+'),
        optional(
          seq(
            'sep',
            field(
              'separator',
              choice($.string, $.character_class, $.dotted_path, $.pattern_fragment),
            ),
          ),
        ),
        optional('trailing'),
        optional(field('bounds', $.pattern_bounds)),
        $._pattern_body,
        optional(field('bind', $.pattern_bind)),
      ),

    pattern_bounds: ($) => seq('[', $.int_literal, ',', $.int_literal, ']'),

    pattern_oneof: ($) =>
      seq(
        'oneof',
        '{',
        repeat(choice($._newline, ',', $.oneof_branch)),
        '}',
        optional(field('bind', $.pattern_bind)),
      ),

    // prec(1): an empty branch body `=> ()` reads as an empty branch, not an
    // empty pattern_group.
    oneof_branch: ($) =>
      prec(
        1,
        seq(
          field('tag', $.identifier),
          '=>',
          repeat($._newline),
          choice(
            seq('(', repeat(choice($._newline, $._pattern_item)), ')'),
            $._pattern_item,
          ),
        ),
      ),

    pattern_peek: ($) => seq('peek', $._pattern_body),

    pattern_not: ($) => seq('not', $._pattern_body),

    pattern_group: ($) =>
      seq(
        '(',
        repeat(choice($._newline, $._pattern_item)),
        ')',
        optional(field('bind', $.pattern_bind)),
      ),

    pattern_indent: ($) =>
      seq(
        'indent',
        choice($._pattern_body, seq('verbatim', optional(field('bind', $.pattern_bind)))),
      ),

    pattern_raw: ($) => seq('raw', $._pattern_body),

    pattern_label: ($) => seq('label', $.string, $._pattern_body),

    pattern_where: ($) => seq('where', field('condition', $._expr)),

    pattern_rule_ref: ($) =>
      seq(
        field('rule', choice($.dotted_path, $.recur)),
        optional(field('context', $.context_binding)),
        optional(field('bind', $.pattern_bind)),
      ),

    recur: ($) => 'recur',

    // `styleRule with context { parent: sel }` (§8.3.7)
    context_binding: ($) =>
      seq(
        'with',
        'context',
        '{',
        repeat(choice($._newline, ',', $.context_binding_field)),
        '}',
      ),

    context_binding_field: ($) =>
      seq(
        field('name', $.identifier),
        ':',
        repeat($._newline),
        field('value', $._expr),
      ),

    pattern_bind: ($) => seq('as', field('name', $.identifier)),

    _pattern_body: ($) =>
      seq('{', repeat(choice($._newline, $._pattern_item)), '}'),

    // Fragments bind their capture directly: `$str key`, `$tag model`,
    // `$tt<"{{" "}}"> value`, `$raw<js.program> body`, `i$tag name`.
    fragment_token: ($) =>
      token(
        prec(
          1,
          /i?\$(?:ident|word|tag|int|float|str|tt|text|template|raw|expr|type|block)/,
        ),
      ),

    pattern_fragment: ($) =>
      seq(
        $.fragment_token,
        // Validator or parameter spec: `$ident<self.notReserved>`,
        // `$raw<js.program>`, `$tt<"{{" "}}">`, `$template<"${" "}" expr>`.
        // A single segment is a valid dotted_path, so no bare-identifier
        // alternative is needed here.
        optional(
          seq('<', repeat1(choice($.string, $.dotted_path)), '>'),
        ),
        // The capture name binds directly: `$str key`, `$tag model`.
        // `$word key` may also read as `$word` followed by the rule
        // reference `key`, so GLR explores both; the bind reading survives
        // when nothing else can follow the fragment.
        optional(field('bind', $.identifier)),
      ),

    // Inert editor metadata: `#complete(...)`, `#hover("...")`, `#token("...")`.
    pattern_annotation: ($) =>
      seq(
        '#',
        field('kind', choice('complete', 'hover', 'token')),
        '(',
        repeat(choice($.dotted_path, $.string, ',')),
        ')',
      ),

    // ------------------------------------------------------------------
    // §8.1/§8.4/§8.6 — magic declarations, invocations, regions
    // ------------------------------------------------------------------

    // Magic names are module-qualified: `magic agent.spawn(...)`,
    // `magic jsonValue(...)` (§8.1 names).
    magic_declaration: ($) =>
      seq(
        'magic',
        field('name', $.dotted_path),
        '(',
        repeat(choice($._newline, $._pattern_item)),
        ')',
        '{',
        repeat(choice($._newline, $._template_item)),
        '}',
      ),

    magic_invocation: ($) =>
      prec(
        1,
        seq(
          'magic',
          '(',
          field('macro', $.dotted_path),
          ')',
          choice($.region, $.heredoc_region),
        ),
      ),

    region: ($) =>
      seq('{', repeat(choice($._newline, $._region_item)), '}'),

    _region_item: ($) =>
      choice(
        $.region_text,
        $.region_slash,
        $.region_string,
        $.region_backtick_string,
        $._region_group,
      ),

    // Region text excludes structural delimiters only. Quotes and the slash
    // are excluded so real strings and comments win the longest match; the
    // apostrophe stays IN so prose like `don't` never breaks the region
    // (there is no single-quote string token to collide with).
    region_text: ($) => token(prec(-1, /[^{}"`\/]+/)),

    region_slash: ($) => token('/'),

    region_string: ($) => token(/"(?:\\[^\r\n]|[^"\\\r\n])*"/),

    region_backtick_string: ($) => token(/`(?:[^`\\]|\\[\s\S])*`/),

    _region_group: ($) =>
      seq('{', repeat(choice($._newline, $._region_item)), '}'),

    // ------------------------------------------------------------------
    // §8.4 — expansion templates
    // ------------------------------------------------------------------

    _template_item: ($) =>
      choice(
        $._statement,
        $.template_each,
        $.template_when,
        $.template_infer_hole,
      ),

    template_each: ($) =>
      seq(
        'each',
        choice(seq(field('element', $.identifier), 'in'), 'in'),
        repeat($._newline),
        field('list', $._expr),
        optional(
          seq(
            repeat($._newline),
            'where',
            repeat($._newline),
            field('filter', $._expr),
          ),
        ),
        repeat($._newline),
        '{',
        repeat(choice($._newline, $._template_item)),
        '}',
      ),

    template_when: ($) =>
      seq(
        'when',
        repeat($._newline),
        field('condition', $._expr),
        repeat($._newline),
        '{',
        repeat(choice($._newline, $._template_item)),
        '}',
        optional(
          seq(
            'else',
            repeat($._newline),
            '{',
            repeat(choice($._newline, $._template_item)),
            '}',
          ),
        ),
      ),
  },
});
