// src/mast/lower.rs
use super::ast::*;
use crate::driver::Context;
use crate::parser::ast::{self, Expr, ExprKind};
use crate::sema::def::Def;
use crate::sema::ty::{PrimitiveType, TypeId, TypeKind};
use crate::sema::typeck::subst::Substituter;
use crate::utils::{Span, SymbolId};
use std::collections::HashMap;

/// MAST 降级引擎 (Monomorphization & Lowering)
pub struct Lowerer<'a> {
    pub ctx: &'a mut Context,
    pub module: MastModule,

    /// 单态化缓存：记录 `(DefId, [TypeId, ...])` 对应的 MonoId，防止重复克隆
    mono_cache: HashMap<(crate::sema::ty::DefId, Vec<TypeId>), MonoId>,
    next_mono_id: u32,

    /// Defer 栈：处理块级作用域的清理。每个 Block 对应一个 Vec<MastExpr>
    defer_stack: Vec<Vec<MastExpr>>,

    // 维护前端 Global DefId 到 MAST MonoId 的映射
    global_map: HashMap<crate::sema::ty::DefId, MonoId>,

    // VTable 缓存，键是 (SourceType, TraitType)
    vtable_cache: HashMap<(TypeId, TypeId), MonoId>,

    // 维护降级时的局部变量类型栈
    pub local_types: Vec<HashMap<SymbolId, TypeId>>,

    // 记录进入循环时 defer_stack 的深度
    loop_frames: Vec<usize>,

    // 专门记录 ADT Wrapper ID -> Payload Union ID 的安全映射
    adt_union_map: HashMap<MonoId, MonoId>,
}

impl<'a> Lowerer<'a> {
    pub fn new(ctx: &'a mut Context) -> Self {
        Self {
            ctx,
            module: MastModule {
                name: "kern_out".to_string(),
                structs: Vec::new(),
                globals: Vec::new(),
                functions: Vec::new(),
            },
            mono_cache: HashMap::new(),
            next_mono_id: 1,
            defer_stack: Vec::new(),
            global_map: HashMap::new(),
            vtable_cache: HashMap::new(),
            local_types: Vec::new(),
            loop_frames: Vec::new(),
            adt_union_map: HashMap::new(),
        }
    }

    fn new_mono_id(&mut self) -> MonoId {
        let id = self.next_mono_id;
        self.next_mono_id += 1;
        MonoId(id)
    }

    /// 降级入口：寻找所有非泛型的根节点向下递归单态化
    pub fn lower_all(&mut self) -> MastModule {
        let def_ids: Vec<_> = (0..self.ctx.defs.len())
            .map(|i| crate::sema::ty::DefId(i as u32))
            .collect();

        // Phase 1: 预分配全局变量的 MonoId
        for &id in &def_ids {
            if let Def::Global(_) = &self.ctx.defs[id.0 as usize] {
                let mono_id = self.new_mono_id();
                self.global_map.insert(id, mono_id);
            }
        }

        // Phase 2: 执行真正的实体降级
        for id in def_ids {
            let def = self.ctx.defs[id.0 as usize].clone();
            match def {
                Def::Function(f) => {
                    // 内置函数没有物理实体，直接跳过，不进入 MAST
                    if f.is_intrinsic {
                        continue;
                    }
                    // 检查函数自身和其父级（Impl块）是否包含泛型
                    // 只有自己没泛型，且爹也没泛型的函数，才是真正的“自由函数”，才能在此刻被实例化
                    let mut is_generic = !f.generics.is_empty();
                    if let Some(parent_id) = f.parent {
                        if let Def::Impl(impl_def) = &self.ctx.defs[parent_id.0 as usize] {
                            if !impl_def.generics.is_empty() {
                                is_generic = true;
                            }
                        }
                    }

                    if !is_generic {
                        self.instantiate_function(id, &[]);
                    }
                }
                Def::Global(g) => self.lower_global(&g),
                _ => {}
            }
        }

        self.module.clone()
    }

    // ==========================================
    //          Monomorphization Engine
    // ==========================================

    fn instantiate_function(&mut self, def_id: crate::sema::ty::DefId, args: &[TypeId]) -> MonoId {
        let key = (def_id, args.to_vec());
        if let Some(&id) = self.mono_cache.get(&key) {
            return id;
        }

        let id = self.new_mono_id();
        self.mono_cache.insert(key, id);

        let def = if let Def::Function(f) = &self.ctx.defs[def_id.0 as usize] {
            f.clone()
        } else {
            unreachable!()
        };

        // 合并父级作用域 (Impl 块) 的泛型参数
        // 泛型参数环境 = [Impl 泛型] + [函数自身泛型]
        let mut all_generic_params = Vec::new();

        // 1. 如果这个函数属于某个 Impl 块，先把它身上的 T, U 拿过来
        if let Some(parent_id) = def.parent {
            if let Def::Impl(impl_def) = &self.ctx.defs[parent_id.0 as usize] {
                all_generic_params.extend(impl_def.generics.clone());
            }
        }

        // 2. 追加函数自身的泛型参数
        all_generic_params.extend(def.generics.clone());

        // 3. 将外部传入的具体类型 args 依次与收集到的泛型名对齐
        let mut subst_map = HashMap::new();
        for (i, param) in all_generic_params.iter().enumerate() {
            if i < args.len() {
                subst_map.insert(param.name, args[i]);
            }
        }

        let mut mangled_name = self.ctx.resolve(def.name).to_string();
        for arg in args {
            mangled_name.push_str(&format!("_{}", arg.0));
        }

        let raw_ret = def.resolved_sig.map_or(TypeId::VOID, |sig| {
            if let TypeKind::Function { ret, .. } = self.ctx.type_registry.get(sig) {
                *ret
            } else {
                TypeId::VOID
            }
        });

        let mut subst = Substituter::new(&mut self.ctx.type_registry, &subst_map);

        let mut mast_params = Vec::new();
        for p in &def.params {
            let raw_ty = self
                .ctx
                .node_types
                .get(&p.type_node.id)
                .copied()
                .unwrap_or(TypeId::ERROR);
            let conc_ty = subst.substitute(raw_ty);
            mast_params.push(MastParam {
                name: p.name,
                ty: conc_ty,
            });
        }

        let conc_ret = subst.substitute(raw_ret);

        self.local_types.push(std::collections::HashMap::new());
        for p in &mast_params {
            self.local_types.last_mut().unwrap().insert(p.name, p.ty);
        }

        let body = if let Some(body_expr) = &def.body {
            Some(self.lower_block_as_body(body_expr, &subst_map, conc_ret))
        } else {
            None
        };

        self.local_types.pop();

        let mast_fn = MastFunction {
            id,
            name: mangled_name,
            params: mast_params,
            ret_ty: conc_ret,
            body,
            is_extern: def.is_extern,
            is_variadic: def.is_variadic,
            attributes: self.extract_meta_items(&def.attributes),
        };

        self.module.functions.push(mast_fn);
        id
    }

    fn instantiate_struct(&mut self, def_id: crate::sema::ty::DefId, args: &[TypeId]) -> MonoId {
        let key = (def_id, args.to_vec());
        if let Some(&id) = self.mono_cache.get(&key) {
            return id;
        }

        let id = self.new_mono_id();
        self.mono_cache.insert(key, id);

        let def = if let Def::Struct(s) = &self.ctx.defs[def_id.0 as usize] {
            s.clone()
        } else if let Def::Union(_) = &self.ctx.defs[def_id.0 as usize] {
            return self.instantiate_union(def_id, args, id);
        } else {
            unreachable!()
        };

        let mut subst_map = HashMap::new();
        for (i, param) in def.generics.iter().enumerate() {
            subst_map.insert(param.name, args[i]);
        }

        let mut mangled_name = self.ctx.resolve(def.name).to_string();
        for arg in args {
            mangled_name.push_str(&format!("_{}", arg.0));
        }

        let mut mast_fields = Vec::new();
        let mut subst = Substituter::new(&mut self.ctx.type_registry, &subst_map);

        for f in &def.fields {
            let raw_ty = self
                .ctx
                .node_types
                .get(&f.type_node.id)
                .copied()
                .unwrap_or(TypeId::ERROR);
            let conc_ty = subst.substitute(raw_ty);
            mast_fields.push(MastField {
                name: f.name,
                ty: conc_ty,
            });
        }

        self.module.structs.push(MastStruct {
            id,
            name: mangled_name,
            fields: mast_fields,
            is_extern: def.is_extern,
            is_union: false,
            largest_field_idx: 0,
            attributes: self.extract_meta_items(&def.attributes),
        });
        id
    }

    fn instantiate_union(
        &mut self,
        def_id: crate::sema::ty::DefId,
        args: &[TypeId],
        id: MonoId,
    ) -> MonoId {
        let def = if let Def::Union(u) = &self.ctx.defs[def_id.0 as usize] {
            u.clone()
        } else {
            unreachable!()
        };

        let mut subst_map = HashMap::new();
        for (i, param) in def.generics.iter().enumerate() {
            subst_map.insert(param.name, args[i]);
        }

        let mut mangled_name = self.ctx.resolve(def.name).to_string();
        for arg in args {
            mangled_name.push_str(&format!("_{}", arg.0));
        }

        let mut mast_fields = Vec::new();
        let mut max_size = 0;
        let mut largest_field_idx = 0;

        for (idx, f) in def.fields.iter().enumerate() {
            let raw_ty = self
                .ctx
                .node_types
                .get(&f.type_node.id)
                .copied()
                .unwrap_or(TypeId::ERROR);
            let conc_ty = {
                let mut subst = Substituter::new(&mut self.ctx.type_registry, &subst_map);
                subst.substitute(raw_ty)
            };
            mast_fields.push(MastField {
                name: f.name,
                ty: conc_ty,
            });
            let mut ce = crate::sema::typeck::const_eval::ConstEvaluator::new(self.ctx);
            let size = ce.compute_type_size(conc_ty);

            if size > max_size {
                max_size = size;
                largest_field_idx = idx;
            }
        }

        self.module.structs.push(MastStruct {
            id,
            name: mangled_name,
            fields: mast_fields,
            is_extern: false,
            is_union: true,
            largest_field_idx,
            attributes: vec![],
        });
        id
    }

