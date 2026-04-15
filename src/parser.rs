use crate::token::{Token, TokenType};
use crate::ast::*;

pub struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

#[derive(Debug)]
pub struct ParseError {
    pub message: String,
    pub line: usize,
    pub col: usize,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Parse error at line {}, col {}: {}", self.line, self.col, self.message)
    }
}

impl Parser {
    pub fn new(tokens: Vec<Token>) -> Self {
        Parser { tokens, pos: 0 }
    }

    fn peek(&self) -> &Token {
        &self.tokens[self.pos]
    }

    fn advance(&mut self) -> Token {
        let tok = self.tokens[self.pos].clone();
        if self.pos + 1 < self.tokens.len() {
            self.pos += 1;
        }
        tok
    }

    fn check(&self, ty: &TokenType) -> bool {
        &self.peek().ty == ty
    }

    fn consume(&mut self, ty: &TokenType) -> Result<Token, ParseError> {
        if self.check(ty) {
            Ok(self.advance())
        } else {
            let tok = self.peek();
            Err(ParseError {
                message: format!("Expected {:?}, found {:?} ('{}')", ty, tok.ty, tok.value),
                line: tok.line,
                col: tok.col,
            })
        }
    }

    fn at_end(&self) -> bool {
        self.peek().ty == TokenType::Eof
    }

    // ──────────────────────────────────────────────────────────────
    // Top-level
    // ──────────────────────────────────────────────────────────────

    pub fn parse(&mut self) -> Result<Program, ParseError> {
        let mut decls = Vec::new();
        while !self.at_end() {
            decls.push(self.parse_decl()?);
        }
        Ok(Program { decls })
    }

    fn parse_decl(&mut self) -> Result<Decl, ParseError> {
        // Check for 'gen' prefix
        let is_gen = if self.check(&TokenType::Gen) {
            self.advance();
            true
        } else {
            false
        };

        match self.peek().ty {
            TokenType::Fn     => Ok(Decl::Fn(self.parse_fn_decl(is_gen)?)),
            TokenType::Struct => {
                if is_gen {
                    let tok = self.peek();
                    return Err(ParseError {
                        message: "Cannot use 'gen' with struct declaration".into(),
                        line: tok.line,
                        col: tok.col,
                    });
                }
                Ok(Decl::Struct(self.parse_struct_decl()?))
            }
            _ => {
                let tok = self.peek();
                Err(ParseError {
                    message: format!("Expected 'fn' or 'struct', found '{}'", tok.value),
                    line: tok.line,
                    col: tok.col,
                })
            }
        }
    }

    fn parse_fn_decl(&mut self, is_gen: bool) -> Result<FnDecl, ParseError> {
        self.consume(&TokenType::Fn)?;
        let name = self.consume(&TokenType::Identifier)?.value;

        self.consume(&TokenType::LParen)?;
        let mut params = Vec::new();
        while !self.check(&TokenType::RParen) && !self.at_end() {
            let pname = self.consume(&TokenType::Identifier)?.value;
            self.consume(&TokenType::Colon)?;
            let pty = self.parse_type()?;
            params.push(Param { name: pname, ty: pty });
            if !self.check(&TokenType::RParen) {
                self.consume(&TokenType::Comma)?;
            }
        }
        self.consume(&TokenType::RParen)?;

        let ret_ty = if self.check(&TokenType::Arrow) {
            self.advance();
            self.parse_type()?
        } else {
            Type::Void
        };

        let body = self.parse_block()?;
        Ok(FnDecl { name, params, ret_ty, body, is_gen })
    }

    fn parse_struct_decl(&mut self) -> Result<StructDecl, ParseError> {
        self.consume(&TokenType::Struct)?;
        let name = self.consume(&TokenType::Identifier)?.value;
        self.consume(&TokenType::LBrace)?;

        let mut fields = Vec::new();
        while !self.check(&TokenType::RBrace) && !self.at_end() {
            let fname = self.consume(&TokenType::Identifier)?.value;
            self.consume(&TokenType::Colon)?;
            let fty = self.parse_type()?;
            fields.push((fname, fty));
            if !self.check(&TokenType::RBrace) {
                self.consume(&TokenType::Comma)?;
            }
        }
        self.consume(&TokenType::RBrace)?;
        Ok(StructDecl { name, fields })
    }

