// Cranelift-based native codegen for Vyrn.
// Supports two modes:
//   JIT  — compile in-process via cranelift-jit, execute directly (zero external deps)
//   AOT  — emit a native object file via cranelift-object

use crate::ast::*;
use cranelift_codegen::entity::EntityRef;
use cranelift_codegen::ir::{
    types, AbiParam, Block, FuncRef, InstBuilder, MemFlags, Signature,
    StackSlotData, StackSlotKind, UserFuncName, Value,
};
use cranelift_codegen::ir::condcodes::{FloatCC, IntCC};
use cranelift_codegen::settings::{self, Configurable};
use cranelift_codegen::Context;
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext, Variable};
use cranelift_module::{FuncId, Linkage, Module};
use std::collections::{HashMap, HashSet};
use std::ffi::CString;

// ─── Type helpers ─────────────────────────────────────────────────────────────

fn cl_type(ty: &Type) -> types::Type {
    match ty {
        Type::I32 | Type::U32 => types::I32,
        Type::I64              => types::I64,
        Type::F32              => types::F32,
        Type::F64              => types::F64,
        // B1 removed in modern Cranelift; bool = I8 (0 or 1)
        Type::Bool             => types::I8,
        // All pointer-like types: 64-bit address
        Type::Str | Type::Array(_) | Type::Custom(_) => types::I64,
        Type::Void => panic!("void has no Cranelift type"),
    }
}

fn elem_size(ty: &Type) -> u32 {
    match ty {
        Type::I32 | Type::U32 | Type::F32 => 4,
        Type::I64 | Type::F64             => 8,
        Type::Bool                        => 1,
        // pointers
        Type::Str | Type::Array(_) | Type::Custom(_) => 8,
        Type::Void => 0,
    }
}

fn is_float(ty: &Type) -> bool {
    matches!(ty, Type::F32 | Type::F64)
}

fn is_unsigned(ty: &Type) -> bool {
    matches!(ty, Type::U32)
}

/// Natural-alignment field offset within a struct.
fn field_offset(fields: &[(String, Type)], name: &str) -> (i32, Type) {
    let mut off = 0u32;
    for (fname, fty) in fields {
        let sz = elem_size(fty);
        // align field to its own size
        if sz > 0 { off = (off + sz - 1) & !(sz - 1); }
        if fname == name {
            return (off as i32, fty.clone());
        }
        off += sz;
    }
    panic!("field '{}' not found in struct", name);
}

fn struct_size(fields: &[(String, Type)]) -> u32 {
    let mut off = 0u32;
    for (_, fty) in fields {
        let sz = elem_size(fty);
        if sz > 0 { off = (off + sz - 1) & !(sz - 1); }
        off += sz;
    }
    off
}

// ─── Backend enum ─────────────────────────────────────────────────────────────

enum BackendModule {
    Jit(cranelift_jit::JITModule),
    Object(cranelift_object::ObjectModule),
}

macro_rules! with_module {
    ($self:expr, $m:ident, $body:expr) => {
        match &mut $self.module {
            BackendModule::Jit($m)    => $body,
            BackendModule::Object($m) => $body,
        }
    };
}

macro_rules! with_module_ref {
    ($self:expr, $m:ident, $body:expr) => {
        match &$self.module {
            BackendModule::Jit($m)    => $body,
            BackendModule::Object($m) => $body,
        }
    };
}

// ─── Variable metadata ────────────────────────────────────────────────────────

#[derive(Clone)]
struct VarEntry {
    var:         Variable,
    ty:          Type,
    arr_size:    Option<usize>,
    arr_elem_ty: Option<Type>,
}

// ─── CodeGen ──────────────────────────────────────────────────────────────────

pub struct CodeGen {
    cl_ctx:      Context,
    module:      BackendModule,

    fn_sigs:     HashMap<String, (Vec<Type>, Type)>,
    struct_meta: HashMap<String, Vec<(String, Type)>>,
    gen_fns:     HashSet<String>,  // Track generator functions

    // Per-function state
    vars:        HashMap<String, VarEntry>,
    var_counter: usize,
    cur_ret_ty:  Type,
    is_main:     bool,

    break_stack: Vec<(Option<String>, Block)>,  // (label, exit_block)
    cont_stack:  Vec<(Option<String>, Block)>,  // (label, header_block)
    cur_ctx_var: Option<Variable>,              // Context pointer in generator body

    // String pool: CStrings kept alive for JIT execution
    string_pool: Vec<CString>,

    // Module-level function ID caches
    func_ids:         HashMap<String, FuncId>,
    runtime_func_ids: HashMap<String, FuncId>,
}

impl CodeGen {
    // ── Constructors ─────────────────────────────────────────────────────────

    pub fn new_jit() -> Self {
        let mut flag_builder = settings::builder();
        flag_builder.set("use_colocated_libcalls", "false").unwrap();
        flag_builder.set("is_pic", "false").unwrap();
        flag_builder.set("opt_level", "speed").unwrap();

        let isa_builder = cranelift_native::builder()
            .expect("host machine is not supported");
        let isa = isa_builder
            .finish(settings::Flags::new(flag_builder))
            .unwrap();

        let mut jit_builder =
            cranelift_jit::JITBuilder::with_isa(isa, cranelift_module::default_libcall_names());

        // Register all runtime symbols so JIT can resolve them
        for (name, ptr) in crate::runtime::all_symbols() {
            jit_builder.symbol(name, ptr);
        }

        let module = cranelift_jit::JITModule::new(jit_builder);

        CodeGen {

            cl_ctx:           Context::new(),
            module:           BackendModule::Jit(module),
            fn_sigs:          HashMap::new(),
            struct_meta:      HashMap::new(),
            gen_fns:          HashSet::new(),
            vars:             HashMap::new(),
            var_counter:      0,
            cur_ret_ty:       Type::Void,
            is_main:          false,
            break_stack:      Vec::new(),
            cont_stack:       Vec::new(),
            cur_ctx_var:      None,
            string_pool:      Vec::new(),
            func_ids:         HashMap::new(),
            runtime_func_ids: HashMap::new(),
        }
    }

    pub fn new_object(name: &str) -> Self {
        let mut flag_builder = settings::builder();
        flag_builder.set("opt_level", "speed").unwrap();

        let isa_builder = cranelift_native::builder()
            .expect("host machine is not supported");
        let isa = isa_builder
            .finish(settings::Flags::new(flag_builder))
            .unwrap();

        let obj_builder = cranelift_object::ObjectBuilder::new(
            isa,
            name,
            cranelift_module::default_libcall_names(),
        )
        .unwrap();
        let module = cranelift_object::ObjectModule::new(obj_builder);

        CodeGen {

            cl_ctx:           Context::new(),
            module:           BackendModule::Object(module),
            fn_sigs:          HashMap::new(),
            struct_meta:      HashMap::new(),
            gen_fns:          HashSet::new(),
            vars:             HashMap::new(),
            var_counter:      0,
            cur_ret_ty:       Type::Void,
            is_main:          false,
            break_stack:      Vec::new(),
            cont_stack:       Vec::new(),
            cur_ctx_var:      None,
            string_pool:      Vec::new(),
            func_ids:         HashMap::new(),
            runtime_func_ids: HashMap::new(),
        }
    }

    // ── Runtime function registration ─────────────────────────────────────────

    fn ensure_runtime_func(&mut self, name: &str, params: &[types::Type],
                            ret: Option<types::Type>) -> FuncId
    {
        if let Some(&id) = self.runtime_func_ids.get(name) {
            return id;
        }
        let call_conv = with_module_ref!(self, m, m.target_config().default_call_conv);
        let mut sig = Signature::new(call_conv);
        for &p in params { sig.params.push(AbiParam::new(p)); }
        if let Some(r) = ret { sig.returns.push(AbiParam::new(r)); }

        let id = with_module!(self, m, m.declare_function(name, Linkage::Import, &sig).unwrap());
        self.runtime_func_ids.insert(name.to_string(), id);
        id
    }

    // ── Top-level generation ──────────────────────────────────────────────────