    fn instantiate_adt(&mut self, def_id: crate::sema::ty::DefId, args: &[TypeId]) -> MonoId {
        let key = (def_id, args.to_vec());
        if let Some(&id) = self.mono_cache.get(&key) {
            return id;
        }

        let wrapper_id = self.new_mono_id();
        let payload_union_id = self.new_mono_id();
        self.mono_cache.insert(key, wrapper_id);
        self.adt_union_map.insert(wrapper_id, payload_union_id);

        let def = if let Def::Adt(a) = &self.ctx.defs[def_id.0 as usize] {
            a.clone()
        } else {
            unreachable!()
        };

        let mut mangled_name = self.ctx.resolve(def.name).to_string();
        for arg in args {
            mangled_name.push_str(&format!("_{}", arg.0));
        }

        let mut subst_map = HashMap::new();
        for (i, param) in def.generics.iter().enumerate() {
            subst_map.insert(param.name, args[i]);
        }

        // 1. 构建内部的 Payload Union
        let mut union_fields = Vec::new();
        let mut largest_idx = 0;
        let mut max_size = 0;

        for (idx, variant) in def.variants.iter().enumerate() {
            let field_ty = if let Some(payload_ast) = &variant.payload_type {
                let raw_ty = self
                    .ctx
                    .node_types
                    .get(&payload_ast.id)
                    .copied()
                    .unwrap_or(TypeId::ERROR);
                let conc_ty = {
                    let mut subst = Substituter::new(&mut self.ctx.type_registry, &subst_map);
                    subst.substitute(raw_ty)
                };
                conc_ty
            } else {
                TypeId::VOID // LLVM 中对于空 Union 的处理可以是 i8 或者忽略
            };

            union_fields.push(MastField {
                name: variant.name,
                ty: field_ty,
            });

            if field_ty != TypeId::VOID && field_ty != TypeId::ERROR {
                let size = {
                    let mut ce = crate::sema::typeck::const_eval::ConstEvaluator::new(self.ctx);
                    ce.compute_type_size(field_ty)
                };

                if size > max_size {
                    max_size = size;
                    largest_idx = idx;
                }
            }
        }

        self.module.structs.push(MastStruct {
            id: payload_union_id,
            name: format!("{}_payload", mangled_name),
            fields: union_fields,
            is_extern: false,
            is_union: true,
            largest_field_idx: largest_idx,
            attributes: vec![],
        });

        // 2. 构建外部的 Wrapper Struct (Tag + Union)
        // 动态获取并泛型替换 ADT 的 Tag 类型
        let tag_ty = if let Some(bt) = &def.backing_type {
            let raw_tag_ty = self
                .ctx
                .node_types
                .get(&bt.id)
                .copied()
                .unwrap_or(TypeId::U32);
            let mut subst = Substituter::new(&mut self.ctx.type_registry, &subst_map);
            subst.substitute(raw_tag_ty)
        } else {
            TypeId::U32 // 如果没有指定，默认退化为 u32
        };

        let union_ty = self
            .ctx
            .type_registry
            .intern(TypeKind::AdtPayload(def_id, args.to_vec()));

        self.module.structs.push(MastStruct {
            id: wrapper_id,
            name: mangled_name,
            fields: vec![
                MastField {
                    name: self.ctx.intern("__tag"),
                    ty: tag_ty,
                },
                MastField {
                    name: self.ctx.intern("__payload"),
                    ty: union_ty,
                },
            ],
            is_extern: false,
            is_union: false,
            largest_field_idx: 0,
            attributes: vec![],
        });

        wrapper_id
    }

    fn lower_global(&mut self, g: &crate::sema::def::GlobalDef) {
        let id = *self
            .global_map
            .get(&g.id)
            .expect("Global MonoId should be pre-allocated");
        let ty = self
            .ctx
            .node_types
            .get(&g.value.id)
            .copied()
            .unwrap_or(TypeId::ERROR);

        let is_mut = matches!(
            self.ctx
                .type_registry
                .get(self.ctx.type_registry.normalize(ty)),
            TypeKind::Mut(_)
        );

        let init = if !g.is_extern {
            Some(self.lower_expr(&g.value, &HashMap::new(), Some(ty)))
        } else {
            None
        };

        self.module.globals.push(MastGlobal {
            id,
            name: self.ctx.resolve(g.name).to_string(),
            ty,
            is_mut,
            init,
            is_extern: g.is_extern,
            attributes: self.extract_meta_items(&g.attributes),
        });
    }

    // ==========================================
    //          Block & Defer Unrolling
    // ==========================================

    fn lower_block_as_body(
        &mut self,
        block_expr: &Expr,
        subst_map: &HashMap<SymbolId, TypeId>,
        expected_ty: TypeId,
    ) -> MastBlock {
        self.defer_stack.push(Vec::new());
        self.local_types.push(HashMap::new());

        let mut stmts = Vec::new();
        let mut result = None;

        if let ExprKind::Block {
            stmts: ast_stmts,
            result: ast_res,
        } = &block_expr.kind
        {
            for stmt in ast_stmts {
                match &stmt.kind {
                    ast::StmtKind::ExprStmt(e) | ast::StmtKind::ExprValue(e) => {
                        if let ExprKind::Defer { expr: def_expr } = &e.kind {
                            let lowered = self.lower_expr(def_expr, subst_map, None);
                            self.defer_stack.last_mut().unwrap().push(lowered);
                        } else {
                            if let ExprKind::Let { name, init } = &e.kind {
                                let init_mast = self.lower_expr(init, subst_map, None);
                                // 如果是忽略绑定，转换为单纯的 ExprStmt 执行副作用
                                if self.ctx.resolve(*name) == "_" {
                                    stmts.push(MastStmt::Expr(init_mast));
                                } else {
                                    let var_ty = init_mast.ty;
                                    self.local_types.last_mut().unwrap().insert(*name, var_ty);
                                    stmts.push(MastStmt::Let {
                                        name: *name,
                                        ty: var_ty,
                                        init: init_mast,
                                    });
                                }
                            } else {
                                let lowered = self.lower_expr(e, subst_map, None);
                                if !matches!(e.kind, ExprKind::Static { .. }) {
                                    stmts.push(MastStmt::Expr(lowered));
                                }
                            }
                        }
                    }
                }
            }
            if let Some(res) = ast_res {
                result = Some(Box::new(self.lower_expr(res, subst_map, Some(expected_ty))));
            }
        } else {
            result = Some(Box::new(self.lower_expr(
                block_expr,
                subst_map,
                Some(expected_ty),
            )));
        }

        let popped_defers = self.defer_stack.pop().unwrap();
        let mut defers = Vec::new();
        for d in popped_defers.into_iter().rev() {
            defers.push(d); // 保持 LIFO 顺序存入单独的数组
        }

        self.local_types.pop();
        MastBlock {
            stmts,
            result,
            defers,
        } // 将 defers 独立传递给后端
    }

    // ==========================================
    //            Lambda Lifting
    // ==========================================

    fn lower_lambda_expr(
        &mut self,
        params: &[ast::FuncParam],
        body: &Expr,
        concrete_ty: TypeId,
        subst_map: &HashMap<SymbolId, TypeId>,
    ) -> MastExprKind {
        let mono_id = self.new_mono_id();

        // 1. 生成独一无二的内部函数名，直接映射到 C ABI
        let lambda_name = format!("__kern_lambda_{}", mono_id.0);

        // 2. 从 TypeId 提取确切的签名
        let (param_tys, ret_ty) = if let TypeKind::Function { params, ret, .. } =
            self.ctx.type_registry.get(concrete_ty)
        {
            (params.clone(), *ret)
        } else {
            unreachable!()
        };

        let mut mast_params = Vec::new();
        for (i, p) in params.iter().enumerate() {
            mast_params.push(MastParam {
                name: p.name,
                ty: param_tys[i],
            });
        }

        // 暂时“屏蔽”外部所有的局部变量
        // 通过将 local_types 替换为全新的栈，Lambda 内部在降级时将绝对看不见外部变量。
        let saved_local_types = std::mem::take(&mut self.local_types);

        // 为 Lambda 开启自己独有的局部作用域
        self.local_types.push(HashMap::new());
        for p in &mast_params {
            self.local_types.last_mut().unwrap().insert(p.name, p.ty);
        }

        // 3. 降级 Lambda 的函数体 (Block)
        let body_block = self.lower_block_as_body(body, subst_map, ret_ty);

        self.local_types.pop();

        // 恢复外部的作用域
        self.local_types = saved_local_types;

        // 4. 将提取出的逻辑打包成一个全局的 MastFunction
        let mast_fn = MastFunction {
            id: mono_id,
            name: lambda_name,
            params: mast_params,
            ret_ty,
            body: Some(body_block),
            is_extern: false,
            is_variadic: false,
            attributes: vec![],
        };

        // 强势插入到模块的顶层函数列表中
        self.module.functions.push(mast_fn);

        // 5. 在原地返回一个指向该新函数的指针引用
        MastExprKind::FuncRef(mono_id)
    }

    // ==========================================
    //          Expression Lowering Dispatcher
    // ==========================================

