mod token;
mod lexer;
mod ast;
mod parser;
mod codegen;

use std::{env, fs, path::Path, process::Command};

fn usage() {
    eprintln!("Usage: vyrn <file.vyn> [--run] [--output <file.cpp>]");
    eprintln!("       vyrn <file.vyn> --run          compile and execute");
    eprintln!("       vyrn <file.vyn> --output out.cpp");
}

fn main() {
    let args: Vec<String> = env::args().collect();

    if args.len() < 2 {
        usage();
        std::process::exit(1);
    }

    let input_file = &args[1];
    let mut run = false;
    let mut output_file: Option<String> = None;

    let mut i = 2;
    while i < args.len() {
        match args[i].as_str() {
            "--run" => run = true,
            "--output" => {
                i += 1;
                if i < args.len() {
                    output_file = Some(args[i].clone());
                } else {
                    eprintln!("--output requires a filename");
                    std::process::exit(1);
                }
            }
            arg => {
                eprintln!("Unknown argument: {}", arg);
                usage();
                std::process::exit(1);
            }
        }
        i += 1;
    }

    // Default output path: replace .vyn extension with .cpp
    let cpp_file = output_file.unwrap_or_else(|| {
        let stem = Path::new(input_file)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| input_file.clone());
        format!("{}.cpp", stem)
    });

    // ── Read source ──────────────────────────────────────────────────────────
    let source = match fs::read_to_string(input_file) {
        Ok(s) => s,
        Err(e) => { eprintln!("Error reading '{}': {}", input_file, e); std::process::exit(1); }
    };

    // ── Lex ──────────────────────────────────────────────────────────────────
    let mut lex = lexer::Lexer::new(&source);
    let tokens = match lex.tokenize() {
        Ok(t) => t,
        Err(e) => { eprintln!("{}", e); std::process::exit(1); }
    };

    // ── Parse ────────────────────────────────────────────────────────────────
    let mut par = parser::Parser::new(tokens);
    let program = match par.parse() {
        Ok(p) => p,
        Err(e) => { eprintln!("{}", e); std::process::exit(1); }
    };

    // ── Codegen ──────────────────────────────────────────────────────────────
    let mut cg = codegen::CodeGen::new();
    let cpp_code = cg.generate(&program);

    // ── Write C++ ────────────────────────────────────────────────────────────
    if let Err(e) = fs::write(&cpp_file, &cpp_code) {
        eprintln!("Error writing '{}': {}", cpp_file, e);
        std::process::exit(1);
    }
    println!("✓ Generated: {}", cpp_file);

    // ── Compile & run ────────────────────────────────────────────────────────
    if run {
        let exe_file = if cfg!(windows) {
            cpp_file.trim_end_matches(".cpp").to_string() + ".exe"
        } else {
            cpp_file.trim_end_matches(".cpp").to_string()
        };

        // Try multiple C++ compilers in order of preference
        let compilers = ["g++", "clang++", "c++"];
        let mut compiled = false;
        for compiler in &compilers {
            let status = Command::new(compiler)
                .args(["-std=c++17", &cpp_file, "-o", &exe_file])
                .status();
            match status {
                Ok(s) if s.success() => {
                    println!("✓ Compiled with {}: {}", compiler, exe_file);
                    compiled = true;
                    break;
                }
                Ok(s) => {
                    eprintln!("{} failed (exit {})", compiler, s.code().unwrap_or(-1));
                    std::process::exit(1);
                }
                Err(_) => { /* compiler not found, try next */ }
            }
        }
        if !compiled {
            eprintln!("No C++ compiler found. Please install g++, clang++, or c++.");
            eprintln!("On Windows: install MinGW-w64 or LLVM.");
            eprintln!("The generated C++ is saved to: {}", cpp_file);
            std::process::exit(1);
        }

        // Run the executable
        let exe_path = env::current_dir()
            .map(|d| d.join(&exe_file))
            .unwrap_or_else(|_| exe_file.clone().into());

        let run_status = Command::new(&exe_path).status();
        if let Err(e) = run_status {
            eprintln!("Error executing '{}': {}", exe_path.display(), e);
            std::process::exit(1);
        }
    }
}