    pub fn generate(&mut self, program: &Program) -> Result<(), String> {
        // Pass 1: collect metadata and identify generators
        for decl in &program.decls {
            match decl {
                Decl::Fn(f) => {
                    let param_tys = f.params.iter().map(|p| p.ty.clone()).collect();
                    self.fn_sigs.insert(f.name.clone(), (param_tys, f.ret_ty.clone()));
                    if f.is_gen {
                        self.gen_fns.insert(f.name.clone());
                    }
                }
                Decl::Struct(s) => {
                    self.struct_meta.insert(s.name.clone(), s.fields.clone());
                }
            }
        }

        // Pass 2: pre-declare all user functions in the module
        let call_conv = with_module_ref!(self, m, m.target_config().default_call_conv);
        for (name, (param_tys, ret_ty)) in self.fn_sigs.clone() {
            if self.gen_fns.contains(&name) {
                // For generator functions, declare:
                // 1. wrapper: (params...) -> i64 (returns GenHandle)
                // 2. body: (i64) -> void
                let mut wrapper_sig = Signature::new(call_conv);
                for pt in &param_tys {
                    if *pt != Type::Void {
                        wrapper_sig.params.push(AbiParam::new(cl_type(pt)));
                    }
                }
                wrapper_sig.returns.push(AbiParam::new(types::I64));  // GenHandle
                let wrapper_fid = with_module!(self, m,
                    m.declare_function(&name, Linkage::Local, &wrapper_sig).unwrap());
                self.func_ids.insert(name.clone(), wrapper_fid);

                // Body function
                let body_name = format!("__{}_body", name);
                let mut body_sig = Signature::new(call_conv);
                body_sig.params.push(AbiParam::new(types::I64));  // ctx
                let body_fid = with_module!(self, m,
                    m.declare_function(&body_name, Linkage::Local, &body_sig).unwrap());
                self.func_ids.insert(body_name, body_fid);
            } else {
                // Regular function
                let mut sig = Signature::new(call_conv);
                for pt in &param_tys {
                    if *pt != Type::Void {
                        sig.params.push(AbiParam::new(cl_type(pt)));
                    }
                }
                let is_m = name == "main";
                if is_m {
                    sig.returns.push(AbiParam::new(types::I32));
                } else if ret_ty != Type::Void {
                    sig.returns.push(AbiParam::new(cl_type(&ret_ty)));
                }
                let linkage = if is_m { Linkage::Export } else { Linkage::Local };
                let fid = with_module!(self, m, m.declare_function(&name, linkage, &sig).unwrap());
                self.func_ids.insert(name, fid);
            }
        }

        // Pass 3: compile each function
        let fns: Vec<FnDecl> = program.decls.iter().filter_map(|d| {
            if let Decl::Fn(f) = d { Some(f.clone()) } else { None }
        }).collect();

        for f in fns {
            if f.is_gen {
                self.compile_gen_fn(&f);
            } else {
                self.compile_fn(&f);
            }
        }

        // Finalize JIT definitions
        if let BackendModule::Jit(m) = &mut self.module {
            m.finalize_definitions().map_err(|e| e.to_string())?;
        }

        Ok(())
    }

    // ── JIT execution ─────────────────────────────────────────────────────────

    pub fn run_jit(&mut self) -> i32 {
        let func_id = *self.func_ids.get("main").expect("no 'main' function defined");
        let BackendModule::Jit(m) = &self.module else {
            panic!("run_jit called on non-JIT backend");
        };
        let ptr = m.get_finalized_function(func_id);
        let main_fn: fn() -> i32 = unsafe { std::mem::transmute(ptr) };
        main_fn()
    }

    // ── AOT object emission ───────────────────────────────────────────────────

    pub fn finish_object(self) -> Vec<u8> {
        let BackendModule::Object(m) = self.module else {
            panic!("finish_object called on non-Object backend");
        };
        m.finish().emit().unwrap()
    }

    // ── Per-function compilation ──────────────────────────────────────────────

    fn compile_fn(&mut self, f: &FnDecl) {
        let is_main = f.name == "main";
        self.is_main = is_main;
        self.cur_ret_ty = f.ret_ty.clone();
        self.vars.clear();
        self.var_counter = 0;
        self.break_stack.clear();
        self.cont_stack.clear();

        let func_id = self.func_ids[&f.name];

        // Rebuild signature on cl_ctx
        let call_conv = with_module_ref!(self, m, m.target_config().default_call_conv);
        let mut sig = Signature::new(call_conv);
        for p in &f.params {
            if p.ty != Type::Void {
                sig.params.push(AbiParam::new(cl_type(&p.ty)));
            }
        }
        if is_main {
            sig.returns.push(AbiParam::new(types::I32));
        } else if f.ret_ty != Type::Void {
            sig.returns.push(AbiParam::new(cl_type(&f.ret_ty)));
        }
        self.cl_ctx.func.signature = sig;
        self.cl_ctx.func.name = UserFuncName::user(0, func_id.as_u32());

        // Pre-declare runtime and user functions BEFORE creating FunctionBuilder
        // (declare_func_in_func borrows &mut cl_ctx.func, incompatible with builder borrow)
        let fn_refs = self.predeclare_fn_refs(f);

        // Extract func from cl_ctx so the builder doesn't hold a borrow on `self`,
        // which would prevent calling emit_* methods that also need &mut self.
        let mut func = std::mem::replace(
            &mut self.cl_ctx.func,
            cranelift_codegen::ir::Function::new(),
        );
        let mut builder_ctx = FunctionBuilderContext::new();

        // Build function body
        {
            let mut builder = FunctionBuilder::new(&mut func, &mut builder_ctx);

            let entry_block = builder.create_block();
            builder.append_block_params_for_function_params(entry_block);
            builder.switch_to_block(entry_block);
            builder.seal_block(entry_block);

            // Bind parameters to Variables
            for (i, p) in f.params.iter().enumerate() {
                let param_val = builder.block_params(entry_block)[i];
                let var = Variable::new(self.var_counter);
                self.var_counter += 1;
                builder.declare_var(var, cl_type(&p.ty));
                builder.def_var(var, param_val);
                self.vars.insert(p.name.clone(), VarEntry {
                    var, ty: p.ty.clone(),
                    arr_size: None, arr_elem_ty: None,
                });
            }

            let mut terminated = false;
            self.emit_stmts(&f.body.clone(), &mut builder, &fn_refs, &mut terminated);

            // Implicit return
            if !terminated {
                if is_main {
                    let zero = builder.ins().iconst(types::I32, 0);
                    builder.ins().return_(&[zero]);
                } else if f.ret_ty == Type::Void {
                    builder.ins().return_(&[]);
                }
            }

            builder.finalize();
        }

        // Put the built function back before define_function
        self.cl_ctx.func = func;

        with_module!(self, m, m.define_function(func_id, &mut self.cl_ctx).unwrap());
        with_module!(self, m, m.clear_context(&mut self.cl_ctx));
    }

    /// Compile a generator function as two functions:
    /// 1. Wrapper: allocates context, spawns body thread, returns handle
    /// 2. Body: runs in thread, executes generator code with yields
    fn compile_gen_fn(&mut self, f: &FnDecl) {
        // Calculate args size
        let args_size: i64 = f.params.iter().map(|p| elem_size(&p.ty) as i64).sum();

        // Compile wrapper function
        self.compile_gen_wrapper(f, args_size);

        // Compile body function
        self.compile_gen_body(f, args_size);
    }

