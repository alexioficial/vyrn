# Vyrn Compiler — Developer Guide

## What is Vyrn?

Vyrn is a statically-typed compiled language with a **self-contained compiler** written in Rust.
No external tools required — the compiler uses **Cranelift** for native code generation.

## Quick Start

```bash
cargo build --release

# JIT compile and run (zero external dependencies)
./target/release/vyrn examples/hello.vyn --run

# Or just (--run is the default)
./target/release/vyrn examples/hello.vyn

# AOT compile to a native binary (needs system linker: cc/gcc)
./target/release/vyrn examples/hello.vyn --build

# AOT with custom output path
./target/release/vyrn examples/hello.vyn --build --output my_program
```

## Dependencies

**`--run` (JIT mode):** Zero external dependencies. The compiler is fully self-contained.

**`--build` (AOT mode):** Requires a system C linker (`cc`, `gcc`, or `clang`) to link the
produced `.o` file. The Vyrn runtime functions are embedded in the compiler binary.

---

## Project Layout

```
src/
  token.rs    — Token types (TokenType enum, Token struct)
  lexer.rs    — Tokeniser (Lexer::new(&src).tokenize())
  ast.rs      — AST nodes: Expr, Stmt, Decl, Type, …
  parser.rs   — Recursive-descent parser (Parser::new(tokens).parse())
  codegen.rs  — Cranelift-based native codegen
                  CodeGen::new_jit()    → JIT mode
                  CodeGen::new_object() → AOT mode
  runtime.rs  — Embedded Rust runtime (println, print, abs, sqrt, pow, …)
  main.rs     — CLI: lex → parse → codegen → JIT execute or AOT link

examples/     — Sample Vyrn programs
```

---

## Pipeline

```
.vyn source
   │
   ▼ Lexer::tokenize()
Vec<Token>
   │
   ▼ Parser::parse()
Program { decls: Vec<Decl> }
   │
   ▼ CodeGen::generate()    (Cranelift IR builder)
   │
   ├─[--run]──▶ JITModule::finalize() → execute in-process
   │
   └─[--build]─▶ ObjectModule::finish() → .o file → system linker → binary
```

---

## Language Reference

### Types

| Vyrn   | Cranelift | Notes |
|--------|-----------|-------|
| `i32`  | `I32`     | signed 32-bit |
| `i64`  | `I64`     | signed 64-bit |
| `f32`  | `F32`     | |
| `f64`  | `F64`     | |
| `bool` | `I8`      | 0 = false, 1 = true |
| `str`  | `I64`     | pointer to null-terminated C string |
| `u32`  | `I32`     | unsigned semantics (udiv/urem/ult/ugt) |
| `void` | —         | |
| `[T]`  | `I64`     | pointer to fixed-size stack array |
| `Name` | `I64`     | pointer to stack-allocated struct |

### Variable declaration

```vyrn
let x: i32 = 42;          // immutable (by convention)
let mut count = 0;         // mutable
let arr = [1, 2, 3];       // fixed-size array (3 elements)
let arr: [i32] = [1,2,3];  // with explicit element type
```

### Control flow

```vyrn
if cond { ... } else { ... }

while cond { ... }

for i in 0..10  { ... }    // exclusive range
for i in 0..=9  { ... }    // inclusive range
for x in arr    { ... }    // array iteration
```

### Functions & structs

```vyrn
fn add(a: i32, b: i32): i32 {
    return a + b;
}

struct Point {
    x: f64,
    y: f64,
}

let p = Point { x: 1.0, y: 2.0 };
println(p.x);
```

### Built-in functions

| Call | Behaviour |
|------|-----------|
| `println(v)` | print value + newline |
| `print(v)` | print value, no newline |
| `len(arr)` | static array length (compile-time constant) |
| `abs(x)` | absolute value |
| `sqrt(x)` | square root |
| `pow(a, b)` | exponentiation |

### F-strings

```vyrn
let name = "Alice";
println(f"Hello, {name}!");       // simple ident interpolation
println(f"Name: {person.name}");  // struct field access
```

---

## Codegen internals (`src/codegen.rs`)

### Variable representation

Scalars (i32, i64, f32, f64, bool, str, pointers): Cranelift `Variable` (SSA via `def_var`/`use_var`).

Arrays and structs: allocated on the stack via `create_sized_stack_slot`; the `Variable` holds
an `I64` pointer to the slot.

### Runtime (`src/runtime.rs`)

All built-in functions (`println`, `print`, `abs`, `sqrt`, `pow`, `fmod`, `to_string`) are
Rust functions marked `#[no_mangle] pub extern "C"`. In JIT mode they are registered directly
as in-process symbols — no libc dependency at all.

### Block sealing rules (loops)

Loop header blocks must **not** be sealed until after the back-edge is added:

```
jump → header          ← don't seal yet
header: brif → body | exit
body: [seal body] → ... → jump header   ← now seal header
exit: [seal exit]
```

---

## Known Limitations

- **Dynamic arrays** (heap-allocated) are not implemented. Only fixed-size stack arrays.
- **String operations** (concatenation, comparison) are not yet implemented.
- **Closures / lambdas** are not in the language yet.
- The **`mut` keyword** is parsed but mutability is not enforced at compile time.
- **AOT `--build`** still needs a system linker for the final link step.

---

## Adding a New Feature

1. **New syntax** → add tokens in `token.rs`, handle in `lexer.rs`
2. **New AST node** → add variant to the relevant enum in `ast.rs`
3. **New parse rule** → add a method in `parser.rs`
4. **New codegen** → add a match arm in `codegen.rs` (`emit_expr` / `emit_stmt`)
5. **New runtime function** → add to `runtime.rs` and register in `all_symbols()`
6. **Test** → add a `.vyn` file under `examples/`

## Building

```bash
cargo build            # debug
cargo build --release  # optimised
cargo clippy           # lint
```
