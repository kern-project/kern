use inkwell::AddressSpace;
use inkwell::builder::Builder;
use inkwell::context::Context as LlvmContext;
use inkwell::module::Module as LlvmModule;
use inkwell::targets::{CodeModel, FileType, InitializationConfig, RelocMode, Target};
use inkwell::types::{BasicType, BasicTypeEnum, StructType};
use inkwell::values::{BasicValueEnum, FunctionValue, GlobalValue, PointerValue};
use std::collections::HashMap;

use crate::driver::config::OptLevel;
use crate::mast::ast::*;
use crate::parser::ast;
use crate::sema::def::Def;
use crate::sema::ty::{PrimitiveType, TypeId, TypeKind, TypeRegistry};

pub struct CodeGenerator<'ctx, 'a> {
    pub context: &'ctx LlvmContext,
    pub builder: Builder<'ctx>,
    pub module: LlvmModule<'ctx>,

    // 前端类型注册表与原始定义，用于反查结构体名
    pub type_registry: &'a TypeRegistry,
    pub ctx_defs: &'a Vec<Def>,
    pub ctx_resolve: &'a dyn Fn(crate::utils::SymbolId) -> &'a str,

    pub structs: HashMap<MonoId, StructType<'ctx>>,
    pub union_ids: std::collections::HashSet<MonoId>,
    pub globals: HashMap<MonoId, GlobalValue<'ctx>>,
    pub functions: HashMap<MonoId, FunctionValue<'ctx>>,

    pub locals: HashMap<crate::utils::SymbolId, PointerValue<'ctx>>,
    pub loop_targets: Vec<(
        inkwell::basic_block::BasicBlock<'ctx>,
        inkwell::basic_block::BasicBlock<'ctx>,
    )>,
    pub asm_dialect: inkwell::InlineAsmDialect,
}

impl<'ctx, 'a> CodeGenerator<'ctx, 'a> {
    pub fn new(
        context: &'ctx LlvmContext,
        module_name: &str,
        type_registry: &'a TypeRegistry,
        ctx_defs: &'a Vec<Def>,
        ctx_resolve: &'a dyn Fn(crate::utils::SymbolId) -> &'a str,
    ) -> Self {
        Self {
            context,
            builder: context.create_builder(),
            module: context.create_module(module_name),
            type_registry,
            ctx_defs,
            ctx_resolve,
            structs: HashMap::new(),
            union_ids: std::collections::HashSet::new(),
            globals: HashMap::new(),
            functions: HashMap::new(),
            locals: HashMap::new(),
            loop_targets: Vec::new(),
            asm_dialect: inkwell::InlineAsmDialect::Intel,
        }
    }

    pub fn compile(&mut self, module: &MastModule) {
        self.declare_structs(&module.structs);
        self.declare_globals(&module.globals);
        self.declare_functions(&module.functions);

        for global in &module.globals {
            self.compile_global(global);
        }

        for function in &module.functions {
            if function.body.is_some() {
                self.compile_function(function);
            }
        }
    }