    fn compile_gen_wrapper(&mut self, f: &FnDecl, args_size: i64) {
        self.vars.clear();
        self.var_counter = 0;
        let wrapper_func_id = self.func_ids[&f.name];

        let call_conv = with_module_ref!(self, m, m.target_config().default_call_conv);
        let mut sig = Signature::new(call_conv);
        for p in &f.params {
            if p.ty != Type::Void {
                sig.params.push(AbiParam::new(cl_type(&p.ty)));
            }
        }
        sig.returns.push(AbiParam::new(types::I64));  // GenHandle

        self.cl_ctx.func.signature = sig;
        self.cl_ctx.func.name = UserFuncName::user(0, wrapper_func_id.as_u32());

        let fn_refs = self.predeclare_fn_refs(f);

        let mut func = std::mem::replace(
            &mut self.cl_ctx.func,
            cranelift_codegen::ir::Function::new(),
        );
        let mut builder_ctx = FunctionBuilderContext::new();

        {
            let mut builder = FunctionBuilder::new(&mut func, &mut builder_ctx);

            let entry_block = builder.create_block();
            builder.append_block_params_for_function_params(entry_block);
            builder.switch_to_block(entry_block);
            builder.seal_block(entry_block);

            // Allocate context: 8 bytes (sender) + args
            let ctx_alloc_fn = fn_refs["vyrn_gen_ctx_alloc"];
            let args_bytes = builder.ins().iconst(types::I64, args_size);
            let ctx_alloc_call = builder.ins().call(ctx_alloc_fn, &[args_bytes]);
            let ctx_ptr = builder.inst_results(ctx_alloc_call)[0];

            // Write arguments into context at offset 8
            let mut offset = 8i64;
            for (i, p) in f.params.iter().enumerate() {
                let param_val = builder.block_params(entry_block)[i];
                let addr = builder.ins().iadd_imm(ctx_ptr, offset);
                let flags = MemFlags::new();
                builder.ins().store(flags, param_val, addr, 0);
                offset += elem_size(&p.ty) as i64;
            }

            // Get function pointer for body
            let body_name = format!("__{}_body", f.name);
            let body_func_ref = fn_refs[&body_name];
            let fn_ptr = builder.ins().func_addr(types::I64, body_func_ref);

            // Call vyrn_gen_start to spawn thread
            let gen_start_fn = fn_refs["vyrn_gen_start"];
            let gen_start_call = builder.ins().call(gen_start_fn, &[fn_ptr, ctx_ptr]);
            let handle = builder.inst_results(gen_start_call)[0];

            builder.ins().return_(&[handle]);
            builder.finalize();
        }

        self.cl_ctx.func = func;
        with_module!(self, m, m.define_function(wrapper_func_id, &mut self.cl_ctx).unwrap());
        with_module!(self, m, m.clear_context(&mut self.cl_ctx));
    }

    fn compile_gen_body(&mut self, f: &FnDecl, _args_size: i64) {
        self.vars.clear();
        self.var_counter = 0;
        self.break_stack.clear();
        self.cont_stack.clear();

        let body_name = format!("__{}_body", f.name);
        let body_func_id = self.func_ids[&body_name];
        self.cur_ret_ty = Type::Void;

        let call_conv = with_module_ref!(self, m, m.target_config().default_call_conv);
        let mut sig = Signature::new(call_conv);
        sig.params.push(AbiParam::new(types::I64));  // ctx

        self.cl_ctx.func.signature = sig;
        self.cl_ctx.func.name = UserFuncName::user(0, body_func_id.as_u32());

        let fn_refs = self.predeclare_fn_refs(f);

        let mut func = std::mem::replace(
            &mut self.cl_ctx.func,
            cranelift_codegen::ir::Function::new(),
        );
        let mut builder_ctx = FunctionBuilderContext::new();

        {
            let mut builder = FunctionBuilder::new(&mut func, &mut builder_ctx);

            let entry_block = builder.create_block();
            builder.append_block_params_for_function_params(entry_block);
            builder.switch_to_block(entry_block);
            builder.seal_block(entry_block);

            // Extract context pointer and store it
            let ctx_param = builder.block_params(entry_block)[0];
            let ctx_var = Variable::new(self.var_counter);
            self.var_counter += 1;
            builder.declare_var(ctx_var, types::I64);
            builder.def_var(ctx_var, ctx_param);
            self.cur_ctx_var = Some(ctx_var);

            // Read parameters from context at offset 8
            let mut offset = 8i64;
            for p in &f.params {
                let addr = builder.ins().iadd_imm(ctx_param, offset);
                let flags = MemFlags::new();
                let val = builder.ins().load(cl_type(&p.ty), flags, addr, 0);
                let var = Variable::new(self.var_counter);
                self.var_counter += 1;
                builder.declare_var(var, cl_type(&p.ty));
                builder.def_var(var, val);
                self.vars.insert(p.name.clone(), VarEntry {
                    var, ty: p.ty.clone(),
                    arr_size: None, arr_elem_ty: None,
                });
                offset += elem_size(&p.ty) as i64;
            }

            // Emit body
            let mut terminated = false;
            self.emit_stmts(&f.body.clone(), &mut builder, &fn_refs, &mut terminated);

            // Call vyrn_gen_end to close the channel
            if !terminated {
                let gen_end_fn = fn_refs["vyrn_gen_end"];
                builder.ins().call(gen_end_fn, &[ctx_param]);
                builder.ins().return_(&[]);
            }

            builder.finalize();
        }

        self.cl_ctx.func = func;
        with_module!(self, m, m.define_function(body_func_id, &mut self.cl_ctx).unwrap());
        with_module!(self, m, m.clear_context(&mut self.cl_ctx));

        self.cur_ctx_var = None;
    }

    /// Pre-declare all needed function references before the FunctionBuilder
    /// is created (because declare_func_in_func needs &mut Function).
    fn predeclare_fn_refs(&mut self, _f: &FnDecl) -> HashMap<String, FuncRef> {
        // Pre-register all runtime functions we might need
        self.ensure_runtime_func("vyrn_println_i32",     &[types::I32],              None);
        self.ensure_runtime_func("vyrn_println_u32",     &[types::I32],              None);
        self.ensure_runtime_func("vyrn_println_i64",     &[types::I64],              None);
        self.ensure_runtime_func("vyrn_println_f64",     &[types::F64],              None);
        self.ensure_runtime_func("vyrn_println_f32",     &[types::F32],              None);
        self.ensure_runtime_func("vyrn_println_bool",    &[types::I8],               None);
        self.ensure_runtime_func("vyrn_println_str",     &[types::I64],              None);
        self.ensure_runtime_func("vyrn_println_newline", &[],                         None);
        self.ensure_runtime_func("vyrn_print_i32",       &[types::I32],              None);
        self.ensure_runtime_func("vyrn_print_u32",       &[types::I32],              None);
        self.ensure_runtime_func("vyrn_print_i64",       &[types::I64],              None);
        self.ensure_runtime_func("vyrn_print_f64",       &[types::F64],              None);
        self.ensure_runtime_func("vyrn_print_f32",       &[types::F32],              None);
        self.ensure_runtime_func("vyrn_print_bool",      &[types::I8],               None);
        self.ensure_runtime_func("vyrn_print_str",       &[types::I64],              None);
        self.ensure_runtime_func("vyrn_abs_i32",         &[types::I32],              Some(types::I32));
        self.ensure_runtime_func("vyrn_abs_f64",         &[types::F64],              Some(types::F64));
        self.ensure_runtime_func("vyrn_sqrt_f64",        &[types::F64],              Some(types::F64));
        self.ensure_runtime_func("vyrn_pow_f64",         &[types::F64, types::F64],  Some(types::F64));
        self.ensure_runtime_func("vyrn_i32_to_string",   &[types::I32],              Some(types::I64));
        self.ensure_runtime_func("vyrn_f64_to_string",   &[types::F64],              Some(types::I64));
        self.ensure_runtime_func("vyrn_bool_to_string",  &[types::I8],               Some(types::I64));
        self.ensure_runtime_func("vyrn_fmod_f64",        &[types::F64, types::F64],  Some(types::F64));

        // Generator functions
        self.ensure_runtime_func("vyrn_gen_ctx_alloc",   &[types::I64],              Some(types::I64));
        self.ensure_runtime_func("vyrn_gen_start",       &[types::I64, types::I64],  Some(types::I64));
        self.ensure_runtime_func("vyrn_gen_advance",     &[types::I64],              Some(types::I8));
        self.ensure_runtime_func("vyrn_gen_value_i64",   &[types::I64],              Some(types::I64));
        self.ensure_runtime_func("vyrn_yield_i64",       &[types::I64, types::I64],  None);
        self.ensure_runtime_func("vyrn_gen_end",         &[types::I64],              None);

        // Now build the FuncRef map (must be done before FunctionBuilder is alive)
        let mut refs: HashMap<String, FuncRef> = HashMap::new();

        // Runtime refs
        for (name, &fid) in &self.runtime_func_ids.clone() {
            let fref = match &mut self.module {
                BackendModule::Jit(m)    => m.declare_func_in_func(fid, &mut self.cl_ctx.func),
                BackendModule::Object(m) => m.declare_func_in_func(fid, &mut self.cl_ctx.func),
            };
            refs.insert(name.clone(), fref);
        }

        // User-defined function refs (for calls to other Vyrn functions, including recursive)
        for (name, &fid) in &self.func_ids.clone() {
            let fref = match &mut self.module {
                BackendModule::Jit(m)    => m.declare_func_in_func(fid, &mut self.cl_ctx.func),
                BackendModule::Object(m) => m.declare_func_in_func(fid, &mut self.cl_ctx.func),
            };
            refs.insert(name.clone(), fref);
        }

        refs
    }

