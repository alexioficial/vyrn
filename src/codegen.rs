use crate::ast::*;

pub struct CodeGen {
    output: String,
    indent: usize,
    tmp_counter: usize,
}

// ── Pure helpers (no &self needed) ──────────────────────────────────────────

pub fn type_to_cpp(ty: &Type) -> String {
    match ty {
        Type::I32 => "int".into(),
        Type::I64 => "long long".into(),
        Type::F32 => "float".into(),
        Type::F64 => "double".into(),
        Type::Str => "std::string".into(),
        Type::Bool => "bool".into(),
        Type::Void => "void".into(),
        Type::U32 => "unsigned int".into(),
        Type::Array(t) => format!("std::vector<{}>", type_to_cpp(t)),
        Type::Custom(name) => name.clone(),
    }
}

fn escape_str(s: &str) -> String {
    let mut r = String::new();
    for c in s.chars() {
        match c {
            '\n' => r.push_str("\\n"),
            '\t' => r.push_str("\\t"),
            '\r' => r.push_str("\\r"),
            '\\' => r.push_str("\\\\"),
            '"'  => r.push_str("\\\""),
            c    => r.push(c),
        }
    }
    r
}

// ── CodeGen impl ─────────────────────────────────────────────────────────────

impl CodeGen {
    pub fn new() -> Self {
        CodeGen { output: String::new(), indent: 0, tmp_counter: 0 }
    }

    fn line(&mut self, s: &str) {
        for _ in 0..self.indent {
            self.output.push_str("    ");
        }
        self.output.push_str(s);
        self.output.push('\n');
    }

    fn fresh_tmp(&mut self) -> String {
        let n = self.tmp_counter;
        self.tmp_counter += 1;
        format!("__tmp{}", n)
    }

    // ── Expression emission ─────────────────────────────────────────────────

    pub fn emit_expr(&mut self, expr: &Expr) -> String {
        match expr {
            Expr::Int(n)   => n.to_string(),
            Expr::Float(f) => {
                let s = format!("{}", f);
                if s.contains('.') || s.contains('e') { s } else { format!("{}.0", s) }
            }
            Expr::Str(s) => format!("std::string(\"{}\")", escape_str(s)),
            Expr::FStr(s) => self.emit_fstring(s),
            Expr::Bool(b) => if *b { "true".into() } else { "false".into() },
            Expr::Ident(name) => name.clone(),

            Expr::Binary { left, op, right } => {
                let l = self.emit_expr(left);
                let r = self.emit_expr(right);
                let op_s = match op {
                    BinaryOp::Add    => "+",
                    BinaryOp::Sub    => "-",
                    BinaryOp::Mul    => "*",
                    BinaryOp::Div    => "/",
                    BinaryOp::Mod    => "%",
                    BinaryOp::Eq     => "==",
                    BinaryOp::NotEq  => "!=",
                    BinaryOp::Lt     => "<",
                    BinaryOp::Le     => "<=",
                    BinaryOp::Gt     => ">",
                    BinaryOp::Ge     => ">=",
                    BinaryOp::And    => "&&",
                    BinaryOp::Or     => "||",
                    BinaryOp::BitAnd => "&",
                    BinaryOp::BitOr  => "|",
                    BinaryOp::BitXor => "^",
                    BinaryOp::Shl    => "<<",
                    BinaryOp::Shr    => ">>",
                };
                format!("({} {} {})", l, op_s, r)
            }

            Expr::Unary { op, expr } => {
                let e = self.emit_expr(expr);
                match op {
                    UnaryOp::Neg => format!("(-{})", e),
                    UnaryOp::Not => format!("(!{})", e),
                }
            }

            Expr::Call { name, args } => self.emit_call(name, args),

            Expr::Field { object, field } => {
                let obj = self.emit_expr(object);
                format!("{}.{}", obj, field)
            }

            Expr::Index { array, index } => {
                let arr = self.emit_expr(array);
                let idx = self.emit_expr(index);
                format!("{}[{}]", arr, idx)
            }

            Expr::Array(elems) => self.emit_array_lit(elems),

            Expr::StructLit { name, fields } => self.emit_struct_lit(name, fields),
        }
    }

    fn emit_call(&mut self, name: &str, args: &[Expr]) -> String {
        match name {
            "println" => {
                if args.is_empty() {
                    "(std::cout << std::endl)".into()
                } else {
                    let mut parts = Vec::new();
                    for a in args { parts.push(self.emit_expr(a)); }
                    format!("(std::cout << {} << std::endl)", parts.join(" << \" \" << "))
                }
            }
            "print" => {
                if args.is_empty() {
                    "((void)0)".into()
                } else {
                    let mut parts = Vec::new();
                    for a in args { parts.push(self.emit_expr(a)); }
                    format!("(std::cout << {})", parts.join(" << \" \" << "))
                }
            }
            "len" if args.len() == 1 => {
                let a = self.emit_expr(&args[0]);
                format!("((int){}.size())", a)
            }
            "abs" if args.len() == 1 => {
                let a = self.emit_expr(&args[0]);
                format!("std::abs({})", a)
            }
            "to_string" if args.len() == 1 => {
                let a = self.emit_expr(&args[0]);
                format!("std::to_string({})", a)
            }
            "sqrt" if args.len() == 1 => {
                let a = self.emit_expr(&args[0]);
                format!("std::sqrt({})", a)
            }
            "pow" if args.len() == 2 => {
                let a = self.emit_expr(&args[0]);
                let b = self.emit_expr(&args[1]);
                format!("std::pow({}, {})", a, b)
            }
            _ => {
                let mut arg_strs = Vec::new();
                for a in args { arg_strs.push(self.emit_expr(a)); }
                format!("{}({})", name, arg_strs.join(", "))
            }
        }
    }

