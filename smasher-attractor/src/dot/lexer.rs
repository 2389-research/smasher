// ABOUTME: Tokenizer for the DOT graph language, splitting input into lexical tokens.
// ABOUTME: Produces a stream of tokens consumed by the DOT parser.

use std::time::Duration;

/// Tokens produced by the DOT lexer.
#[derive(Debug, Clone, PartialEq)]
pub enum Token {
    // Keywords
    Digraph,
    Graph,
    Node,
    Edge,
    Subgraph,
    Strict,

    // Literals
    Ident(String),
    StringLit(String),
    Number(f64),
    Duration(Duration),

    // Punctuation
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    Semi,
    Comma,
    Equals,
    Arrow,
    DashDash,

    // End
    Eof,
}

/// Errors that can occur during lexing.
#[derive(Debug, thiserror::Error)]
pub enum LexerError {
    #[error("unexpected character '{ch}' at position {pos}")]
    UnexpectedChar { ch: char, pos: usize },
    #[error("unterminated string at position {pos}")]
    UnterminatedString { pos: usize },
    #[error("unterminated comment at position {pos}")]
    UnterminatedComment { pos: usize },
}

/// Tokenize a DOT language input string into a vector of tokens.
pub fn tokenize(input: &str) -> Result<Vec<Token>, LexerError> {
    let chars: Vec<char> = input.chars().collect();
    let mut tokens = Vec::new();
    let mut pos = 0;

    while pos < chars.len() {
        let ch = chars[pos];

        // Skip whitespace
        if ch.is_ascii_whitespace() {
            pos += 1;
            continue;
        }

        // Comments
        if ch == '/' && pos + 1 < chars.len() {
            if chars[pos + 1] == '/' {
                // Line comment: skip to end of line
                pos += 2;
                while pos < chars.len() && chars[pos] != '\n' {
                    pos += 1;
                }
                continue;
            }
            if chars[pos + 1] == '*' {
                // Block comment: skip to */
                let start = pos;
                pos += 2;
                loop {
                    if pos + 1 >= chars.len() {
                        return Err(LexerError::UnterminatedComment { pos: start });
                    }
                    if chars[pos] == '*' && chars[pos + 1] == '/' {
                        pos += 2;
                        break;
                    }
                    pos += 1;
                }
                continue;
            }
        }

        // String literals
        if ch == '"' {
            let start = pos;
            pos += 1;
            let mut value = String::new();
            loop {
                if pos >= chars.len() {
                    return Err(LexerError::UnterminatedString { pos: start });
                }
                if chars[pos] == '\\' && pos + 1 < chars.len() {
                    match chars[pos + 1] {
                        '"' => {
                            value.push('"');
                            pos += 2;
                        }
                        '\\' => {
                            value.push('\\');
                            pos += 2;
                        }
                        'n' => {
                            value.push('\n');
                            pos += 2;
                        }
                        't' => {
                            value.push('\t');
                            pos += 2;
                        }
                        other => {
                            value.push('\\');
                            value.push(other);
                            pos += 2;
                        }
                    }
                    continue;
                }
                if chars[pos] == '"' {
                    pos += 1;
                    break;
                }
                value.push(chars[pos]);
                pos += 1;
            }
            tokens.push(Token::StringLit(value));
            continue;
        }

        // Numbers
        if ch.is_ascii_digit()
            || (ch == '.' && pos + 1 < chars.len() && chars[pos + 1].is_ascii_digit())
        {
            let start = pos;
            while pos < chars.len() && chars[pos].is_ascii_digit() {
                pos += 1;
            }
            if pos < chars.len()
                && chars[pos] == '.'
                && pos + 1 < chars.len()
                && chars[pos + 1].is_ascii_digit()
            {
                pos += 1;
                while pos < chars.len() && chars[pos].is_ascii_digit() {
                    pos += 1;
                }
            }
            let num_str: String = chars[start..pos].iter().collect();
            let num: f64 = num_str.parse().expect("valid number");
            tokens.push(Token::Number(num));
            continue;
        }

        // Identifiers and keywords
        if ch.is_ascii_alphabetic() || ch == '_' {
            let start = pos;
            while pos < chars.len() && (chars[pos].is_ascii_alphanumeric() || chars[pos] == '_') {
                pos += 1;
            }
            let word: String = chars[start..pos].iter().collect();
            let token = match word.to_lowercase().as_str() {
                "digraph" => Token::Digraph,
                "graph" => Token::Graph,
                "node" => Token::Node,
                "edge" => Token::Edge,
                "subgraph" => Token::Subgraph,
                "strict" => Token::Strict,
                _ => Token::Ident(word),
            };
            tokens.push(token);
            continue;
        }

        // Punctuation
        match ch {
            '{' => {
                tokens.push(Token::LBrace);
                pos += 1;
            }
            '}' => {
                tokens.push(Token::RBrace);
                pos += 1;
            }
            '[' => {
                tokens.push(Token::LBracket);
                pos += 1;
            }
            ']' => {
                tokens.push(Token::RBracket);
                pos += 1;
            }
            ';' => {
                tokens.push(Token::Semi);
                pos += 1;
            }
            ',' => {
                tokens.push(Token::Comma);
                pos += 1;
            }
            '=' => {
                tokens.push(Token::Equals);
                pos += 1;
            }
            '-' => {
                if pos + 1 < chars.len() {
                    if chars[pos + 1] == '>' {
                        tokens.push(Token::Arrow);
                        pos += 2;
                    } else if chars[pos + 1] == '-' {
                        tokens.push(Token::DashDash);
                        pos += 2;
                    } else {
                        return Err(LexerError::UnexpectedChar { ch, pos });
                    }
                } else {
                    return Err(LexerError::UnexpectedChar { ch, pos });
                }
            }
            _ => {
                return Err(LexerError::UnexpectedChar { ch, pos });
            }
        }
    }

    tokens.push(Token::Eof);
    Ok(tokens)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenize_empty_digraph() {
        let tokens = tokenize("digraph {}").unwrap();
        assert_eq!(
            tokens,
            vec![Token::Digraph, Token::LBrace, Token::RBrace, Token::Eof]
        );
    }

    #[test]
    fn tokenize_identifiers_and_keywords() {
        let tokens =
            tokenize("digraph graph node edge subgraph strict myIdent _foo bar42").unwrap();
        assert_eq!(
            tokens,
            vec![
                Token::Digraph,
                Token::Graph,
                Token::Node,
                Token::Edge,
                Token::Subgraph,
                Token::Strict,
                Token::Ident("myIdent".to_string()),
                Token::Ident("_foo".to_string()),
                Token::Ident("bar42".to_string()),
                Token::Eof,
            ]
        );
    }

    #[test]
    fn tokenize_case_insensitive_keywords() {
        let tokens = tokenize("DIGRAPH DiGraph").unwrap();
        assert_eq!(tokens, vec![Token::Digraph, Token::Digraph, Token::Eof]);
    }

    #[test]
    fn tokenize_string_literals_with_escapes() {
        let tokens = tokenize(r#""hello" "world \"escaped\"" "line\nbreak""#).unwrap();
        assert_eq!(
            tokens,
            vec![
                Token::StringLit("hello".to_string()),
                Token::StringLit("world \"escaped\"".to_string()),
                Token::StringLit("line\nbreak".to_string()),
                Token::Eof,
            ]
        );
    }

    #[test]
    fn tokenize_numbers_int_and_float() {
        let tokens = tokenize("42 3.14 0 100").unwrap();
        assert_eq!(
            tokens,
            vec![
                Token::Number(42.0),
                Token::Number(3.14),
                Token::Number(0.0),
                Token::Number(100.0),
                Token::Eof,
            ]
        );
    }

    #[test]
    fn tokenize_arrows() {
        let tokens = tokenize("-> --").unwrap();
        assert_eq!(tokens, vec![Token::Arrow, Token::DashDash, Token::Eof]);
    }

    #[test]
    fn skip_line_comments() {
        let tokens = tokenize("digraph // this is a comment\n{}").unwrap();
        assert_eq!(
            tokens,
            vec![Token::Digraph, Token::LBrace, Token::RBrace, Token::Eof]
        );
    }

    #[test]
    fn skip_block_comments() {
        let tokens = tokenize("digraph /* block comment */ {}").unwrap();
        assert_eq!(
            tokens,
            vec![Token::Digraph, Token::LBrace, Token::RBrace, Token::Eof]
        );
    }

    #[test]
    fn unterminated_string_error() {
        let err = tokenize(r#""unterminated"#).unwrap_err();
        match err {
            LexerError::UnterminatedString { pos } => assert_eq!(pos, 0),
            other => panic!("expected UnterminatedString, got {other:?}"),
        }
    }

    #[test]
    fn unexpected_character_error() {
        let err = tokenize("digraph { @ }").unwrap_err();
        match err {
            LexerError::UnexpectedChar { ch, pos: _ } => assert_eq!(ch, '@'),
            other => panic!("expected UnexpectedChar, got {other:?}"),
        }
    }

    #[test]
    fn unterminated_block_comment_error() {
        let err = tokenize("digraph /* never closed").unwrap_err();
        match err {
            LexerError::UnterminatedComment { pos: _ } => {}
            other => panic!("expected UnterminatedComment, got {other:?}"),
        }
    }

    #[test]
    fn tokenize_all_punctuation() {
        let tokens = tokenize("{ } [ ] ; , =").unwrap();
        assert_eq!(
            tokens,
            vec![
                Token::LBrace,
                Token::RBrace,
                Token::LBracket,
                Token::RBracket,
                Token::Semi,
                Token::Comma,
                Token::Equals,
                Token::Eof,
            ]
        );
    }
}