    // ── Statement emission ────────────────────────────────────────────────────

    fn emit_stmts(&mut self, stmts: &[Stmt], builder: &mut FunctionBuilder,
                   fn_refs: &HashMap<String, FuncRef>, terminated: &mut bool)
    {
        for stmt in stmts {
            if *terminated { break; }
            self.emit_stmt(stmt, builder, fn_refs, terminated);
        }
    }

    fn emit_stmt(&mut self, stmt: &Stmt, builder: &mut FunctionBuilder,
                  fn_refs: &HashMap<String, FuncRef>, terminated: &mut bool)
    {
        match stmt {
            Stmt::VarDecl(vd)  => self.emit_var_decl(vd, builder, fn_refs),

            Stmt::Assign { target, value } => {
                let (new_val, vty) = self.emit_expr(value, builder, fn_refs);
                self.emit_assign(target, new_val, &vty, builder, fn_refs);
            }

            Stmt::If { cond, then_block, else_block } => {
                let (cv, _) = self.emit_expr(cond, builder, fn_refs);
                let then_blk  = builder.create_block();
                let else_blk  = builder.create_block();
                let merge_blk = builder.create_block();

                builder.ins().brif(cv, then_blk, &[], else_blk, &[]);

                // Then block
                builder.switch_to_block(then_blk);
                builder.seal_block(then_blk);
                let mut t_term = false;
                self.emit_stmts(then_block, builder, fn_refs, &mut t_term);
                if !t_term { builder.ins().jump(merge_blk, &[]); }

                // Else block
                builder.switch_to_block(else_blk);
                builder.seal_block(else_blk);
                let mut e_term = false;
                if let Some(stmts) = else_block {
                    self.emit_stmts(stmts, builder, fn_refs, &mut e_term);
                }
                if !e_term { builder.ins().jump(merge_blk, &[]); }

                // Merge
                builder.switch_to_block(merge_blk);
                builder.seal_block(merge_blk);
            }

            Stmt::While { label, cond, body } => {
                let header_blk = builder.create_block();
                let body_blk   = builder.create_block();
                let back_edge_blk = builder.create_block();  // NEW: separate back-edge block
                let exit_blk   = builder.create_block();

                builder.ins().jump(header_blk, &[]);

                // Header — DO NOT seal yet (back-edge from body is unknown)
                builder.switch_to_block(header_blk);

                self.break_stack.push((label.clone(), exit_blk));
                self.cont_stack.push((label.clone(), back_edge_blk));  // Continue jumps to back-edge

                let (cv, _) = self.emit_expr(cond, builder, fn_refs);
                builder.ins().brif(cv, body_blk, &[], exit_blk, &[]);

                // Body
                builder.switch_to_block(body_blk);
                builder.seal_block(body_blk);
                let mut b_term = false;
                self.emit_stmts(body, builder, fn_refs, &mut b_term);
                if !b_term { builder.ins().jump(back_edge_blk, &[]); }

                // Back-edge: jump to header
                builder.switch_to_block(back_edge_blk);
                builder.seal_block(back_edge_blk);
                builder.ins().jump(header_blk, &[]);

                // Seal header now that back-edge is known
                builder.seal_block(header_blk);

                // Exit
                builder.switch_to_block(exit_blk);
                builder.seal_block(exit_blk);

                self.break_stack.pop();
                self.cont_stack.pop();
            }

            Stmt::For { label, var, iter, body } => {
                self.emit_for(label, var, iter, body, builder, fn_refs);
            }

            Stmt::Return(expr) => {
                let ret = self.cur_ret_ty.clone();
                let is_main = self.is_main;
                if let Some(e) = expr {
                    let (v, _) = self.emit_expr(e, builder, fn_refs);
                    builder.ins().return_(&[v]);
                } else if is_main {
                    let zero = builder.ins().iconst(types::I32, 0);
                    builder.ins().return_(&[zero]);
                } else if ret == Type::Void {
                    builder.ins().return_(&[]);
                } else {
                    builder.ins().return_(&[]);
                }
                *terminated = true;
            }

            Stmt::Break(target_label) => {
                let target_block = match target_label {
                    Some(label) => {
                        // Search stack for matching label (reverse order = innermost first)
                        self.break_stack.iter().rev()
                            .find(|(l, _)| l.as_ref() == Some(label))
                            .map(|(_, block)| block)
                    }
                    None => {
                        // No label: use innermost loop (top of stack)
                        self.break_stack.last()
                            .map(|(_, block)| block)
                    }
                };

                if let Some(&block) = target_block {
                    builder.ins().jump(block, &[]);
                    *terminated = true;
                }
            }

            Stmt::Continue(target_label) => {
                let target_block = match target_label {
                    Some(label) => {
                        self.cont_stack.iter().rev()
                            .find(|(l, _)| l.as_ref() == Some(label))
                            .map(|(_, block)| block)
                    }
                    None => {
                        self.cont_stack.last()
                            .map(|(_, block)| block)
                    }
                };

                if let Some(&block) = target_block {
                    builder.ins().jump(block, &[]);
                    *terminated = true;
                }
            }

            Stmt::Yield(expr) => {
                // Only valid in generator bodies
                if let Some(ctx_var) = self.cur_ctx_var {
                    let (val, val_ty) = self.emit_expr(expr, builder, fn_refs);
                    // Cast value to i64
                    let val_i64 = if matches!(val_ty, Type::I32) {
                        builder.ins().sextend(types::I64, val)
                    } else {
                        val
                    };
                    let ctx_ptr = builder.use_var(ctx_var);
                    let yield_fn = fn_refs["vyrn_yield_i64"];
                    builder.ins().call(yield_fn, &[ctx_ptr, val_i64]);
                }
            }

            Stmt::Expr(e) => {
                self.emit_expr(e, builder, fn_refs);
            }
        }
    }

    // ── Variable declaration ──────────────────────────────────────────────────

    fn emit_var_decl(&mut self, vd: &VarDecl, builder: &mut FunctionBuilder,
                      fn_refs: &HashMap<String, FuncRef>)
    {
        match &vd.value {
            Expr::Array(elems) => {
                // Infer element type
                let elem_ty = vd.ty.as_ref().and_then(|t| {
                    if let Type::Array(e) = t { Some(*e.clone()) } else { None }
                }).unwrap_or_else(|| {
                    match elems.first() {
                        Some(Expr::Int(_))   => Type::I32,
                        Some(Expr::Float(_)) => Type::F64,
                        Some(Expr::Bool(_))  => Type::Bool,
                        Some(Expr::Str(_))   => Type::Str,
                        _                    => Type::I32,
                    }
                });
                let n = elems.len();
                let esz = elem_size(&elem_ty);
                let total = n as u32 * esz;

                let align_shift = esz.max(1).trailing_zeros() as u8;
                let slot = builder.create_sized_stack_slot(StackSlotData::new(
                    StackSlotKind::ExplicitSlot,
                    total,
                    align_shift,
                ));

                for (i, elem_expr) in elems.iter().enumerate() {
                    let (v, _) = self.emit_expr(elem_expr, builder, fn_refs);
                    builder.ins().stack_store(v, slot, (i as u32 * esz) as i32);
                }

                let addr = builder.ins().stack_addr(types::I64, slot, 0);
                let var = Variable::new(self.var_counter);
                self.var_counter += 1;
                builder.declare_var(var, types::I64);
                builder.def_var(var, addr);

                self.vars.insert(vd.name.clone(), VarEntry {
                    var,
                    ty: Type::Array(Box::new(elem_ty.clone())),
                    arr_size: Some(n),
                    arr_elem_ty: Some(elem_ty),
                });
            }

            _ => {
                let (val, inferred_ty) = self.emit_expr(&vd.value, builder, fn_refs);
                let ty = vd.ty.clone().unwrap_or(inferred_ty);
                let cl = cl_type(&ty);

                let var = Variable::new(self.var_counter);
                self.var_counter += 1;
                builder.declare_var(var, cl);
                builder.def_var(var, val);

                self.vars.insert(vd.name.clone(), VarEntry {
                    var, ty, arr_size: None, arr_elem_ty: None,
                });
            }
        }
    }

    // ── Assignment ────────────────────────────────────────────────────────────

