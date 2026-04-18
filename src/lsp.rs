// Vyrn Language Server Protocol implementation.
//
// Transport: JSON-RPC 2.0 over stdin/stdout with Content-Length framing.
// Activate with:  vyrn --lsp
//
// Capabilities provided:
//   • textDocumentSync   — full-document sync (mode 1)
//   • hoverProvider      — function signatures, struct defs, keyword docs
//   • completionProvider — keywords, built-ins, user-defined symbols
//   • diagnostics        — lexer and parser errors pushed on every change

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use serde_json::{json, Value};

// ─── Built-in function catalogue ─────────────────────────────────────────────

struct Builtin {
    name: &'static str,
    sig:  &'static str,
    doc:  &'static str,
}

const BUILTINS: &[Builtin] = &[
    Builtin { name: "println",     sig: "println(value)",                    doc: "Print a value followed by a newline." },
    Builtin { name: "print",       sig: "print(value)",                      doc: "Print a value without a trailing newline." },
    Builtin { name: "len",         sig: "len(arr: [T]) -> i32",              doc: "Return the number of elements in a fixed-size array (compile-time constant)." },
    Builtin { name: "abs",         sig: "abs(x) -> same",                    doc: "Absolute value. Works for i32 and f64." },
    Builtin { name: "sqrt",        sig: "sqrt(x: f64) -> f64",               doc: "Square root." },
    Builtin { name: "pow",         sig: "pow(base: f64, exp: f64) -> f64",   doc: "Raise `base` to the power `exp`." },
    Builtin { name: "to_string",   sig: "to_string(v) -> str",               doc: "Convert a numeric or bool value to its string representation." },
    Builtin { name: "min",         sig: "min(a, b) -> same",                 doc: "Return the smaller of two values. Accepts i32 or f64." },
    Builtin { name: "max",         sig: "max(a, b) -> same",                 doc: "Return the larger of two values. Accepts i32 or f64." },
    Builtin { name: "floor",       sig: "floor(x: f64) -> f64",              doc: "Round down to the nearest integer (returns f64)." },
    Builtin { name: "ceil",        sig: "ceil(x: f64) -> f64",               doc: "Round up to the nearest integer (returns f64)." },
    Builtin { name: "round",       sig: "round(x: f64) -> f64",              doc: "Round to the nearest integer (returns f64)." },
    Builtin { name: "clamp",       sig: "clamp(val, lo, hi) -> same",        doc: "Clamp a value to the inclusive range [lo, hi]." },
    Builtin { name: "str_len",     sig: "str_len(s: str) -> i32",            doc: "Return the byte length of a string (not character count)." },
    Builtin { name: "str_concat",  sig: "str_concat(a: str, b: str) -> str", doc: "Concatenate two strings and return a new string." },
    Builtin { name: "str_eq",      sig: "str_eq(a: str, b: str) -> bool",    doc: "Compare two strings for equality." },
    Builtin { name: "input",       sig: "input() -> str",                    doc: "Read one line from standard input (newline stripped)." },
    Builtin { name: "parse_int",   sig: "parse_int(s: str) -> i32",          doc: "Parse a decimal string as a 32-bit integer. Returns 0 on failure." },
    Builtin { name: "parse_float", sig: "parse_float(s: str) -> f64",        doc: "Parse a decimal string as a 64-bit float. Returns 0.0 on failure." },
    Builtin { name: "int",         sig: "int(x) -> i32",                     doc: "Convert a value to i32 (truncates floats toward zero)." },
    Builtin { name: "float",       sig: "float(x) -> f64",                   doc: "Convert a value to f64." },
    Builtin { name: "to_i64",      sig: "to_i64(x) -> i64",                  doc: "Convert a value to i64 (sign-extends integers)." },
];

