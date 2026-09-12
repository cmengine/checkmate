//! Macro input parsing: `"path"` or `path = "path", crate = ::some::Api`.
//! Hand-rolled over the token trees — the input grammar is three tokens
//! wide, and `syn` would be a tower for a hut.

use std::path::PathBuf;

use proc_macro::{Delimiter, TokenStream, TokenTree};

/// Everything the generator needs from the invocation.
pub struct Settings {
    /// The schema file path, as written.
    pub path: PathBuf,
    /// The crate path generated code references (`::cme_api` by default).
    pub api_crate: String,
}

/// A deferred `compile_error!` message.
#[derive(Debug)]
pub struct CompileErrorMessage {
    pub(crate) message: String,
}

impl CompileErrorMessage {
    pub fn new(message: impl Into<String>) -> CompileErrorMessage {
        CompileErrorMessage {
            message: message.into(),
        }
    }

    pub fn into_compile_error(self) -> TokenStream {
        // quote-free error emission: one macro call, one string literal.
        let message = &self.message;
        format!("::core::compile_error!({message:?});")
            .parse()
            .expect("a compile_error! invocation is valid Rust")
    }
}

/// Parses the invocation grammar. Anything else is a usage error.
pub fn parse_settings(input: TokenStream) -> Result<Settings, CompileErrorMessage> {
    let mut trees: Vec<TokenTree> = input.into_iter().collect();
    let mut position = 0;
    let mut path: Option<String> = None;
    let mut api_crate: Option<String> = None;

    let usage = || {
        CompileErrorMessage::new(
            "cme_schema_bindings! usage: cme_schema_bindings!(\"schema.cm\") \
             or cme_schema_bindings!(path = \"schema.cm\", crate = ::some::Api)",
        )
    };

    while position < trees.len() {
        match &trees[position] {
            // A bare string literal: the path.
            TokenTree::Literal(literal) => {
                let text = literal.to_string();
                if path.is_some() {
                    return Err(usage());
                }
                path = Some(unquote(&text).ok_or_else(usage)?);
                position += 1;
            }
            // `name = value`
            TokenTree::Ident(ident) => {
                let name = ident.to_string();
                position += 1;
                expect_punct(&trees, &mut position, '=')?;
                position += 1;
                match name.as_str() {
                    "path" => {
                        let value = match trees.get(position) {
                            Some(TokenTree::Literal(literal)) => literal.to_string(),
                            _ => return Err(usage()),
                        };
                        path = Some(unquote(&value).ok_or_else(usage)?);
                        position += 1;
                    }
                    "crate" => {
                        // The value is a path: `::a::b` or `a::b`.
                        let mut rendered = String::new();
                        while position < trees.len() {
                            match &trees[position] {
                                TokenTree::Ident(ident) => {
                                    rendered.push_str(&ident.to_string());
                                    position += 1;
                                }
                                TokenTree::Punct(punct) if punct.as_char() == ':' => {
                                    rendered.push(':');
                                    position += 1;
                                }
                                _ => break,
                            }
                        }
                        if rendered.is_empty() {
                            return Err(usage());
                        }
                        api_crate = Some(rendered);
                    }
                    other => {
                        return Err(CompileErrorMessage::new(format!(
                            "cme_schema_bindings!: unknown option `{other}` (expected `path` or `crate`)"
                        )));
                    }
                }
            }
            TokenTree::Punct(punct) if punct.as_char() == ',' => {
                position += 1;
            }
            TokenTree::Group(group) if group.delimiter() == Delimiter::None => {
                // `#[path = ...]`-style invisible groups can wrap the
                // literal; flatten and retry.
                let inner = group.stream();
                trees.splice(position..position + 1, inner);
            }
            other => {
                return Err(CompileErrorMessage::new(format!(
                    "cme_schema_bindings!: unexpected token {other}"
                )));
            }
        }
    }

    let Some(path) = path else {
        return Err(usage());
    };
    Ok(Settings {
        path: PathBuf::from(path),
        api_crate: api_crate.unwrap_or_else(|| "::cme_api".to_string()),
    })
}

fn expect_punct(
    trees: &[TokenTree],
    position: &mut usize,
    expected: char,
) -> Result<(), CompileErrorMessage> {
    match trees.get(*position) {
        Some(TokenTree::Punct(punct)) if punct.as_char() == expected => Ok(()),
        _ => Err(CompileErrorMessage::new(format!(
            "cme_schema_bindings!: expected `{expected}` in the invocation"
        ))),
    }
}

/// Strips the quotes a string literal carries, handling the `\\` escapes
/// the preprocessor produces for paths.
fn unquote(literal: &str) -> Option<String> {
    let mut text = literal;
    for prefix in ["r\"", "br\"", "b\""] {
        if let Some(rest) = literal.strip_prefix(prefix) {
            text = rest;
            break;
        }
    }
    let inner = text.strip_prefix('"')?.strip_suffix('"')?;
    Some(inner.replace("\\\"", "\"").replace("\\\\", "\\"))
}