    fn emit_assign(&mut self, target: &Expr, new_val: Value, _vty: &Type,
                    builder: &mut FunctionBuilder, fn_refs: &HashMap<String, FuncRef>)
    {
        match target {
            Expr::Ident(name) => {
                if let Some(entry) = self.vars.get(name).cloned() {
                    builder.def_var(entry.var, new_val);
                }
            }
            Expr::Field { object, field } => {
                if let Expr::Ident(obj_name) = object.as_ref() {
                    if let Some(entry) = self.vars.get(obj_name).cloned() {
                        let struct_name = match &entry.ty {
                            Type::Custom(n) => n.clone(),
                            _ => panic!("field assign on non-struct"),
                        };
                        let fields = self.struct_meta.get(&struct_name).cloned().unwrap_or_default();
                        let (off, fty) = field_offset(&fields, field);
                        let base = builder.use_var(entry.var);
                        let addr = builder.ins().iadd_imm(base, off as i64);
                        builder.ins().store(MemFlags::new(), new_val, addr, 0);
                        let _ = fty;
                    }
                }
            }
            Expr::Index { array, index } => {
                if let Expr::Ident(arr_name) = array.as_ref() {
                    if let Some(entry) = self.vars.get(arr_name).cloned() {
                        let elem_ty = entry.arr_elem_ty.clone().unwrap_or(Type::I32);
                        let esz = elem_size(&elem_ty) as i64;
                        let (idx_val, _) = self.emit_expr(index, builder, fn_refs);
                        let base = builder.use_var(entry.var);
                        let idx64 = self.extend_to_i64(idx_val, &Type::I32, builder);
                        let stride = builder.ins().iconst(types::I64, esz);
                        let offset = builder.ins().imul(idx64, stride);
                        let addr = builder.ins().iadd(base, offset);
                        builder.ins().store(MemFlags::new(), new_val, addr, 0);
                    }
                }
            }
            _ => {}
        }
    }

    // ── For loop ──────────────────────────────────────────────────────────────

    fn emit_for(&mut self, label: &Option<String>, var: &str, iter: &ForIter, body: &[Stmt],
                 builder: &mut FunctionBuilder, fn_refs: &HashMap<String, FuncRef>)
    {
        match iter {
            ForIter::Range { start, end, inclusive } => {
                let loop_var = Variable::new(self.var_counter);
                self.var_counter += 1;
                builder.declare_var(loop_var, types::I32);

                let (sv, _) = self.emit_expr(start, builder, fn_refs);
                builder.def_var(loop_var, sv);

                self.vars.insert(var.to_string(), VarEntry {
                    var: loop_var, ty: Type::I32,
                    arr_size: None, arr_elem_ty: None,
                });

                let header_blk = builder.create_block();
                let body_blk   = builder.create_block();
                let back_edge_blk = builder.create_block();  // NEW: separate back-edge block
                let exit_blk   = builder.create_block();

                builder.ins().jump(header_blk, &[]);

                // Header — don't seal yet
                builder.switch_to_block(header_blk);

                self.break_stack.push((label.clone(), exit_blk));
                self.cont_stack.push((label.clone(), back_edge_blk));  // Continue jumps to back-edge, not header

                let cur = builder.use_var(loop_var);
                let (ev, _) = self.emit_expr(end, builder, fn_refs);
                let cmp = if *inclusive {
                    builder.ins().icmp(IntCC::SignedLessThanOrEqual, cur, ev)
                } else {
                    builder.ins().icmp(IntCC::SignedLessThan, cur, ev)
                };
                builder.ins().brif(cmp, body_blk, &[], exit_blk, &[]);

                // Body
                builder.switch_to_block(body_blk);
                builder.seal_block(body_blk);
                let mut b_term = false;
                self.emit_stmts(body, builder, fn_refs, &mut b_term);

                if !b_term {
                    builder.ins().jump(back_edge_blk, &[]);
                }

                // Back-edge: increment and jump to header
                builder.switch_to_block(back_edge_blk);
                builder.seal_block(back_edge_blk);
                let cur2 = builder.use_var(loop_var);
                let one  = builder.ins().iconst(types::I32, 1);
                let next = builder.ins().iadd(cur2, one);
                builder.def_var(loop_var, next);
                builder.ins().jump(header_blk, &[]);

                builder.seal_block(header_blk);

                builder.switch_to_block(exit_blk);
                builder.seal_block(exit_blk);

                self.vars.remove(var);
                self.break_stack.pop();
                self.cont_stack.pop();
            }

            ForIter::Expr(arr_expr) => {
                // Check if this is a generator function call
                if let Expr::Call { name, args: _ } = arr_expr {
                    if self.gen_fns.contains(name) {
                        // Generator iteration: call gen fn, then advance until done
                        let (handle, _) = self.emit_expr(arr_expr, builder, fn_refs);

                        // Loop variable holds the yielded value
                        let x_var = Variable::new(self.var_counter);
                        self.var_counter += 1;
                        builder.declare_var(x_var, types::I64);  // Values from gen are i64
                        let dummy = builder.ins().iconst(types::I64, 0);
                        builder.def_var(x_var, dummy);
                        self.vars.insert(var.to_string(), VarEntry {
                            var: x_var, ty: Type::I64,
                            arr_size: None, arr_elem_ty: None,
                        });

                        let header_blk = builder.create_block();
                        let body_blk = builder.create_block();
                        let exit_blk = builder.create_block();

                        builder.ins().jump(header_blk, &[]);
                        builder.switch_to_block(header_blk);

                        self.break_stack.push((label.clone(), exit_blk));
                        self.cont_stack.push((label.clone(), header_blk));

                        // Call vyrn_gen_advance
                        let advance_fn = fn_refs["vyrn_gen_advance"];
                        let advance_call = builder.ins().call(advance_fn, &[handle]);
                        let ok = builder.inst_results(advance_call)[0];

                        // Branch on result
                        let one = builder.ins().iconst(types::I8, 1);
                        let cmp = builder.ins().icmp(IntCC::Equal, ok, one);
                        builder.ins().brif(cmp, body_blk, &[], exit_blk, &[]);

                        // Body
                        builder.switch_to_block(body_blk);
                        builder.seal_block(body_blk);

                        // Get value from handle
                        let value_fn = fn_refs["vyrn_gen_value_i64"];
                        let value_call = builder.ins().call(value_fn, &[handle]);
                        let val = builder.inst_results(value_call)[0];
                        builder.def_var(x_var, val);

                        let mut b_term = false;
                        self.emit_stmts(body, builder, fn_refs, &mut b_term);
                        if !b_term {
                            builder.ins().jump(header_blk, &[]);
                        }

                        builder.seal_block(header_blk);

                        builder.switch_to_block(exit_blk);
                        builder.seal_block(exit_blk);

                        self.vars.remove(var);
                        self.break_stack.pop();
                        self.cont_stack.pop();
                        return;
                    }
                }

                // Regular array iteration
                let arr_name = match arr_expr {
                    Expr::Ident(n) => n.clone(),
                    _ => panic!("for-in: expected array variable or generator call"),
                };
                let entry = self.vars.get(&arr_name).cloned()
                    .expect("for-in: unknown array");
                let arr_size = entry.arr_size.expect("for-in: array with unknown size");
                let elem_ty  = entry.arr_elem_ty.clone().unwrap_or(Type::I32);
                let elem_cl  = cl_type(&elem_ty);
                let esz      = elem_size(&elem_ty) as i64;

                // Index variable
                let idx_var = Variable::new(self.var_counter);
                self.var_counter += 1;
                builder.declare_var(idx_var, types::I64);
                let zero = builder.ins().iconst(types::I64, 0);
                builder.def_var(idx_var, zero);

                // Element variable (the loop var the user sees)
                let x_var = Variable::new(self.var_counter);
                self.var_counter += 1;
                builder.declare_var(x_var, elem_cl);
                let dummy = self.default_val(&elem_ty, builder);
                builder.def_var(x_var, dummy);
                self.vars.insert(var.to_string(), VarEntry {
                    var: x_var, ty: elem_ty.clone(),
                    arr_size: None, arr_elem_ty: None,
                });

                let header_blk = builder.create_block();
                let body_blk   = builder.create_block();
                let back_edge_blk = builder.create_block();  // NEW: separate back-edge block
                let exit_blk   = builder.create_block();

                builder.ins().jump(header_blk, &[]);
                builder.switch_to_block(header_blk);
                // Don't seal header yet

                self.break_stack.push((label.clone(), exit_blk));
                self.cont_stack.push((label.clone(), back_edge_blk));  // Continue jumps to back-edge

                let cur_idx = builder.use_var(idx_var);
                let limit = builder.ins().iconst(types::I64, arr_size as i64);
                let cmp = builder.ins().icmp(IntCC::SignedLessThan, cur_idx, limit);
                builder.ins().brif(cmp, body_blk, &[], exit_blk, &[]);

                builder.switch_to_block(body_blk);
                builder.seal_block(body_blk);

                // Load arr[idx] into x
                let base = builder.use_var(entry.var);
                let stride = builder.ins().iconst(types::I64, esz);
                let idx_now = builder.use_var(idx_var);
                let offset = builder.ins().imul(idx_now, stride);
                let addr = builder.ins().iadd(base, offset);
                let elem = builder.ins().load(elem_cl, MemFlags::new(), addr, 0);
                builder.def_var(x_var, elem);

                let mut b_term = false;
                self.emit_stmts(body, builder, fn_refs, &mut b_term);

                if !b_term {
                    builder.ins().jump(back_edge_blk, &[]);
                }

                // Back-edge: increment index and jump to header
                builder.switch_to_block(back_edge_blk);
                builder.seal_block(back_edge_blk);
                let old = builder.use_var(idx_var);
                let one = builder.ins().iconst(types::I64, 1);
                let next = builder.ins().iadd(old, one);
                builder.def_var(idx_var, next);
                builder.ins().jump(header_blk, &[]);

                builder.seal_block(header_blk);

                builder.switch_to_block(exit_blk);
                builder.seal_block(exit_blk);

                self.vars.remove(var);
                self.break_stack.pop();
                self.cont_stack.pop();
            }
        }
    }