    fn parse_type(&mut self) -> Result<Type, ParseError> {
        match self.peek().ty.clone() {
            TokenType::TyI32  => { self.advance(); Ok(Type::I32) }
            TokenType::TyI64  => { self.advance(); Ok(Type::I64) }
            TokenType::TyF32  => { self.advance(); Ok(Type::F32) }
            TokenType::TyF64  => { self.advance(); Ok(Type::F64) }
            TokenType::TyStr  => { self.advance(); Ok(Type::Str) }
            TokenType::TyBool => { self.advance(); Ok(Type::Bool) }
            TokenType::TyVoid => { self.advance(); Ok(Type::Void) }
            TokenType::TyU32  => { self.advance(); Ok(Type::U32) }
            TokenType::LBracket => {
                self.advance();
                let elem = self.parse_type()?;
                self.consume(&TokenType::RBracket)?;
                Ok(Type::Array(Box::new(elem)))
            }
            TokenType::Identifier => {
                let name = self.advance().value;
                Ok(Type::Custom(name))
            }
            _ => {
                let tok = self.peek();
                Err(ParseError {
                    message: format!("Expected type, found '{}'", tok.value),
                    line: tok.line,
                    col: tok.col,
                })
            }
        }
    }

    fn parse_block(&mut self) -> Result<Vec<Stmt>, ParseError> {
        self.consume(&TokenType::LBrace)?;
        let mut stmts = Vec::new();
        while !self.check(&TokenType::RBrace) && !self.at_end() {
            stmts.push(self.parse_stmt()?);
        }
        self.consume(&TokenType::RBrace)?;
        Ok(stmts)
    }

    // ──────────────────────────────────────────────────────────────
    // Statements
    // ──────────────────────────────────────────────────────────────

    fn parse_stmt(&mut self) -> Result<Stmt, ParseError> {
        // Check for loop label: 'label: while/for
        let label = if self.check(&TokenType::Tick) {
            self.advance();  // consume '
            let name = self.consume(&TokenType::Identifier)?.value;
            self.consume(&TokenType::Colon)?;
            Some(name)
        } else {
            None
        };

        match self.peek().ty {
            TokenType::Let      => self.parse_var_decl(),
            TokenType::If       => self.parse_if(),
            TokenType::For      => self.parse_for(label),
            TokenType::While    => self.parse_while(label),
            TokenType::Return   => self.parse_return(),
            TokenType::Break    => {
                self.advance();
                let target = if self.check(&TokenType::Tick) {
                    self.advance();
                    let lbl = self.consume(&TokenType::Identifier)?.value;
                    Some(lbl)
                } else {
                    None
                };
                self.consume(&TokenType::Semicolon)?;
                Ok(Stmt::Break(target))
            }
            TokenType::Continue => {
                self.advance();
                let target = if self.check(&TokenType::Tick) {
                    self.advance();
                    let lbl = self.consume(&TokenType::Identifier)?.value;
                    Some(lbl)
                } else {
                    None
                };
                self.consume(&TokenType::Semicolon)?;
                Ok(Stmt::Continue(target))
            }
            TokenType::Yield    => {
                self.advance();
                let val = self.parse_expr()?;
                self.consume(&TokenType::Semicolon)?;
                Ok(Stmt::Yield(val))
            }
            _                   => {
                if label.is_some() {
                    let tok = self.peek();
                    return Err(ParseError {
                        message: "Loop label can only be used with 'while' or 'for'".into(),
                        line: tok.line,
                        col: tok.col,
                    });
                }
                self.parse_expr_or_assign()
            }
        }
    }

    fn parse_var_decl(&mut self) -> Result<Stmt, ParseError> {
        self.consume(&TokenType::Let)?;

        let mutable = if self.check(&TokenType::Mut) { self.advance(); true } else { false };

        let name = self.consume(&TokenType::Identifier)?.value;

        let ty = if self.check(&TokenType::Colon) {
            self.advance();
            Some(self.parse_type()?)
        } else {
            None
        };

        self.consume(&TokenType::Assign)?;
        let value = self.parse_expr()?;
        self.consume(&TokenType::Semicolon)?;

        Ok(Stmt::VarDecl(VarDecl { name, ty, value, mutable }))
    }

    fn parse_if(&mut self) -> Result<Stmt, ParseError> {
        self.consume(&TokenType::If)?;
        let cond = self.parse_expr()?;
        let then_block = self.parse_block()?;

        let else_block = if self.check(&TokenType::Else) {
            self.advance();
            if self.check(&TokenType::If) {
                Some(vec![self.parse_if()?])
            } else {
                Some(self.parse_block()?)
            }
        } else {
            None
        };

        Ok(Stmt::If { cond, then_block, else_block })
    }

    fn parse_for(&mut self, label: Option<String>) -> Result<Stmt, ParseError> {
        self.consume(&TokenType::For)?;
        let var = self.consume(&TokenType::Identifier)?.value;
        self.consume(&TokenType::In)?;

        // Parse start expression (stops before .. / ..=)
        let start = self.parse_additive()?;

        let iter = if self.check(&TokenType::DotDot) {
            self.advance();
            let end = self.parse_additive()?;
            ForIter::Range { start, end, inclusive: false }
        } else if self.check(&TokenType::DotDotEq) {
            self.advance();
            let end = self.parse_additive()?;
            ForIter::Range { start, end, inclusive: true }
        } else {
            ForIter::Expr(start)
        };

        let body = self.parse_block()?;
        Ok(Stmt::For { label, var, iter, body })
    }

