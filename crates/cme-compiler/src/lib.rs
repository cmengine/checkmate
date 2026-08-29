pub mod lexer;

#[cfg(test)]
mod tests {
    use super::lexer::Token;
    use logos::Logos;

    #[test]
    fn test_basic_lexing() {
        let source = r#"
            // This is a checkmate script
            int score = 10
            infer name = 42
        "#;

        // Logos creates an iterator over the tokens
        let mut lexer = Token::lexer(source);

        // Assert the tokens are exactly what we expect
        assert_eq!(lexer.next(), Some(Ok(Token::KwInt)));
        assert_eq!(lexer.next(), Some(Ok(Token::Ident("score"))));
        assert_eq!(lexer.next(), Some(Ok(Token::Assign)));
        assert_eq!(lexer.next(), Some(Ok(Token::IntLit(10))));

        assert_eq!(lexer.next(), Some(Ok(Token::KwInfer)));
        assert_eq!(lexer.next(), Some(Ok(Token::Ident("name"))));
        assert_eq!(lexer.next(), Some(Ok(Token::Assign)));
        assert_eq!(lexer.next(), Some(Ok(Token::IntLit(42))));

        // We should be at the end of the file
        assert_eq!(lexer.next(), None);
    }
}