    fn lower_expr(
        &mut self,
        expr: &Expr,
        subst_map: &HashMap<SymbolId, TypeId>,
        expected_ty: Option<TypeId>,
    ) -> MastExpr {
        let raw_ty = self.resolve_expr_type(expr);

        let mut subst = Substituter::new(&mut self.ctx.type_registry, subst_map);
        let concrete_ty = subst.substitute(raw_ty);
        let mut exp_ty = expected_ty.unwrap_or(concrete_ty);

        if exp_ty == TypeId::ERROR {
            self.ctx
                .emit_ice(expr.span, "Lowering encountered an unresolved ERROR type.");
            // 立即打印并终止降级，防止带着污染数据进入 LLVM 导致玄学 Panic
            self.ctx.print_diagnostics();
            std::process::exit(1);
        }

        // 统一剥离最外层的 Mut (后端只需要物理类型)
        exp_ty = self.strip_mut_modifier(exp_ty);

        let mast_kind = match &expr.kind {
            ExprKind::Integer(val) => MastExprKind::Integer(*val),
            ExprKind::Float(val) => MastExprKind::Float(*val),
            ExprKind::Bool(val) => MastExprKind::Bool(*val),
            ExprKind::Char(c) => MastExprKind::Integer(*c as u32 as u128),
            ExprKind::String(s) => self.lower_string_literal(s, expr.span),
            ExprKind::Identifier(name) => {
                let expr_ty = self
                    .ctx
                    .node_types
                    .get(&expr.id)
                    .copied()
                    .unwrap_or(TypeId::ERROR);
                let norm_ty = self.ctx.type_registry.normalize(expr_ty);

                if let TypeKind::FnDef(fn_id, fn_args) = self.ctx.type_registry.get(norm_ty).clone()
                {
                    let mono_id = self.instantiate_function(fn_id, &fn_args);
                    MastExprKind::FuncRef(mono_id)
                } else {
                    let kind = self.lower_identifier(*name);

                    // 检测是否属于非法闭包捕获
                    if let MastExprKind::Var(v) = kind {
                        let mut found = false;
                        for scope in self.local_types.iter().rev() {
                            if scope.contains_key(&v) {
                                found = true;
                                break;
                            }
                        }
                        if !found {
                            let var_str = self.ctx.resolve(v).to_string();
                            self.ctx.struct_error(expr.span, "closures cannot capture environmental variables in Kern")
                                .with_hint(format!("variable `{}` belongs to an outer scope", var_str))
                                .with_hint("Kern anonymous functions compile directly to static C function pointers")
                                .emit();
                        }
                    }
                    kind
                }
            }

            ExprKind::Let { init, .. } => {
                return self.lower_expr(init, subst_map, Some(concrete_ty));
            }
            ExprKind::Static { name, init } => {
                self.lower_static_decl(*name, init, subst_map, concrete_ty)
            }

            ExprKind::Binary { lhs, op, rhs } => {
                self.lower_binary(lhs, *op, rhs, subst_map, expr.span)
            }
            ExprKind::Unary { op, operand } => self.lower_unary(*op, operand, subst_map),

            ExprKind::Call { callee, args } => self.lower_call(callee, args, subst_map, expr.span),
            ExprKind::FieldAccess { lhs, field } => {
                // 通过 AST 节点的推导类型直接判断是否是静态函数
                // 解决跨模块调用时，Scope 丢失导致的误判问题
                let expr_ty = self
                    .ctx
                    .node_types
                    .get(&expr.id)
                    .copied()
                    .unwrap_or(TypeId::ERROR);
                let norm_ty = self.ctx.type_registry.normalize(expr_ty);

                if let TypeKind::FnDef(fn_id, fn_args) = self.ctx.type_registry.get(norm_ty).clone()
                {
                    let mono_id = self.instantiate_function(fn_id, &fn_args);
                    return MastExpr::new(exp_ty, MastExprKind::FuncRef(mono_id), expr.span);
                }

                // 老老实实走普通的结构体/联合体字段访问
                self.lower_field_access(lhs, *field, subst_map, expr.span)
            }
            ExprKind::IndexAccess { lhs, index } => self.lower_index_access(lhs, index, subst_map),

            ExprKind::DataInit { literal, .. } => {
                self.lower_data_init(literal, subst_map, concrete_ty, expr.span)
            }
            ExprKind::EnumLiteral(variant_name) => {
                self.lower_enum_literal(*variant_name, concrete_ty)
            }

            ExprKind::As { lhs, target } => {
                return self.lower_as_expr(lhs, target, concrete_ty, subst_map, expr.span);
            }

            ExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => self.lower_if(cond, then_branch, else_branch.as_deref(), subst_map, exp_ty),
            ExprKind::For {
                init,
                cond,
                post,
                body,
            } => self.lower_for(
                init.as_deref(),
                cond.as_deref(),
                post.as_deref(),
                body,
                subst_map,
                expr.span,
            ),
            ExprKind::Switch {
                target,
                cases,
                default_case,
            } => self.lower_switch(target, cases, default_case.as_deref(), subst_map, exp_ty),
            ExprKind::Match { target, arms } => self.lower_match(target, arms, subst_map, exp_ty),
            ExprKind::Lambda {
                params,
                ret_type: _,
                body,
            } => self.lower_lambda_expr(params, body, concrete_ty, subst_map),
            ExprKind::Block { .. } => {
                MastExprKind::Block(self.lower_block_as_body(expr, subst_map, exp_ty))
            }

            ExprKind::Return(val) => {
                self.lower_return(val.as_deref(), subst_map, expr.span)
            }
            ExprKind::Assign { lhs, op, rhs } => self.lower_assign(lhs, *op, rhs, subst_map),
            ExprKind::GenericInstantiation { .. } => self.lower_generic_instantiation(concrete_ty),

            ExprKind::SelfValue => MastExprKind::Var(self.ctx.intern("self")),
            ExprKind::Break => self.lower_jump(MastExprKind::Break, expr.span),
            ExprKind::Continue => self.lower_jump(MastExprKind::Continue, expr.span),
            ExprKind::Undef => MastExprKind::Undef,

            ExprKind::SliceOp { .. } => {
                unreachable!("SliceOp should be handled or forbidden before lowering")
            }
            _ => unreachable!("Unhandled ExprKind in lowering: {:?}", expr.kind),
        };

        self.apply_implicit_cast(mast_kind, concrete_ty, exp_ty, expr.span)
    }

    // ==========================================
    //          Lowering Helpers
    // ==========================================

    fn resolve_expr_type(&self, expr: &Expr) -> TypeId {
        let raw_ty = self
            .ctx
            .node_types
            .get(&expr.id)
            .copied()
            .unwrap_or(TypeId::ERROR);
        if raw_ty == TypeId::ERROR {
            if let ExprKind::Identifier(name) = &expr.kind {
                for scope in self.local_types.iter().rev() {
                    if let Some(&local_ty) = scope.get(name) {
                        return local_ty;
                    }
                }
            }
        }
        raw_ty
    }

    fn strip_mut_modifier(&self, mut ty: TypeId) -> TypeId {
        loop {
            let norm = self.ctx.type_registry.normalize(ty);
            if let TypeKind::Mut(inner) = self.ctx.type_registry.get(norm) {
                ty = *inner;
            } else {
                return norm;
            }
        }
    }

    fn lower_string_literal(&mut self, s: &str, span: Span) -> MastExprKind {
        let global_id = self.new_mono_id();
        let len = s.len() as u64;
        let array_ty = self.ctx.type_registry.intern(TypeKind::Array {
            elem: TypeId::U8,
            len,
        });

        self.module.globals.push(MastGlobal {
            id: global_id,
            name: format!(".str.{}", global_id.0),
            ty: array_ty,
            is_mut: false,
            init: Some(MastExpr::new(
                array_ty,
                MastExprKind::StringLiteral(s.to_string()),
                span,
            )),
            is_extern: false,
            attributes: vec![],
        });

        let data_ptr = MastExpr::new(
            self.ctx.type_registry.intern(TypeKind::Pointer(array_ty)),
            MastExprKind::AddressOf(Box::new(MastExpr::new(
                array_ty,
                MastExprKind::GlobalRef(global_id),
                span,
            ))),
            span,
        );
        let meta = MastExpr::new(TypeId::USIZE, MastExprKind::Integer(len as u128), span);

        MastExprKind::ConstructFatPointer {
            data_ptr: Box::new(data_ptr),
            meta: Box::new(meta),
        }
    }

    fn lower_identifier(&mut self, name: SymbolId) -> MastExprKind {
        if let Some(info) = self.ctx.scopes.resolve(name).cloned() {
            match info.kind {
                crate::sema::scope::SymbolKind::Static | crate::sema::scope::SymbolKind::Const => {
                    let def_id = info.def_id.unwrap();
                    if let Some(&mono_id) = self.global_map.get(&def_id) {
                        MastExprKind::GlobalRef(mono_id)
                    } else {
                        unreachable!()
                    }
                }
                crate::sema::scope::SymbolKind::Function => {
                    let fn_def_id = info.def_id.unwrap();
                    let mono_id = self.instantiate_function(fn_def_id, &[]);
                    MastExprKind::FuncRef(mono_id)
                }
                _ => MastExprKind::Var(name),
            }
        } else {
            MastExprKind::Var(name)
        }
    }

    fn lower_static_decl(
        &mut self,
        name: SymbolId,
        init: &Expr,
        subst_map: &HashMap<SymbolId, TypeId>,
        concrete_ty: TypeId,
    ) -> MastExprKind {
        let global_id = self.new_mono_id();
        let lower_init = self.lower_expr(init, subst_map, Some(concrete_ty));
        let is_mut = matches!(
            self.ctx
                .type_registry
                .get(self.ctx.type_registry.normalize(concrete_ty)),
            TypeKind::Mut(_)
        );

        self.module.globals.push(MastGlobal {
            id: global_id,
            name: format!("local_static_{}_{}", self.ctx.resolve(name), global_id.0),
            ty: concrete_ty,
            is_mut,
            init: Some(lower_init),
            is_extern: false,
            attributes: vec![],
        });
        MastExprKind::GlobalRef(global_id)
    }

    fn lower_binary(
        &mut self,
        lhs: &Expr,
        op: ast::BinaryOperator,
        rhs: &Expr,
        subst_map: &HashMap<SymbolId, TypeId>,
        span: Span,
    ) -> MastExprKind {
        if op == ast::BinaryOperator::LogicalAnd {
            let l = self.lower_expr(lhs, subst_map, Some(TypeId::BOOL));
            let r = self.lower_expr(rhs, subst_map, Some(TypeId::BOOL));
            MastExprKind::If {
                cond: Box::new(l),
                then_branch: MastBlock {
                    stmts: vec![],
                    result: Some(Box::new(r)),
                    defers: vec![],
                },
                else_branch: Some(MastBlock {
                    stmts: vec![],
                    result: Some(Box::new(MastExpr::new(
                        TypeId::BOOL,
                        MastExprKind::Bool(false),
                        span,
                    ))),
                    defers: vec![],
                }),
            }
        } else if op == ast::BinaryOperator::LogicalOr {
            let l = self.lower_expr(lhs, subst_map, Some(TypeId::BOOL));
            let r = self.lower_expr(rhs, subst_map, Some(TypeId::BOOL));
            MastExprKind::If {
                cond: Box::new(l),
                then_branch: MastBlock {
                    stmts: vec![],
                    result: Some(Box::new(MastExpr::new(
                        TypeId::BOOL,
                        MastExprKind::Bool(true),
                        span,
                    ))),
                    defers: vec![],
                },
                else_branch: Some(MastBlock {
                    stmts: vec![],
                    result: Some(Box::new(r)),
                    defers: vec![],
                }),
            }
        } else {
            let l = self.lower_expr(lhs, subst_map, None);
            let r = self.lower_expr(rhs, subst_map, Some(l.ty));
            MastExprKind::Binary {
                op,
                lhs: Box::new(l),
                rhs: Box::new(r),
            }
        }
    }

    fn lower_unary(
        &mut self,
        op: ast::UnaryOperator,
        operand: &Expr,
        subst_map: &HashMap<SymbolId, TypeId>,
    ) -> MastExprKind {
        let op_mast = self.lower_expr(operand, subst_map, None);
        match op {
            ast::UnaryOperator::AddressOf => MastExprKind::AddressOf(Box::new(op_mast)),
            ast::UnaryOperator::PointerDeRef => MastExprKind::Deref(Box::new(op_mast)),
            _ => MastExprKind::Unary {
                op,
                operand: Box::new(op_mast),
            },
        }
    }

    fn lower_assign(
        &mut self,
        lhs: &Expr,
        op: ast::AssignmentOperator,
        rhs: &Expr,
        subst_map: &HashMap<SymbolId, TypeId>,
    ) -> MastExprKind {
        let l = self.lower_expr(lhs, subst_map, None);
        let r = self.lower_expr(rhs, subst_map, Some(l.ty));
        MastExprKind::Assign {
            op,
            lhs: Box::new(l),
            rhs: Box::new(r),
        }
    }