    fn parse_while(&mut self, label: Option<String>) -> Result<Stmt, ParseError> {
        self.consume(&TokenType::While)?;
        let cond = self.parse_expr()?;
        let body = self.parse_block()?;
        Ok(Stmt::While { label, cond, body })
    }

    fn parse_return(&mut self) -> Result<Stmt, ParseError> {
        self.consume(&TokenType::Return)?;
        if self.check(&TokenType::Semicolon) {
            self.advance();
            return Ok(Stmt::Return(None));
        }
        let expr = self.parse_expr()?;
        self.consume(&TokenType::Semicolon)?;
        Ok(Stmt::Return(Some(expr)))
    }

    fn parse_expr_or_assign(&mut self) -> Result<Stmt, ParseError> {
        let lhs = self.parse_expr()?;
        if self.check(&TokenType::Assign) {
            self.advance();
            let rhs = self.parse_expr()?;
            self.consume(&TokenType::Semicolon)?;
            Ok(Stmt::Assign { target: lhs, value: rhs })
        } else {
            self.consume(&TokenType::Semicolon)?;
            Ok(Stmt::Expr(lhs))
        }
    }

    // ──────────────────────────────────────────────────────────────
    // Expressions (recursive descent, lowest → highest precedence)
    // ──────────────────────────────────────────────────────────────

    fn parse_expr(&mut self) -> Result<Expr, ParseError> {
        self.parse_or()
    }

    fn parse_or(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.parse_and()?;
        while self.check(&TokenType::OrOr) {
            self.advance();
            let right = self.parse_and()?;
            left = Expr::Binary { left: Box::new(left), op: BinaryOp::Or, right: Box::new(right) };
        }
        Ok(left)
    }

    fn parse_and(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.parse_bitwise_or()?;
        while self.check(&TokenType::AndAnd) {
            self.advance();
            let right = self.parse_bitwise_or()?;
            left = Expr::Binary { left: Box::new(left), op: BinaryOp::And, right: Box::new(right) };
        }
        Ok(left)
    }

    fn parse_bitwise_or(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.parse_bitwise_xor()?;
        while self.check(&TokenType::Pipe) {
            self.advance();
            let right = self.parse_bitwise_xor()?;
            left = Expr::Binary { left: Box::new(left), op: BinaryOp::BitOr, right: Box::new(right) };
        }
        Ok(left)
    }

    fn parse_bitwise_xor(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.parse_bitwise_and()?;
        while self.check(&TokenType::Caret) {
            self.advance();
            let right = self.parse_bitwise_and()?;
            left = Expr::Binary { left: Box::new(left), op: BinaryOp::BitXor, right: Box::new(right) };
        }
        Ok(left)
    }

    fn parse_bitwise_and(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.parse_equality()?;
        while self.check(&TokenType::Ampersand) {
            self.advance();
            let right = self.parse_equality()?;
            left = Expr::Binary { left: Box::new(left), op: BinaryOp::BitAnd, right: Box::new(right) };
        }
        Ok(left)
    }

    fn parse_equality(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.parse_comparison()?;
        loop {
            let op = match self.peek().ty {
                TokenType::EqEq  => BinaryOp::Eq,
                TokenType::BangEq => BinaryOp::NotEq,
                _ => break,
            };
            self.advance();
            let right = self.parse_comparison()?;
            left = Expr::Binary { left: Box::new(left), op, right: Box::new(right) };
        }
        Ok(left)
    }

    fn parse_comparison(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.parse_shift()?;
        loop {
            let op = match self.peek().ty {
                TokenType::Lt   => BinaryOp::Lt,
                TokenType::LtEq => BinaryOp::Le,
                TokenType::Gt   => BinaryOp::Gt,
                TokenType::GtEq => BinaryOp::Ge,
                _ => break,
            };
            self.advance();
            let right = self.parse_shift()?;
            left = Expr::Binary { left: Box::new(left), op, right: Box::new(right) };
        }
        Ok(left)
    }

    fn parse_shift(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.parse_additive()?;
        loop {
            let op = match self.peek().ty {
                TokenType::LtLt => BinaryOp::Shl,
                TokenType::GtGt => BinaryOp::Shr,
                _ => break,
            };
            self.advance();
            let right = self.parse_additive()?;
            left = Expr::Binary { left: Box::new(left), op, right: Box::new(right) };
        }
        Ok(left)
    }

