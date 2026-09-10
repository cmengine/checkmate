use cme_compiler::lexer::lex_with_errors;
use cme_compiler::parser::Parser;

fn parts(source: &str) {
    let (tokens, lex_errors) = lex_with_errors(source);
    let mut count = lex_errors.len();
    let (tokens, strip_errors) = Parser::strip_insignificant_newlines_with_errors(tokens);
    count += strip_errors.len();
    let (statements, parse_errors) = Parser::new(&tokens).parse_program_with_errors();
    count += parse_errors.len();
    for e in parse_errors.iter().take(4) {
        println!("diag: {e}");
    }
    println!("stmts={} errors={}", statements.len(), count);
}

fn main() {
    for source in [
        "int f() {\nint total = 10 +\n    20\nreturn total\n}",
        "int f() {\nif (a &&\n    b) {\nreturn 1\n}",
    ] {
        match Parser::strip_insignificant_newlines(lex_with_errors(source).0) {
            Ok(_) => println!("strip ok"),
            Err(e) => println!("STRIP ERR: {e}"),
        }
    }
    let depth = 5000;
    let source = format!(
        "int f() {{\n{}x += 1\n{}return x\n}}\n",
        "if (true) {\n".repeat(depth),
        "}\n".repeat(depth)
    );
    parts(&source);
}