    fn lower_index_access(
        &mut self,
        lhs: &Expr,
        index: &Expr,
        subst_map: &HashMap<SymbolId, TypeId>,
    ) -> MastExprKind {
        let l = self.lower_expr(lhs, subst_map, None);
        let idx = self.lower_expr(index, subst_map, Some(TypeId::USIZE));
        MastExprKind::IndexAccess {
            lhs: Box::new(l),
            index: Box::new(idx),
        }
    }

    fn lower_if(
        &mut self,
        cond: &Expr,
        then_branch: &Expr,
        else_branch: Option<&Expr>,
        subst_map: &HashMap<SymbolId, TypeId>,
        exp_ty: TypeId,
    ) -> MastExprKind {
        let c = self.lower_expr(cond, subst_map, Some(TypeId::BOOL));
        let t = self.lower_block_as_body(then_branch, subst_map, exp_ty);
        let e = else_branch.map(|eb| self.lower_block_as_body(eb, subst_map, exp_ty));
        MastExprKind::If {
            cond: Box::new(c),
            then_branch: t,
            else_branch: e,
        }
    }

    fn lower_call(
        &mut self,
        callee: &Expr,
        args: &[Expr],
        subst_map: &HashMap<SymbolId, TypeId>,
        span: Span,
    ) -> MastExprKind {
        // 拦截 @asm 宏调用
        // 必须在查询节点类型之前，因为 @asm 不是一个真实的函数
        if let ExprKind::Identifier(sym) = &callee.kind {
            if self.ctx.resolve(*sym) == "@asm" {
                return self.lower_asm_call(args, subst_map, span);
            }
        }
        let mut receiver_mast = None;
        let mut is_method = false;
        let mut method_field_sym = None;

        // 1. 嗅探是否为方法调用
        if let ExprKind::FieldAccess { lhs, field } = &callee.kind {
            let lhs_ty = self
                .ctx
                .node_types
                .get(&lhs.id)
                .copied()
                .unwrap_or(TypeId::ERROR);
            let norm_lhs = self.ctx.type_registry.normalize(lhs_ty);
            let is_module = matches!(self.ctx.type_registry.get(norm_lhs), TypeKind::Module(_));

            if !is_module {
                let callee_ty = self
                    .ctx
                    .node_types
                    .get(&callee.id)
                    .copied()
                    .unwrap_or(TypeId::ERROR);
                let norm_callee = self.ctx.type_registry.normalize(callee_ty);

                if matches!(
                    self.ctx.type_registry.get(norm_callee),
                    TypeKind::FnDef(..) | TypeKind::Function { .. }
                ) {
                    is_method = true;
                    method_field_sym = Some(*field);
                    receiver_mast = Some(self.lower_expr(lhs, subst_map, None));
                }
            }
        }

        // 2. 提取预期的参数签名 (处理泛型替换)
        let norm_callee = self.ctx.type_registry.normalize(
            self.ctx
                .node_types
                .get(&callee.id)
                .copied()
                .unwrap_or(TypeId::ERROR),
        );
        let expected_param_tys = self.get_callee_expected_params(norm_callee);

        // 3. 准备实参 (处理方法调用的参数偏移)
        let mut arg_masts = Vec::new();
        for (i, a) in args.iter().enumerate() {
            let param_idx = if is_method { i + 1 } else { i };
            let exp_ty = expected_param_tys.get(param_idx).copied();
            arg_masts.push(self.lower_expr(a, subst_map, exp_ty));
        }

        // 4. 执行调用的具体分发
        if is_method {
            let field = method_field_sym.unwrap();
            let recv = receiver_mast.unwrap();
            self.lower_method_call(recv, field, arg_masts, norm_callee, span)
        } else {
            self.lower_normal_call(callee, arg_masts, subst_map)
        }
    }

    fn lower_asm_call(
        &mut self,
        args: &[Expr],
        subst_map: &HashMap<SymbolId, TypeId>,
        _span: Span,
    ) -> MastExprKind {
        let config_arg = &args[0];
        let fields = if let ExprKind::DataInit {
            literal: ast::DataLiteralKind::Struct(f),
            ..
        } = &config_arg.kind
        {
            f
        } else {
            unreachable!()
        };

        let mut asm_template = String::new();
        let mut is_volatile = false;

        let mut outputs = Vec::new();
        let mut inputs = Vec::new();
        let mut clobbers = Vec::new();

        for field in fields {
            let field_name = self.ctx.resolve(field.name);
            match field_name {
                "asm" => {
                    match &field.value.kind {
                        // 支持单字符串 asm: "nop"
                        ExprKind::String(s) => asm_template = s.clone(),
                        // 支持数组形式 asm: .{ "out dx, al", "in al, dx" }
                        ExprKind::DataInit {
                            literal: ast::DataLiteralKind::Array(elems),
                            ..
                        } => {
                            let mut lines = Vec::new();
                            for e in elems {
                                if let ExprKind::String(s) = &e.kind {
                                    lines.push(s.as_str());
                                }
                            }
                            asm_template = lines.join("\n");
                        }
                        _ => unreachable!(),
                    }
                }
                "volatile" => {
                    if let ExprKind::Bool(b) = &field.value.kind {
                        is_volatile = *b;
                    }
                }
                "outputs" => {
                    if let ExprKind::DataInit {
                        literal: ast::DataLiteralKind::Struct(regs),
                        ..
                    } = &field.value.kind
                    {
                        for reg in regs {
                            let reg_name = self.ctx.resolve(reg.name);
                            // LLVM 约束：reg -> "=r", freg -> "=f", eax -> "={eax}"
                            let constraint = if reg_name == "reg" {
                                "=r".to_string()
                            } else if reg_name == "freg" {
                                "=f".to_string()
                            } else {
                                format!("={{{}}}", reg_name)
                            };

                            let ptr_expr = self.lower_expr(&reg.value, subst_map, None);
                            let val_ty = self.ctx.type_registry.get_elem_type(ptr_expr.ty).unwrap();
                            outputs.push((constraint, ptr_expr, val_ty));
                        }
                    }
                }
                "inputs" => {
                    if let ExprKind::DataInit {
                        literal: ast::DataLiteralKind::Struct(regs),
                        ..
                    } = &field.value.kind
                    {
                        for reg in regs {
                            let reg_name = self.ctx.resolve(reg.name);
                            // LLVM 约束：reg -> "r", freg -> "f", eax -> "{eax}"
                            let constraint = if reg_name == "reg" {
                                "r".to_string()
                            } else if reg_name == "freg" {
                                "f".to_string()
                            } else {
                                format!("{{{}}}", reg_name)
                            };

                            let val_expr = self.lower_expr(&reg.value, subst_map, None);
                            inputs.push((constraint, val_expr));
                        }
                    }
                }
                "clobbers" => {
                    if let ExprKind::DataInit {
                        literal: ast::DataLiteralKind::Array(elems),
                        ..
                    } = &field.value.kind
                    {
                        for e in elems {
                            if let ExprKind::String(s) = &e.kind {
                                clobbers.push(format!("~{{{}}}", s));
                            }
                        }
                    }
                }
                _ => {}
            }
        }

        // 组装最终的 LLVM 约束字符串 (顺序必须是: outputs, inputs, clobbers)
        let mut all_constraints = Vec::new();
        let mut output_ptrs = Vec::new();
        let mut output_tys = Vec::new();
        for (c, ptr, ty) in outputs {
            all_constraints.push(c);
            output_ptrs.push(ptr);
            output_tys.push(ty);
        }

        let mut input_args = Vec::new();
        for (c, expr) in inputs {
            all_constraints.push(c);
            input_args.push(expr);
        }

        for c in clobbers {
            all_constraints.push(c);
        }

        MastExprKind::Asm(MastAsmBlock {
            asm_template,
            constraints: all_constraints.join(","),
            input_args,
            output_ptrs,
            output_tys,
            is_volatile,
        })
    }

    fn lower_method_call(
        &mut self,
        recv: MastExpr,
        field: SymbolId,
        mut arg_masts: Vec<MastExpr>,
        norm_callee: TypeId,
        span: Span,
    ) -> MastExprKind {
        let mut base_ty = recv.ty;
        loop {
            let norm = self.ctx.type_registry.normalize(base_ty);
            match self.ctx.type_registry.get(norm) {
                TypeKind::Mut(inner) | TypeKind::Pointer(inner) | TypeKind::VolatilePtr(inner) => {
                    base_ty = *inner
                }
                _ => break,
            }
        }
        let norm_base = self.ctx.type_registry.normalize(base_ty);

        // 分支 A：动态分发 (Trait Object 虚表查表)
        if let TypeKind::TraitObject(trait_id, _) = self.ctx.type_registry.get(norm_base) {
            let trait_def = if let Def::Trait(t) = &self.ctx.defs[trait_id.0 as usize] {
                t
            } else {
                unreachable!()
            };
            let vtable_idx = trait_def
                .methods
                .iter()
                .position(|m| m.name == field)
                .expect("Method not found in trait");
            let void_ptr_ty = self
                .ctx
                .type_registry
                .intern(TypeKind::Pointer(TypeId::VOID));

            let data_ptr = MastExpr::new(
                void_ptr_ty,
                MastExprKind::ExtractFatPtrData(Box::new(recv.clone())),
                span,
            );
            arg_masts.insert(0, data_ptr);

            let vtable_meta = MastExpr::new(
                TypeId::USIZE,
                MastExprKind::ExtractFatPtrMeta(Box::new(recv)),
                span,
            );
            let vtable_ptr_ty = self
                .ctx
                .type_registry
                .intern(TypeKind::Pointer(void_ptr_ty));

            let vtable_ptr = MastExpr::new(
                vtable_ptr_ty,
                MastExprKind::Cast {
                    kind: MastCastKind::IntToPtr,
                    operand: Box::new(vtable_meta),
                },
                span,
            );

            let func_ptr = MastExpr::new(
                void_ptr_ty,
                MastExprKind::IndexAccess {
                    lhs: Box::new(vtable_ptr),
                    index: Box::new(MastExpr::new(
                        TypeId::USIZE,
                        MastExprKind::Integer(vtable_idx as u128),
                        span,
                    )),
                },
                span,
            );

            let (ret_ty, is_variadic, mut patched_params) = if let TypeKind::Function {
                ret,
                is_variadic,
                params,
            } =
                self.ctx.type_registry.get(norm_callee)
            {
                (*ret, *is_variadic, params.clone())
            } else {
                unreachable!()
            };

            if !patched_params.is_empty() {
                patched_params[0] = void_ptr_ty;
            }

            let patched_fn_ty = self.ctx.type_registry.intern(TypeKind::Function {
                params: patched_params,
                ret: ret_ty,
                is_variadic,
            });
            let func_ptr_typed = MastExpr::new(
                patched_fn_ty,
                MastExprKind::Cast {
                    kind: MastCastKind::Bitcast,
                    operand: Box::new(func_ptr),
                },
                span,
            );

            return MastExprKind::Call {
                callee: Box::new(func_ptr_typed),
                args: arg_masts,
            };
        }

        // 分支 B：静态分发 (普通泛型方法)
        if let TypeKind::FnDef(method_id, generics) =
            self.ctx.type_registry.get(norm_callee).clone()
        {
            arg_masts.insert(0, recv);
            let func_id = self.instantiate_function(method_id, &generics);
            let func_ref = MastExpr::new(norm_callee, MastExprKind::FuncRef(func_id), span);
            return MastExprKind::Call {
                callee: Box::new(func_ref),
                args: arg_masts,
            };
        }

        unreachable!("Invalid method call resolution")
    }