    fn emit_array_lit(&mut self, elems: &[Expr]) -> String {
        if elems.is_empty() {
            return "std::vector<int>{}".into();
        }
        // Infer element type from first element
        let elem_ty = match &elems[0] {
            Expr::Int(_)  => "int",
            Expr::Float(_) => "double",
            Expr::Str(_) | Expr::FStr(_) => "std::string",
            Expr::Bool(_) => "bool",
            _ => "auto",
        };

        let mut parts = Vec::new();
        for e in elems { parts.push(self.emit_expr(e)); }

        if elem_ty == "auto" {
            // Use C++17 CTAD with explicit first element type
            format!("std::vector<decltype({})>{{{}}}", parts[0], parts.join(", "))
        } else {
            format!("std::vector<{}>{{{}}}", elem_ty, parts.join(", "))
        }
    }

    fn emit_struct_lit(&mut self, name: &str, fields: &[(String, Expr)]) -> String {
        let tmp = self.fresh_tmp();
        let mut body = format!("{} {}; ", name, tmp);
        for (fname, fval) in fields {
            let v = self.emit_expr(fval);
            body.push_str(&format!("{}.{} = {}; ", tmp, fname, v));
        }
        body.push_str(&format!("return {};", tmp));
        format!("([&]() -> {} {{ {} }}())", name, body)
    }

    /// Parse and emit an f-string like "Hello, {name}! Value: {x + 1}"
    fn emit_fstring(&mut self, content: &str) -> String {
        let chars: Vec<char> = content.chars().collect();
        let mut pieces: Vec<String> = Vec::new();
        let mut literal = String::new();
        let mut i = 0;

        while i < chars.len() {
            if chars[i] == '{' {
                if !literal.is_empty() {
                    pieces.push(format!("\"{}\"", escape_str(&literal)));
                    literal.clear();
                }
                i += 1;
                let mut expr_src = String::new();
                while i < chars.len() && chars[i] != '}' {
                    expr_src.push(chars[i]);
                    i += 1;
                }
                i += 1; // skip '}'
                pieces.push(expr_src.trim().to_string());
            } else {
                literal.push(chars[i]);
                i += 1;
            }
        }
        if !literal.is_empty() {
            pieces.push(format!("\"{}\"", escape_str(&literal)));
        }

        if pieces.is_empty() {
            return "std::string(\"\")".into();
        }

        let stream = pieces.join(" << ");
        format!("([&](){{ std::ostringstream __oss; __oss << {}; return __oss.str(); }}())", stream)
    }

    // ── Statement emission ──────────────────────────────────────────────────

    pub fn emit_stmt(&mut self, stmt: &Stmt) {
        match stmt {
            Stmt::VarDecl(vd) => self.emit_var_decl(vd),
            Stmt::Assign { target, value } => {
                let t = self.emit_expr(target);
                let v = self.emit_expr(value);
                self.line(&format!("{} = {};", t, v));
            }
            Stmt::If { cond, then_block, else_block } => {
                self.emit_if(cond, then_block, else_block);
            }
            Stmt::For { var, iter, body } => {
                self.emit_for(var, iter, body);
            }
            Stmt::While { cond, body } => {
                let c = self.emit_expr(cond);
                self.line(&format!("while ({}) {{", c));
                self.indent += 1;
                for s in body { self.emit_stmt(s); }
                self.indent -= 1;
                self.line("}");
            }
            Stmt::Return(expr) => {
                match expr {
                    Some(e) => { let v = self.emit_expr(e); self.line(&format!("return {};", v)); }
                    None    => self.line("return;"),
                }
            }
            Stmt::Break    => self.line("break;"),
            Stmt::Continue => self.line("continue;"),
            Stmt::Expr(e)  => {
                let v = self.emit_expr(e);
                self.line(&format!("{};", v));
            }
        }
    }

    fn emit_var_decl(&mut self, vd: &VarDecl) {
        let val = self.emit_expr(&vd.value);

        let decl = if let Some(ty) = &vd.ty {
            let cpp_ty = type_to_cpp(ty);
            // For array types with an array literal, use brace initializer
            if matches!(ty, Type::Array(_)) {
                if let Expr::Array(elems) = &vd.value {
                    let mut parts = Vec::new();
                    for e in elems { parts.push(self.emit_expr(e)); }
                    format!("{} {} = {{{}}}", cpp_ty, vd.name, parts.join(", "))
                } else {
                    format!("{} {} = {}", cpp_ty, vd.name, val)
                }
            } else {
                format!("{} {} = {}", cpp_ty, vd.name, val)
            }
        } else {
            format!("auto {} = {}", vd.name, val)
        };
        self.line(&format!("{};", decl));
    }

