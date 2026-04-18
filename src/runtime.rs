// Vyrn embedded runtime — pure Rust, zero external dependencies.
// These functions are called by JIT-compiled Vyrn programs via symbol resolution.
// All functions use the C calling convention so Cranelift can call them directly.

use std::ffi::CStr;
use std::io::{self, Write};
use std::sync::mpsc;

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

// ─── Generators via mpsc ──────────────────────────────────────────────────────
// YieldCtx: allocated in generator wrapper. Layout:
//   [0..8]   = *mut SyncSender<Option<i64>>
//   [8..N]   = args (variable size, written by wrapper)
// GenHandle: returned by vyrn_gen_start. Layout:
//   [0..8]   = *mut Receiver<Option<i64>>
//   [8..16]  = current value (i64)
//   [16..17] = done flag (u8)

#[repr(C)]
#[allow(dead_code)]
pub struct VyrnYieldCtx {
    tx_ptr: *mut (),  // Box<SyncSender<Option<i64>>>
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct VyrnGenHandle {
    rx_ptr: *mut (),  // Box<Receiver<Option<i64>>>
    current: i64,
    done: u8,
}

#[no_mangle]
pub extern "C" fn vyrn_gen_ctx_alloc(args_bytes: i64) -> i64 {
    // Allocate space for VyrnYieldCtx (8 bytes) + args
    let total_bytes = 8 + args_bytes as usize;
    let layout = std::alloc::Layout::from_size_align(total_bytes, 8).unwrap();
    let ptr = unsafe { std::alloc::alloc(layout) };
    ptr as i64
}

#[no_mangle]
pub extern "C" fn vyrn_gen_start(fn_ptr: i64, ctx_ptr: i64) -> i64 {
    let ctx_ptr = ctx_ptr as *mut u8;

    // Create the channel
    let (tx, rx): (mpsc::SyncSender<Option<i64>>, mpsc::Receiver<Option<i64>>) = mpsc::sync_channel(0);
    let rx_box = Box::new(rx);

    // Store sender pointer at offset 0 of context
    let tx_box = Box::new(tx);
    let tx_ptr_ptr = ctx_ptr as *mut *mut ();
    unsafe {
        *tx_ptr_ptr = Box::into_raw(tx_box) as *mut ();
    }

    // Spawn thread running the generator body
    let ctx_ptr_i64 = ctx_ptr as i64;
    std::thread::spawn(move || {
        // Cast fn_ptr to function pointer and call it
        let fn_body: extern "C" fn(i64) = unsafe { std::mem::transmute(fn_ptr) };
        fn_body(ctx_ptr_i64);
    });

    // Return GenHandle
    let handle = VyrnGenHandle {
        rx_ptr: Box::into_raw(rx_box) as *mut (),
        current: 0,
        done: 0,
    };

    let handle_box = Box::new(handle);
    Box::into_raw(handle_box) as i64
}

#[no_mangle]
pub extern "C" fn vyrn_gen_advance(handle_ptr: i64) -> i8 {
    let handle = unsafe { &mut *(handle_ptr as *mut VyrnGenHandle) };

    let rx = unsafe { &mut *(handle.rx_ptr as *mut mpsc::Receiver<Option<i64>>) };

    match rx.recv() {
        Ok(Some(val)) => {
            handle.current = val;
            handle.done = 0;
            1
        }
        Ok(None) => {
            handle.done = 1;
            0
        }
        Err(_) => {
            handle.done = 1;
            0
        }
    }
}

#[no_mangle]
pub extern "C" fn vyrn_gen_value_i64(handle_ptr: i64) -> i64 {
    let handle = unsafe { *(handle_ptr as *mut VyrnGenHandle) };
    handle.current
}

#[no_mangle]
pub extern "C" fn vyrn_yield_i64(ctx_ptr: i64, val: i64) {
    let ctx_ptr = ctx_ptr as *mut u8;
    let tx_ptr_ptr = ctx_ptr as *mut *mut ();
    let tx_ptr = unsafe { *tx_ptr_ptr };
    let tx = unsafe { &*(tx_ptr as *mut mpsc::SyncSender<Option<i64>>) };

    let _ = tx.send(Some(val));
}

#[no_mangle]
pub extern "C" fn vyrn_gen_end(ctx_ptr: i64) {
    // Close the sender by dropping it
    let ctx_ptr = ctx_ptr as *mut u8;
    let tx_ptr_ptr = ctx_ptr as *mut *mut ();
    let tx_ptr = unsafe { *tx_ptr_ptr };
    let _tx = unsafe { Box::from_raw(tx_ptr as *mut mpsc::SyncSender<Option<i64>>) };
    // tx is dropped here, closing the channel
}

// ─── Standard library: Math ───────────────────────────────────────────────────

#[no_mangle]
pub extern "C" fn vyrn_min_i32(a: i32, b: i32) -> i32 { a.min(b) }

#[no_mangle]
pub extern "C" fn vyrn_max_i32(a: i32, b: i32) -> i32 { a.max(b) }

#[no_mangle]
pub extern "C" fn vyrn_min_f64(a: f64, b: f64) -> f64 { a.min(b) }

#[no_mangle]
pub extern "C" fn vyrn_max_f64(a: f64, b: f64) -> f64 { a.max(b) }

#[no_mangle]
pub extern "C" fn vyrn_floor_f64(x: f64) -> f64 { x.floor() }

#[no_mangle]
pub extern "C" fn vyrn_ceil_f64(x: f64) -> f64 { x.ceil() }

#[no_mangle]
pub extern "C" fn vyrn_round_f64(x: f64) -> f64 { x.round() }

#[no_mangle]
pub extern "C" fn vyrn_clamp_i32(val: i32, lo: i32, hi: i32) -> i32 { val.clamp(lo, hi) }

#[no_mangle]
pub extern "C" fn vyrn_clamp_f64(val: f64, lo: f64, hi: f64) -> f64 { val.clamp(lo, hi) }

// ─── Standard library: String ─────────────────────────────────────────────────

#[no_mangle]
pub unsafe extern "C" fn vyrn_str_len(ptr: i64) -> i32 {
    if ptr == 0 { return 0; }
    CStr::from_ptr(ptr as *const i8).to_bytes().len() as i32
}

#[no_mangle]
pub unsafe extern "C" fn vyrn_str_concat(a: i64, b: i64) -> i64 {
    let sa = if a == 0 { "" } else { CStr::from_ptr(a as *const i8).to_str().unwrap_or("") };
    let sb = if b == 0 { "" } else { CStr::from_ptr(b as *const i8).to_str().unwrap_or("") };
    let s = std::ffi::CString::new(format!("{}{}", sa, sb)).unwrap_or_default();
    s.into_raw() as i64
}

#[no_mangle]
pub unsafe extern "C" fn vyrn_str_eq(a: i64, b: i64) -> i8 {
    let sa = if a == 0 { "" } else { CStr::from_ptr(a as *const i8).to_str().unwrap_or("") };
    let sb = if b == 0 { "" } else { CStr::from_ptr(b as *const i8).to_str().unwrap_or("") };
    if sa == sb { 1 } else { 0 }
}

// ─── Standard library: I/O ───────────────────────────────────────────────────

#[no_mangle]
pub extern "C" fn vyrn_input_line() -> i64 {
    let mut line = String::new();
    let _ = std::io::stdin().read_line(&mut line);
    if line.ends_with('\n') { line.pop(); }
    if line.ends_with('\r') { line.pop(); }
    let s = std::ffi::CString::new(line).unwrap_or_default();
    s.into_raw() as i64
}

// ─── Standard library: Parse ─────────────────────────────────────────────────

#[no_mangle]
pub unsafe extern "C" fn vyrn_parse_i32(ptr: i64) -> i32 {
    if ptr == 0 { return 0; }
    CStr::from_ptr(ptr as *const i8).to_string_lossy().trim().parse::<i32>().unwrap_or(0)
}

#[no_mangle]
pub unsafe extern "C" fn vyrn_parse_f64(ptr: i64) -> f64 {
    if ptr == 0 { return 0.0; }
    CStr::from_ptr(ptr as *const i8).to_string_lossy().trim().parse::<f64>().unwrap_or(0.0)
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
        ("vyrn_gen_ctx_alloc",    vyrn_gen_ctx_alloc    as *const u8),
        ("vyrn_gen_start",        vyrn_gen_start        as *const u8),
        ("vyrn_gen_advance",      vyrn_gen_advance      as *const u8),
        ("vyrn_gen_value_i64",    vyrn_gen_value_i64    as *const u8),
        ("vyrn_yield_i64",        vyrn_yield_i64        as *const u8),
        ("vyrn_gen_end",          vyrn_gen_end          as *const u8),
        // stdlib — math
        ("vyrn_min_i32",          vyrn_min_i32          as *const u8),
        ("vyrn_max_i32",          vyrn_max_i32          as *const u8),
        ("vyrn_min_f64",          vyrn_min_f64          as *const u8),
        ("vyrn_max_f64",          vyrn_max_f64          as *const u8),
        ("vyrn_floor_f64",        vyrn_floor_f64        as *const u8),
        ("vyrn_ceil_f64",         vyrn_ceil_f64         as *const u8),
        ("vyrn_round_f64",        vyrn_round_f64        as *const u8),
        ("vyrn_clamp_i32",        vyrn_clamp_i32        as *const u8),
        ("vyrn_clamp_f64",        vyrn_clamp_f64        as *const u8),
        // stdlib — string
        ("vyrn_str_len",          vyrn_str_len          as *const u8),
        ("vyrn_str_concat",       vyrn_str_concat       as *const u8),
        ("vyrn_str_eq",           vyrn_str_eq           as *const u8),
        // stdlib — I/O
        ("vyrn_input_line",       vyrn_input_line       as *const u8),
        // stdlib — parse
        ("vyrn_parse_i32",        vyrn_parse_i32        as *const u8),
        ("vyrn_parse_f64",        vyrn_parse_f64        as *const u8),
    ]
}