    fn lower_normal_call(
        &mut self,
        callee: &Expr,
        mut arg_masts: Vec<MastExpr>,
        subst_map: &HashMap<SymbolId, TypeId>,
    ) -> MastExprKind {
        let callee_mast = self.lower_expr(callee, subst_map, None);
        if let TypeKind::FnDef(fn_id, fn_args) = self.ctx.type_registry.get(callee_mast.ty).clone()
        {
            // 拦截内置函数 (Intrinsic)
            if let Def::Function(f) = &self.ctx.defs[fn_id.0 as usize] {
                if f.is_intrinsic {
                    let name_str = self.ctx.resolve(f.name);

                    // 1. 编译期常量折叠: @sizeof[T]() -> usize
                    if name_str == "@sizeOf" {
                        // 从函数调用的泛型参数中提取 T
                        let target_ty = if let TypeKind::FnDef(_, args) =
                            self.ctx.type_registry.get(callee_mast.ty)
                        {
                            args[0] // T 是第一个泛型参数
                        } else {
                            TypeId::ERROR
                        };
                        let mut ce = crate::sema::typeck::const_eval::ConstEvaluator::new(self.ctx);
                        let size = ce.compute_type_size(target_ty);
                        return MastExprKind::Integer(size as u128);
                    }
                    // 2. 对齐计算 @alignOf[T]() -> usize
                    else if name_str == "@alignOf" {
                        let target_ty = if let TypeKind::FnDef(_, args) =
                            self.ctx.type_registry.get(callee_mast.ty)
                        {
                            args[0]
                        } else {
                            TypeId::ERROR
                        };
                        let mut ce = crate::sema::typeck::const_eval::ConstEvaluator::new(self.ctx);
                        let align = ce.compute_type_align(target_ty);
                        return MastExprKind::Integer(align as u128);
                    }
                    // 3. 整数转型: @intCast[T, U](val) -> U
                    else if name_str == "@intCast" {
                        let operand = arg_masts.remove(0);

                        // 从函数调用的泛型参数中提取目标类型 U
                        let target_ty = if let TypeKind::FnDef(_, args) =
                            self.ctx.type_registry.get(callee_mast.ty)
                        {
                            args[1] // U 是第二个泛型参数
                        } else {
                            TypeId::ERROR
                        };

                        // 动态判定 Trunc / ZeroExt / SignExt
                        let cast_kind = self.determine_int_cast_kind(operand.ty, target_ty);
                        return MastExprKind::Cast {
                            kind: cast_kind,
                            operand: Box::new(operand),
                        };
                    }
                    // 4. 浮点精度转换: @floatCast[T, U](val) -> U
                    else if name_str == "@floatCast" {
                        return MastExprKind::Cast {
                            kind: MastCastKind::FloatCast,
                            operand: Box::new(arg_masts.remove(0)),
                        };
                    }
                    // 5. 整型转浮点: @intToFloat[T, U](val) -> U
                    else if name_str == "@intToFloat" {
                        return MastExprKind::Cast {
                            kind: MastCastKind::IntToFloat,
                            operand: Box::new(arg_masts.remove(0)),
                        };
                    }
                    // 6. 浮点转整型: @floatToInt[T, U](val) -> U
                    else if name_str == "@floatToInt" {
                        return MastExprKind::Cast {
                            kind: MastCastKind::FloatToInt,
                            operand: Box::new(arg_masts.remove(0)),
                        };
                    }
                    // 7. 不可达: @unreachable() -> !
                    else if name_str == "@unreachable" {
                        return MastExprKind::Unreachable;
                    } else if name_str == "@popCount" {
                        return MastExprKind::BitIntrinsic {
                            kind: BitIntrinsicKind::PopCount,
                            operand: Box::new(arg_masts.remove(0)),
                        };
                    } else if name_str == "@clz" {
                        return MastExprKind::BitIntrinsic {
                            kind: BitIntrinsicKind::Clz,
                            operand: Box::new(arg_masts.remove(0)),
                        };
                    } else if name_str == "@ctz" {
                        return MastExprKind::BitIntrinsic {
                            kind: BitIntrinsicKind::Ctz,
                            operand: Box::new(arg_masts.remove(0)),
                        };
                    } else if name_str == "@bswap" {
                        return MastExprKind::BitIntrinsic {
                            kind: BitIntrinsicKind::Bswap,
                            operand: Box::new(arg_masts.remove(0)),
                        };
                    } else if name_str == "@trap" {
                        return MastExprKind::Trap;
                    } else if name_str == "@fence" {
                        return MastExprKind::Fence;
                    } else if name_str == "@breakpoint" {
                        return MastExprKind::Breakpoint;
                    } else if name_str == "@memcpy" {
                        return MastExprKind::Memcpy {
                            dest: Box::new(arg_masts.remove(0)),
                            src: Box::new(arg_masts.remove(0)),
                            len: Box::new(arg_masts.remove(0)),
                        };
                    } else if name_str == "@memset" {
                        return MastExprKind::Memset {
                            dest: Box::new(arg_masts.remove(0)),
                            val: Box::new(arg_masts.remove(0)),
                            len: Box::new(arg_masts.remove(0)),
                        };
                    }
                }
            }

            // 如果不是内置函数，走正常的实例化和函数调用逻辑
            let mono_id = self.instantiate_function(fn_id, &fn_args);
            let func_ref =
                MastExpr::new(callee_mast.ty, MastExprKind::FuncRef(mono_id), callee.span);
            MastExprKind::Call {
                callee: Box::new(func_ref),
                args: arg_masts,
            }
        } else {
            MastExprKind::Call {
                callee: Box::new(callee_mast),
                args: arg_masts,
            }
        }
    }

    fn lower_field_access(
        &mut self,
        lhs: &Expr,
        field: SymbolId,
        subst_map: &HashMap<SymbolId, TypeId>,
        span: Span,
    ) -> MastExprKind {
        let expr_ty = self
            .ctx
            .node_types
            .get(&lhs.id)
            .copied()
            .unwrap_or(TypeId::ERROR);
        let norm_expr = self.ctx.type_registry.normalize(expr_ty);

        if matches!(
            self.ctx.type_registry.get(norm_expr),
            TypeKind::FnDef(..) | TypeKind::Function { .. }
        ) {
            unreachable!(
                "Attempted to access method `{}` without calling it. Bound Methods are not supported.",
                self.ctx.resolve(field)
            );
        }

        let l = self.lower_expr(lhs, subst_map, None);
        let mut base_ty = l.ty;
        let mut deref_expr = l.clone();

        loop {
            let norm = self.ctx.type_registry.normalize(base_ty);
            match self.ctx.type_registry.get(norm) {
                TypeKind::Mut(inner) => base_ty = *inner,
                TypeKind::Pointer(inner) | TypeKind::VolatilePtr(inner) => {
                    base_ty = *inner;
                    deref_expr =
                        MastExpr::new(base_ty, MastExprKind::Deref(Box::new(deref_expr)), span);
                }
                _ => break,
            }
        }

        // 拦截 Enum 变体的访问 (例如 Color.Red)
        if let TypeKind::Def(def_id, _) = self
            .ctx
            .type_registry
            .get(self.ctx.type_registry.normalize(base_ty))
        {
            if let Def::Enum(_) = &self.ctx.defs[def_id.0 as usize] {
                return self.lower_enum_literal(field, expr_ty); // 复用 enum literal 逻辑
            }
        }

        let field_idx = self.get_field_index(base_ty, field);
        let struct_def_info = if let TypeKind::Def(def_id, gen_args) = self
            .ctx
            .type_registry
            .get(self.ctx.type_registry.normalize(base_ty))
        {
            Some((*def_id, gen_args.clone()))
        } else {
            None
        };

        let struct_id = if let Some((def_id, gen_args)) = struct_def_info {
            self.instantiate_struct(def_id, &gen_args)
        } else {
            unreachable!("Field access on non-struct type");
        };

        MastExprKind::FieldAccess {
            lhs: Box::new(deref_expr),
            struct_id,
            field_idx,
        }
    }

    fn lower_data_init(
        &mut self,
        literal: &ast::DataLiteralKind,
        subst_map: &HashMap<SymbolId, TypeId>,
        concrete_ty: TypeId,
        span: Span,
    ) -> MastExprKind {
        match literal {
            ast::DataLiteralKind::Struct(fields) => {
                self.lower_struct_union_adt_init(fields, subst_map, concrete_ty)
            }
            ast::DataLiteralKind::Array(elems) => {
                self.lower_array_init(elems, subst_map, concrete_ty)
            }
            ast::DataLiteralKind::Repeat { value, .. } => {
                self.lower_repeat_init(value, subst_map, concrete_ty)
            }
            ast::DataLiteralKind::Scalar(inner) => {
                self.lower_scalar_init(inner, subst_map, concrete_ty, span)
            }
        }
    }

