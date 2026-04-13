#[derive(Debug, Clone, PartialEq)]
pub enum Type {
    I32,
    I64,
    F32,
    F64,
    Str,
    Bool,
    Void,
    U32,
    Array(Box<Type>),
    Custom(String),
}

impl std::fmt::Display for Type {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Type::I32 => write!(f, "i32"),
            Type::I64 => write!(f, "i64"),
            Type::F32 => write!(f, "f32"),
            Type::F64 => write!(f, "f64"),
            Type::Str => write!(f, "str"),
            Type::Bool => write!(f, "bool"),
            Type::Void => write!(f, "void"),
            Type::U32 => write!(f, "u32"),
            Type::Array(t) => write!(f, "[{}]", t),
            Type::Custom(name) => write!(f, "{}", name),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum BinaryOp {
    Add, Sub, Mul, Div, Mod,
    Eq, NotEq,
    Lt, Le, Gt, Ge,
    And, Or,
    BitAnd, BitOr, BitXor,
    Shl, Shr,
}

#[derive(Debug, Clone, PartialEq)]
pub enum UnaryOp {
    Neg,
    Not,
}

#[derive(Debug, Clone)]
pub enum Expr {
    Int(i64),
    Float(f64),
    Str(String),
    FStr(String),
    Bool(bool),
    Ident(String),
    Binary { left: Box<Expr>, op: BinaryOp, right: Box<Expr> },
    Unary  { op: UnaryOp, expr: Box<Expr> },
    Call   { name: String, args: Vec<Expr> },
    Index  { array: Box<Expr>, index: Box<Expr> },
    Field  { object: Box<Expr>, field: String },
    Array  (Vec<Expr>),
    StructLit { name: String, fields: Vec<(String, Expr)> },
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct VarDecl {
    pub name: String,
    pub ty: Option<Type>,
    pub value: Expr,
    pub mutable: bool,  // tracked but not enforced in codegen (C++ vars are mutable by default)
}

#[derive(Debug, Clone)]
pub enum ForIter {
    Range { start: Expr, end: Expr, inclusive: bool },
    Expr(Expr),
}

#[derive(Debug, Clone)]
pub enum Stmt {
    VarDecl(VarDecl),
    Assign { target: Expr, value: Expr },
    If {
        cond: Expr,
        then_block: Vec<Stmt>,
        else_block: Option<Vec<Stmt>>,
    },
    For {
        var: String,
        iter: ForIter,
        body: Vec<Stmt>,
    },
    While { cond: Expr, body: Vec<Stmt> },
    Return(Option<Expr>),
    Break,
    Continue,
    Expr(Expr),
}

#[derive(Debug, Clone)]
pub struct Param {
    pub name: String,
    pub ty: Type,
}

#[derive(Debug, Clone)]
pub struct FnDecl {
    pub name: String,
    pub params: Vec<Param>,
    pub ret_ty: Type,
    pub body: Vec<Stmt>,
}

#[derive(Debug, Clone)]
pub struct StructDecl {
    pub name: String,
    pub fields: Vec<(String, Type)>,
}

#[derive(Debug, Clone)]
pub enum Decl {
    Fn(FnDecl),
    Struct(StructDecl),
}

#[derive(Debug, Clone)]
pub struct Program {
    pub decls: Vec<Decl>,
}