    // ── Expression emission ───────────────────────────────────────────────────

    pub fn emit_expr(&mut self, expr: &Expr, builder: &mut FunctionBuilder,
                      fn_refs: &HashMap<String, FuncRef>) -> (Value, Type)
    {
        match expr {
            Expr::Int(n) => {
                let v = builder.ins().iconst(types::I32, *n as i64);
                (v, Type::I32)
            }

            Expr::Float(f) => {
                let v = builder.ins().f64const(*f);
                (v, Type::F64)
            }

            Expr::Bool(b) => {
                let v = builder.ins().iconst(types::I8, if *b { 1 } else { 0 });
                (v, Type::Bool)
            }

            Expr::Str(s) => {
                let cs = CString::new(s.as_bytes()).unwrap_or_default();
                let ptr = cs.as_ptr() as i64;
                self.string_pool.push(cs);
                let v = builder.ins().iconst(types::I64, ptr);
                (v, Type::Str)
            }

            Expr::FStr(content) => {
                // F-strings: print each piece immediately, return empty string ptr
                self.emit_fstr(content, builder, fn_refs);
                let cs = CString::new("").unwrap();
                let ptr = cs.as_ptr() as i64;
                self.string_pool.push(cs);
                let v = builder.ins().iconst(types::I64, ptr);
                (v, Type::Str)
            }

            Expr::Ident(name) => {
                if let Some(entry) = self.vars.get(name).cloned() {
                    let v = builder.use_var(entry.var);
                    (v, entry.ty.clone())
                } else {
                    let v = builder.ins().iconst(types::I32, 0);
                    (v, Type::I32)
                }
            }

            Expr::Binary { left, op, right } => {
                self.emit_binary(left, op, right, builder, fn_refs)
            }

            Expr::Unary { op, expr } => {
                let (v, ty) = self.emit_expr(expr, builder, fn_refs);
                match op {
                    UnaryOp::Neg => {
                        let neg = if is_float(&ty) {
                            builder.ins().fneg(v)
                        } else {
                            let zero = builder.ins().iconst(cl_type(&ty), 0);
                            builder.ins().isub(zero, v)
                        };
                        (neg, ty)
                    }
                    UnaryOp::Not => {
                        let one = builder.ins().iconst(types::I8, 1);
                        let v8  = self.coerce_to_i8(v, &ty, builder);
                        let neg = builder.ins().bxor(v8, one);
                        (neg, Type::Bool)
                    }
                }
            }

            Expr::Call { name, args } => {
                self.emit_call(name, args, builder, fn_refs)
            }

            Expr::Field { object, field } => {
                if let Expr::Ident(obj_name) = object.as_ref() {
                    if let Some(entry) = self.vars.get(obj_name).cloned() {
                        let struct_name = match &entry.ty {
                            Type::Custom(n) => n.clone(),
                            _ => panic!("field access on non-struct"),
                        };
                        let fields = self.struct_meta.get(&struct_name).cloned().unwrap_or_default();
                        let (off, fty) = field_offset(&fields, field);
                        let base = builder.use_var(entry.var);
                        let addr = builder.ins().iadd_imm(base, off as i64);
                        let fty_cl = cl_type(&fty);
                        let v = builder.ins().load(fty_cl, MemFlags::new(), addr, 0);
                        return (v, fty);
                    }
                }
                let v = builder.ins().iconst(types::I32, 0);
                (v, Type::I32)
            }

            Expr::Index { array, index } => {
                if let Expr::Ident(arr_name) = array.as_ref() {
                    if let Some(entry) = self.vars.get(arr_name).cloned() {
                        let elem_ty = entry.arr_elem_ty.clone().unwrap_or(Type::I32);
                        let elem_cl = cl_type(&elem_ty);
                        let esz = elem_size(&elem_ty) as i64;
                        let (idx_val, idx_ty) = self.emit_expr(index, builder, fn_refs);
                        let base = builder.use_var(entry.var);
                        let idx64 = self.extend_to_i64(idx_val, &idx_ty, builder);
                        let stride = builder.ins().iconst(types::I64, esz);
                        let offset = builder.ins().imul(idx64, stride);
                        let addr = builder.ins().iadd(base, offset);
                        let v = builder.ins().load(elem_cl, MemFlags::new(), addr, 0);
                        return (v, elem_ty);
                    }
                }
                let v = builder.ins().iconst(types::I32, 0);
                (v, Type::I32)
            }

            Expr::Array(_) => panic!("Array literal must appear as VarDecl value"),

            Expr::StructLit { name, fields } => {
                self.emit_struct_lit(name, fields, builder, fn_refs)
            }
        }
    }

    // ── Binary operations ─────────────────────────────────────────────────────