    fn lower_struct_union_adt_init(
        &mut self,
        fields: &[ast::StructFieldInit],
        subst_map: &HashMap<SymbolId, TypeId>,
        concrete_ty: TypeId,
    ) -> MastExprKind {
        let base_ty = self.strip_mut_modifier(concrete_ty);
        let norm = self.ctx.type_registry.get(base_ty).clone();

        // 1. 处理 ADT 变体初始化
        if let TypeKind::Adt(def_id, gen_args) = norm {
            let mono_id = self.instantiate_adt(def_id, &gen_args);
            let def = if let Def::Adt(a) = &self.ctx.defs[def_id.0 as usize] {
                a.clone()
            } else {
                unreachable!()
            };

            let init_f = &fields[0];
            let tag_val = def
                .variants
                .iter()
                .position(|v| v.name == init_f.name)
                .unwrap() as u128;

            let mut variant_subst_map = HashMap::new();
            for (i, param) in def.generics.iter().enumerate() {
                variant_subst_map.insert(param.name, gen_args[i]);
            }

            let raw_payload_ty = self
                .ctx
                .node_types
                .get(
                    &def.variants[tag_val as usize]
                        .payload_type
                        .as_ref()
                        .unwrap()
                        .id,
                )
                .copied()
                .unwrap();

            let conc_payload_ty = Substituter::new(&mut self.ctx.type_registry, &variant_subst_map)
                .substitute(raw_payload_ty);

            let payload_expr = self.lower_expr(&init_f.value, subst_map, Some(conc_payload_ty));

            return MastExprKind::AdtInit {
                adt_struct_id: mono_id,
                tag_value: tag_val,
                payload: Box::new(payload_expr),
            };
        }

        // 2. 处理 Struct / Union 初始化
        let (def_id, gen_args) = if let TypeKind::Def(id, args) = norm {
            (id, args)
        } else {
            unreachable!()
        };

        let mono_id = self.instantiate_struct(def_id, &gen_args);
        let def = self.ctx.defs[def_id.0 as usize].clone();

        match def {
            Def::Struct(s) => {
                let mut struct_subst_map = HashMap::new();
                for (i, param) in s.generics.iter().enumerate() {
                    struct_subst_map.insert(param.name, gen_args[i]);
                }

                let mut ordered_fields = Vec::new();
                for f_def in &s.fields {
                    let raw_f_ty = self
                        .ctx
                        .node_types
                        .get(&f_def.type_node.id)
                        .copied()
                        .unwrap_or(TypeId::ERROR);
                    let conc_f_ty =
                        Substituter::new(&mut self.ctx.type_registry, &struct_subst_map)
                            .substitute(raw_f_ty);

                    if let Some(init_f) = fields.iter().find(|f| f.name == f_def.name) {
                        ordered_fields.push(self.lower_expr(
                            &init_f.value,
                            subst_map,
                            Some(conc_f_ty),
                        ));
                    } else {
                        ordered_fields.push(self.lower_expr(
                            f_def.default_value.as_ref().unwrap(),
                            subst_map,
                            Some(conc_f_ty),
                        ));
                    }
                }
                MastExprKind::StructInit {
                    struct_id: mono_id,
                    fields: ordered_fields,
                }
            }
            Def::Union(u) => {
                let mut union_subst_map = HashMap::new();
                for (i, param) in u.generics.iter().enumerate() {
                    union_subst_map.insert(param.name, gen_args[i]);
                }
                let init_f = &fields[0];
                let field_idx = u.fields.iter().position(|f| f.name == init_f.name).unwrap();
                let raw_f_ty = self
                    .ctx
                    .node_types
                    .get(&u.fields[field_idx].type_node.id)
                    .copied()
                    .unwrap_or(TypeId::ERROR);
                let conc_f_ty = Substituter::new(&mut self.ctx.type_registry, &union_subst_map)
                    .substitute(raw_f_ty);

                let val_expr = self.lower_expr(&init_f.value, subst_map, Some(conc_f_ty));
                MastExprKind::UnionInit {
                    union_id: mono_id,
                    field_idx,
                    value: Box::new(val_expr),
                }
            }
            _ => unreachable!(),
        }
    }

    fn lower_array_init(
        &mut self,
        elems: &[Expr],
        subst_map: &HashMap<SymbolId, TypeId>,
        concrete_ty: TypeId,
    ) -> MastExprKind {
        let elem_ty = self.ctx.type_registry.get_elem_type(concrete_ty);
        let lowered_elems = elems
            .iter()
            .map(|e| self.lower_expr(e, subst_map, elem_ty))
            .collect();
        MastExprKind::ArrayInit(lowered_elems)
    }

    fn lower_repeat_init(
        &mut self,
        value: &Expr,
        subst_map: &HashMap<SymbolId, TypeId>,
        concrete_ty: TypeId,
    ) -> MastExprKind {
        let elem_ty = self.ctx.type_registry.get_elem_type(concrete_ty);
        let elem = self.lower_expr(value, subst_map, elem_ty);
        let array_len = if let TypeKind::Array { len, .. } = self
            .ctx
            .type_registry
            .get(self.ctx.type_registry.normalize(concrete_ty))
        {
            *len
        } else {
            0
        };
        MastExprKind::ArrayInit(vec![elem; array_len as usize])
    }

    fn lower_scalar_init(
        &mut self,
        inner: &Expr,
        subst_map: &HashMap<SymbolId, TypeId>,
        concrete_ty: TypeId,
        span: Span,
    ) -> MastExprKind {
        let base_ty = self.strip_mut_modifier(concrete_ty);
        let norm = self.ctx.type_registry.get(base_ty).clone();

        // 1. 无负载 ADT 初始化: `.{ None }`
        if let TypeKind::Adt(def_id, gen_args) = norm {
            let mono_id = self.instantiate_adt(def_id, &gen_args);
            let def = if let Def::Adt(a) = &self.ctx.defs[def_id.0 as usize] {
                a.clone()
            } else {
                unreachable!()
            };

            let variant_name = if let ExprKind::Identifier(id) = &inner.kind {
                *id
            } else {
                unreachable!()
            };

            let tag_val = def
                .variants
                .iter()
                .position(|v| v.name == variant_name)
                .unwrap() as u128;

            return MastExprKind::AdtInit {
                adt_struct_id: mono_id,
                tag_value: tag_val,
                payload: Box::new(MastExpr::new(TypeId::VOID, MastExprKind::Undef, inner.span)),
            };
        }
        // 2. Trait Object 组装: `Reader.{ ptr }`
        else if let TypeKind::TraitObject(..) = norm {
            let l = self.lower_expr(inner, subst_map, None);
            let vtable_id = self.get_or_create_vtable(l.ty, concrete_ty);

            let global_array_ty = self
                .module
                .globals
                .iter()
                .find(|g| g.id == vtable_id)
                .unwrap()
                .ty;
            let array_ptr_ty = self
                .ctx
                .type_registry
                .intern(TypeKind::Pointer(global_array_ty));

            return MastExprKind::ConstructFatPointer {
                data_ptr: Box::new(l),
                meta: Box::new(MastExpr::new(
                    TypeId::USIZE,
                    MastExprKind::Cast {
                        kind: MastCastKind::PtrToInt,
                        operand: Box::new(MastExpr::new(
                            array_ptr_ty,
                            MastExprKind::AddressOf(Box::new(MastExpr::new(
                                global_array_ty,
                                MastExprKind::GlobalRef(vtable_id),
                                span,
                            ))),
                            span,
                        )),
                    },
                    span,
                )),
            };
        }
        // 3. 基础标量兜底
        else {
            self.lower_expr(inner, subst_map, Some(concrete_ty)).kind
        }
    }

    fn lower_enum_literal(&mut self, variant_name: SymbolId, concrete_ty: TypeId) -> MastExprKind {
        let norm_ty = self.ctx.type_registry.normalize(concrete_ty);
        let enum_def = if let TypeKind::Def(def_id, _) = self.ctx.type_registry.get(norm_ty) {
            if let Def::Enum(e) = &self.ctx.defs[def_id.0 as usize] {
                e.clone()
            } else {
                unreachable!()
            }
        } else {
            unreachable!()
        };

        let mut current_val: i128 = 0;
        for v in enum_def.variants {
            if let Some(v_expr) = v.value {
                let mut ce = crate::sema::typeck::const_eval::ConstEvaluator::new(self.ctx);
                if let Ok(val) = ce.eval_math(&v_expr) {
                    current_val = val;
                }
            }
            if v.name == variant_name {
                return MastExprKind::Integer(current_val as u128);
            }
            current_val += 1;
        }
        unreachable!(
            "Enum variant `{}` not found in lowered Enum",
            self.ctx.resolve(variant_name)
        );
    }

    fn lower_as_expr(
        &mut self,
        lhs: &Expr,
        target: &ast::TypeNode,
        concrete_ty: TypeId,
        subst_map: &HashMap<SymbolId, TypeId>,
        span: Span,
    ) -> MastExpr {
        let target_ty = self
            .ctx
            .node_types
            .get(&target.id)
            .copied()
            .unwrap_or(concrete_ty);
        let l = self.lower_expr(lhs, subst_map, None);
        let cast_kind = self.determine_cast_kind(l.ty, target_ty);

        MastExpr::new(
            target_ty,
            MastExprKind::Cast {
                kind: cast_kind,
                operand: Box::new(l),
            },
            span,
        )
    }

    fn lower_for(
        &mut self,
        init: Option<&Expr>,
        cond: Option<&Expr>,
        post: Option<&Expr>,
        body: &Expr,
        subst_map: &HashMap<SymbolId, TypeId>,
        span: Span,
    ) -> MastExprKind {
        let mut loop_stmts = Vec::new();

        if let Some(c) = cond {
            let c_expr = self.lower_expr(c, subst_map, Some(TypeId::BOOL));
            let not_c = MastExpr::new(
                TypeId::BOOL,
                MastExprKind::Unary {
                    op: ast::UnaryOperator::LogicalNot,
                    operand: Box::new(c_expr),
                },
                c.span,
            );

            loop_stmts.push(MastStmt::Expr(MastExpr::new(
                TypeId::VOID,
                MastExprKind::If {
                    cond: Box::new(not_c),
                    then_branch: MastBlock {
                        stmts: vec![MastStmt::Expr(MastExpr::new(
                            TypeId::VOID,
                            MastExprKind::Break,
                            c.span,
                        ))],
                        result: None,
                        defers: vec![],
                    },
                    else_branch: None,
                },
                c.span,
            )));
        }

        // 记录进入循环体前 defer_stack 的高度
        self.loop_frames.push(self.defer_stack.len());
        // 仅仅降级循环体，不包含 post
        loop_stmts.push(MastStmt::Expr(self.lower_expr(body, subst_map, None)));
        // 退出循环体，弹出边界
        self.loop_frames.pop();

        let body_block = MastBlock {
            stmts: loop_stmts,
            result: None,
            defers: vec![],
        };

        // 独立降级 post 语句，将其作为 Latch 块
        let latch_block = post.map(|p| MastBlock {
            stmts: vec![MastStmt::Expr(self.lower_expr(p, subst_map, None))],
            result: None,
            defers: vec![],
        });

        let loop_expr = MastExpr::new(
            TypeId::VOID,
            // 采用新的 AST 结构
            MastExprKind::Loop {
                body: body_block,
                latch: latch_block,
            },
            span,
        );

        if let Some(i) = init {
            let mut outer_stmts = Vec::new();
            if let ExprKind::Let {
                name,
                init: let_init,
            } = &i.kind
            {
                let lowered_init = self.lower_expr(let_init, subst_map, None);
                outer_stmts.push(MastStmt::Let {
                    name: *name,
                    ty: lowered_init.ty,
                    init: lowered_init,
                });
            } else {
                outer_stmts.push(MastStmt::Expr(self.lower_expr(i, subst_map, None)));
            }
            outer_stmts.push(MastStmt::Expr(loop_expr));
            MastExprKind::Block(MastBlock {
                stmts: outer_stmts,
                result: None,
                defers: vec![],
            })
        } else {
            loop_expr.kind
        }
    }