const KEYWORD_DOCS: &[(&str, &str)] = &[
    ("let",      "Variable declaration. Immutable by convention; use `let mut` for a mutable variable."),
    ("mut",      "Marks a variable as mutable: `let mut x = 0;`"),
    ("fn",       "Function declaration: `fn name(param: type) -> ret_type { ... }`"),
    ("gen",      "Generator function prefix. The body may use `yield` to lazily produce values: `gen fn name() -> T { yield v; }`"),
    ("if",       "Conditional branch: `if cond { ... } else { ... }`"),
    ("else",     "Else branch of an `if` expression."),
    ("for",      "For loop.\n- Range: `for i in 0..10 { ... }` (exclusive)\n- Inclusive range: `for i in 0..=9 { ... }`\n- Array iteration: `for x in arr { ... }`\n- Generator iteration: `for v in gen_fn() { ... }`"),
    ("while",    "While loop: `while cond { ... }`"),
    ("return",   "Return a value from the current function: `return expr;`"),
    ("break",    "Exit the innermost loop, or a specific labeled loop: `break;` / `break 'outer;`"),
    ("continue", "Skip to the next loop iteration: `continue;` / `continue 'outer;`"),
    ("yield",    "Yield a value from a generator function. Suspends execution until the caller advances the generator."),
    ("in",       "Used in `for x in expr` iteration syntax."),
    ("struct",   "Define a named aggregate type:\n```vyrn\nstruct Point { x: f64, y: f64 }\n```"),
    ("true",     "Boolean literal `true`."),
    ("false",    "Boolean literal `false`."),
    ("i32",      "Signed 32-bit integer type. Default type for integer literals."),
    ("i64",      "Signed 64-bit integer type."),
    ("f32",      "32-bit single-precision floating-point type."),
    ("f64",      "64-bit double-precision floating-point type. Default type for float literals."),
    ("str",      "String type — a pointer to a null-terminated C string. String literals are `\"text\"`."),
    ("bool",     "Boolean type. Values: `true` or `false`."),
    ("void",     "Unit/void return type. Used when a function returns nothing."),
    ("u32",      "Unsigned 32-bit integer type. Division and comparison use unsigned semantics."),
];

// ─── JSON-RPC framing ─────────────────────────────────────────────────────────

fn read_message(reader: &mut (impl BufRead + Read)) -> Option<Value> {
    let mut content_length: usize = 0;

    // Read headers until empty line
    loop {
        let mut line = String::new();
        let n = reader.read_line(&mut line).unwrap_or(0);
        if n == 0 { return None; }  // EOF
        let line = line.trim();
        if line.is_empty() { break; }
        if let Some(rest) = line.strip_prefix("Content-Length: ") {
            content_length = rest.trim().parse().unwrap_or(0);
        }
    }

    if content_length == 0 { return None; }

    let mut buf = vec![0u8; content_length];
    reader.read_exact(&mut buf).ok()?;
    serde_json::from_slice(&buf).ok()
}

fn write_message(writer: &mut impl Write, msg: &Value) {
    let body = msg.to_string();
    let header = format!("Content-Length: {}\r\n\r\n", body.len());
    let _ = writer.write_all(header.as_bytes());
    let _ = writer.write_all(body.as_bytes());
    let _ = writer.flush();
}

// ─── Diagnostics ─────────────────────────────────────────────────────────────