    fn emit_binary(&mut self, left: &Expr, op: &BinaryOp, right: &Expr,
                    builder: &mut FunctionBuilder,
                    fn_refs: &HashMap<String, FuncRef>) -> (Value, Type)
    {
        let (lv, lt) = self.emit_expr(left, builder, fn_refs);
        let (rv, _)  = self.emit_expr(right, builder, fn_refs);
        let float = is_float(&lt);
        let unsig = is_unsigned(&lt);

        match op {
            BinaryOp::Add => (if float { builder.ins().fadd(lv, rv) }
                              else { builder.ins().iadd(lv, rv) }, lt),
            BinaryOp::Sub => (if float { builder.ins().fsub(lv, rv) }
                              else { builder.ins().isub(lv, rv) }, lt),
            BinaryOp::Mul => (if float { builder.ins().fmul(lv, rv) }
                              else { builder.ins().imul(lv, rv) }, lt),
            BinaryOp::Div => (if float { builder.ins().fdiv(lv, rv) }
                              else if unsig { builder.ins().udiv(lv, rv) }
                              else { builder.ins().sdiv(lv, rv) }, lt),
            BinaryOp::Mod => if float {
                let fref = fn_refs["vyrn_fmod_f64"];
                let inst = builder.ins().call(fref, &[lv, rv]);
                (builder.inst_results(inst)[0], lt)
            } else if unsig {
                (builder.ins().urem(lv, rv), lt)
            } else {
                (builder.ins().srem(lv, rv), lt)
            },

            BinaryOp::Eq    => {
                let v = if float { builder.ins().fcmp(FloatCC::Equal, lv, rv) }
                        else { builder.ins().icmp(IntCC::Equal, lv, rv) };
                (v, Type::Bool)
            }
            BinaryOp::NotEq => {
                let v = if float { builder.ins().fcmp(FloatCC::NotEqual, lv, rv) }
                        else { builder.ins().icmp(IntCC::NotEqual, lv, rv) };
                (v, Type::Bool)
            }
            BinaryOp::Lt => {
                let v = if float { builder.ins().fcmp(FloatCC::LessThan, lv, rv) }
                        else if unsig { builder.ins().icmp(IntCC::UnsignedLessThan, lv, rv) }
                        else { builder.ins().icmp(IntCC::SignedLessThan, lv, rv) };
                (v, Type::Bool)
            }
            BinaryOp::Le => {
                let v = if float { builder.ins().fcmp(FloatCC::LessThanOrEqual, lv, rv) }
                        else if unsig { builder.ins().icmp(IntCC::UnsignedLessThanOrEqual, lv, rv) }
                        else { builder.ins().icmp(IntCC::SignedLessThanOrEqual, lv, rv) };
                (v, Type::Bool)
            }
            BinaryOp::Gt => {
                let v = if float { builder.ins().fcmp(FloatCC::GreaterThan, lv, rv) }
                        else if unsig { builder.ins().icmp(IntCC::UnsignedGreaterThan, lv, rv) }
                        else { builder.ins().icmp(IntCC::SignedGreaterThan, lv, rv) };
                (v, Type::Bool)
            }
            BinaryOp::Ge => {
                let v = if float { builder.ins().fcmp(FloatCC::GreaterThanOrEqual, lv, rv) }
                        else if unsig { builder.ins().icmp(IntCC::UnsignedGreaterThanOrEqual, lv, rv) }
                        else { builder.ins().icmp(IntCC::SignedGreaterThanOrEqual, lv, rv) };
                (v, Type::Bool)
            }
            BinaryOp::And    => {
                let la = self.coerce_to_i8(lv, &lt, builder);
                let ra = self.coerce_to_i8(rv, &lt, builder);
                (builder.ins().band(la, ra), Type::Bool)
            }
            BinaryOp::Or     => {
                let la = self.coerce_to_i8(lv, &lt, builder);
                let ra = self.coerce_to_i8(rv, &lt, builder);
                (builder.ins().bor(la, ra), Type::Bool)
            }
            BinaryOp::BitAnd => (builder.ins().band(lv, rv), lt),
            BinaryOp::BitOr  => (builder.ins().bor(lv, rv),  lt),
            BinaryOp::BitXor => (builder.ins().bxor(lv, rv), lt),
            BinaryOp::Shl    => (builder.ins().ishl(lv, rv), lt),
            BinaryOp::Shr    => (if unsig { builder.ins().ushr(lv, rv) }
                                 else { builder.ins().sshr(lv, rv) }, lt),
        }
    }

    // ── Function calls ────────────────────────────────────────────────────────

    fn emit_call(&mut self, name: &str, args: &[Expr],
                  builder: &mut FunctionBuilder,
                  fn_refs: &HashMap<String, FuncRef>) -> (Value, Type)
    {
        match name {
            "println" => {
                if args.is_empty() {
                    let fref = fn_refs["vyrn_println_newline"];
                    builder.ins().call(fref, &[]);
                } else {
                    for (i, arg) in args.iter().enumerate() {
                        let newline = i == args.len() - 1;
                        self.emit_print_one(arg, newline, builder, fn_refs);
                    }
                }
                let v = builder.ins().iconst(types::I32, 0);
                (v, Type::Void)
            }

            "print" => {
                for arg in args {
                    self.emit_print_one(arg, false, builder, fn_refs);
                }
                let v = builder.ins().iconst(types::I32, 0);
                (v, Type::Void)
            }

            "len" if args.len() == 1 => {
                if let Expr::Ident(arr_name) = &args[0] {
                    if let Some(entry) = self.vars.get(arr_name).cloned() {
                        if let Some(sz) = entry.arr_size {
                            let v = builder.ins().iconst(types::I32, sz as i64);
                            return (v, Type::I32);
                        }
                    }
                }
                let v = builder.ins().iconst(types::I32, 0);
                (v, Type::I32)
            }

            "abs" if args.len() == 1 => {
                let (v, ty) = self.emit_expr(&args[0], builder, fn_refs);
                if is_float(&ty) {
                    let fref = fn_refs["vyrn_abs_f64"];
                    let dv = self.coerce_to_f64(v, &ty, builder);
                    let call = builder.ins().call(fref, &[dv]);
                    let r = builder.inst_results(call)[0];
                    (r, Type::F64)
                } else {
                    let fref = fn_refs["vyrn_abs_i32"];
                    let call = builder.ins().call(fref, &[v]);
                    let r = builder.inst_results(call)[0];
                    (r, Type::I32)
                }
            }

            "sqrt" if args.len() == 1 => {
                let (v, ty) = self.emit_expr(&args[0], builder, fn_refs);
                let fref = fn_refs["vyrn_sqrt_f64"];
                let dv = self.coerce_to_f64(v, &ty, builder);
                let call = builder.ins().call(fref, &[dv]);
                let r = builder.inst_results(call)[0];
                (r, Type::F64)
            }

            "pow" if args.len() == 2 => {
                let (v0, ty0) = self.emit_expr(&args[0], builder, fn_refs);
                let (v1, ty1) = self.emit_expr(&args[1], builder, fn_refs);
                let fref = fn_refs["vyrn_pow_f64"];
                let d0 = self.coerce_to_f64(v0, &ty0, builder);
                let d1 = self.coerce_to_f64(v1, &ty1, builder);
                let call = builder.ins().call(fref, &[d0, d1]);
                let r = builder.inst_results(call)[0];
                (r, Type::F64)
            }

            "to_string" if args.len() == 1 => {
                let (v, ty) = self.emit_expr(&args[0], builder, fn_refs);
                let sym = match &ty {
                    Type::F32 | Type::F64 => "vyrn_f64_to_string",
                    Type::Bool            => "vyrn_bool_to_string",
                    _                     => "vyrn_i32_to_string",
                };
                let fref = fn_refs[sym];
                let arg = match &ty {
                    Type::F32 => self.coerce_to_f64(v, &ty, builder),
                    Type::F64 => v,
                    Type::Bool => self.coerce_to_i8(v, &ty, builder),
                    _         => v,
                };
                let call = builder.ins().call(fref, &[arg]);
                let r = builder.inst_results(call)[0];
                (r, Type::Str)
            }

            _ => {
                // User-defined function
                let sig = self.fn_sigs.get(name).cloned();
                let ret_ty = sig.as_ref().map(|(_, r)| r.clone()).unwrap_or(Type::Void);
                let param_tys: Vec<Type> = sig.as_ref()
                    .map(|(p, _)| p.clone())
                    .unwrap_or_default();

                let mut call_args: Vec<Value> = Vec::new();
                for (i, arg) in args.iter().enumerate() {
                    let (mut v, ty) = self.emit_expr(arg, builder, fn_refs);
                    // Coerce if parameter type is known
                    if i < param_tys.len() {
                        v = self.coerce_value(v, &ty, &param_tys[i], builder);
                    }
                    call_args.push(v);
                }

                if let Some(&fref) = fn_refs.get(name) {
                    if ret_ty == Type::Void {
                        builder.ins().call(fref, &call_args);
                        let v = builder.ins().iconst(types::I32, 0);
                        (v, Type::Void)
                    } else {
                        let call = builder.ins().call(fref, &call_args);
                        let r = builder.inst_results(call)[0];
                        (r, ret_ty)
                    }
                } else {
                    let v = builder.ins().iconst(types::I32, 0);
                    (v, Type::Void)
                }
            }
        }
    }

    fn emit_print_one(&mut self, arg: &Expr, newline: bool,
                       builder: &mut FunctionBuilder,
                       fn_refs: &HashMap<String, FuncRef>)
    {
        let (v, ty) = self.emit_expr(arg, builder, fn_refs);
        let sym = match (&ty, newline) {
            (Type::I32,  true)  => "vyrn_println_i32",
            (Type::I32,  false) => "vyrn_print_i32",
            (Type::U32,  true)  => "vyrn_println_u32",
            (Type::U32,  false) => "vyrn_print_u32",
            (Type::I64,  true)  => "vyrn_println_i64",
            (Type::I64,  false) => "vyrn_print_i64",
            (Type::F64,  true)  => "vyrn_println_f64",
            (Type::F64,  false) => "vyrn_print_f64",
            (Type::F32,  true)  => "vyrn_println_f32",
            (Type::F32,  false) => "vyrn_print_f32",
            (Type::Bool, true)  => "vyrn_println_bool",
            (Type::Bool, false) => "vyrn_print_bool",
            (Type::Str,  true)  => "vyrn_println_str",
            (Type::Str,  false) => "vyrn_print_str",
            (_,          true)  => "vyrn_println_i32",
            (_,          false) => "vyrn_print_i32",
        };
        let fref = fn_refs[sym];

        // Coerce the value to the function's expected type
        let arg_val = match &ty {
            Type::Bool => self.coerce_to_i8(v, &ty, builder),
            Type::Str | Type::Array(_) | Type::Custom(_) => v,  // already I64
            Type::F32 => {
                // float variants use their natural type
                v
            }
            _ => v,
        };
        builder.ins().call(fref, &[arg_val]);
    }