    fn lower_switch(
        &mut self,
        target: &Expr,
        cases: &[ast::SwitchCase],
        default_case: Option<&Expr>,
        subst_map: &HashMap<SymbolId, TypeId>,
        exp_ty: TypeId,
    ) -> MastExprKind {
        let t = self.lower_expr(target, subst_map, None);
        let mut mast_cases = Vec::new();

        for case in cases {
            let mut case_vals = Vec::new();
            for pat in &case.patterns {
                match pat {
                    ast::SwitchPattern::Value(val_expr) => {
                        if let Ok(val) =
                            crate::sema::typeck::const_eval::ConstEvaluator::new(self.ctx)
                                .eval_math(val_expr)
                        {
                            case_vals.push(val as u128);
                        }
                    }
                    ast::SwitchPattern::Range {
                        start,
                        end,
                        inclusive,
                    } => {
                        let mut ce = crate::sema::typeck::const_eval::ConstEvaluator::new(self.ctx);
                        if let (Ok(s), Ok(e)) = (ce.eval_math(start), ce.eval_math(end)) {
                            let end_bound = if *inclusive { e } else { e - 1 };
                            for v in s..=end_bound {
                                case_vals.push(v as u128);
                            }
                        }
                    }
                }
            }
            mast_cases.push(MastSwitchCase {
                values: case_vals,
                body: self.lower_block_as_body(&case.body, subst_map, exp_ty),
            });
        }
        let def_case = default_case.map(|b| self.lower_block_as_body(b, subst_map, exp_ty));
        MastExprKind::Switch {
            target: Box::new(t),
            cases: mast_cases,
            default_case: def_case,
        }
    }

    fn lower_match(
        &mut self,
        target: &Expr,
        arms: &[ast::MatchArm],
        subst_map: &HashMap<SymbolId, TypeId>,
        exp_ty: TypeId,
    ) -> MastExprKind {
        // 1. 降级目标表达式
        let t = self.lower_expr(target, subst_map, None);
        let base_ty = self.strip_mut_modifier(t.ty);

        let (def_id, gen_args) =
            if let TypeKind::Adt(id, args) = self.ctx.type_registry.get(base_ty).clone() {
                (id, args)
            } else {
                unreachable!()
            };
        let mono_id = self.instantiate_adt(def_id, &gen_args);
        let def = if let Def::Adt(a) = &self.ctx.defs[def_id.0 as usize] {
            a.clone()
        } else {
            unreachable!()
        };

        // 2. 提取 Tag (相当于 target.__tag)
        let tag_access = MastExpr::new(
            TypeId::U32,
            MastExprKind::FieldAccess {
                lhs: Box::new(t.clone()),
                struct_id: mono_id,
                field_idx: 0, // 第 0 个字段是 __tag
            },
            target.span,
        );

        // 3. 构建 Switch 分支
        let mut mast_cases = Vec::new();
        let mut def_case = None;

        for arm in arms {
            match &arm.pattern {
                ast::MatchPattern::Variant {
                    variant_name,
                    binding,
                    ..
                } => {
                    let tag_idx = def
                        .variants
                        .iter()
                        .position(|v| v.name == *variant_name)
                        .unwrap();

                    self.local_types.push(HashMap::new());
                    let mut arm_stmts = Vec::new();

                    // 如果有绑定 (例如 `.Ok: val`)，我们要在执行块前隐式生成提取动作
                    if let Some(bind_name) = binding {
                        let variant_def = &def.variants[tag_idx];

                        // 准备泛型环境
                        let mut variant_subst_map = HashMap::new();
                        for (i, param) in def.generics.iter().enumerate() {
                            variant_subst_map.insert(param.name, gen_args[i]);
                        }
                        let raw_payload_ty = self
                            .ctx
                            .node_types
                            .get(&variant_def.payload_type.as_ref().unwrap().id)
                            .copied()
                            .unwrap();
                        let conc_payload_ty =
                            Substituter::new(&mut self.ctx.type_registry, &variant_subst_map)
                                .substitute(raw_payload_ty);

                        // 提取 Payload (相当于 target.__payload.as_Variant)
                        // __payload 是 struct 的第 1 个字段
                        let union_access = MastExpr::new(
                            TypeId::VOID, // 中间类型无所谓
                            MastExprKind::FieldAccess {
                                lhs: Box::new(t.clone()),
                                struct_id: mono_id,
                                field_idx: 1,
                            },
                            arm.span,
                        );

                        // 再从 Union 中通过字段索引提取具体的 Payload
                        let target_union_id = *self
                            .adt_union_map
                            .get(&mono_id)
                            .expect("Kern ICE: Missing ADT to Union mapping");

                        let payload_extract = MastExpr::new(
                            conc_payload_ty,
                            MastExprKind::FieldAccess {
                                lhs: Box::new(union_access),
                                struct_id: target_union_id,
                                field_idx: tag_idx,
                            },
                            arm.span,
                        );

                        self.local_types
                            .last_mut()
                            .unwrap()
                            .insert(*bind_name, conc_payload_ty);
                        arm_stmts.push(MastStmt::Let {
                            name: *bind_name,
                            ty: conc_payload_ty,
                            init: payload_extract,
                        });
                    }

                    // 降级执行块，拼接到隐式提取语句之后
                    let mut block = self.lower_block_as_body(&arm.body, subst_map, exp_ty);
                    arm_stmts.append(&mut block.stmts);
                    block.stmts = arm_stmts;

                    self.local_types.pop();

                    mast_cases.push(MastSwitchCase {
                        values: vec![tag_idx as u128],
                        body: block,
                    });
                }
                ast::MatchPattern::CatchAll(_) => {
                    def_case = Some(self.lower_block_as_body(&arm.body, subst_map, exp_ty));
                }
            }
        }

        // 把 Match 降维打击成普通的 Switch 表达式！
        MastExprKind::Switch {
            target: Box::new(tag_access),
            cases: mast_cases,
            default_case: def_case,
        }
    }

    fn lower_return(
        &mut self,
        val: Option<&Expr>,
        subst_map: &HashMap<SymbolId, TypeId>,
        span: Span,
    ) -> MastExprKind {
        let v = val.map(|e| Box::new(self.lower_expr(e, subst_map, None)));
        let mut defer_stmts = Vec::new();

        // 倒序展开当前作用域栈中所有的 defer
        for stack in self.defer_stack.iter().rev() {
            for d in stack.iter().rev() {
                defer_stmts.push(MastStmt::Expr(d.clone()));
            }
        }

        if defer_stmts.is_empty() {
            MastExprKind::Return(v)
        } else {
            defer_stmts.push(MastStmt::Expr(MastExpr::new(
                TypeId::VOID,
                MastExprKind::Return(v),
                span,
            )));
            MastExprKind::Block(MastBlock {
                stmts: defer_stmts,
                result: None,
                defers: vec![],
            })
        }
    }

    /// 专门处理 Break 和 Continue 的 Defer 展开
    fn lower_jump(&mut self, jump_kind: MastExprKind, span: Span) -> MastExprKind {
        let mut defer_stmts = Vec::new();

        // 获取当前所属循环的起始栈深度
        let boundary = *self
            .loop_frames
            .last()
            .expect("Kern Sema failed to catch jump outside of loop");

        // 从当前栈顶一路向下倒序提取 defer，直到达到循环的边界
        for stack in self.defer_stack[boundary..].iter().rev() {
            for d in stack.iter().rev() {
                defer_stmts.push(MastStmt::Expr(d.clone()));
            }
        }

        if defer_stmts.is_empty() {
            jump_kind
        } else {
            // 将实际的跳转指令放在所有清理工作之后
            defer_stmts.push(MastStmt::Expr(MastExpr::new(
                TypeId::NEVER,
                jump_kind,
                span,
            )));
            MastExprKind::Block(MastBlock {
                stmts: defer_stmts,
                result: None,
                defers: vec![],
            })
        }
    }

    fn lower_generic_instantiation(&mut self, concrete_ty: TypeId) -> MastExprKind {
        let fn_info =
            if let TypeKind::FnDef(fn_id, fn_args) = self.ctx.type_registry.get(concrete_ty) {
                Some((*fn_id, fn_args.clone()))
            } else {
                None
            };
        if let Some((fn_id, fn_args)) = fn_info {
            let mono_id = self.instantiate_function(fn_id, &fn_args);
            MastExprKind::FuncRef(mono_id)
        } else {
            MastExprKind::Integer(0)
        }
    }

    fn get_callee_expected_params(&mut self, norm_callee: TypeId) -> Vec<TypeId> {
        match self.ctx.type_registry.get(norm_callee).clone() {
            TypeKind::Function { params, .. } => params,
            TypeKind::FnDef(def_id, gen_args) => {
                if let Def::Function(f) = &self.ctx.defs[def_id.0 as usize] {
                    if let Some(sig) = f.resolved_sig {
                        let norm_sig = self.ctx.type_registry.normalize(sig);
                        let raw_params = if let TypeKind::Function { params, .. } =
                            self.ctx.type_registry.get(norm_sig).clone()
                        {
                            params
                        } else {
                            Vec::new()
                        };

                        let mut all_generic_params = Vec::new();
                        if let Some(parent_id) = f.parent {
                            if let Def::Impl(impl_def) = &self.ctx.defs[parent_id.0 as usize] {
                                all_generic_params.extend(impl_def.generics.clone());
                            }
                        }
                        all_generic_params.extend(f.generics.clone());

                        let mut sig_subst_map = HashMap::new();
                        for (idx, param) in all_generic_params.iter().enumerate() {
                            if idx < gen_args.len() {
                                sig_subst_map.insert(param.name, gen_args[idx]);
                            }
                        }

                        let mut sig_subst =
                            Substituter::new(&mut self.ctx.type_registry, &sig_subst_map);
                        raw_params
                            .into_iter()
                            .map(|p| sig_subst.substitute(p))
                            .collect()
                    } else {
                        Vec::new()
                    }
                } else {
                    Vec::new()
                }
            }
            _ => Vec::new(),
        }
    }

    /// 应用 Kern 的核心隐式转换规则：不可变/可变数组可以退化为对应切片
    fn apply_implicit_cast(
        &mut self,
        mut mast_kind: MastExprKind,
        concrete_ty: TypeId,
        exp_ty: TypeId,
        span: Span,
    ) -> MastExpr {
        let conc_base = self.strip_mut_modifier(concrete_ty);
        let exp_base = self.strip_mut_modifier(exp_ty);

        if let TypeKind::Slice(_) = self.ctx.type_registry.get(exp_base) {
            if let TypeKind::Array { .. } = self.ctx.type_registry.get(conc_base) {
                mast_kind = MastExprKind::Cast {
                    kind: MastCastKind::ArrayToSlice,
                    operand: Box::new(MastExpr::new(concrete_ty, mast_kind, span)),
                };
            }
        }
        MastExpr::new(exp_ty, mast_kind, span)
    }

