use crate::token::{Token, TokenType};

pub struct Lexer {
    source: Vec<char>,
    pos: usize,
    line: usize,
    col: usize,
}

#[derive(Debug)]
pub struct LexError {
    pub message: String,
    pub line: usize,
    pub col: usize,
}

impl std::fmt::Display for LexError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Lexer error at line {}, col {}: {}", self.line, self.col, self.message)
    }
}

impl Lexer {
    pub fn new(source: &str) -> Self {
        Lexer { source: source.chars().collect(), pos: 0, line: 1, col: 1 }
    }

    fn peek(&self) -> Option<char> {
        self.source.get(self.pos).copied()
    }

    fn peek_next(&self) -> Option<char> {
        self.source.get(self.pos + 1).copied()
    }

    fn advance(&mut self) -> Option<char> {
        let ch = self.source.get(self.pos).copied()?;
        self.pos += 1;
        if ch == '\n' {
            self.line += 1;
            self.col = 1;
        } else {
            self.col += 1;
        }
        Some(ch)
    }

    fn skip_whitespace(&mut self) {
        while self.peek().map(|c| c.is_whitespace()).unwrap_or(false) {
            self.advance();
        }
    }

    fn skip_line_comment(&mut self) {
        while let Some(c) = self.peek() {
            if c == '\n' { break; }
            self.advance();
        }
    }

    fn skip_block_comment(&mut self, start_line: usize, start_col: usize) -> Result<(), LexError> {
        loop {
            match self.advance() {
                None => return Err(LexError {
                    message: "Unclosed block comment".into(),
                    line: start_line,
                    col: start_col,
                }),
                Some('*') if self.peek() == Some('/') => {
                    self.advance();
                    return Ok(());
                }
                _ => {}
            }
        }
    }

    fn read_string(&mut self, line: usize, col: usize) -> Result<Token, LexError> {
        let mut s = String::new();
        loop {
            match self.advance() {
                None => return Err(LexError { message: "Unterminated string".into(), line, col }),
                Some('"') => break,
                Some('\\') => match self.advance() {
                    Some('n')  => s.push('\n'),
                    Some('t')  => s.push('\t'),
                    Some('r')  => s.push('\r'),
                    Some('\\') => s.push('\\'),
                    Some('"')  => s.push('"'),
                    Some(c)    => { s.push('\\'); s.push(c); }
                    None => return Err(LexError { message: "Unexpected end in escape".into(), line, col }),
                },
                Some(c) => s.push(c),
            }
        }
        Ok(Token::new(TokenType::StringLit, s, line, col))
    }

    fn read_fstring(&mut self, line: usize, col: usize) -> Result<Token, LexError> {
        // The 'f' has been consumed; consume the opening '"'
        if self.advance() != Some('"') {
            return Err(LexError { message: "Expected '\"' after 'f'".into(), line, col });
        }
        let mut s = String::new();
        loop {
            match self.advance() {
                None => return Err(LexError { message: "Unterminated f-string".into(), line, col }),
                Some('"') => break,
                Some(c) => s.push(c),
            }
        }
        Ok(Token::new(TokenType::FString, s, line, col))
    }

    fn read_number(&mut self, first: char, line: usize, col: usize) -> Token {
        let mut s = first.to_string();
        let mut is_float = false;
        loop {
            match self.peek() {
                Some(c) if c.is_ascii_digit() => { s.push(c); self.advance(); }
                Some('.') if !is_float => {
                    // Only consume '.' if the next char after it is a digit (not ..)
                    if self.peek_next().map(|x| x.is_ascii_digit()).unwrap_or(false) {
                        is_float = true;
                        s.push('.');
                        self.advance();
                    } else {
                        break;
                    }
                }
                _ => break,
            }
        }
        let ty = if is_float { TokenType::Float } else { TokenType::Integer };
        Token::new(ty, s, line, col)
    }

    fn read_ident_or_keyword(&mut self, first: char, line: usize, col: usize) -> Token {
        let mut s = first.to_string();
        while let Some(c) = self.peek() {
            if c.is_alphanumeric() || c == '_' {
                s.push(c);
                self.advance();
            } else {
                break;
            }
        }
        let ty = match s.as_str() {
            "let"      => TokenType::Let,
            "fn"       => TokenType::Fn,
            "if"       => TokenType::If,
            "else"     => TokenType::Else,
            "for"      => TokenType::For,
            "while"    => TokenType::While,
            "struct"   => TokenType::Struct,
            "return"   => TokenType::Return,
            "in"       => TokenType::In,
            "break"    => TokenType::Break,
            "continue" => TokenType::Continue,
            "yield"    => TokenType::Yield,
            "gen"      => TokenType::Gen,
            "true"     => TokenType::True,
            "false"    => TokenType::False,
            "mut"      => TokenType::Mut,
            "i32"      => TokenType::TyI32,
            "i64"      => TokenType::TyI64,
            "f32"      => TokenType::TyF32,
            "f64"      => TokenType::TyF64,
            "str"      => TokenType::TyStr,
            "bool"     => TokenType::TyBool,
            "void"     => TokenType::TyVoid,
            "u32"      => TokenType::TyU32,
            _          => TokenType::Identifier,
        };
        Token::new(ty, s, line, col)
    }

