// Vyrn embedded runtime — pure Rust, zero external dependencies.
// These functions are called by JIT-compiled Vyrn programs via symbol resolution.
// All functions use the C calling convention so Cranelift can call them directly.

use std::ffi::CStr;
use std::io::{self, Write};

// ─── println (with newline) ───────────────────────────────────────────────────

#[no_mangle]
pub extern "C" fn vyrn_println_i32(v: i32) {
    println!("{}", v);
}

#[no_mangle]
pub extern "C" fn vyrn_println_u32(v: u32) {
    println!("{}", v);
}

#[no_mangle]
pub extern "C" fn vyrn_println_i64(v: i64) {
    println!("{}", v);
}

#[no_mangle]
pub extern "C" fn vyrn_println_f64(v: f64) {
    println!("{}", v);
}

#[no_mangle]
pub extern "C" fn vyrn_println_f32(v: f32) {
    println!("{}", v);
}

#[no_mangle]
pub extern "C" fn vyrn_println_bool(v: i8) {
    if v != 0 { println!("true"); } else { println!("false"); }
}

#[no_mangle]
pub unsafe extern "C" fn vyrn_println_str(ptr: *const i8) {
    if ptr.is_null() {
        println!();
        return;
    }
    let s = CStr::from_ptr(ptr).to_string_lossy();
    println!("{}", s);
}

#[no_mangle]
pub extern "C" fn vyrn_println_newline() {
    println!();
}

// ─── print (no newline) ───────────────────────────────────────────────────────

#[no_mangle]
pub extern "C" fn vyrn_print_i32(v: i32) {
    print!("{}", v);
    let _ = io::stdout().flush();
}

#[no_mangle]
pub extern "C" fn vyrn_print_u32(v: u32) {
    print!("{}", v);
    let _ = io::stdout().flush();
}

#[no_mangle]
pub extern "C" fn vyrn_print_i64(v: i64) {
    print!("{}", v);
    let _ = io::stdout().flush();
}

#[no_mangle]
pub extern "C" fn vyrn_print_f64(v: f64) {
    print!("{}", v);
    let _ = io::stdout().flush();
}

#[no_mangle]
pub extern "C" fn vyrn_print_f32(v: f32) {
    print!("{}", v);
    let _ = io::stdout().flush();
}

#[no_mangle]
pub extern "C" fn vyrn_print_bool(v: i8) {
    if v != 0 { print!("true"); } else { print!("false"); }
    let _ = io::stdout().flush();
}

#[no_mangle]
pub unsafe extern "C" fn vyrn_print_str(ptr: *const i8) {
    if ptr.is_null() { return; }
    let s = CStr::from_ptr(ptr).to_string_lossy();
    print!("{}", s);
    let _ = io::stdout().flush();
}

// ─── Math ─────────────────────────────────────────────────────────────────────

#[no_mangle]
pub extern "C" fn vyrn_abs_i32(v: i32) -> i32 {
    v.abs()
}

#[no_mangle]
pub extern "C" fn vyrn_fmod_f64(a: f64, b: f64) -> f64 {
    a % b
}

#[no_mangle]
pub extern "C" fn vyrn_abs_f64(v: f64) -> f64 {
    v.abs()
}

#[no_mangle]
pub extern "C" fn vyrn_sqrt_f64(v: f64) -> f64 {
    v.sqrt()
}

#[no_mangle]
pub extern "C" fn vyrn_pow_f64(base: f64, exp: f64) -> f64 {
    base.powf(exp)
}

// ─── to_string ────────────────────────────────────────────────────────────────
// Returns a heap-allocated null-terminated string.
// Memory is intentionally leaked (program-lifetime; acceptable for a compiler).

#[no_mangle]
pub extern "C" fn vyrn_i32_to_string(v: i32) -> *const i8 {
    let s = std::ffi::CString::new(v.to_string()).unwrap();
    s.into_raw()
}

#[no_mangle]
pub extern "C" fn vyrn_f64_to_string(v: f64) -> *const i8 {
    let s = std::ffi::CString::new(v.to_string()).unwrap();
    s.into_raw()
}

#[no_mangle]
pub extern "C" fn vyrn_bool_to_string(v: i8) -> *const i8 {
    let s = std::ffi::CString::new(if v != 0 { "true" } else { "false" }).unwrap();
    s.into_raw()
}

// ─── Symbol table for JITBuilder ──────────────────────────────────────────────
// List every (name, fn_ptr) pair so main.rs can register them all at once.

pub fn all_symbols() -> Vec<(&'static str, *const u8)> {
    vec![
        ("vyrn_println_i32",      vyrn_println_i32      as *const u8),
        ("vyrn_println_u32",      vyrn_println_u32      as *const u8),
        ("vyrn_println_i64",      vyrn_println_i64      as *const u8),
        ("vyrn_println_f64",      vyrn_println_f64      as *const u8),
        ("vyrn_println_f32",      vyrn_println_f32      as *const u8),
        ("vyrn_println_bool",     vyrn_println_bool     as *const u8),
        ("vyrn_println_str",      vyrn_println_str      as *const u8),
        ("vyrn_println_newline",  vyrn_println_newline  as *const u8),
        ("vyrn_print_i32",        vyrn_print_i32        as *const u8),
        ("vyrn_print_u32",        vyrn_print_u32        as *const u8),
        ("vyrn_print_i64",        vyrn_print_i64        as *const u8),
        ("vyrn_print_f64",        vyrn_print_f64        as *const u8),
        ("vyrn_print_f32",        vyrn_print_f32        as *const u8),
        ("vyrn_print_bool",       vyrn_print_bool       as *const u8),
        ("vyrn_print_str",        vyrn_print_str        as *const u8),
        ("vyrn_abs_i32",          vyrn_abs_i32          as *const u8),
        ("vyrn_fmod_f64",         vyrn_fmod_f64         as *const u8),
        ("vyrn_abs_f64",          vyrn_abs_f64          as *const u8),
        ("vyrn_sqrt_f64",         vyrn_sqrt_f64         as *const u8),
        ("vyrn_pow_f64",          vyrn_pow_f64          as *const u8),
        ("vyrn_i32_to_string",    vyrn_i32_to_string    as *const u8),
        ("vyrn_f64_to_string",    vyrn_f64_to_string    as *const u8),
        ("vyrn_bool_to_string",   vyrn_bool_to_string   as *const u8),
    ]
}