    // ==========================================
    //          Helpers
    // ==========================================

    fn get_field_index(&self, struct_ty: TypeId, field_name: SymbolId) -> usize {
        let norm = self.ctx.type_registry.normalize(struct_ty);
        if let TypeKind::Def(def_id, _) = self.ctx.type_registry.get(norm) {
            if let Def::Struct(s) = &self.ctx.defs[def_id.0 as usize] {
                return s.fields.iter().position(|f| f.name == field_name).unwrap();
            } else if let Def::Union(u) = &self.ctx.defs[def_id.0 as usize] {
                return u.fields.iter().position(|f| f.name == field_name).unwrap();
            }
        }
        0
    }

    fn determine_cast_kind(&mut self, from: TypeId, to: TypeId) -> MastCastKind {
        let f_norm = self.ctx.type_registry.normalize(from);
        let t_norm = self.ctx.type_registry.normalize(to);

        let f_int = self.ctx.type_registry.is_integer(f_norm);
        let t_int = self.ctx.type_registry.is_integer(t_norm);
        let f_ptr = matches!(
            self.ctx.type_registry.get(f_norm),
            TypeKind::Pointer(_) | TypeKind::VolatilePtr(_)
        );
        let t_ptr = matches!(
            self.ctx.type_registry.get(t_norm),
            TypeKind::Pointer(_) | TypeKind::VolatilePtr(_)
        );

        // 1. 指针与整数的相互转换 (Bit-pattern preserving)
        if f_ptr && t_ptr {
            return MastCastKind::Bitcast;
        }
        if f_int && t_ptr {
            return MastCastKind::IntToPtr;
        }
        if f_ptr && t_int {
            return MastCastKind::PtrToInt;
        }

        // 2. 整数到整数的精细转换
        if f_int && t_int {
            return self.determine_int_cast_kind(f_norm, t_norm);
        }

        // 兜底
        MastCastKind::Bitcast
    }

    /// 专门处理整数之间的转换逻辑 (未来应由 @zext, @truncate 等内置函数直接调用此逻辑)
    fn determine_int_cast_kind(&mut self, from: TypeId, to: TypeId) -> MastCastKind {
        let mut ce = crate::sema::typeck::const_eval::ConstEvaluator::new(self.ctx);
        let f_size = ce.compute_type_size(from);
        let t_size = ce.compute_type_size(to);

        if f_size > t_size {
            MastCastKind::Trunc
        } else if f_size < t_size {
            // 判断目标类型是否为有符号整数
            let is_signed = matches!(
                self.ctx.type_registry.get(to),
                TypeKind::Primitive(
                    PrimitiveType::I8
                        | PrimitiveType::I16
                        | PrimitiveType::I32
                        | PrimitiveType::I64
                        | PrimitiveType::I128
                        | PrimitiveType::ISize
                )
            );
            if is_signed {
                MastCastKind::SignExt
            } else {
                MastCastKind::ZeroExt
            }
        } else {
            // 大小相等 (例如 i32 到 u32，或者 i64 到 usize 在 64位机器上)
            MastCastKind::Bitcast
        }
    }

    // ==========================================
    //          VTable Generation Engine
    // ==========================================

    fn get_or_create_vtable(&mut self, source_ty: TypeId, trait_ty: TypeId) -> MonoId {
        // 1. 净化目标 Trait 类型 (去除 Mut 修饰符)
        let actual_trait_ty = self.strip_mut_modifier(trait_ty);

        // 2. 检查缓存，防止重复生成相同的 VTable
        let key = (source_ty, actual_trait_ty);
        if let Some(&id) = self.vtable_cache.get(&key) {
            return id;
        }

        // 3. 解析 Trait 定义
        let trait_def_id = match self.ctx.type_registry.get(actual_trait_ty) {
            TypeKind::TraitObject(id, _) => *id,
            _ => unreachable!(
                "Target must be a TraitObject, found: {:?}",
                self.ctx.type_registry.get(actual_trait_ty)
            ),
        };
        let trait_def = if let Def::Trait(t) = &self.ctx.defs[trait_def_id.0 as usize] {
            t.clone()
        } else {
            unreachable!()
        };

        // 4. 解析来源类型的基底 (跳过多层指针) 并获取其实参
        let (base_source_ty, source_args) = self.resolve_vtable_source_base(source_ty);

        // 5. 在全局 Defs 中寻找匹配的 Impl 块
        let impl_def = self
            .find_matching_impl_block(base_source_ty, trait_def_id)
            .expect("Impl block must exist for valid Trait Object cast. Sema missed this.");

        // 6. 生成 VTable 内容并注入全局常量区
        let vtable_id = self.new_mono_id();
        self.vtable_cache.insert(key, vtable_id);

        self.build_and_inject_vtable_global(
            vtable_id,
            source_ty,
            actual_trait_ty,
            &trait_def,
            &impl_def,
            &source_args,
        );

        vtable_id
    }

    /// 辅助方法 1：剥离来源指针的所有包装，获取真正的具名底层类型和泛型实参
    fn resolve_vtable_source_base(&self, source_ty: TypeId) -> (TypeId, Vec<TypeId>) {
        let mut base_ty = source_ty;
        loop {
            // 每次循环必须先 normalize，防止被 Alias 阻断剥离过程
            let norm = self.ctx.type_registry.normalize(base_ty);
            match self.ctx.type_registry.get(norm) {
                TypeKind::Pointer(inner) | TypeKind::VolatilePtr(inner) | TypeKind::Mut(inner) => {
                    base_ty = *inner;
                }
                _ => {
                    // 确保跳出循环时，base_ty 处于绝对的正规化状态
                    base_ty = norm;
                    break;
                }
            }
        }

        let source_args = match self.ctx.type_registry.get(base_ty) {
            TypeKind::Def(_, args) | TypeKind::Adt(_, args) => args.clone(),
            _ => Vec::new(),
        };

        (base_ty, source_args)
    }

    /// 辅助方法 2：在全局寻找 (SourceBaseType -> TargetTrait) 的确切 Impl 块实现
    fn find_matching_impl_block(
        &self,
        base_source_ty: TypeId,
        target_trait_id: crate::sema::ty::DefId,
    ) -> Option<crate::sema::def::ImplDef> {
        // 辅助闭包：提取底层类型的 DefId，完美兼容 Struct/Union (Def) 和 Enum (Adt)
        let get_base_def_id = |ty: TypeId| -> Option<crate::sema::ty::DefId> {
            let norm = self.ctx.type_registry.normalize(ty);
            match self.ctx.type_registry.get(norm) {
                TypeKind::Def(id, _) | TypeKind::Adt(id, _) => Some(*id),
                _ => None,
            }
        };

        let src_base_id = get_base_def_id(base_source_ty);
        let norm_src_base = self.ctx.type_registry.normalize(base_source_ty);

        for def in &self.ctx.defs {
            if let Def::Impl(impl_def) = def {
                if let Some(impl_trait_node) = &impl_def.trait_type {
                    // 检查 Impl 块声称实现的 Trait
                    let i_trait_ty = self
                        .ctx
                        .node_types
                        .get(&impl_trait_node.id)
                        .copied()
                        .unwrap_or(TypeId::ERROR);
                    let i_trait_norm = self.strip_mut_modifier(i_trait_ty);

                    if let TypeKind::TraitObject(i_trait_id, _) =
                        self.ctx.type_registry.get(i_trait_norm)
                    {
                        if *i_trait_id == target_trait_id {
                            // 检查 Impl 块的目标类型是否匹配
                            let i_target_ty = self
                                .ctx
                                .node_types
                                .get(&impl_def.target_type.id)
                                .copied()
                                .unwrap_or(TypeId::ERROR);
                            let (i_target_base, _) = self.resolve_vtable_source_base(i_target_ty);

                            // 1. 如果两者都是聚合类型 (Struct/Union/Enum)，比对 DefId (忽略具体泛型参数)
                            if let (Some(target_id), Some(src_id)) =
                                (get_base_def_id(i_target_base), src_base_id)
                            {
                                if target_id == src_id {
                                    return Some(impl_def.clone());
                                }
                            }
                            // 2. 兜底比对：支持标量类型匹配 (例如 impl Trait for i32)
                            else if self.ctx.type_registry.normalize(i_target_base)
                                == norm_src_base
                            {
                                return Some(impl_def.clone());
                            }
                        }
                    }
                }
            }
        }
        None
    }

    /// 辅助方法 3：将提取出来的方法单态化，组装成数组，并插入到全局 MastGlobal
    fn build_and_inject_vtable_global(
        &mut self,
        vtable_id: MonoId,
        source_ty: TypeId,
        actual_trait_ty: TypeId,
        trait_def: &crate::sema::def::TraitDef,
        impl_def: &crate::sema::def::ImplDef,
        source_args: &[TypeId],
    ) {
        let void_ptr_ty = self
            .ctx
            .type_registry
            .intern(TypeKind::Pointer(TypeId::VOID));
        let mut vtable_methods = Vec::new();

        // 遍历 Trait 定义的每一个方法契约
        for trait_method in &trait_def.methods {
            let mut method_mono_id = None;

            // 在 Impl 块中找到对应的实现
            for &m_id in &impl_def.methods {
                if let Def::Function(f) = &self.ctx.defs[m_id.0 as usize] {
                    if f.name == trait_method.name {
                        method_mono_id = Some(self.instantiate_function(m_id, source_args));
                        break;
                    }
                }
            }

            let m_id = method_mono_id
                .expect("Missing trait method implementation. Sema failed to enforce contract.");

            // 将单态化后的函数指针强转为 *void 存入虚表
            vtable_methods.push(MastExpr::new(
                void_ptr_ty,
                MastExprKind::FuncRef(m_id),
                crate::utils::Span::default(),
            ));
        }

        let vtable_len = vtable_methods.len() as u64;
        let vtable_array_ty = self.ctx.type_registry.intern(TypeKind::Array {
            elem: void_ptr_ty,
            len: vtable_len,
        });

        let vtable_init = MastExpr::new(
            vtable_array_ty,
            MastExprKind::ArrayInit(vtable_methods),
            crate::utils::Span::default(),
        );

        self.module.globals.push(MastGlobal {
            id: vtable_id,
            name: format!("__vtable_{}_{}", source_ty.0, actual_trait_ty.0),
            ty: vtable_array_ty,
            is_mut: false, // 虚表永远是静态不可变的只读数据
            init: Some(vtable_init),
            is_extern: false,
            attributes: vec![],
        });
    }

    fn extract_meta_items(&self, attrs: &[ast::Attribute]) -> Vec<ast::MetaItem> {
        let mut meta = Vec::new();
        for attr in attrs {
            if let ast::AttributeKind::Meta(items) = &attr.kind {
                meta.extend(items.clone());
            }
        }
        meta
    }
}