    pub fn tokenize(&mut self) -> Result<Vec<Token>, LexError> {
        let mut tokens = Vec::new();
        loop {
            self.skip_whitespace();
            let line = self.line;
            let col = self.col;

            let ch = match self.advance() {
                None => { tokens.push(Token::new(TokenType::Eof, "", line, col)); break; }
                Some(c) => c,
            };

            let tok = match ch {
                // Comments
                '/' if self.peek() == Some('/') => { self.advance(); self.skip_line_comment(); continue; }
                '/' if self.peek() == Some('*') => { self.advance(); self.skip_block_comment(line, col)?; continue; }
                '/' => Token::new(TokenType::Slash, "/", line, col),

                // f-string (f followed immediately by ")
                'f' if self.peek() == Some('"') => self.read_fstring(line, col)?,

                '"' => self.read_string(line, col)?,

                c if c.is_ascii_digit() => self.read_number(c, line, col),
                c if c.is_alphabetic() || c == '_' => self.read_ident_or_keyword(c, line, col),

                // Operators
                '+' => Token::new(TokenType::Plus,    "+", line, col),
                '-' if self.peek() == Some('>') => { self.advance(); Token::new(TokenType::Arrow, "->", line, col) }
                '-' => Token::new(TokenType::Minus,   "-", line, col),
                '*' => Token::new(TokenType::Star,    "*", line, col),
                '%' => Token::new(TokenType::Percent, "%", line, col),

                '=' if self.peek() == Some('=') => { self.advance(); Token::new(TokenType::EqEq,   "==", line, col) }
                '=' => Token::new(TokenType::Assign,  "=", line, col),
                '!' if self.peek() == Some('=') => { self.advance(); Token::new(TokenType::BangEq, "!=", line, col) }
                '!' => Token::new(TokenType::Bang,    "!",  line, col),

                '<' if self.peek() == Some('=') => { self.advance(); Token::new(TokenType::LtEq, "<=", line, col) }
                '<' if self.peek() == Some('<') => { self.advance(); Token::new(TokenType::LtLt, "<<", line, col) }
                '<' => Token::new(TokenType::Lt, "<", line, col),

                '>' if self.peek() == Some('=') => { self.advance(); Token::new(TokenType::GtEq, ">=", line, col) }
                '>' if self.peek() == Some('>') => { self.advance(); Token::new(TokenType::GtGt, ">>", line, col) }
                '>' => Token::new(TokenType::Gt, ">", line, col),

                '&' if self.peek() == Some('&') => { self.advance(); Token::new(TokenType::AndAnd,    "&&", line, col) }
                '&' => Token::new(TokenType::Ampersand, "&", line, col),
                '|' if self.peek() == Some('|') => { self.advance(); Token::new(TokenType::OrOr,      "||", line, col) }
                '|' => Token::new(TokenType::Pipe,  "|", line, col),
                '^' => Token::new(TokenType::Caret, "^", line, col),

                '.' if self.peek() == Some('.') => {
                    self.advance();
                    if self.peek() == Some('=') { self.advance(); Token::new(TokenType::DotDotEq, "..=", line, col) }
                    else { Token::new(TokenType::DotDot, "..", line, col) }
                }
                '.' => Token::new(TokenType::Dot, ".", line, col),

                // Delimiters
                '(' => Token::new(TokenType::LParen,    "(", line, col),
                ')' => Token::new(TokenType::RParen,    ")", line, col),
                '{' => Token::new(TokenType::LBrace,    "{", line, col),
                '}' => Token::new(TokenType::RBrace,    "}", line, col),
                '[' => Token::new(TokenType::LBracket,  "[", line, col),
                ']' => Token::new(TokenType::RBracket,  "]", line, col),
                ',' => Token::new(TokenType::Comma,     ",", line, col),
                ';' => Token::new(TokenType::Semicolon, ";", line, col),
                ':' => Token::new(TokenType::Colon,     ":", line, col),
                '\'' => Token::new(TokenType::Tick,     "'", line, col),

                c => return Err(LexError { message: format!("Unexpected character: '{}'", c), line, col }),
            };

            tokens.push(tok);
        }
        Ok(tokens)
    }
}