    // ── Struct literal ────────────────────────────────────────────────────────

    fn emit_struct_lit(&mut self, name: &str, fields: &[(String, Expr)],
                        builder: &mut FunctionBuilder,
                        fn_refs: &HashMap<String, FuncRef>) -> (Value, Type)
    {
        let meta = self.struct_meta.get(name).cloned().unwrap_or_default();
        let total = struct_size(&meta);
        let align_shift = meta.iter().map(|(_, t)| elem_size(t)).max().unwrap_or(4).max(1).trailing_zeros() as u8;
        let slot = builder.create_sized_stack_slot(StackSlotData::new(
            StackSlotKind::ExplicitSlot,
            total,
            align_shift,
        ));

        for (fname, fexpr) in fields {
            let (v, vty) = self.emit_expr(fexpr, builder, fn_refs);
            let (off, fty) = field_offset(&meta, fname);
            let cv = self.coerce_value(v, &vty, &fty, builder);
            builder.ins().stack_store(cv, slot, off);
        }

        let ptr = builder.ins().stack_addr(types::I64, slot, 0);
        (ptr, Type::Custom(name.to_string()))
    }

    // ── F-string ──────────────────────────────────────────────────────────────

    fn emit_fstr(&mut self, content: &str, builder: &mut FunctionBuilder,
                  fn_refs: &HashMap<String, FuncRef>)
    {
        let chars: Vec<char> = content.chars().collect();
        let mut i = 0;
        let mut literal = String::new();

        while i < chars.len() {
            if chars[i] == '{' {
                // Flush literal segment
                if !literal.is_empty() {
                    let cs = CString::new(literal.as_bytes()).unwrap_or_default();
                    let ptr = cs.as_ptr() as i64;
                    self.string_pool.push(cs);
                    let v = builder.ins().iconst(types::I64, ptr);
                    let fref = fn_refs["vyrn_print_str"];
                    builder.ins().call(fref, &[v]);
                    literal.clear();
                }
                i += 1;
                let mut expr_src = String::new();
                while i < chars.len() && chars[i] != '}' {
                    expr_src.push(chars[i]);
                    i += 1;
                }
                i += 1; // skip '}'

                let varname = expr_src.trim();
                if let Some(entry) = self.vars.get(varname).cloned() {
                    let v = builder.use_var(entry.var);
                    let ty = entry.ty.clone();
                    self.emit_print_one_val(v, &ty, false, builder, fn_refs);
                } else if let Some(dot) = varname.find('.') {
                    // Handle obj.field access (e.g. {person.name})
                    let obj_name = &varname[..dot];
                    let field_name = &varname[dot + 1..];
                    if let Some(entry) = self.vars.get(obj_name).cloned() {
                        let struct_name = match &entry.ty {
                            Type::Custom(n) => n.clone(),
                            _ => String::new(),
                        };
                        if !struct_name.is_empty() {
                            let fields = self.struct_meta.get(&struct_name).cloned().unwrap_or_default();
                            let (off, fty) = field_offset(&fields, field_name);
                            let base = builder.use_var(entry.var);
                            let addr = builder.ins().iadd_imm(base, off as i64);
                            let v = builder.ins().load(cl_type(&fty), MemFlags::new(), addr, 0);
                            self.emit_print_one_val(v, &fty, false, builder, fn_refs);
                        }
                    }
                } else if let Ok(n) = varname.parse::<i64>() {
                    let v = builder.ins().iconst(types::I32, n);
                    self.emit_print_one_val(v, &Type::I32, false, builder, fn_refs);
                }
            } else {
                literal.push(chars[i]);
                i += 1;
            }
        }
        // Flush remaining literal
        if !literal.is_empty() {
            let cs = CString::new(literal.as_bytes()).unwrap_or_default();
            let ptr = cs.as_ptr() as i64;
            self.string_pool.push(cs);
            let v = builder.ins().iconst(types::I64, ptr);
            let fref = fn_refs["vyrn_print_str"];
            builder.ins().call(fref, &[v]);
        }
    }

    fn emit_print_one_val(&mut self, v: Value, ty: &Type, newline: bool,
                            builder: &mut FunctionBuilder,
                            fn_refs: &HashMap<String, FuncRef>)
    {
        let sym = match (ty, newline) {
            (Type::I32,  true)  => "vyrn_println_i32",
            (Type::I32,  false) => "vyrn_print_i32",
            (Type::U32,  true)  => "vyrn_println_u32",
            (Type::U32,  false) => "vyrn_print_u32",
            (Type::I64,  true)  => "vyrn_println_i64",
            (Type::I64,  false) => "vyrn_print_i64",
            (Type::F64,  true)  => "vyrn_println_f64",
            (Type::F64,  false) => "vyrn_print_f64",
            (Type::F32,  true)  => "vyrn_println_f32",
            (Type::F32,  false) => "vyrn_print_f32",
            (Type::Bool, true)  => "vyrn_println_bool",
            (Type::Bool, false) => "vyrn_print_bool",
            (Type::Str,  true)  => "vyrn_println_str",
            (Type::Str,  false) => "vyrn_print_str",
            (_,          true)  => "vyrn_println_i32",
            (_,          false) => "vyrn_print_i32",
        };
        let fref = fn_refs[sym];
        let av = match ty {
            Type::Bool => self.coerce_to_i8(v, ty, builder),
            _ => v,
        };
        builder.ins().call(fref, &[av]);
    }

    // ── Coercion helpers ──────────────────────────────────────────────────────

    fn extend_to_i64(&self, v: Value, ty: &Type, builder: &mut FunctionBuilder) -> Value {
        match ty {
            Type::I64 => v,
            Type::I32 | Type::U32 => builder.ins().uextend(types::I64, v),
            Type::Bool => builder.ins().uextend(types::I64, v),
            _ => v,
        }
    }

    fn coerce_to_f64(&self, v: Value, ty: &Type, builder: &mut FunctionBuilder) -> Value {
        match ty {
            Type::F64 => v,
            Type::F32 => builder.ins().fpromote(types::F64, v),
            Type::I32 | Type::U32 => builder.ins().fcvt_from_sint(types::F64, v),
            Type::I64 => builder.ins().fcvt_from_sint(types::F64, v),
            _ => v,
        }
    }

    fn coerce_to_i8(&self, v: Value, ty: &Type, builder: &mut FunctionBuilder) -> Value {
        match ty {
            Type::Bool => v,
            Type::I32 | Type::U32 => builder.ins().ireduce(types::I8, v),
            _ => v,
        }
    }

    fn coerce_value(&self, v: Value, from: &Type, to: &Type,
                    builder: &mut FunctionBuilder) -> Value
    {
        if from == to { return v; }
        match (from, to) {
            (Type::I32, Type::I64) => builder.ins().sextend(types::I64, v),
            (Type::I64, Type::I32) => builder.ins().ireduce(types::I32, v),
            (Type::F32, Type::F64) => builder.ins().fpromote(types::F64, v),
            (Type::F64, Type::F32) => builder.ins().fdemote(types::F32, v),
            (Type::I32, Type::F64) => builder.ins().fcvt_from_sint(types::F64, v),
            (Type::I32, Type::F32) => builder.ins().fcvt_from_sint(types::F32, v),
            _ => v,
        }
    }

    fn default_val(&self, ty: &Type, builder: &mut FunctionBuilder) -> Value {
        match ty {
            Type::F32 => builder.ins().f32const(0.0),
            Type::F64 => builder.ins().f64const(0.0),
            Type::I64 => builder.ins().iconst(types::I64, 0),
            Type::Str | Type::Array(_) | Type::Custom(_) => builder.ins().iconst(types::I64, 0),
            _ => builder.ins().iconst(cl_type(ty), 0),
        }
    }
}