fn diagnostics_notification(uri: &str, source: &str) -> Value {
    let mut diags: Vec<Value> = Vec::new();

    let mut lexer = crate::lexer::Lexer::new(source);
    match lexer.tokenize() {
        Err(e) => {
            let ln = e.line.saturating_sub(1);
            let ch = e.col.saturating_sub(1);
            diags.push(json!({
                "range": {
                    "start": { "line": ln, "character": ch },
                    "end":   { "line": ln, "character": ch + 1 }
                },
                "severity": 1,
                "source": "vyrn",
                "message": e.message
            }));
        }
        Ok(tokens) => {
            let mut parser = crate::parser::Parser::new(tokens);
            if let Err(e) = parser.parse() {
                let ln = e.line.saturating_sub(1);
                let ch = e.col.saturating_sub(1);
                diags.push(json!({
                    "range": {
                        "start": { "line": ln, "character": ch },
                        "end":   { "line": ln, "character": ch + 30 }
                    },
                    "severity": 1,
                    "source": "vyrn",
                    "message": e.message
                }));
            }
        }
    }

    json!({
        "jsonrpc": "2.0",
        "method": "textDocument/publishDiagnostics",
        "params": { "uri": uri, "diagnostics": diags }
    })
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

/// Parse source for best-effort AST (ignores errors).
fn try_parse(source: &str) -> Option<crate::ast::Program> {
    let mut lexer = crate::lexer::Lexer::new(source);
    let tokens = lexer.tokenize().ok()?;
    crate::parser::Parser::new(tokens).parse().ok()
}

/// Return the identifier word that spans (line, col) in `source`.
fn word_at(source: &str, line: usize, col: usize) -> Option<String> {
    let line_str = source.lines().nth(line)?;
    let chars: Vec<char> = line_str.chars().collect();
    if col > chars.len() { return None; }

    let is_ident = |c: char| c.is_alphanumeric() || c == '_';

    let mut start = col;
    while start > 0 && is_ident(chars[start - 1]) { start -= 1; }
    let mut end = col;
    while end < chars.len() && is_ident(chars[end]) { end += 1; }

    if start == end { return None; }
    Some(chars[start..end].iter().collect())
}

// ─── Symbol index (go-to-definition, references, code-lens) ─────────────────

#[derive(Debug, Clone)]
struct SymbolDef {
    name:   String,
    kind:   &'static str,   // "function" | "struct" | "field" | "variable" | "parameter"
    line:   usize,           // 0-based
    col:    usize,           // 0-based
    parent: Option<String>,  // owning struct name, set for kind "field"
}

struct StdlibInfo {
    uri:     String,
    entries: Vec<(String, usize)>,  // (fn_name, 0-based line of fn declaration)
}

fn write_stdlib() -> Option<StdlibInfo> {
    let dir = std::env::current_exe().ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(std::env::temp_dir);
    let path = dir.join("vyrn_stdlib.vyn");

    let header = "// Vyrn Standard Library — Built-in Functions\n\
                  //\n\
                  // Auto-generated by vyrn-lsp. Do not edit.\n\
                  \n";
    let mut content = header.to_string();
    let mut entries = Vec::new();
    // header is 4 lines (0-3); each builtin: comment (+0), fn decl (+1), blank (+2)
    let base_line = 4usize;

    for (idx, b) in BUILTINS.iter().enumerate() {
        let fn_line = base_line + idx * 3 + 1;
        entries.push((b.name.to_string(), fn_line));
        content.push_str(&format!("// {}\nfn {}(...) {{}}\n\n", b.doc, b.name));
    }

    std::fs::write(&path, &content).ok()?;

    let uri = if cfg!(windows) {
        format!("file:///{}", path.to_string_lossy().replace('\\', "/"))
    } else {
        format!("file://{}", path.to_string_lossy())
    };

    Some(StdlibInfo { uri, entries })
}

/// Returns true when the identifier at (line, col) is preceded by a `.` (field access).
fn is_field_access_at(source: &str, line: usize, col: usize) -> bool {
    let line_str = source.lines().nth(line).unwrap_or("");
    let chars: Vec<char> = line_str.chars().collect();
    let is_ident = |c: char| c.is_alphanumeric() || c == '_';
    let mut start = col.min(chars.len());
    while start > 0 && is_ident(chars[start - 1]) { start -= 1; }
    start > 0 && chars[start - 1] == '.'
}

/// Walk the token stream and collect every definition site.
fn collect_definitions(tokens: &[crate::token::Token]) -> Vec<SymbolDef> {
    use crate::token::TokenType;
    let mut defs = Vec::new();
    let len = tokens.len();
    let mut i = 0;
    while i < len {
        match tokens[i].ty {
            // fn NAME(params…)  /  gen fn NAME(params…)
            TokenType::Fn => {
                if i + 1 < len && tokens[i + 1].ty == TokenType::Identifier {
                    let t = &tokens[i + 1];
                    defs.push(SymbolDef { name: t.value.clone(), kind: "function",
                        line: t.line - 1, col: t.col - 1, parent: None });
                    // scan parameters inside ( … )
                    if i + 2 < len && tokens[i + 2].ty == TokenType::LParen {
                        let mut j = i + 3;
                        while j < len && tokens[j].ty != TokenType::RParen {
                            if tokens[j].ty == TokenType::Identifier
                                && j + 1 < len && tokens[j + 1].ty == TokenType::Colon
                            {
                                defs.push(SymbolDef { name: tokens[j].value.clone(),
                                    kind: "parameter",
                                    line: tokens[j].line - 1, col: tokens[j].col - 1,
                                    parent: None });
                            }
                            j += 1;
                        }
                    }
                }
            }
            // struct NAME { field: Type, … }
            TokenType::Struct => {
                if i + 1 < len && tokens[i + 1].ty == TokenType::Identifier {
                    let t = &tokens[i + 1];
                    let struct_name = t.value.clone();
                    defs.push(SymbolDef { name: struct_name.clone(), kind: "struct",
                        line: t.line - 1, col: t.col - 1, parent: None });
                    // walk fields inside { … }
                    if i + 2 < len && tokens[i + 2].ty == TokenType::LBrace {
                        let mut j = i + 3;
                        while j < len && tokens[j].ty != TokenType::RBrace {
                            if tokens[j].ty == TokenType::Identifier
                                && j + 1 < len && tokens[j + 1].ty == TokenType::Colon
                            {
                                defs.push(SymbolDef {
                                    name:   tokens[j].value.clone(),
                                    kind:   "field",
                                    line:   tokens[j].line - 1,
                                    col:    tokens[j].col - 1,
                                    parent: Some(struct_name.clone()),
                                });
                            }
                            j += 1;
                        }
                    }
                }
            }
            // let [mut] NAME
            TokenType::Let => {
                let mut j = i + 1;
                if j < len && tokens[j].ty == TokenType::Mut { j += 1; }
                if j < len && tokens[j].ty == TokenType::Identifier {
                    defs.push(SymbolDef { name: tokens[j].value.clone(), kind: "variable",
                        line: tokens[j].line - 1, col: tokens[j].col - 1, parent: None });
                }
            }
            // for NAME in …
            TokenType::For => {
                if i + 1 < len && tokens[i + 1].ty == TokenType::Identifier {
                    defs.push(SymbolDef { name: tokens[i + 1].value.clone(), kind: "variable",
                        line: tokens[i + 1].line - 1, col: tokens[i + 1].col - 1, parent: None });
                }
            }
            _ => {}
        }
        i += 1;
    }
    defs
}

/// Tokenize source (returns empty vec on error).
fn tokenize_source(source: &str) -> Vec<crate::token::Token> {
    crate::lexer::Lexer::new(source).tokenize().unwrap_or_default()
}

/// Extract `//` comment lines immediately above `line` (0-based) as a doc string.
fn doc_comment_above(source: &str, line: usize) -> Option<String> {
    let lines: Vec<&str> = source.lines().collect();
    let mut doc = Vec::new();
    let mut i = line;
    while i > 0 {
        i -= 1;
        let trimmed = lines.get(i)?.trim();
        if let Some(rest) = trimmed.strip_prefix("//") {
            doc.push(rest.trim().to_string());
        } else {
            break;
        }
    }
    if doc.is_empty() { return None; }
    doc.reverse();
    Some(doc.join("\n"))
}

// ─── Hover ────────────────────────────────────────────────────────────────────

fn hover_for_word(word: &str, source: &str, cursor_line: usize) -> Option<String> {
    // 1. Built-in functions
    for b in BUILTINS {
        if b.name == word {
            return Some(format!("```vyrn\n{}\n```\n\n{}", b.sig, b.doc));
        }
    }

    // 2. Keywords
    for (kw, doc) in KEYWORD_DOCS {
        if *kw == word {
            return Some(format!("**{}** *(keyword)*\n\n{}", kw, doc));
        }
    }

    // 3. User-defined functions / structs from the current file
    let tokens = tokenize_source(source);
    let defs = collect_definitions(&tokens);

    if let Some(program) = try_parse(source) {
        for decl in &program.decls {
            match decl {
                crate::ast::Decl::Fn(f) if f.name == word => {
                    let params = f.params.iter()
                        .map(|p| format!("{}: {}", p.name, p.ty))
                        .collect::<Vec<_>>()
                        .join(", ");
                    let ret = if f.ret_ty == crate::ast::Type::Void {
                        String::new()
                    } else {
                        format!(" -> {}", f.ret_ty)
                    };
                    let prefix = if f.is_gen { "gen " } else { "" };
                    let sig = format!("{}fn {}({}){}", prefix, f.name, params, ret);

                    // doc-comment above definition
                    let def_line = defs.iter()
                        .find(|d| d.name == word && d.kind == "function")
                        .map(|d| d.line);
                    let doc = def_line.and_then(|l| doc_comment_above(source, l));

                    let mut hover = format!("```vyrn\n{}\n```", sig);
                    if let Some(d) = doc { hover.push_str(&format!("\n\n{}", d)); }
                    return Some(hover);
                }
                crate::ast::Decl::Struct(s) if s.name == word => {
                    let fields = s.fields.iter()
                        .map(|(n, t)| format!("    {}: {}", n, t))
                        .collect::<Vec<_>>()
                        .join(",\n");

                    let def_line = defs.iter()
                        .find(|d| d.name == word && d.kind == "struct")
                        .map(|d| d.line);
                    let doc = def_line.and_then(|l| doc_comment_above(source, l));

                    let mut hover = format!("```vyrn\nstruct {} {{\n{}\n}}\n```", s.name, fields);
                    if let Some(d) = doc { hover.push_str(&format!("\n\n{}", d)); }
                    return Some(hover);
                }
                _ => {}
            }
        }
    }

    // 4. Variables / parameters — show the source line of the declaration
    let mut best: Option<&SymbolDef> = None;
    for def in &defs {
        if def.name == word && (def.kind == "variable" || def.kind == "parameter") {
            if def.line <= cursor_line {
                best = Some(def);
            }
        }
    }
    if let Some(def) = best {
        if let Some(src_line) = source.lines().nth(def.line) {
            let label = if def.kind == "parameter" { "parameter" } else { "variable" };
            return Some(format!("*({})* \n```vyrn\n{}\n```", label, src_line.trim()));
        }
    }

    None
}

// ─── Completions ──────────────────────────────────────────────────────────────

fn completion_items(source: &str) -> Vec<Value> {
    let mut items: Vec<Value> = Vec::new();

    // Keywords  (CompletionItemKind::Keyword = 14)
    for (kw, doc) in KEYWORD_DOCS {
        items.push(json!({
            "label": kw,
            "kind": 14,
            "documentation": { "kind": "markdown", "value": *doc }
        }));
    }

    // Built-in functions  (CompletionItemKind::Function = 3)
    for b in BUILTINS {
        items.push(json!({
            "label": b.name,
            "kind": 3,
            "detail": b.sig,
            "documentation": { "kind": "markdown", "value": b.doc }
        }));
    }

    // User-defined symbols from the current file
    if let Some(program) = try_parse(source) {
        for decl in &program.decls {
            match decl {
                crate::ast::Decl::Fn(f) => {
                    let params = f.params.iter()
                        .map(|p| format!("{}: {}", p.name, p.ty))
                        .collect::<Vec<_>>()
                        .join(", ");
                    let ret = if f.ret_ty == crate::ast::Type::Void {
                        String::new()
                    } else {
                        format!(" -> {}", f.ret_ty)
                    };
                    let prefix = if f.is_gen { "gen " } else { "" };
                    items.push(json!({
                        "label": f.name,
                        "kind": 3,
                        "detail": format!("{}fn {}({}){}", prefix, f.name, params, ret)
                    }));
                }
                crate::ast::Decl::Struct(s) => {
                    // CompletionItemKind::Struct = 22
                    items.push(json!({
                        "label": s.name,
                        "kind": 22,
                        "detail": format!("struct {}", s.name)
                    }));
                }
            }
        }
    }

    items
}

// ─── Server loop ──────────────────────────────────────────────────────────────

pub fn run() {
    let stdin  = std::io::stdin();
    let stdout = std::io::stdout();
    let mut reader = BufReader::new(stdin.lock());
    let mut writer = stdout.lock();

    let stdlib = write_stdlib();
    // uri → source text
    let mut files: HashMap<String, String> = HashMap::new();

    loop {
        let msg = match read_message(&mut reader) {
            Some(m) => m,
            None    => break,
        };

        let method = match msg["method"].as_str() {
            Some(m) => m.to_string(),
            None    => continue,
        };

        let id     = msg["id"].clone();
        let params = msg["params"].clone();

        match method.as_str() {
            // ── Lifecycle ───────────────────────────────────────────────────

            "initialize" => {
                write_message(&mut writer, &json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {
                        "capabilities": {
                            "textDocumentSync": 1,
                            "hoverProvider": true,
                            "completionProvider": {
                                "resolveProvider": false,
                                "triggerCharacters": [".", "(", ":"]
                            },
                            "definitionProvider": true,
                            "referencesProvider": true,
                            "codeLensProvider": { "resolveProvider": false }
                        },
                        "serverInfo": { "name": "vyrn-lsp", "version": "0.1.0" }
                    }
                }));
            }

            "initialized" | "$/setTrace" | "$/cancelRequest" => {
                // notifications — no response needed
            }

            "shutdown" => {
                write_message(&mut writer, &json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": null
                }));
            }

            "exit" => {
                std::process::exit(0);
            }

            // ── Document sync ───────────────────────────────────────────────

            "textDocument/didOpen" => {
                if let (Some(uri), Some(text)) = (
                    params["textDocument"]["uri"].as_str(),
                    params["textDocument"]["text"].as_str(),
                ) {
                    let uri  = uri.to_string();
                    let text = text.to_string();
                    let diag = diagnostics_notification(&uri, &text);
                    files.insert(uri, text);
                    write_message(&mut writer, &diag);
                }
            }

            "textDocument/didChange" => {
                if let (Some(uri), Some(text)) = (
                    params["textDocument"]["uri"].as_str(),
                    params["contentChanges"][0]["text"].as_str(),
                ) {
                    let uri  = uri.to_string();
                    let text = text.to_string();
                    let diag = diagnostics_notification(&uri, &text);
                    files.insert(uri, text);
                    write_message(&mut writer, &diag);
                }
            }

            "textDocument/didSave" => {
                if let Some(uri) = params["textDocument"]["uri"].as_str() {
                    if let Some(src) = files.get(uri).cloned() {
                        let diag = diagnostics_notification(uri, &src);
                        write_message(&mut writer, &diag);
                    }
                }
            }

            "textDocument/didClose" => {
                if let Some(uri) = params["textDocument"]["uri"].as_str() {
                    files.remove(uri);
                    // Clear diagnostics for closed file
                    write_message(&mut writer, &json!({
                        "jsonrpc": "2.0",
                        "method": "textDocument/publishDiagnostics",
                        "params": { "uri": uri, "diagnostics": [] }
                    }));
                }
            }

            // ── Language features ───────────────────────────────────────────

            "textDocument/hover" => {
                let line = params["position"]["line"].as_u64().unwrap_or(0) as usize;
                let col  = params["position"]["character"].as_u64().unwrap_or(0) as usize;
                let uri  = params["textDocument"]["uri"].as_str().unwrap_or("");

                let result: Value = files.get(uri)
                    .and_then(|src| word_at(src, line, col).map(|w| (w, src.clone())))
                    .and_then(|(word, src)| hover_for_word(&word, &src, line))
                    .map(|md| json!({ "contents": { "kind": "markdown", "value": md } }))
                    .unwrap_or(Value::Null);

                write_message(&mut writer, &json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": result
                }));
            }

            "textDocument/completion" => {
                let uri  = params["textDocument"]["uri"].as_str().unwrap_or("");
                let src  = files.get(uri).map(|s| s.as_str()).unwrap_or("");
                let items = completion_items(src);

                write_message(&mut writer, &json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": { "isIncomplete": false, "items": items }
                }));
            }

            // ── Go to definition ────────────────────────────────────────────

            "textDocument/definition" => {
                let line = params["position"]["line"].as_u64().unwrap_or(0) as usize;
                let col  = params["position"]["character"].as_u64().unwrap_or(0) as usize;
                let uri  = params["textDocument"]["uri"].as_str().unwrap_or("");

                let result: Value = (|| {
                    let source = files.get(uri)?;
                    let word   = word_at(source, line, col)?;

                    // Built-ins → jump to generated stdlib stub
                    if let Some(ref sl) = stdlib {
                        if let Some(&(_, fn_line)) = sl.entries.iter().find(|(n, _)| n == &word) {
                            return Some(json!({
                                "uri": sl.uri,
                                "range": {
                                    "start": { "line": fn_line, "character": 3 },
                                    "end":   { "line": fn_line, "character": 3 + word.len() }
                                }
                            }));
                        }
                    }

                    let tokens = tokenize_source(source);
                    let defs   = collect_definitions(&tokens);

                    // Field access (p.x) → search only fields
                    if is_field_access_at(source, line, col) {
                        for def in &defs {
                            if def.name == word && def.kind == "field" {
                                return Some(json!({
                                    "uri": uri,
                                    "range": {
                                        "start": { "line": def.line, "character": def.col },
                                        "end":   { "line": def.line, "character": def.col + word.len() }
                                    }
                                }));
                            }
                        }
                    }

                    // Global symbols: fn / struct — first match
                    for def in &defs {
                        if def.name == word && (def.kind == "function" || def.kind == "struct") {
                            return Some(json!({
                                "uri": uri,
                                "range": {
                                    "start": { "line": def.line, "character": def.col },
                                    "end":   { "line": def.line, "character": def.col + word.len() }
                                }
                            }));
                        }
                    }

                    // Local: variable / parameter — closest definition before cursor
                    let mut best: Option<&SymbolDef> = None;
                    for def in &defs {
                        if def.name == word
                            && (def.kind == "variable" || def.kind == "parameter")
                            && (def.line < line || (def.line == line && def.col <= col))
                        {
                            best = Some(def);
                        }
                    }
                    if let Some(d) = best {
                        return Some(json!({
                            "uri": uri,
                            "range": {
                                "start": { "line": d.line, "character": d.col },
                                "end":   { "line": d.line, "character": d.col + word.len() }
                            }
                        }));
                    }

                    // Fallback: struct field by name (struct literals: Point { x: … })
                    for def in &defs {
                        if def.name == word && def.kind == "field" {
                            return Some(json!({
                                "uri": uri,
                                "range": {
                                    "start": { "line": def.line, "character": def.col },
                                    "end":   { "line": def.line, "character": def.col + word.len() }
                                }
                            }));
                        }
                    }

                    None
                })().unwrap_or(Value::Null);

                write_message(&mut writer, &json!({
                    "jsonrpc": "2.0", "id": id, "result": result
                }));
            }

            // ── Find references ────────────────────────────────────────────

            "textDocument/references" => {
                let line = params["position"]["line"].as_u64().unwrap_or(0) as usize;
                let col  = params["position"]["character"].as_u64().unwrap_or(0) as usize;
                let uri  = params["textDocument"]["uri"].as_str().unwrap_or("");

                let result: Value = (|| {
                    let source = files.get(uri)?;
                    let word = word_at(source, line, col)?;
                    let tokens = tokenize_source(source);
                    let locs: Vec<Value> = tokens.iter()
                        .filter(|t| t.ty == crate::token::TokenType::Identifier && t.value == word)
                        .map(|t| json!({
                            "uri": uri,
                            "range": {
                                "start": { "line": t.line - 1, "character": t.col - 1 },
                                "end":   { "line": t.line - 1, "character": t.col - 1 + word.len() }
                            }
                        }))
                        .collect();
                    if locs.is_empty() { None } else { Some(json!(locs)) }
                })().unwrap_or(json!([]));

                write_message(&mut writer, &json!({
                    "jsonrpc": "2.0", "id": id, "result": result
                }));
            }

            // ── Code lens (reference counts) ───────────────────────────────

            "textDocument/codeLens" => {
                let uri = params["textDocument"]["uri"].as_str().unwrap_or("");

                let lenses: Vec<Value> = (|| {
                    let source = files.get(uri)?;
                    let tokens = tokenize_source(source);
                    let defs = collect_definitions(&tokens);
                    let mut out = Vec::new();
                    for def in &defs {
                        if def.kind != "function" && def.kind != "struct" { continue; }
                        let n = tokens.iter()
                            .filter(|t| t.ty == crate::token::TokenType::Identifier
                                && t.value == def.name
                                && !(t.line.saturating_sub(1) == def.line
                                     && t.col.saturating_sub(1) == def.col))
                            .count();
                        out.push(json!({
                            "range": {
                                "start": { "line": def.line, "character": 0 },
                                "end":   { "line": def.line, "character": 0 }
                            },
                            "command": {
                                "title": format!("{} reference{}", n,
                                    if n == 1 { "" } else { "s" }),
                                "command": ""
                            }
                        }));
                    }
                    Some(out)
                })().unwrap_or_default();

                write_message(&mut writer, &json!({
                    "jsonrpc": "2.0", "id": id, "result": lenses
                }));
            }

            // ── Unknown requests ────────────────────────────────────────────

            _ => {
                if !id.is_null() {
                    write_message(&mut writer, &json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": null
                    }));
                }
            }
        }
    }
}