    fn emit_if(&mut self, cond: &Expr, then_block: &[Stmt], else_block: &Option<Vec<Stmt>>) {
        let c = self.emit_expr(cond);
        self.line(&format!("if ({}) {{", c));
        self.indent += 1;
        for s in then_block { self.emit_stmt(s); }
        self.indent -= 1;

        match else_block {
            None => { self.line("}"); }
            Some(stmts) => {
                // Detect else-if to emit cleaner code
                if stmts.len() == 1 {
                    if let Stmt::If { cond: ec, then_block: et, else_block: ee } = &stmts[0] {
                        // Trim trailing newline so we can append "} else if ..."
                        while self.output.ends_with('\n') { self.output.pop(); }
                        self.output.push('\n');
                        let ec_str = self.emit_expr(ec);
                        self.line(&format!("}} else if ({}) {{", ec_str));
                        self.indent += 1;
                        for s in et { self.emit_stmt(s); }
                        self.indent -= 1;
                        // Recurse for chained else
                        match ee {
                            None => self.line("}"),
                            Some(ee_stmts) => {
                                // Simplify: just put stmts in an else block
                                while self.output.ends_with('\n') { self.output.pop(); }
                                self.output.push('\n');
                                self.line("} else {");
                                self.indent += 1;
                                for s in ee_stmts { self.emit_stmt(s); }
                                self.indent -= 1;
                                self.line("}");
                            }
                        }
                        return;
                    }
                }
                while self.output.ends_with('\n') { self.output.pop(); }
                self.output.push('\n');
                self.line("} else {");
                self.indent += 1;
                for s in stmts { self.emit_stmt(s); }
                self.indent -= 1;
                self.line("}");
            }
        }
    }

    fn emit_for(&mut self, var: &str, iter: &ForIter, body: &[Stmt]) {
        match iter {
            ForIter::Range { start, end, inclusive } => {
                let s = self.emit_expr(start);
                let e = self.emit_expr(end);
                let cmp = if *inclusive { "<=" } else { "<" };
                self.line(&format!("for (auto {v} = {s}; {v} {cmp} {e}; {v}++) {{",
                    v = var, s = s, cmp = cmp, e = e));
            }
            ForIter::Expr(arr) => {
                let a = self.emit_expr(arr);
                self.line(&format!("for (const auto& {} : {}) {{", var, a));
            }
        }
        self.indent += 1;
        for s in body { self.emit_stmt(s); }
        self.indent -= 1;
        self.line("}");
    }

    // ── Function & struct emission ──────────────────────────────────────────

    fn emit_fn(&mut self, f: &FnDecl) {
        let ret = if f.name == "main" { "int".into() } else { type_to_cpp(&f.ret_ty) };

        let mut param_parts = Vec::new();
        for p in &f.params {
            param_parts.push(format!("{} {}", type_to_cpp(&p.ty), p.name));
        }
        let params = param_parts.join(", ");

        self.line(&format!("{} {}({}) {{", ret, f.name, params));
        self.indent += 1;
        for s in &f.body { self.emit_stmt(s); }

        // Ensure main always returns 0
        if f.name == "main" && !f.body.iter().any(|s| matches!(s, Stmt::Return(_))) {
            self.line("return 0;");
        }
        self.indent -= 1;
        self.line("}");
        self.line("");
    }

    fn emit_struct(&mut self, s: &StructDecl) {
        self.line(&format!("struct {} {{", s.name));
        self.indent += 1;
        for (fname, fty) in &s.fields {
            self.line(&format!("{} {};", type_to_cpp(fty), fname));
        }
        self.indent -= 1;
        self.line("};");
        self.line("");
    }

    // ── Top-level code generation ───────────────────────────────────────────

    pub fn generate(&mut self, program: &Program) -> String {
        // Standard headers
        self.line("#include <iostream>");
        self.line("#include <string>");
        self.line("#include <vector>");
        self.line("#include <sstream>");
        self.line("#include <cmath>");
        self.line("#include <cstdlib>");
        self.line("using namespace std;");
        self.line("");

        // Struct declarations
        for decl in &program.decls {
            if let Decl::Struct(s) = decl {
                self.emit_struct(s);
            }
        }

        // Forward declarations for functions
        for decl in &program.decls {
            if let Decl::Fn(f) = decl {
                let ret = if f.name == "main" { "int".into() } else { type_to_cpp(&f.ret_ty) };
                let mut param_parts = Vec::new();
                for p in &f.params {
                    param_parts.push(format!("{} {}", type_to_cpp(&p.ty), p.name));
                }
                self.line(&format!("{} {}({});", ret, f.name, param_parts.join(", ")));
            }
        }
        self.line("");

        // Function definitions
        for decl in &program.decls {
            if let Decl::Fn(f) = decl {
                self.emit_fn(f);
            }
        }

        self.output.clone()
    }
}
