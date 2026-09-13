/**
 * External scanner for Checkmate heredoc mega regions (WHITEPAPER §8.6):
 *
 *     name! <<TAG
 *     ...verbatim region text...
 *     TAG
 *
 * The whole region (including `<<TAG` and the closing `TAG` line) is one
 * token, so no other rule ever sees its braces. The region ends at the first
 * line whose content — ignoring surrounding horizontal whitespace — is
 * exactly the tag. An unterminated region fails to scan and surfaces as a
 * parse error at the `<<`, mirroring the compiler's own diagnostic.
 */

#include "tree_sitter/parser.h"

#include <stdbool.h>
#include <stdint.h>

enum TokenType {
  HEREDOC_REGION,
};

static bool is_tag_char(int32_t c) {
  return (c >= 'A' && c <= 'Z') || (c >= 'a' && c <= 'z') ||
         (c >= '0' && c <= '9') || c == '_';
}

void *tree_sitter_checkmate_external_scanner_create(void) { return NULL; }

void tree_sitter_checkmate_external_scanner_destroy(void *payload) {
  (void)payload;
}

unsigned tree_sitter_checkmate_external_scanner_serialize(void *payload,
                                                          char *buffer) {
  (void)payload;
  (void)buffer;
  return 0;
}

void tree_sitter_checkmate_external_scanner_deserialize(void *payload,
                                                        const char *buffer,
                                                        unsigned length) {
  (void)payload;
  (void)buffer;
  (void)length;
}

static bool scan_tag_line(TSLexer *lexer, const char *tag, unsigned tag_len) {
  // The line may be preceded and followed by horizontal whitespace.
  for (;;) {
    int32_t c = lexer->lookahead;
    if (c != ' ' && c != '\t') {
      break;
    }
    lexer->advance(lexer, false);
  }
  for (unsigned i = 0; i < tag_len; i++) {
    if (lexer->lookahead != (int32_t)tag[i]) {
      return false;
    }
    lexer->advance(lexer, false);
  }
  for (;;) {
    int32_t c = lexer->lookahead;
    if (c != ' ' && c != '\t') {
      break;
    }
    lexer->advance(lexer, false);
  }
  if (lexer->lookahead == '\r') {
    lexer->advance(lexer, false);
  }
  if (lexer->lookahead == '\n') {
    lexer->advance(lexer, false);
    return true;
  }
  if (lexer->lookahead == 0 || lexer->eof(lexer)) {
    // Tag line at end of file is still a terminator.
    return true;
  }
  return false;
}

bool tree_sitter_checkmate_external_scanner_scan(void *payload, TSLexer *lexer,
                                                 const bool *valid_symbols) {
  (void)payload;

  if (!valid_symbols[HEREDOC_REGION]) {
    return false;
  }
  // The external scanner runs before the internal lexer skips extras, so
  // horizontal whitespace before `<<` must be skipped here (marked as
  // whitespace so it stays outside the token).
  while (lexer->lookahead == ' ' || lexer->lookahead == '\t') {
    lexer->advance(lexer, true);
  }
  if (lexer->lookahead != '<') {
    return false;
  }
  lexer->advance(lexer, false);
  if (lexer->lookahead != '<') {
    return false;
  }
  lexer->advance(lexer, false);
  lexer->mark_end(lexer); // `<<` committed; the tag decides everything else.

  char tag[64];
  unsigned tag_len = 0;
  while (is_tag_char(lexer->lookahead)) {
    if (tag_len < sizeof(tag)) {
      tag[tag_len] = (char)lexer->lookahead;
    }
    tag_len++;
    lexer->advance(lexer, false);
  }
  if (tag_len == 0 || tag_len > sizeof(tag)) {
    return false;
  }

  // One line terminator after the tag.
  if (lexer->lookahead == '\r') {
    lexer->advance(lexer, false);
  }
  if (lexer->lookahead != '\n') {
    return false;
  }
  lexer->advance(lexer, false);

  // Consume lines until the terminator line. Bail out on EOF so an
  // unterminated heredoc fails instead of consuming the world.
  for (;;) {
    int32_t c = lexer->lookahead;
    if (c == 0 || lexer->eof(lexer)) {
      return false;
    }
    if (c == '\n') {
      // Empty line; keep scanning.
      lexer->advance(lexer, false);
      continue;
    }
    if (c == ' ' || c == '\t' || c == '\r' || is_tag_char(c)) {
      // Only a line that could be the terminator needs the full check;
      // anything else is content and must be consumed line-wise.
      lexer->mark_end(lexer);
      if (scan_tag_line(lexer, tag, tag_len)) {
        lexer->mark_end(lexer);
        lexer->result_symbol = HEREDOC_REGION;
        return true;
      }
      // Consume the remainder of this content line.
      for (;;) {
        int32_t l = lexer->lookahead;
        if (l == 0 || lexer->eof(lexer)) {
          return false;
        }
        if (l == '\n') {
          lexer->advance(lexer, false);
          break;
        }
        lexer->advance(lexer, false);
      }
      continue;
    }
    // Ordinary content character: consume the whole line.
    for (;;) {
      int32_t l = lexer->lookahead;
      if (l == 0 || lexer->eof(lexer)) {
        return false;
      }
      if (l == '\n') {
        lexer->advance(lexer, false);
        break;
      }
      lexer->advance(lexer, false);
    }
  }
}
