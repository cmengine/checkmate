use logos::Logos;

#[derive(Logos, Debug, PartialEq, Clone)]
#[logos(skip r"[ \t\f]+")]
#[logos(skip r"//[^\r\n]*")]
pub enum Token<'a> {
    #[regex(r"[\r\n]+")] // one or more line breaks -> one token
    Newline,

    // Keywords
    #[token("int")]
    KwInt,
    #[token("float")]
    KwFloat,
    #[token("infer")]
    KwInfer,
    #[token("return")]
    KwReturn,
    #[token("str")]
    KwStr,
    #[token("bool")]
    KwBool,

    // Symbols
    #[token("=")]
    Assign,
    #[token("(")]
    LParen,
    #[token(")")]
    RParen,
    #[token("{")]
    LBrace,
    #[token("}")]
    RBrace,

    // Identifiers (e.g., variable names, function names)
    // This regex matches a letter or underscore, followed by any number of letters, numbers, or underscores.
    #[regex(r"[a-zA-Z_][a-zA-Z0-9_]*")]
    Ident(&'a str),

    // Integer Literals
    // This regex matches digits, and the closure parses it into an i64.
    #[regex(r"[0-9]+", |lex| lex.slice().parse::<i64>().unwrap())]
    IntLit(i64),

    // Float Literals
    #[regex(r"[0-9]+\.[0-9]+", |lex| lex.slice().parse::<f64>().unwrap())]
    FloatLit(f64),
}
