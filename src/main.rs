mod token;
mod lexer;
mod ast;
mod parser;
mod codegen;
mod runtime;
mod lsp;

use std::{env, fs, path::Path, process::Command};

fn usage() {
    eprintln!("Usage: vyrn <file.vyn> [--run] [--build] [--output <file>]");
    eprintln!("  --run        JIT compile and execute (default, zero external deps)");
    eprintln!("  --build      AOT compile to native binary (needs system linker)");
    eprintln!("  --output f   output filename for --build (default: <stem>[.exe])");
}

fn main() {
    let args: Vec<String> = env::args().collect();

    // LSP mode: vyrn --lsp  (no source file needed)
    if args.get(1).map(|s| s.as_str()) == Some("--lsp") {
        lsp::run();
        return;
    }

    if args.len() < 2 {
        usage();
        std::process::exit(1);
    }

    let input_file = &args[1];
    let mut do_run   = false;
    let mut do_build = false;
    let mut out_file: Option<String> = None;

    let mut i = 2;
    while i < args.len() {
        match args[i].as_str() {
            "--run"    => do_run   = true,
            "--build"  => do_build = true,
            "--output" => {
                i += 1;
                if i < args.len() { out_file = Some(args[i].clone()); }
                else { eprintln!("--output requires a filename"); std::process::exit(1); }
            }
            arg => { eprintln!("Unknown argument: {}", arg); usage(); std::process::exit(1); }
        }
        i += 1;
    }

    // Default: --run
    if !do_run && !do_build { do_run = true; }

    let stem = Path::new(input_file)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| input_file.clone());

    // ── Read source ──────────────────────────────────────────────────────────
    let source = match fs::read_to_string(input_file) {
        Ok(s)  => s,
        Err(e) => { eprintln!("Error reading '{}': {}", input_file, e); std::process::exit(1); }
    };

    // ── Lex ──────────────────────────────────────────────────────────────────
    let mut lex = lexer::Lexer::new(&source);
    let tokens = match lex.tokenize() {
        Ok(t)  => t,
        Err(e) => { eprintln!("{}", e); std::process::exit(1); }
    };

    // ── Parse ────────────────────────────────────────────────────────────────
    let mut par = parser::Parser::new(tokens);
    let program = match par.parse() {
        Ok(p)  => p,
        Err(e) => { eprintln!("{}", e); std::process::exit(1); }
    };

    // ── JIT mode ─────────────────────────────────────────────────────────────
    if do_run {
        let mut cg = codegen::CodeGen::new_jit();
        if let Err(e) = cg.generate(&program) {
            eprintln!("Codegen error: {}", e);
            std::process::exit(1);
        }
        let code = cg.run_jit();
        std::process::exit(code);
    }

    // ── AOT mode ─────────────────────────────────────────────────────────────
    if do_build {
        let obj_file = format!("{}.o", stem);
        let exe_file = out_file.unwrap_or_else(|| {
            if cfg!(windows) { format!("{}.exe", stem) } else { stem.clone() }
        });

        let mut cg = codegen::CodeGen::new_object(&stem);
        if let Err(e) = cg.generate(&program) {
            eprintln!("Codegen error: {}", e);
            std::process::exit(1);
        }
        let obj_bytes = cg.finish_object();
        if let Err(e) = fs::write(&obj_file, &obj_bytes) {
            eprintln!("Error writing '{}': {}", obj_file, e);
            std::process::exit(1);
        }

        // Link using system compiler
        let linkers = ["cc", "gcc", "clang", "x86_64-w64-mingw32-gcc"];
        let mut linked = false;

        for linker in &linkers {
            let status = Command::new(linker)
                .args([&obj_file as &str, "-o", &exe_file])
                .status();
            match status {
                Ok(s) if s.success() => {
                    println!("✓ Built: {}", exe_file);
                    linked = true;
                    break;
                }
                Ok(s) => {
                    eprintln!("{} failed (exit {})", linker, s.code().unwrap_or(-1));
                    std::process::exit(1);
                }
                Err(_) => { /* not found, try next */ }
            }
        }

        if !linked {
            eprintln!("No system linker found. The object file is at: {}", obj_file);
            eprintln!("Link manually: cc {} -o {}", obj_file, exe_file);
            std::process::exit(1);
        }
    }
}
