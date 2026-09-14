/*
 * checkmate-highlight.js — registers a `checkmate` language mode with the
 * highlight.js instance mdBook ships, so fenced ```checkmate blocks get
 * real highlighting instead of plain text.
 *
 * Checkmate is a C-like surface: C++-style comments, string literals,
 * decimal / float numbers, and a compact keyword set. Schema files reuse
 * the same lexer, so `schema`, `capability`, `interface`, `since`,
 * `optional`, `requires`, `suspend`, and `impl` are included here too.
 */
(function () {
  "use strict";
  if (typeof window === "undefined" || !window.hljs) return;

  var KEYWORDS = {
    keyword:
      "if else while for in match return infer import self impl struct enum " +
      "void int float bool str option result some none " +
      "grammar mega rule skip comment string island each sep trailing " +
      "optional oneof peek not until lineRest eol line eof soft raw indent " +
      "verbatim where context with label recur as and " +
      "schema capability interface since requires suspend " +
      "let require",
    literal: "true false",
    built_in: "Ok Err Some None length"
  };

  window.hljs.registerLanguage("checkmate", function (hljs) {
    var LITERAL = {
      className: "string",
      begin: '"', end: '"',
      contains: [hljs.BACKSLASH_ESCAPE],
      relevance: 0
    };
    var INTERP_STRING = {
      className: "string",
      begin: '\\$', beginScope: "operator", end: '"',
      contains: [
        hljs.BACKSLASH_ESCAPE,
        {
          className: "subst",
          begin: "\\{", end: "\\}",
          keywords: KEYWORDS,
          contains: [
            { className: "number", begin: hljs.C_NUMBER_RE, relevance: 0 },
            LITERAL
          ]
        }
      ],
      relevance: 5
    };
    var LINE_COMMENT = hljs.COMMENT("//", "$", { contains: [{ begin: /\\\n/, relevance: 0 }] });
    var BLOCK_COMMENT = hljs.COMMENT("/\\*", "\\*/", { contains: ["self"] });

    return {
      name: "checkmate",
      aliases: ["cm", "cme"],
      keywords: KEYWORDS,
      contains: [
        LINE_COMMENT,
        BLOCK_COMMENT,
        INTERP_STRING,
        LITERAL,
        { className: "char", begin: "'", end: "'", contains: [hljs.BACKSLASH_ESCAPE], relevance: 0 },
        { className: "number", begin: hljs.C_NUMBER_RE, relevance: 0 },
        { className: "operator", begin: /[+\-*/%=<>!?:&|]+/, relevance: 0 }
      ]
    };
  });
})();