    fn parse_additive(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.parse_multiplicative()?;
        loop {
            let op = match self.peek().ty {
                TokenType::Plus  => BinaryOp::Add,
                TokenType::Minus => BinaryOp::Sub,
                _ => break,
            };
            self.advance();
            let right = self.parse_multiplicative()?;
            left = Expr::Binary { left: Box::new(left), op, right: Box::new(right) };
        }
        Ok(left)
    }

    fn parse_multiplicative(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.parse_unary()?;
        loop {
            let op = match self.peek().ty {
                TokenType::Star    => BinaryOp::Mul,
                TokenType::Slash   => BinaryOp::Div,
                TokenType::Percent => BinaryOp::Mod,
                _ => break,
            };
            self.advance();
            let right = self.parse_unary()?;
            left = Expr::Binary { left: Box::new(left), op, right: Box::new(right) };
        }
        Ok(left)
    }

    fn parse_unary(&mut self) -> Result<Expr, ParseError> {
        match self.peek().ty {
            TokenType::Minus => {
                self.advance();
                let e = self.parse_unary()?;
                Ok(Expr::Unary { op: UnaryOp::Neg, expr: Box::new(e) })
            }
            TokenType::Bang => {
                self.advance();
                let e = self.parse_unary()?;
                Ok(Expr::Unary { op: UnaryOp::Not, expr: Box::new(e) })
            }
            _ => self.parse_postfix(),
        }
    }

    fn parse_postfix(&mut self) -> Result<Expr, ParseError> {
        let mut expr = self.parse_primary()?;
        loop {
            if self.check(&TokenType::Dot) {
                self.advance();
                let field = self.consume(&TokenType::Identifier)?.value;
                expr = Expr::Field { object: Box::new(expr), field };
            } else if self.check(&TokenType::LBracket) {
                self.advance();
                let idx = self.parse_expr()?;
                self.consume(&TokenType::RBracket)?;
                expr = Expr::Index { array: Box::new(expr), index: Box::new(idx) };
            } else {
                break;
            }
        }
        Ok(expr)
    }

    fn parse_primary(&mut self) -> Result<Expr, ParseError> {
        match self.peek().ty.clone() {
            TokenType::Integer => {
                let v = self.advance().value.parse::<i64>().unwrap_or(0);
                Ok(Expr::Int(v))
            }
            TokenType::Float => {
                let v = self.advance().value.parse::<f64>().unwrap_or(0.0);
                Ok(Expr::Float(v))
            }
            TokenType::StringLit => {
                let v = self.advance().value;
                Ok(Expr::Str(v))
            }
            TokenType::FString => {
                let v = self.advance().value;
                Ok(Expr::FStr(v))
            }
            TokenType::True  => { self.advance(); Ok(Expr::Bool(true))  }
            TokenType::False => { self.advance(); Ok(Expr::Bool(false)) }

            TokenType::Identifier => {
                let name = self.advance().value;
                if self.check(&TokenType::LParen) {
                    // Function call
                    self.advance();
                    let mut args = Vec::new();
                    while !self.check(&TokenType::RParen) && !self.at_end() {
                        args.push(self.parse_expr()?);
                        if !self.check(&TokenType::RParen) {
                            self.consume(&TokenType::Comma)?;
                        }
                    }
                    self.consume(&TokenType::RParen)?;
                    Ok(Expr::Call { name, args })
                } else if self.check(&TokenType::LBrace) {
                    // Struct literal: Name { field: val, ... }
                    self.advance();
                    let mut fields = Vec::new();
                    while !self.check(&TokenType::RBrace) && !self.at_end() {
                        let fname = self.consume(&TokenType::Identifier)?.value;
                        self.consume(&TokenType::Colon)?;
                        let fval = self.parse_expr()?;
                        fields.push((fname, fval));
                        if !self.check(&TokenType::RBrace) {
                            self.consume(&TokenType::Comma)?;
                        }
                    }
                    self.consume(&TokenType::RBrace)?;
                    Ok(Expr::StructLit { name, fields })
                } else {
                    Ok(Expr::Ident(name))
                }
            }

            TokenType::LParen => {
                self.advance();
                let e = self.parse_expr()?;
                self.consume(&TokenType::RParen)?;
                Ok(e)
            }

            TokenType::LBracket => {
                self.advance();
                let mut elems = Vec::new();
                while !self.check(&TokenType::RBracket) && !self.at_end() {
                    elems.push(self.parse_expr()?);
                    if !self.check(&TokenType::RBracket) {
                        self.consume(&TokenType::Comma)?;
                    }
                }
                self.consume(&TokenType::RBracket)?;
                Ok(Expr::Array(elems))
            }

            _ => {
                let tok = self.peek();
                Err(ParseError {
                    message: format!("Unexpected token '{}' in expression", tok.value),
                    line: tok.line,
                    col: tok.col,
                })
            }
        }
    }
}
