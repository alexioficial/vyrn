#[derive(Debug, Clone, PartialEq)]
pub enum TokenType {
    // Literals
    Integer,
    Float,
    StringLit,
    FString,
    Identifier,

    // Keywords
    Let,
    Fn,
    If,
    Else,
    For,
    While,
    Struct,
    Return,
    In,
    Break,
    Continue,
    Yield,
    Gen,
    True,
    False,
    Mut,

    // Type keywords
    TyI32,
    TyI64,
    TyF32,
    TyF64,
    TyStr,
    TyBool,
    TyVoid,
    TyU32,

    // Arithmetic
    Plus,
    Minus,
    Star,
    Slash,
    Percent,

    // Comparison
    EqEq,
    BangEq,
    Lt,
    LtEq,
    Gt,
    GtEq,

    // Logical
    AndAnd,
    OrOr,
    Bang,

    // Assignment
    Assign,

    // Bitwise
    Ampersand,
    Pipe,
    Caret,
    LtLt,
    GtGt,

    // Delimiters
    LParen,
    RParen,
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    Comma,
    Semicolon,
    Colon,
    Arrow,
    DotDot,
    DotDotEq,
    Dot,
    Tick,  // ' (apostrophe for loop labels)

    Eof,
}

#[derive(Debug, Clone)]
pub struct Token {
    pub ty: TokenType,
    pub value: String,
    pub line: usize,
    pub col: usize,
}

impl Token {
    pub fn new(ty: TokenType, value: impl Into<String>, line: usize, col: usize) -> Self {
        Token { ty, value: value.into(), line, col }
    }
}