    fn compile_global(&mut self, global: &MastGlobal) {
        if global.is_extern {
            return;
        }
        let global_val = *self
            .globals
            .get(&global.id)
            .expect("Global should be declared");

        if let Some(init) = &global.init {
            let const_val: inkwell::values::BasicValueEnum<'ctx> = match &init.kind {
                MastExprKind::Integer(val) => {
                    let int_type = self.get_llvm_type(init.ty).into_int_type();
                    int_type.const_int(*val as u64, false).into()
                }
                MastExprKind::Float(val) => {
                    let float_type = self.get_llvm_type(init.ty).into_float_type();
                    float_type.const_float(*val).into()
                }
                MastExprKind::Bool(val) => self
                    .context
                    .bool_type()
                    .const_int(if *val { 1 } else { 0 }, false)
                    .into(),
                MastExprKind::StringLiteral(s) => {
                    let bytes = self.context.const_string(s.as_bytes(), false);
                    bytes.into()
                }
                MastExprKind::ArrayInit(elems) => {
                    let mut ptr_vals = Vec::new();
                    for e in elems {
                        if let MastExprKind::FuncRef(mono_id) = e.kind {
                            let func_val = self.functions.get(&mono_id).unwrap();
                            ptr_vals.push(func_val.as_global_value().as_pointer_value());
                        } else {
                            // 如果不是函数引用，塞入 Null 指针
                            ptr_vals
                                .push(self.context.ptr_type(AddressSpace::default()).const_null());
                        }
                    }
                    let ptr_ty = self.context.ptr_type(AddressSpace::default());
                    ptr_ty.const_array(&ptr_vals).into()
                }
                _ => self.get_llvm_type(global.ty).const_zero(),
            };

            global_val.set_initializer(&const_val);
        } else if !global.is_extern {
            let llvm_ty = self.get_llvm_type(global.ty);
            global_val.set_initializer(&llvm_ty.const_zero());
        }
    }

    pub fn print_ir(&self) {
        self.module.print_to_stderr();
    }

    // ==========================================
    //          Type Translation
    // ==========================================

    pub fn get_llvm_type(&self, ty: TypeId) -> BasicTypeEnum<'ctx> {
        let mut norm = self.type_registry.normalize(ty);
        loop {
            match self.type_registry.get(norm) {
                TypeKind::Mut(inner) => norm = *inner,
                _ => break,
            }
        }
        match self.type_registry.get(norm) {
            TypeKind::Primitive(p) => match p {
                PrimitiveType::I8 | PrimitiveType::U8 => self.context.i8_type().into(),
                PrimitiveType::I16 | PrimitiveType::U16 => self.context.i16_type().into(),
                PrimitiveType::I32 | PrimitiveType::U32 => self.context.i32_type().into(),
                PrimitiveType::I64
                | PrimitiveType::U64
                | PrimitiveType::ISize
                | PrimitiveType::USize => self.context.i64_type().into(),
                PrimitiveType::I128 | PrimitiveType::U128 => self.context.i128_type().into(),
                PrimitiveType::F32 => self.context.f32_type().into(),
                PrimitiveType::F64 => self.context.f64_type().into(),
                PrimitiveType::Bool => self.context.bool_type().into(),
                PrimitiveType::Str => self.context.ptr_type(AddressSpace::default()).into(),
                PrimitiveType::Void | PrimitiveType::Never => self.context.i8_type().into(),
            },
            TypeKind::Pointer(_)
            | TypeKind::VolatilePtr(_)
            | TypeKind::Function { .. }
            | TypeKind::FnDef(..) => self.context.ptr_type(AddressSpace::default()).into(),
            TypeKind::Array { elem, len } => {
                let elem_ty = self.get_llvm_type(*elem);
                elem_ty.array_type(*len as u32).into()
            }
            TypeKind::TraitObject(_, _) | TypeKind::Slice(_) => {
                let ptr_ty = self.context.ptr_type(AddressSpace::default());
                let len_ty = self.context.i64_type();
                self.context
                    .struct_type(&[ptr_ty.into(), len_ty.into()], false)
                    .into()
            }
            TypeKind::Def(def_id, args) | TypeKind::Adt(def_id, args) => {
                // 如果 def_id 越界了，说明是在 Lowering 时生成的“伪 Union”
                // 它的 def_id.0 其实就是 MonoId.0
                if def_id.0 as usize >= self.ctx_defs.len() {
                    return self
                        .structs
                        .get(&MonoId(def_id.0))
                        .unwrap()
                        .as_basic_type_enum();
                }

                let def = &self.ctx_defs[def_id.0 as usize];
                let mut mangled_name = (self.ctx_resolve)(def.name().unwrap()).to_string();
                for arg in args {
                    mangled_name.push_str(&format!("_{}", arg.0));
                }

                if let Some(struct_ty) = self.module.get_struct_type(&mangled_name) {
                    struct_ty.into()
                } else {
                    self.context.i8_type().into()
                }
            }
            TypeKind::AdtPayload(def_id, args) => {
                let def = &self.ctx_defs[def_id.0 as usize];
                let mut mangled_name = (self.ctx_resolve)(def.name().unwrap()).to_string();
                for arg in args {
                    mangled_name.push_str(&format!("_{}", arg.0));
                }
                mangled_name.push_str("_payload"); // 重点：加上 _payload 后缀寻找 Union 表

                if let Some(struct_ty) = self.module.get_struct_type(&mangled_name) {
                    struct_ty.into()
                } else {
                    self.context.i8_type().into()
                }
            }
            _ => unreachable!(
                "Frontend failed to resolve type! TypeId: {:?}, Kind: {:?}",
                norm,
                self.type_registry.get(norm)
            ),
        }
    }

    /// 辅助函数：绕过 Inkwell BasicTypeEnum 没有统一 get_undef() 的限制
    fn get_undef_val(&self, llvm_ty: BasicTypeEnum<'ctx>) -> BasicValueEnum<'ctx> {
        match llvm_ty {
            BasicTypeEnum::ArrayType(t) => t.get_undef().into(),
            BasicTypeEnum::FloatType(t) => t.get_undef().into(),
            BasicTypeEnum::IntType(t) => t.get_undef().into(),
            BasicTypeEnum::PointerType(t) => t.get_undef().into(),
            BasicTypeEnum::StructType(t) => t.get_undef().into(),
            BasicTypeEnum::VectorType(t) => t.get_undef().into(),
            BasicTypeEnum::ScalableVectorType(t) => t.get_undef().into(),
        }
    }

    /// 判断当前类型是否在物理上是 Void
    fn is_void_type(&self, ty: TypeId) -> bool {
        let mut norm = self.type_registry.normalize(ty);
        loop {
            match self.type_registry.get(norm) {
                TypeKind::Mut(inner) => norm = *inner,
                _ => break,
            }
        }
        matches!(
            self.type_registry.get(norm),
            TypeKind::Primitive(PrimitiveType::Void)
        )
    }

    /// 在当前函数的 entry block 首部安全地分配局部变量内存。
    /// 这样可以避免在循环内部调用 alloca 导致的栈溢出。
    fn create_entry_block_alloca(
        &self,
        llvm_ty: BasicTypeEnum<'ctx>,
        name: &str,
    ) -> inkwell::values::PointerValue<'ctx> {
        let builder = self.context.create_builder();

        // 获取当前正在构建的函数
        let current_block = self.builder.get_insert_block().unwrap();
        let current_func = current_block.get_parent().unwrap();

        // 获取该函数的 entry block
        let entry_block = current_func.get_first_basic_block().unwrap();

        // 将插入点设置在 entry block 的第一条指令之前
        match entry_block.get_first_instruction() {
            Some(first_instr) => builder.position_before(&first_instr),
            None => builder.position_at_end(entry_block),
        }

        builder.build_alloca(llvm_ty, name).unwrap()
    }

    // ==========================================
    //          Phase 1: Declarations
    // ==========================================

    fn declare_structs(&mut self, structs: &[MastStruct]) {
        for s in structs {
            let llvm_struct = self.context.opaque_struct_type(&s.name);
            self.structs.insert(s.id, llvm_struct);
            if s.is_union {
                self.union_ids.insert(s.id);
            }
        }

        for s in structs {
            let llvm_struct = self.structs.get(&s.id).unwrap();

            // 解析 #[packed] 属性
            let is_packed = s.attributes.iter().any(|attr| {
                matches!(attr, ast::MetaItem::Marker(id) if (self.ctx_resolve)(*id) == "packed")
            });

            if s.is_union {
                let target_ty = self.get_llvm_type(s.fields[s.largest_field_idx].ty);
                llvm_struct.set_body(&[target_ty], is_packed);
                let mut field_types = Vec::new();
                for field in &s.fields {
                    field_types.push(self.get_llvm_type(field.ty));
                }
                llvm_struct.set_body(&field_types, is_packed);
            }
        }
    }

    fn declare_globals(&mut self, globals: &[MastGlobal]) {
        for g in globals {
            let mut llvm_symbol_name = g.name.clone();
            let mut link_section = None;
            let mut align_bytes = None;

            for attr in &g.attributes {
                match attr {
                    ast::MetaItem::Call(id, expr) => {
                        let name_str = (self.ctx_resolve)(*id);
                        if name_str == "export_name" {
                            if let ast::ExprKind::String(s) = &expr.kind {
                                llvm_symbol_name = s.clone();
                            }
                        } else if name_str == "link_section" {
                            if let ast::ExprKind::String(s) = &expr.kind {
                                link_section = Some(s.clone());
                            }
                        } else if name_str == "align" {
                            if let ast::ExprKind::Integer(val) = &expr.kind {
                                align_bytes = Some(*val as u32);
                            }
                        }
                    }
                    _ => {}
                }
            }

            if g.is_extern {
                if let Some(existing_global) = self.module.get_global(&llvm_symbol_name) {
                    self.globals.insert(g.id, existing_global);
                    continue;
                }
            }

            let llvm_ty = self.get_llvm_type(g.ty);
            let global_val = self.module.add_global(llvm_ty, None, &llvm_symbol_name);
            global_val.set_constant(!g.is_mut);

            if g.is_extern {
                global_val.set_linkage(inkwell::module::Linkage::External);
            } else {
                global_val.set_initializer(&llvm_ty.const_zero());
            }

            if let Some(sec) = link_section {
                global_val.set_section(Some(&sec));
            }
            if let Some(align) = align_bytes {
                global_val.set_alignment(align);
            }

            self.globals.insert(g.id, global_val);
        }
    }

    fn declare_functions(&mut self, functions: &[MastFunction]) {
        for f in functions {
            let ret_ty = self.get_llvm_type(f.ret_ty);

            let mut param_types = Vec::new();
            for p in &f.params {
                param_types.push(self.get_llvm_type(p.ty).into());
            }

            let fn_type = if f.ret_ty == TypeId::VOID {
                self.context
                    .void_type()
                    .fn_type(&param_types, f.is_variadic)
            } else {
                match ret_ty {
                    BasicTypeEnum::IntType(i) => i.fn_type(&param_types, f.is_variadic),
                    BasicTypeEnum::FloatType(fl) => fl.fn_type(&param_types, f.is_variadic),
                    BasicTypeEnum::PointerType(p) => p.fn_type(&param_types, f.is_variadic),
                    BasicTypeEnum::StructType(s) => s.fn_type(&param_types, f.is_variadic),
                    BasicTypeEnum::ArrayType(a) => a.fn_type(&param_types, f.is_variadic),
                    _ => unreachable!("Invalid return type"),
                }
            };

            let mut llvm_symbol_name = f.name.clone();
            let mut is_cold = false;
            let mut is_naked = false;
            let mut link_section = None;

            for attr in &f.attributes {
                match attr {
                    ast::MetaItem::Call(id, expr) => {
                        let name_str = (self.ctx_resolve)(*id);
                        if name_str == "export_name" {
                            if let ast::ExprKind::String(s) = &expr.kind {
                                llvm_symbol_name = s.clone();
                            }
                        } else if name_str == "link_section" {
                            if let ast::ExprKind::String(s) = &expr.kind {
                                link_section = Some(s.clone());
                            }
                        }
                    }
                    ast::MetaItem::Marker(id) => {
                        let name_str = (self.ctx_resolve)(*id);
                        if name_str == "cold" {
                            is_cold = true;
                        } else if name_str == "naked" {
                            is_naked = true;
                        }
                    }
                }
            }

            if f.is_extern {
                if let Some(existing_func) = self.module.get_function(&llvm_symbol_name) {
                    self.functions.insert(f.id, existing_func);
                    continue;
                }
            }

            let llvm_func = self.module.add_function(&llvm_symbol_name, fn_type, None);

            if is_cold {
                let kind_id = inkwell::attributes::Attribute::get_named_enum_kind_id("cold");
                let cold_attr = self.context.create_enum_attribute(kind_id, 0);
                llvm_func.add_attribute(inkwell::attributes::AttributeLoc::Function, cold_attr);
            }
            if is_naked {
                let kind_id = inkwell::attributes::Attribute::get_named_enum_kind_id("naked");
                let naked_attr = self.context.create_enum_attribute(kind_id, 0);
                llvm_func.add_attribute(inkwell::attributes::AttributeLoc::Function, naked_attr);
            }
            if let Some(sec) = link_section {
                llvm_func.as_global_value().set_section(Some(&sec));
            }

            self.functions.insert(f.id, llvm_func);
        }
    }

    // ==========================================
    //          Phase 2: Code Generation
    // ==========================================

    pub fn compile_function(&mut self, func: &MastFunction) {
        let llvm_func = self.functions.get(&func.id).unwrap().clone();

        let entry_block = self.context.append_basic_block(llvm_func, "entry");
        self.builder.position_at_end(entry_block);
        self.locals.clear();

        for (i, param) in func.params.iter().enumerate() {
            let param_val = llvm_func.get_nth_param(i as u32).unwrap();
            let param_ty = self.get_llvm_type(param.ty);

            let alloca = self.create_entry_block_alloca(param_ty, &format!("arg_{}", param.name.0));
            self.builder.build_store(alloca, param_val).unwrap();
            self.locals.insert(param.name, alloca);
        }

        if let Some(body) = &func.body {
            // 1. 捕获 Block 执行完抛出的最后那个值 (比如 test3 里的 0)
            let block_res = self.compile_block(body);

            let current_block = self.builder.get_insert_block().unwrap();
            if current_block.get_terminator().is_none() {
                // 2. 自动生成 ret 指令 (拦截虚假的 Void 返回值)
                if self.is_void_type(func.ret_ty) {
                    // 如果函数是 void 签名，无论 block 抛出了什么假数据，统统 ret void
                    self.builder.build_return(None).unwrap();
                } else if let Some(val) = block_res {
                    self.builder.build_return(Some(&val)).unwrap();
                } else {
                    self.builder.build_unreachable().unwrap();
                }
            }
        }
    }

    fn compile_block(&mut self, block: &MastBlock) -> Option<BasicValueEnum<'ctx>> {
        // 1. 执行普通语句
        for stmt in &block.stmts {
            let current_block = self.builder.get_insert_block().unwrap();
            if current_block.get_terminator().is_some() {
                break;
            }

            match stmt {
                MastStmt::Let { name, ty, init } => {
                    let init_val = self.compile_expr(init);
                    let llvm_ty = self.get_llvm_type(*ty);
                    let alloca =
                        self.create_entry_block_alloca(llvm_ty, &format!("let_{}", name.0));
                    self.builder.build_store(alloca, init_val).unwrap();
                    self.locals.insert(*name, alloca);
                }
                MastStmt::Expr(expr) => {
                    self.compile_expr(expr);
                }
            }
        }

        let current_block = self.builder.get_insert_block().unwrap();
        if current_block.get_terminator().is_some() {
            return None;
        }

        // 2. 求出返回值，用 SSA 寄存器暂存
        let mut result_val = None;
        if let Some(result_expr) = &block.result {
            result_val = Some(self.compile_expr(result_expr));
        }

        // 3. 在求出返回值之后，块退出之前，执行所有的 defer
        for defer_expr in &block.defers {
            self.compile_expr(defer_expr);
        }

        // 4. Yield 这个在 defer 执行前就已经算好的值
        result_val
    }

    // ==========================================
    //          Expression Compilation
    // ==========================================

    fn compile_expr(&mut self, expr: &MastExpr) -> BasicValueEnum<'ctx> {
        let expected_llvm_ty = self.get_llvm_type(expr.ty);

        match &expr.kind {
            // === 1. 字面量与常量 ===
            MastExprKind::Undef => self.get_undef_val(expected_llvm_ty),
            MastExprKind::Unreachable => {
                self.builder.build_unreachable().unwrap();
                self.get_undef_val(self.context.i8_type().into())
            }
            MastExprKind::Integer(val) => expected_llvm_ty
                .into_int_type()
                .const_int(*val as u64, false)
                .into(),
            MastExprKind::Float(val) => expected_llvm_ty.into_float_type().const_float(*val).into(),
            MastExprKind::Bool(val) => self
                .context
                .bool_type()
                .const_int(if *val { 1 } else { 0 }, false)
                .into(),
            MastExprKind::StringLiteral(_) => unreachable!("Handled dynamically in Globals"),

            // === 2. 引用与解引用 ===
            MastExprKind::Var(name) => self.compile_var_ref(*name, expected_llvm_ty),
            MastExprKind::GlobalRef(mono_id) => self.compile_global_ref(*mono_id, expected_llvm_ty),
            MastExprKind::FuncRef(mono_id) => self.compile_func_ref(*mono_id),
            MastExprKind::AddressOf(operand) => {
                match &operand.kind {
                    // 如果本身就是合法的左值（变量、全局变量、字段访问、索引、解引用），直接安全取地址
                    MastExprKind::Var(_)
                    | MastExprKind::GlobalRef(_)
                    | MastExprKind::FieldAccess { .. }
                    | MastExprKind::IndexAccess { .. }
                    | MastExprKind::Deref(_) => self.compile_lvalue(operand).into(),
                    // 如果是右值取地址（如 i32.{ 404 }.&），立即将其实体化到栈上
                    _ => {
                        let rval = self.compile_expr(operand);
                        let llvm_ty = self.get_llvm_type(operand.ty);

                        // 在当前函数的 entry block 开辟一个隐式的临时变量
                        let temp_ptr = self.create_entry_block_alloca(llvm_ty, "tmp_addrof");

                        // 将右值存入内存
                        self.builder.build_store(temp_ptr, rval).unwrap();

                        // 返回这个临时变量的地址
                        temp_ptr.into()
                    }
                }
            }
            MastExprKind::Deref(operand) => self.compile_deref(operand, expected_llvm_ty),

            // === 3. 聚合数据 (Struct/Union/Array) 构造与访问 ===
            MastExprKind::StructInit { struct_id, fields } => {
                self.compile_struct_init(*struct_id, fields)
            }
            MastExprKind::UnionInit {
                union_id, value, ..
            } => self.compile_union_init(*union_id, value),
            MastExprKind::AdtInit {
                adt_struct_id,
                tag_value,
                payload,
            } => self.compile_adt_init(*adt_struct_id, *tag_value, payload),
            MastExprKind::ArrayInit(elems) => self.compile_array_init(elems, expected_llvm_ty),
            MastExprKind::FieldAccess {
                lhs,
                struct_id,
                field_idx,
            } => self.compile_field_access(lhs, *struct_id, *field_idx, expected_llvm_ty),
            MastExprKind::IndexAccess { lhs, index } => {
                self.compile_index_access(lhs, index, expected_llvm_ty, expr.ty)
            }

            // === 4. 运算与赋值 ===
            MastExprKind::Call { callee, args } => self.compile_call(callee, args, expr.ty),
            MastExprKind::Binary { op, lhs, rhs } => self.compile_binary(*op, lhs, rhs),
            MastExprKind::Unary { op, operand } => self.compile_unary(*op, operand),
            MastExprKind::Assign { op, lhs, rhs } => self.compile_assign(*op, lhs, rhs),

            // === 5. 控制流与块级作用域 ===
            MastExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => self.compile_if(
                cond,
                then_branch,
                else_branch.as_ref(),
                expr.ty,
                expected_llvm_ty,
            ),
            MastExprKind::Loop { body, latch } => self.compile_loop(body, latch.as_ref()),
            MastExprKind::Break => self.compile_break(),
            MastExprKind::Continue => self.compile_continue(),
            MastExprKind::Switch {
                target,
                cases,
                default_case,
            } => self.compile_switch(
                target,
                cases,
                default_case.as_ref(),
                expr.ty,
                expected_llvm_ty,
            ),
            MastExprKind::Block(block) => self.compile_block_expr(block),
            MastExprKind::Return(ret_val) => self.compile_return(ret_val.as_deref()),

            // === 6. 类型转换与胖指针底层操作 ===
            MastExprKind::Cast { kind, operand } => {
                self.compile_cast(*kind, operand, expected_llvm_ty)
            }
            MastExprKind::ConstructFatPointer { data_ptr, meta } => {
                self.compile_construct_fat_ptr(data_ptr, meta)
            }
            MastExprKind::ExtractFatPtrData(fat_ptr_expr) => {
                self.compile_extract_fat_ptr(fat_ptr_expr, 0, "extract_data")
            }
            MastExprKind::ExtractFatPtrMeta(fat_ptr_expr) => {
                self.compile_extract_fat_ptr(fat_ptr_expr, 1, "extract_meta")
            }
            // === 7. LLVM Inline Assembly ===
            MastExprKind::Asm(asm_block) => self.compile_inline_asm(asm_block),
            MastExprKind::BitIntrinsic { kind, operand } => {
                self.compile_bit_intrinsic(*kind, operand, expected_llvm_ty)
            }
            MastExprKind::Trap => {
                let intrinsic = inkwell::intrinsics::Intrinsic::find("llvm.trap").unwrap();
                let decl = intrinsic.get_declaration(&self.module, &[]).unwrap();
                self.builder.build_call(decl, &[], "trap").unwrap();
                self.builder.build_unreachable().unwrap(); // LLVM trap 之后也是不可达的
                self.get_undef_val(expected_llvm_ty)
            }
            MastExprKind::Breakpoint => {
                let intrinsic = inkwell::intrinsics::Intrinsic::find("llvm.debugtrap").unwrap();
                let decl = intrinsic.get_declaration(&self.module, &[]).unwrap();
                self.builder.build_call(decl, &[], "bkpt").unwrap();
                self.context.i8_type().const_zero().into() // Void return
            }
            MastExprKind::Fence => {
                // 生成严格的 Sequential Consistent 内存屏障
                self.builder
                    .build_fence(inkwell::AtomicOrdering::SequentiallyConsistent, 0, "mfence")
                    .unwrap();
                self.context.i8_type().const_zero().into() // Void return
            }
            MastExprKind::Memcpy { dest, src, len } => {
                let d = self.compile_expr(dest).into_pointer_value();
                let s = self.compile_expr(src).into_pointer_value();
                let l = self.compile_expr(len).into_int_value();
                // 1 表示按字节(u8)对齐，这是最安全的假设。高级优化会由LLVM后端处理。
                self.builder.build_memcpy(d, 1, s, 1, l).unwrap();
                self.context.i8_type().const_zero().into() // Void 返回
            }
            MastExprKind::Memset { dest, val, len } => {
                let d = self.compile_expr(dest).into_pointer_value();
                let v = self.compile_expr(val).into_int_value();
                let l = self.compile_expr(len).into_int_value();
                self.builder.build_memset(d, 1, v, l).unwrap();
                self.context.i8_type().const_zero().into() // Void 返回
            }
        }
    }

    // ==========================================
    //          LLVM Generation Helpers
    // ==========================================

    fn compile_var_ref(
        &self,
        name: crate::utils::SymbolId,
        expected_ty: BasicTypeEnum<'ctx>,
    ) -> BasicValueEnum<'ctx> {
        let ptr = self.locals.get(&name).expect("Local variable not found");
        self.builder
            .build_load(expected_ty, *ptr, &format!("load_{}", name.0))
            .unwrap()
    }

    fn compile_global_ref(
        &self,
        mono_id: MonoId,
        expected_ty: BasicTypeEnum<'ctx>,
    ) -> BasicValueEnum<'ctx> {
        let global_val = self.globals.get(&mono_id).expect("Global not found");
        let ptr = global_val.as_pointer_value();
        self.builder
            .build_load(expected_ty, ptr, "global_load")
            .unwrap()
    }

    fn compile_func_ref(&self, mono_id: MonoId) -> BasicValueEnum<'ctx> {
        let func_val = self.functions.get(&mono_id).expect("Function not found");
        func_val.as_global_value().as_pointer_value().into()
    }

    fn compile_deref(
        &mut self,
        operand: &MastExpr,
        expected_ty: BasicTypeEnum<'ctx>,
    ) -> BasicValueEnum<'ctx> {
        let ptr_val = self.compile_expr(operand).into_pointer_value();
        self.builder
            .build_load(expected_ty, ptr_val, "deref")
            .unwrap()
    }

    fn compile_struct_init(
        &mut self,
        struct_id: MonoId,
        fields: &[MastExpr],
    ) -> BasicValueEnum<'ctx> {
        let struct_llvm_ty = self.structs.get(&struct_id).unwrap();
        let mut current_struct = struct_llvm_ty
            .as_basic_type_enum()
            .into_struct_type()
            .const_zero();

        for (idx, field_expr) in fields.iter().enumerate() {
            let field_val = self.compile_expr(field_expr);
            current_struct = self
                .builder
                .build_insert_value(current_struct, field_val, idx as u32, "s_init")
                .unwrap()
                .into_struct_value();
        }
        current_struct.into()
    }

    fn compile_union_init(&mut self, union_id: MonoId, value: &MastExpr) -> BasicValueEnum<'ctx> {
        let union_llvm_ty = *self.structs.get(&union_id).unwrap();
        let alloca =
            self.create_entry_block_alloca(union_llvm_ty.as_basic_type_enum(), "union_init");

        let val = self.compile_expr(value);
        self.builder.build_store(alloca, val).unwrap();

        self.builder
            .build_load(union_llvm_ty.as_basic_type_enum(), alloca, "union_load")
            .unwrap()
    }

    fn compile_adt_init(
        &mut self,
        adt_struct_id: MonoId,
        tag_value: u128,
        payload: &MastExpr,
    ) -> BasicValueEnum<'ctx> {
        let struct_llvm_ty = *self.structs.get(&adt_struct_id).unwrap();

        let tag_llvm_ty = struct_llvm_ty
            .get_field_type_at_index(0)
            .unwrap()
            .into_int_type();
        let tag_val = tag_llvm_ty.const_int(tag_value as u64, false);

        let union_llvm_ty = struct_llvm_ty.get_field_type_at_index(1).unwrap();

        let union_alloca = self.create_entry_block_alloca(union_llvm_ty, "adt_union_init");

        if payload.ty != TypeId::VOID && payload.ty != TypeId::ERROR {
            let payload_val = self.compile_expr(payload);
            self.builder.build_store(union_alloca, payload_val).unwrap();
        }

        let union_val = self
            .builder
            .build_load(union_llvm_ty, union_alloca, "adt_union_load")
            .unwrap();

        let mut adt_struct = struct_llvm_ty.const_zero();
        adt_struct = self
            .builder
            .build_insert_value(adt_struct, tag_val, 0, "adt_insert_tag")
            .unwrap()
            .into_struct_value();
        adt_struct = self
            .builder
            .build_insert_value(adt_struct, union_val, 1, "adt_insert_union")
            .unwrap()
            .into_struct_value();

        adt_struct.into()
    }

    fn compile_array_init(
        &mut self,
        elems: &[MastExpr],
        expected_ty: BasicTypeEnum<'ctx>,
    ) -> BasicValueEnum<'ctx> {
        let array_llvm_ty = expected_ty.into_array_type();
        let mut current_array = array_llvm_ty.const_zero();
        for (idx, elem_expr) in elems.iter().enumerate() {
            let elem_val = self.compile_expr(elem_expr);
            current_array = self
                .builder
                .build_insert_value(current_array, elem_val, idx as u32, "arr_init")
                .unwrap()
                .into_array_value();
        }
        current_array.into()
    }

    fn compile_field_access(
        &mut self,
        lhs: &MastExpr,
        struct_id: MonoId,
        field_idx: usize,
        expected_ty: BasicTypeEnum<'ctx>,
    ) -> BasicValueEnum<'ctx> {
        let struct_ptr = self.compile_lvalue(lhs);
        let struct_llvm_ty = self.structs.get(&struct_id).unwrap();
        let is_union = self.union_ids.contains(&struct_id);

        if is_union {
            self.builder
                .build_load(expected_ty, struct_ptr, "union_field_load")
                .unwrap()
        } else {
            let field_ptr = self
                .builder
                .build_struct_gep(*struct_llvm_ty, struct_ptr, field_idx as u32, "field_gep")
                .unwrap();
            self.builder
                .build_load(expected_ty, field_ptr, "field_load")
                .unwrap()
        }
    }

    fn compile_index_access(
        &mut self,
        lhs: &MastExpr,
        index: &MastExpr,
        expected_ty: BasicTypeEnum<'ctx>,
        expr_ty: TypeId,
    ) -> BasicValueEnum<'ctx> {
        let idx_val = self.compile_expr(index).into_int_value();
        let norm_lhs = self.type_registry.normalize(lhs.ty);

        let elem_ptr = if let TypeKind::Slice(_) = self.type_registry.get(norm_lhs) {
            let slice_val = self.compile_expr(lhs).into_struct_value();
            let ptr_val = self
                .builder
                .build_extract_value(slice_val, 0, "slice_ptr")
                .unwrap()
                .into_pointer_value();
            let elem_ty = self.get_llvm_type(expr_ty);
            unsafe {
                self.builder
                    .build_gep(elem_ty, ptr_val, &[idx_val], "slice_idx")
                    .unwrap()
            }
        } else if let TypeKind::Pointer(_) | TypeKind::VolatilePtr(_) =
            self.type_registry.get(norm_lhs)
        {
            let ptr_val = self.compile_expr(lhs).into_pointer_value();
            let elem_ty = self.get_llvm_type(expr_ty);
            unsafe {
                self.builder
                    .build_gep(elem_ty, ptr_val, &[idx_val], "ptr_idx")
                    .unwrap()
            }
        } else {
            let array_ptr = self.compile_lvalue(lhs);
            let zero = self.context.i64_type().const_zero();
            let array_llvm_ty = self.get_llvm_type(lhs.ty);
            unsafe {
                self.builder
                    .build_gep(array_llvm_ty, array_ptr, &[zero, idx_val], "array_idx")
                    .unwrap()
            }
        };

        self.builder
            .build_load(expected_ty, elem_ptr, "idx_load")
            .unwrap()
    }

    fn compile_call(
        &mut self,
        callee: &MastExpr,
        args: &[MastExpr],
        expr_ty: TypeId,
    ) -> BasicValueEnum<'ctx> {
        let mut llvm_args = Vec::new();
        for arg in args {
            llvm_args.push(self.compile_expr(arg).into());
        }

        let call_site = if let MastExprKind::FuncRef(mono_id) = callee.kind {
            let llvm_func = self.functions.get(&mono_id).unwrap();
            self.builder
                .build_call(*llvm_func, &llvm_args, "call_ret")
                .unwrap()
        } else {
            let ptr_val = self.compile_expr(callee).into_pointer_value();
            let norm_ty = self.type_registry.normalize(callee.ty);

            let fn_type = if let TypeKind::Function {
                params,
                ret,
                is_variadic,
            } = self.type_registry.get(norm_ty)
            {
                let mut param_types = Vec::new();
                for p in params {
                    param_types.push(self.get_llvm_type(*p).into());
                }
                if *ret == TypeId::VOID {
                    self.context.void_type().fn_type(&param_types, *is_variadic)
                } else {
                    match self.get_llvm_type(*ret) {
                        BasicTypeEnum::IntType(i) => i.fn_type(&param_types, *is_variadic),
                        BasicTypeEnum::FloatType(fl) => fl.fn_type(&param_types, *is_variadic),
                        BasicTypeEnum::PointerType(p) => p.fn_type(&param_types, *is_variadic),
                        BasicTypeEnum::StructType(s) => s.fn_type(&param_types, *is_variadic),
                        BasicTypeEnum::ArrayType(a) => a.fn_type(&param_types, *is_variadic),
                        _ => unreachable!(),
                    }
                }
            } else {
                unreachable!()
            };

            self.builder
                .build_indirect_call(fn_type, ptr_val, &llvm_args, "icall")
                .unwrap()
        };

        if expr_ty == TypeId::VOID || expr_ty == TypeId::ERROR {
            self.context.i8_type().const_zero().into()
        } else {
            call_site.try_as_basic_value().unwrap_basic()
        }
    }

    fn compile_inline_asm(&mut self, asm_block: &MastAsmBlock) -> BasicValueEnum<'ctx> {
        // 1. 准备传入给汇编块的参数类型和对应的值
        let mut param_types = Vec::new();
        let mut arg_values = Vec::new();

        for arg_expr in &asm_block.input_args {
            let llvm_val = self.compile_expr(arg_expr);
            arg_values.push(llvm_val.into());
            param_types.push(llvm_val.get_type().into());
        }

        // 2 & 3. 确定返回值类型，并直接构建函数签名 FunctionType
        let asm_fn_type = match asm_block.output_tys.len() {
            0 => {
                // 纯副作用汇编，返回 VoidType
                self.context.void_type().fn_type(&param_types, false)
            }
            1 => {
                // 单一返回值，使用 BasicTypeEnum
                match self.get_llvm_type(asm_block.output_tys[0]) {
                    BasicTypeEnum::IntType(i) => i.fn_type(&param_types, false),
                    BasicTypeEnum::FloatType(f) => f.fn_type(&param_types, false),
                    BasicTypeEnum::PointerType(p) => p.fn_type(&param_types, false),
                    BasicTypeEnum::StructType(s) => s.fn_type(&param_types, false),
                    BasicTypeEnum::ArrayType(a) => a.fn_type(&param_types, false),
                    BasicTypeEnum::VectorType(v) => v.fn_type(&param_types, false),
                    BasicTypeEnum::ScalableVectorType(sv) => sv.fn_type(&param_types, false),
                }
            }
            _ => {
                // 多个返回值，打包成匿名的 StructType
                let mut struct_fields = Vec::new();
                for &ty in &asm_block.output_tys {
                    struct_fields.push(self.get_llvm_type(ty));
                }
                let struct_ty = self.context.struct_type(&struct_fields, false);
                struct_ty.fn_type(&param_types, false)
            }
        };

        // 4. 创建 InlineAsm 实例
        let has_side_effects = asm_block.is_volatile || asm_block.output_tys.is_empty();
        let inline_asm = self.context.create_inline_asm(
            asm_fn_type,
            asm_block.asm_template.clone(),
            asm_block.constraints.clone(),
            has_side_effects,
            false,
            Some(self.asm_dialect),
            false,
        );

        // 5. 调用汇编指令
        let call_site = self
            .builder
            .build_indirect_call(asm_fn_type, inline_asm, &arg_values, "asm_call")
            .unwrap();

        // 6. 将 LLVM 返回的值提取并 Store 到用户的指针中
        if asm_block.output_tys.len() > 0 {
            let asm_result = call_site.try_as_basic_value().unwrap_basic();

            for (i, ptr_expr) in asm_block.output_ptrs.iter().enumerate() {
                let target_ptr = self.compile_expr(ptr_expr).into_pointer_value();

                let extracted_val = if asm_block.output_tys.len() == 1 {
                    asm_result
                } else {
                    self.builder
                        .build_extract_value(
                            asm_result.into_struct_value(),
                            i as u32,
                            &format!("asm_out_{}", i),
                        )
                        .unwrap()
                };

                self.builder.build_store(target_ptr, extracted_val).unwrap();
            }
        }

        self.context.i8_type().const_zero().into()
    }

    // --- 运算与赋值 ---
    fn compile_binary(
        &mut self,
        op: ast::BinaryOperator,
        lhs: &MastExpr,
        rhs: &MastExpr,
    ) -> BasicValueEnum<'ctx> {
        let l_val = self.compile_expr(lhs);
        let r_val = self.compile_expr(rhs);

        if l_val.is_int_value() && r_val.is_int_value() {
            let l_int = l_val.into_int_value();
            let r_int = r_val.into_int_value();
            use crate::parser::ast::BinaryOperator::*;
            match op {
                Add => self
                    .builder
                    .build_int_add(l_int, r_int, "add")
                    .unwrap()
                    .into(),
                Subtract => self
                    .builder
                    .build_int_sub(l_int, r_int, "sub")
                    .unwrap()
                    .into(),
                Multiply => self
                    .builder
                    .build_int_mul(l_int, r_int, "mul")
                    .unwrap()
                    .into(),
                Divide => self
                    .builder
                    .build_int_signed_div(l_int, r_int, "sdiv")
                    .unwrap()
                    .into(),
                Modulo => self
                    .builder
                    .build_int_signed_rem(l_int, r_int, "srem")
                    .unwrap()
                    .into(),
                BitwiseAnd => self.builder.build_and(l_int, r_int, "and").unwrap().into(),
                BitwiseOr => self.builder.build_or(l_int, r_int, "or").unwrap().into(),
                BitwiseXor => self.builder.build_xor(l_int, r_int, "xor").unwrap().into(),
                ShiftLeft => self
                    .builder
                    .build_left_shift(l_int, r_int, "shl")
                    .unwrap()
                    .into(),
                ShiftRight => self
                    .builder
                    .build_right_shift(l_int, r_int, false, "shr")
                    .unwrap()
                    .into(),
                Equal => self
                    .builder
                    .build_int_compare(inkwell::IntPredicate::EQ, l_int, r_int, "eq")
                    .unwrap()
                    .into(),
                NotEqual => self
                    .builder
                    .build_int_compare(inkwell::IntPredicate::NE, l_int, r_int, "ne")
                    .unwrap()
                    .into(),
                LessThan => self
                    .builder
                    .build_int_compare(inkwell::IntPredicate::SLT, l_int, r_int, "slt")
                    .unwrap()
                    .into(),
                LessOrEqual => self
                    .builder
                    .build_int_compare(inkwell::IntPredicate::SLE, l_int, r_int, "sle")
                    .unwrap()
                    .into(),
                GreaterThan => self
                    .builder
                    .build_int_compare(inkwell::IntPredicate::SGT, l_int, r_int, "sgt")
                    .unwrap()
                    .into(),
                GreaterOrEqual => self
                    .builder
                    .build_int_compare(inkwell::IntPredicate::SGE, l_int, r_int, "sge")
                    .unwrap()
                    .into(),
                _ => unreachable!("Operator handled elsewhere"),
            }
        } else if l_val.is_float_value() && r_val.is_float_value() {
            let l_float = l_val.into_float_value();
            let r_float = r_val.into_float_value();
            use ast::BinaryOperator::*;
            match op {
                Add => self
                    .builder
                    .build_float_add(l_float, r_float, "fadd")
                    .unwrap()
                    .into(),
                Subtract => self
                    .builder
                    .build_float_sub(l_float, r_float, "fsub")
                    .unwrap()
                    .into(),
                Multiply => self
                    .builder
                    .build_float_mul(l_float, r_float, "fmul")
                    .unwrap()
                    .into(),
                Divide => self
                    .builder
                    .build_float_div(l_float, r_float, "fdiv")
                    .unwrap()
                    .into(),
                Modulo => self
                    .builder
                    .build_float_rem(l_float, r_float, "frem")
                    .unwrap()
                    .into(),
                Equal => self
                    .builder
                    .build_float_compare(inkwell::FloatPredicate::OEQ, l_float, r_float, "feq")
                    .unwrap()
                    .into(),
                NotEqual => self
                    .builder
                    .build_float_compare(inkwell::FloatPredicate::ONE, l_float, r_float, "fne")
                    .unwrap()
                    .into(),
                LessThan => self
                    .builder
                    .build_float_compare(inkwell::FloatPredicate::OLT, l_float, r_float, "flt")
                    .unwrap()
                    .into(),
                LessOrEqual => self
                    .builder
                    .build_float_compare(inkwell::FloatPredicate::OLE, l_float, r_float, "fle")
                    .unwrap()
                    .into(),
                GreaterThan => self
                    .builder
                    .build_float_compare(inkwell::FloatPredicate::OGT, l_float, r_float, "fgt")
                    .unwrap()
                    .into(),
                GreaterOrEqual => self
                    .builder
                    .build_float_compare(inkwell::FloatPredicate::OGE, l_float, r_float, "fge")
                    .unwrap()
                    .into(),
                _ => unreachable!(),
            }
        } else {
            unreachable!()
        }
    }

    fn compile_unary(
        &mut self,
        op: ast::UnaryOperator,
        operand: &MastExpr,
    ) -> BasicValueEnum<'ctx> {
        let op_val = self.compile_expr(operand);
        match op {
            ast::UnaryOperator::Negate => {
                if op_val.is_int_value() {
                    self.builder
                        .build_int_neg(op_val.into_int_value(), "neg")
                        .unwrap()
                        .into()
                } else {
                    self.builder
                        .build_float_neg(op_val.into_float_value(), "fneg")
                        .unwrap()
                        .into()
                }
            }
            ast::UnaryOperator::LogicalNot | ast::UnaryOperator::BitwiseNot => self
                .builder
                .build_not(op_val.into_int_value(), "not")
                .unwrap()
                .into(),
            ast::UnaryOperator::LengthOf => {
                // MAST 保证了此时的类型已经是纯物理类型
                let norm_ty = self.type_registry.normalize(operand.ty);
                match self.type_registry.get(norm_ty) {
                    TypeKind::Array { len, .. } => {
                        self.context.i64_type().const_int(*len, false).into()
                    }
                    TypeKind::Slice(_) => self
                        .builder
                        .build_extract_value(op_val.into_struct_value(), 1, "slice_len")
                        .unwrap(),
                    _ => unreachable!(),
                }
            }
            _ => unreachable!(),
        }
    }

    fn compile_assign(
        &mut self,
        op: ast::AssignmentOperator,
        lhs: &MastExpr,
        rhs: &MastExpr,
    ) -> BasicValueEnum<'ctx> {
        let ptr = self.compile_lvalue(lhs);
        let rhs_val = self.compile_expr(rhs);

        if op == ast::AssignmentOperator::Assign {
            self.builder.build_store(ptr, rhs_val).unwrap();
        } else {
            let expected_lhs_ty = self.get_llvm_type(lhs.ty);
            let lhs_val = self
                .builder
                .build_load(expected_lhs_ty, ptr, "assign_load")
                .unwrap();

            let new_val: inkwell::values::BasicValueEnum<'ctx> = if lhs_val.is_int_value() {
                let l_int = lhs_val.into_int_value();
                let r_int = rhs_val.into_int_value();
                use ast::AssignmentOperator::*;
                match op {
                    AddAssign => self
                        .builder
                        .build_int_add(l_int, r_int, "add_a")
                        .unwrap()
                        .into(),
                    SubtractAssign => self
                        .builder
                        .build_int_sub(l_int, r_int, "sub_a")
                        .unwrap()
                        .into(),
                    MultiplyAssign => self
                        .builder
                        .build_int_mul(l_int, r_int, "mul_a")
                        .unwrap()
                        .into(),
                    DivideAssign => self
                        .builder
                        .build_int_signed_div(l_int, r_int, "div_a")
                        .unwrap()
                        .into(),
                    ModuloAssign => self
                        .builder
                        .build_int_signed_rem(l_int, r_int, "rem_a")
                        .unwrap()
                        .into(),
                    BitwiseAndAssign => self
                        .builder
                        .build_and(l_int, r_int, "and_a")
                        .unwrap()
                        .into(),
                    BitwiseOrAssign => self.builder.build_or(l_int, r_int, "or_a").unwrap().into(),
                    BitwiseXorAssign => self
                        .builder
                        .build_xor(l_int, r_int, "xor_a")
                        .unwrap()
                        .into(),
                    ShiftLeftAssign => self
                        .builder
                        .build_left_shift(l_int, r_int, "shl_a")
                        .unwrap()
                        .into(),
                    ShiftRightAssign => self
                        .builder
                        .build_right_shift(l_int, r_int, false, "shr_a")
                        .unwrap()
                        .into(),
                    _ => unreachable!(),
                }
            } else if lhs_val.is_float_value() {
                // 新增：处理浮点数复合赋值
                let l_float = lhs_val.into_float_value();
                let r_float = rhs_val.into_float_value();
                use ast::AssignmentOperator::*;
                match op {
                    AddAssign => self
                        .builder
                        .build_float_add(l_float, r_float, "fadd_a")
                        .unwrap()
                        .into(),
                    SubtractAssign => self
                        .builder
                        .build_float_sub(l_float, r_float, "fsub_a")
                        .unwrap()
                        .into(),
                    MultiplyAssign => self
                        .builder
                        .build_float_mul(l_float, r_float, "fmul_a")
                        .unwrap()
                        .into(),
                    DivideAssign => self
                        .builder
                        .build_float_div(l_float, r_float, "fdiv_a")
                        .unwrap()
                        .into(),
                    ModuloAssign => self
                        .builder
                        .build_float_rem(l_float, r_float, "frem_a")
                        .unwrap()
                        .into(),
                    _ => unreachable!("Unsupported float assignment operator"),
                }
            } else {
                unreachable!("Unsupported type for assignment");
            };
            self.builder.build_store(ptr, new_val).unwrap();
        }
        self.context.i8_type().const_zero().into()
    }

    // --- 控制流 (Control Flow) ---

    fn compile_if(
        &mut self,
        cond: &MastExpr,
        then_branch: &MastBlock,
        else_branch: Option<&MastBlock>,
        expr_ty: TypeId,
        expected_llvm_ty: BasicTypeEnum<'ctx>,
    ) -> BasicValueEnum<'ctx> {
        let cond_val = self.compile_expr(cond).into_int_value();
        let parent_func = self
            .builder
            .get_insert_block()
            .unwrap()
            .get_parent()
            .unwrap();

        let then_bb = self.context.append_basic_block(parent_func, "then");
        let else_bb = self.context.append_basic_block(parent_func, "else");
        let merge_bb = self.context.append_basic_block(parent_func, "ifcont");

        if else_branch.is_some() {
            self.builder
                .build_conditional_branch(cond_val, then_bb, else_bb)
                .unwrap();
        } else {
            self.builder
                .build_conditional_branch(cond_val, then_bb, merge_bb)
                .unwrap();
        }

        // 编译 Then 分支
        self.builder.position_at_end(then_bb);
        let then_result = self.compile_block(then_branch);
        let then_exit_bb = self.builder.get_insert_block().unwrap();
        if then_exit_bb.get_terminator().is_none() {
            self.builder.build_unconditional_branch(merge_bb).unwrap();
        }

        // 编译 Else 分支
        self.builder.position_at_end(else_bb);
        let mut else_result = None;
        if let Some(eb) = else_branch {
            else_result = self.compile_block(eb);
        }
        let else_exit_bb = self.builder.get_insert_block().unwrap();
        if else_exit_bb.get_terminator().is_none() {
            self.builder.build_unconditional_branch(merge_bb).unwrap();
        }

        // 生成 PHI 节点，合并两个分支的返回值
        self.builder.position_at_end(merge_bb);
        if expr_ty != TypeId::VOID && then_result.is_some() && else_result.is_some() {
            let phi = self.builder.build_phi(expected_llvm_ty, "iftmp").unwrap();
            phi.add_incoming(&[
                (&then_result.unwrap(), then_exit_bb),
                (&else_result.unwrap(), else_exit_bb),
            ]);
            phi.as_basic_value()
        } else {
            self.context.i8_type().const_zero().into()
        }
    }

    fn compile_loop(
        &mut self,
        body: &MastBlock,
        latch: Option<&MastBlock>,
    ) -> BasicValueEnum<'ctx> {
        let parent_func = self
            .builder
            .get_insert_block()
            .unwrap()
            .get_parent()
            .unwrap();

        let loop_bb = self.context.append_basic_block(parent_func, "loop");
        let latch_bb = self.context.append_basic_block(parent_func, "latch");
        let merge_bb = self.context.append_basic_block(parent_func, "loopcont");

        self.builder.build_unconditional_branch(loop_bb).unwrap();
        self.builder.position_at_end(loop_bb);

        // 把 latch_bb 压入栈,这样遇到 continue 时就会跳转到 latch_bb
        self.loop_targets.push((latch_bb, merge_bb));

        self.compile_block(body);

        let loop_exit_bb = self.builder.get_insert_block().unwrap();
        if loop_exit_bb.get_terminator().is_none() {
            // 循环体自然结束，跳入 latch 块
            self.builder.build_unconditional_branch(latch_bb).unwrap();
        }

        self.loop_targets.pop();

        // --- 编译 Latch Block (步进逻辑) ---
        self.builder.position_at_end(latch_bb);
        if let Some(latch_block) = latch {
            self.compile_block(latch_block);
        }
        let latch_exit_bb = self.builder.get_insert_block().unwrap();
        if latch_exit_bb.get_terminator().is_none() {
            // 步进执行完毕，跳回循环头进行新一轮判断
            self.builder.build_unconditional_branch(loop_bb).unwrap();
        }

        // --- 编译结束后的出口 ---
        self.builder.position_at_end(merge_bb);
        self.context.i8_type().const_zero().into()
    }

    fn compile_break(&mut self) -> BasicValueEnum<'ctx> {
        let (_, merge_bb) = self.loop_targets.last().expect("Break outside of loop");
        self.builder.build_unconditional_branch(*merge_bb).unwrap();
        self.context.i8_type().const_zero().into()
    }

    fn compile_continue(&mut self) -> BasicValueEnum<'ctx> {
        let (loop_bb, _) = self.loop_targets.last().expect("Continue outside of loop");
        self.builder.build_unconditional_branch(*loop_bb).unwrap();
        self.context.i8_type().const_zero().into()
    }

    fn compile_switch(
        &mut self,
        target: &MastExpr,
        cases: &[MastSwitchCase],
        default_case: Option<&MastBlock>,
        expr_ty: TypeId,
        expected_llvm_ty: BasicTypeEnum<'ctx>,
    ) -> BasicValueEnum<'ctx> {
        let target_val = self.compile_expr(target).into_int_value();
        let parent_func = self
            .builder
            .get_insert_block()
            .unwrap()
            .get_parent()
            .unwrap();

        let merge_bb = self.context.append_basic_block(parent_func, "switchcont");
        let default_bb = self.context.append_basic_block(parent_func, "default");
        let mut case_blocks = Vec::new();
        let mut llvm_cases = Vec::new();

        for case in cases {
            let case_bb = self.context.append_basic_block(parent_func, "case");
            case_blocks.push(case_bb);
            for &val in &case.values {
                let int_val = target_val.get_type().const_int(val as u64, false);
                llvm_cases.push((int_val, case_bb));
            }
        }

        self.builder
            .build_switch(target_val, default_bb, &llvm_cases)
            .unwrap();

        let mut incoming = Vec::new();

        // 编译 Default 分支
        self.builder.position_at_end(default_bb);
        if let Some(def_block) = default_case {
            if let Some(val) = self.compile_block(def_block) {
                incoming.push((val, self.builder.get_insert_block().unwrap()));
            }
            if self
                .builder
                .get_insert_block()
                .unwrap()
                .get_terminator()
                .is_none()
            {
                self.builder.build_unconditional_branch(merge_bb).unwrap();
            }
        } else {
            self.builder.build_unreachable().unwrap(); // 前端已保证穷尽性
        }
        if self
            .builder
            .get_insert_block()
            .unwrap()
            .get_terminator()
            .is_none()
        {
            self.builder.build_unconditional_branch(merge_bb).unwrap();
        }

        // 编译所有 Case 分支
        for (i, case) in cases.iter().enumerate() {
            self.builder.position_at_end(case_blocks[i]);
            if let Some(val) = self.compile_block(&case.body) {
                incoming.push((val, self.builder.get_insert_block().unwrap()));
            }
            if self
                .builder
                .get_insert_block()
                .unwrap()
                .get_terminator()
                .is_none()
            {
                self.builder.build_unconditional_branch(merge_bb).unwrap();
            }
        }

        // 构建 PHI 返回值
        self.builder.position_at_end(merge_bb);
        if expr_ty != TypeId::VOID && !incoming.is_empty() {
            let phi = self
                .builder
                .build_phi(expected_llvm_ty, "switchtmp")
                .unwrap();
            let mut incoming_refs = Vec::new();
            for (val, bb) in &incoming {
                incoming_refs.push((val as &dyn inkwell::values::BasicValue<'ctx>, *bb));
            }
            phi.add_incoming(&incoming_refs);
            phi.as_basic_value()
        } else {
            self.context.i8_type().const_zero().into()
        }
    }

    fn compile_block_expr(&mut self, block: &MastBlock) -> BasicValueEnum<'ctx> {
        if let Some(res) = self.compile_block(block) {
            res
        } else {
            self.context.i8_type().const_zero().into()
        }
    }

    fn compile_return(&mut self, ret_val: Option<&MastExpr>) -> BasicValueEnum<'ctx> {
        if let Some(val) = ret_val {
            let llvm_val = self.compile_expr(val);
            // 如果是 void 表达式，忽略它产生的值，强行 build_return(None)
            if self.is_void_type(val.ty) {
                self.builder.build_return(None).unwrap();
            } else {
                self.builder.build_return(Some(&llvm_val)).unwrap();
            }
        } else {
            self.builder.build_return(None).unwrap();
        }
        self.context.i8_type().const_zero().into()
    }

    // --- 类型转换 (Casts) ---

    fn compile_cast(
        &mut self,
        kind: MastCastKind,
        operand: &MastExpr,
        target_llvm_ty: BasicTypeEnum<'ctx>,
    ) -> BasicValueEnum<'ctx> {
        let val = self.compile_expr(operand);
        match kind {
            MastCastKind::Bitcast => {
                if val.is_struct_value() && target_llvm_ty.is_pointer_type() {
                    let fat_ptr = val.into_struct_value();
                    self.builder
                        .build_extract_value(fat_ptr, 0, "slice_ptr_fallback")
                        .unwrap()
                        .into_pointer_value()
                        .into()
                } else {
                    self.builder
                        .build_bit_cast(val, target_llvm_ty, "bitcast")
                        .unwrap()
                }
            }
            MastCastKind::PtrToInt => self
                .builder
                .build_ptr_to_int(
                    val.into_pointer_value(),
                    target_llvm_ty.into_int_type(),
                    "ptr2int",
                )
                .unwrap()
                .into(),
            MastCastKind::IntToPtr => self
                .builder
                .build_int_to_ptr(
                    val.into_int_value(),
                    target_llvm_ty.into_pointer_type(),
                    "int2ptr",
                )
                .unwrap()
                .into(),
            MastCastKind::ZeroExt => self
                .builder
                .build_int_z_extend(val.into_int_value(), target_llvm_ty.into_int_type(), "zext")
                .unwrap()
                .into(),
            MastCastKind::SignExt => self
                .builder
                .build_int_s_extend(val.into_int_value(), target_llvm_ty.into_int_type(), "sext")
                .unwrap()
                .into(),
            MastCastKind::Trunc => self
                .builder
                .build_int_truncate(
                    val.into_int_value(),
                    target_llvm_ty.into_int_type(),
                    "trunc",
                )
                .unwrap()
                .into(),
            MastCastKind::IntToFloat => self
                .builder
                .build_signed_int_to_float(
                    val.into_int_value(),
                    target_llvm_ty.into_float_type(),
                    "i2f",
                )
                .unwrap()
                .into(),
            MastCastKind::FloatToInt => self
                .builder
                .build_float_to_signed_int(
                    val.into_float_value(),
                    target_llvm_ty.into_int_type(),
                    "f2i",
                )
                .unwrap()
                .into(),
            MastCastKind::FloatCast => self
                .builder
                .build_float_cast(
                    val.into_float_value(),
                    target_llvm_ty.into_float_type(),
                    "fcast",
                )
                .unwrap()
                .into(),

            // ArrayDecay: [N]T -> []T (将数组隐式转换为带长度的胖指针)
            MastCastKind::ArrayToSlice => {
                // 临时变量具象化 (Materialize Temporary)
                let array_ptr = match &operand.kind {
                    // 如果本身就是合法的左值（比如变量名），直接取它的地址，避免无意义的拷贝
                    MastExprKind::Var(_)
                    | MastExprKind::GlobalRef(_)
                    | MastExprKind::FieldAccess { .. }
                    | MastExprKind::IndexAccess { .. }
                    | MastExprKind::Deref(_) => self.compile_lvalue(operand),
                    // 如果是右值（比如 ArrayInit 临时数组），在栈上开辟临时空间存进去
                    _ => {
                        let array_val = self.compile_expr(operand);
                        let array_llvm_ty = self.get_llvm_type(operand.ty);
                        let temp_ptr =
                            self.create_entry_block_alloca(array_llvm_ty, "tmp_array_for_slice");
                        self.builder.build_store(temp_ptr, array_val).unwrap();
                        temp_ptr
                    }
                };

                // 获取长度
                let array_len = if let TypeKind::Array { len, .. } = self
                    .type_registry
                    .get(self.type_registry.normalize(operand.ty))
                {
                    *len
                } else {
                    unreachable!()
                };

                // 组装 Slice 胖指针
                let slice_llvm_ty = target_llvm_ty.into_struct_type();
                let mut slice_val = slice_llvm_ty.get_undef();

                slice_val = self
                    .builder
                    .build_insert_value(slice_val, array_ptr, 0, "slice_ptr")
                    .unwrap()
                    .into_struct_value();

                let len_val = self.context.i64_type().const_int(array_len as u64, false);
                slice_val = self
                    .builder
                    .build_insert_value(slice_val, len_val, 1, "slice_len")
                    .unwrap()
                    .into_struct_value();

                slice_val.into()
            }
        }
    }

    fn compile_construct_fat_ptr(
        &mut self,
        data_ptr: &MastExpr,
        meta: &MastExpr,
    ) -> BasicValueEnum<'ctx> {
        let ptr_ty = self.context.ptr_type(AddressSpace::default());
        let len_ty = self.context.i64_type();
        let fat_ptr_ty = self
            .context
            .struct_type(&[ptr_ty.into(), len_ty.into()], false);

        let mut fat_ptr = fat_ptr_ty.const_zero();

        let data_val = self.compile_expr(data_ptr);
        fat_ptr = self
            .builder
            .build_insert_value(fat_ptr, data_val, 0, "fat_data")
            .unwrap()
            .into_struct_value();

        let meta_val = self.compile_expr(meta);
        fat_ptr = self
            .builder
            .build_insert_value(fat_ptr, meta_val, 1, "fat_meta")
            .unwrap()
            .into_struct_value();

        fat_ptr.into()
    }

    fn compile_extract_fat_ptr(
        &mut self,
        fat_ptr_expr: &MastExpr,
        index: u32,
        name: &str,
    ) -> BasicValueEnum<'ctx> {
        let fat_ptr_val = self.compile_expr(fat_ptr_expr).into_struct_value();
        self.builder
            .build_extract_value(fat_ptr_val, index, name)
            .unwrap()
    }

    fn compile_bit_intrinsic(
        &mut self,
        kind: BitIntrinsicKind,
        operand: &MastExpr,
        expected_ty: BasicTypeEnum<'ctx>,
    ) -> BasicValueEnum<'ctx> {
        let val = self.compile_expr(operand);

        let intrinsic_name = match kind {
            BitIntrinsicKind::PopCount => "llvm.ctpop",
            BitIntrinsicKind::Clz => "llvm.ctlz",
            BitIntrinsicKind::Ctz => "llvm.cttz",
            BitIntrinsicKind::Bswap => "llvm.bswap",
        };

        let intrinsic = inkwell::intrinsics::Intrinsic::find(intrinsic_name).unwrap();
        let decl = intrinsic
            .get_declaration(&self.module, &[expected_ty])
            .unwrap();

        let call_site = if kind == BitIntrinsicKind::PopCount || kind == BitIntrinsicKind::Bswap {
            self.builder
                .build_call(decl, &[val.into()], "bit_op")
                .unwrap()
        } else {
            let is_zero_poison = self.context.bool_type().const_zero();
            self.builder
                .build_call(decl, &[val.into(), is_zero_poison.into()], "lz_tz")
                .unwrap()
        };

        call_site.try_as_basic_value().unwrap_basic()
    }

    fn compile_lvalue(&mut self, expr: &MastExpr) -> PointerValue<'ctx> {
        match &expr.kind {
            MastExprKind::Var(name) => *self.locals.get(name).expect("Local variable not found"),
            MastExprKind::GlobalRef(mono_id) => self
                .globals
                .get(mono_id)
                .expect("Global not found")
                .as_pointer_value(),
            MastExprKind::FieldAccess {
                lhs,
                struct_id,
                field_idx,
            } => {
                let struct_ptr = self.compile_lvalue(lhs);
                let struct_llvm_ty = self.structs.get(struct_id).unwrap();
                self.builder
                    .build_struct_gep(*struct_llvm_ty, struct_ptr, *field_idx as u32, "lvalue_gep")
                    .unwrap()
            }
            MastExprKind::IndexAccess { lhs, index } => {
                let idx_val = self.compile_expr(index).into_int_value();
                let norm_lhs = self.type_registry.normalize(lhs.ty);

                if let TypeKind::Slice(_) = self.type_registry.get(norm_lhs) {
                    let slice_val = self.compile_expr(lhs).into_struct_value();
                    let ptr_val = self
                        .builder
                        .build_extract_value(slice_val, 0, "slice_ptr")
                        .unwrap()
                        .into_pointer_value();
                    let elem_ty = self.get_llvm_type(expr.ty);
                    unsafe {
                        self.builder
                            .build_gep(elem_ty, ptr_val, &[idx_val], "slice_lvalue")
                            .unwrap()
                    }
                } else if let TypeKind::Pointer(_) | TypeKind::VolatilePtr(_) =
                    self.type_registry.get(norm_lhs)
                {
                    let ptr_val = self.compile_expr(lhs).into_pointer_value();
                    let elem_ty = self.get_llvm_type(expr.ty);
                    unsafe {
                        self.builder
                            .build_gep(elem_ty, ptr_val, &[idx_val], "ptr_lvalue")
                            .unwrap()
                    }
                } else {
                    let array_ptr = self.compile_lvalue(lhs);
                    let zero = self.context.i64_type().const_zero();
                    let array_llvm_ty = self.get_llvm_type(lhs.ty);
                    unsafe {
                        self.builder
                            .build_gep(array_llvm_ty, array_ptr, &[zero, idx_val], "array_lvalue")
                            .unwrap()
                    }
                }
            }
            MastExprKind::Deref(operand) => self.compile_expr(operand).into_pointer_value(),
            _ => panic!("Expression is not a valid l-value: {:?}", expr.kind),
        }
    }

    // ==========================================
    //          Object File Generation
    // ==========================================

    pub fn emit_to_file(
        &self,
        target_triple_str: &str,
        output_path: &str,
        opt_level: OptLevel,
    ) -> Result<(), String> {
        // 1. 初始化所有的 LLVM Target (x86, ARM, RISCV 等)
        Target::initialize_all(&InitializationConfig::default());

        // 2. 解析目标架构三元组
        let triple = inkwell::targets::TargetTriple::create(target_triple_str);

        let target = Target::from_triple(&triple).map_err(|e| e.to_string())?;

        // 动态映射 Kern 优化等级到 LLVM 优化等级
        let llvm_opt_level = match opt_level {
            OptLevel::O0 => inkwell::OptimizationLevel::None,
            OptLevel::O1 => inkwell::OptimizationLevel::Less,
            OptLevel::O2 => inkwell::OptimizationLevel::Default,
            OptLevel::O3 => inkwell::OptimizationLevel::Aggressive,
        };

        // 3. 创建目标机器实例 (配置优化级别、重定位模式等)
        let target_machine = target
            .create_target_machine(
                &triple,
                "generic", // CPU 类型
                "",        // 特性
                llvm_opt_level,
                RelocMode::Default,
                CodeModel::Default,
            )
            .ok_or("Failed to create target machine")?;

        // 4. 将目标机器的数据布局 (Data Layout) 和三元组写入当前 Module
        self.module
            .set_data_layout(&target_machine.get_target_data().get_data_layout());
        self.module.set_triple(&triple);

        if let Err(err) = self.module.verify() {
            // 如果 IR 有问题，它会打印出极其详细的错误信息（比如哪一行的 PHI 节点类型不对）
            eprintln!("LLVM IR Verification Failed:\n{}", err.to_string());
            // 顺便把畸形的 IR 打印出来，方便肉眼对比
            self.print_ir();
            return Err("Invalid LLVM IR generated".to_string());
        }

        // 5. 触发 LLVM 后端，直接将 IR 编译为二进制的 Object (.o) 文件
        let path = std::path::Path::new(output_path);
        target_machine
            .write_to_file(&self.module, FileType::Object, path)
            .map_err(|e| e.to_string())?;

        Ok(())
    }
}
