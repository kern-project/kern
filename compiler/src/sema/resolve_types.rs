use crate::driver::Context;
use crate::parser::ast;
use crate::sema::def::Def;
use crate::sema::scope::{ScopeId, SymbolInfo, SymbolKind};
use crate::sema::ty::{DefId, TypeId, TypeKind};
use crate::sema::typeck::const_eval::ConstEvaluator;
use crate::utils::SymbolId;

pub struct TypeResolver<'a> {
    pub ctx: &'a mut Context,
}

impl<'a> TypeResolver<'a> {
    pub fn new(ctx: &'a mut Context) -> Self {
        Self { ctx }
    }

    /// 执行完整的类型解析 Pass
    pub fn resolve_all(&mut self) {
        // 基于模块层级进行遍历，以获取正确的词法作用域上下文
        let module_ids: Vec<DefId> = self
            .ctx
            .defs
            .iter()
            .filter_map(|def| {
                if let Def::Module(m) = def {
                    Some(m.id)
                } else {
                    None
                }
            })
            .collect();

        for mod_id in module_ids {
            self.resolve_module(mod_id);
        }
    }

    fn resolve_module(&mut self, mod_id: DefId) {
        let (mod_scope, items) = {
            if let Def::Module(m) = &self.ctx.defs[mod_id.0 as usize] {
                (m.scope_id, m.items.clone())
            } else {
                unreachable!()
            }
        };

        for item_id in items {
            self.resolve_item(item_id, mod_scope);
        }
    }

    /// 解析具体的项，为其开辟作用域并绑定泛型
    fn resolve_item(&mut self, item_id: DefId, parent_scope: ScopeId) {
        let def = self.ctx.defs[item_id.0 as usize].clone();

        match &def {
            Def::Function(f) => {
                self.ctx.scopes.set_current_scope(parent_scope);
                let func_scope = self.ctx.scopes.enter_scope();

                self.bind_generics(&f.generics, func_scope);
                if let Some(parent_id) = f.parent {
                    if let Def::Impl(i) = &self.ctx.defs[parent_id.0 as usize] {
                        let target_ty = self
                            .ctx
                            .node_types
                            .get(&i.target_type.id)
                            .copied()
                            .unwrap_or(TypeId::ERROR);
                        self.bind_self_type(target_ty, func_scope, f.span);
                    }
                }

                let mut param_tys = Vec::new();
                for param in &f.params {
                    param_tys.push(self.resolve_type(&param.type_node, func_scope));
                }
                let ret_ty = self.resolve_type(&f.ret_type, func_scope);

                let sig_ty = self.ctx.type_registry.intern(TypeKind::Function {
                    params: param_tys,
                    ret: ret_ty,
                    is_variadic: f.is_variadic,
                });

                if let Def::Function(mut updated_f) = self.ctx.defs[item_id.0 as usize].clone() {
                    updated_f.resolved_sig = Some(sig_ty);
                    self.ctx.defs[item_id.0 as usize] = Def::Function(updated_f);
                }

                if let Some(body) = &f.body {
                    self.resolve_expr(body, func_scope);
                }

                self.ctx.scopes.exit_scope();

                // 提取泛型参数
                let mut gen_args = Vec::new();
                for param in &f.generics {
                    gen_args.push(self.ctx.type_registry.intern(TypeKind::Param(param.name)));
                }
                let fn_def_ty = self
                    .ctx
                    .type_registry
                    .intern(TypeKind::FnDef(item_id, gen_args));

                // 切换回父作用域
                self.ctx.scopes.set_current_scope(parent_scope);

                // 只有普通的独立函数才需要在当前作用域更新类型。
                let is_impl_method = f.parent.map_or(false, |p_id| {
                    matches!(self.ctx.defs[p_id.0 as usize], Def::Impl(_))
                });

                if !is_impl_method {
                    self.ctx.scopes.update_type(f.name, fn_def_ty);
                }
            }
            Def::Struct(s) => {
                self.ctx.scopes.set_current_scope(parent_scope);
                let struct_scope = self.ctx.scopes.enter_scope();

                self.bind_generics(&s.generics, struct_scope);

                for field in &s.fields {
                    self.resolve_type(&field.type_node, struct_scope);
                }
                self.ctx.scopes.exit_scope();
                let struct_ty = self
                    .ctx
                    .type_registry
                    .intern(TypeKind::Def(item_id, Vec::new()));
                self.ctx.scopes.set_current_scope(parent_scope);
                self.ctx.scopes.update_type(s.name, struct_ty);
            }
            Def::Union(u) => {
                self.ctx.scopes.set_current_scope(parent_scope);
                let union_scope = self.ctx.scopes.enter_scope();

                self.bind_generics(&u.generics, union_scope);

                for field in &u.fields {
                    self.resolve_type(&field.type_node, union_scope);
                }
                self.ctx.scopes.exit_scope();
                let union_ty = self
                    .ctx
                    .type_registry
                    .intern(TypeKind::Def(item_id, Vec::new()));
                self.ctx.scopes.set_current_scope(parent_scope);
                self.ctx.scopes.update_type(u.name, union_ty);
            }
            Def::Enum(e) => {
                self.ctx.scopes.set_current_scope(parent_scope);
                let enum_scope = self.ctx.scopes.enter_scope();

                self.bind_generics(&e.generics, enum_scope);

                if let Some(backing_ty) = &e.backing_type {
                    let resolved_ty = self.resolve_type(backing_ty, enum_scope);
                    // 严格检查 backing type 必须是整数类型
                    if !self.ctx.type_registry.is_integer(resolved_ty)
                        && resolved_ty != TypeId::ERROR
                    {
                        self.ctx
                            .emit_error(backing_ty.span, "Enum backing type must be an integer");
                    }
                }
                self.ctx.scopes.exit_scope();
                let enum_ty = self
                    .ctx
                    .type_registry
                    .intern(TypeKind::Def(item_id, Vec::new()));
                self.ctx.scopes.set_current_scope(parent_scope);
                self.ctx.scopes.update_type(e.name, enum_ty);
            }
            Def::Trait(t) => {
                self.ctx.scopes.set_current_scope(parent_scope);
                let trait_scope = self.ctx.scopes.enter_scope();

                // 为 Trait 强制绑定 Self 类型
                let self_ty = self
                    .ctx
                    .type_registry
                    .intern(TypeKind::TraitObject(item_id, vec![]));
                self.bind_self_type(self_ty, trait_scope, t.span);

                // 解析方法签名并收集
                let mut resolved_methods = Vec::new();
                for method in &t.methods {
                    let sig_ty = self.resolve_type(&method.type_node, trait_scope);
                    resolved_methods.push((method.name, sig_ty));
                }
                self.ctx.scopes.exit_scope();

                // 将解析完成的方法字典回填到全局定义中
                if let Def::Trait(mut updated_t) = self.ctx.defs[item_id.0 as usize].clone() {
                    updated_t.resolved_methods = resolved_methods;
                    self.ctx.defs[item_id.0 as usize] = Def::Trait(updated_t);
                }
            }
            Def::TypeAlias(t) => {
                self.ctx.scopes.set_current_scope(parent_scope);
                let alias_scope = self.ctx.scopes.enter_scope();

                self.bind_generics(&t.generics, alias_scope);
                let target_ty = self.resolve_type(&t.target, alias_scope);

                self.ctx.scopes.exit_scope();
                self.ctx.scopes.set_current_scope(parent_scope);
                self.ctx.scopes.update_type(t.name, target_ty);
            }
            Def::Impl(i) => {
                self.ctx.scopes.set_current_scope(parent_scope);
                let impl_scope = self.ctx.scopes.enter_scope();

                self.bind_generics(&i.generics, impl_scope);

                // 绑定 Self 类型到上下文中
                let target_ty_id = self.resolve_type(&i.target_type, impl_scope);
                self.bind_self_type(target_ty_id, impl_scope, i.span);

                if let Some(trait_ty) = &i.trait_type {
                    self.resolve_type(trait_ty, impl_scope);
                }

                // 递归解析 impl 块内部的方法 (这些方法并没有注册在 module 的 items 里)
                for &method_id in &i.methods {
                    self.resolve_item(method_id, impl_scope);
                }

                self.ctx.scopes.exit_scope();
            }
            Def::Global(g) => {
                self.resolve_expr(&g.value, parent_scope);
                let val_ty = self
                    .ctx
                    .node_types
                    .get(&g.value.id)
                    .copied()
                    .unwrap_or(TypeId::ERROR);
                self.ctx.scopes.set_current_scope(parent_scope);
                self.ctx.scopes.update_type(g.name, val_ty);
            }
            Def::Adt(a) => {
                self.ctx.scopes.set_current_scope(parent_scope);
                let adt_scope = self.ctx.scopes.enter_scope();

                // 绑定泛型参数，比如 Option[T] 里的 T
                self.bind_generics(&a.generics, adt_scope);

                // 解析所有变体的负载类型
                for variant in &a.variants {
                    if let Some(payload_ty) = &variant.payload_type {
                        self.resolve_type(payload_ty, adt_scope);
                    }
                }

                self.ctx.scopes.exit_scope();

                // 生成基础类型的 TypeId (不带泛型实参的形式)
                let adt_ty = self
                    .ctx
                    .type_registry
                    .intern(TypeKind::Adt(item_id, Vec::new()));

                self.ctx.scopes.set_current_scope(parent_scope);
                self.ctx.scopes.update_type(a.name, adt_ty);
            }
            _ => {} // Module 自身无需在此解析
        }
    }

    // ==========================================
    //          核心类型转换逻辑
    // ==========================================

    /// 将 AST TypeNode 转换为语义 TypeId
    pub fn resolve_type(&mut self, ty_node: &ast::TypeNode, env_scope: ScopeId) -> TypeId {
        let ty_id = match &ty_node.kind {
            ast::TypeKind::Path { segments, generics } => {
                self.resolve_path_type(segments, generics, env_scope, ty_node.span)
            }
            ast::TypeKind::Mut(elem) => {
                let base = self.resolve_type(elem, env_scope);
                self.ctx.type_registry.intern(TypeKind::Mut(base))
            }
            ast::TypeKind::Pointer { elem } => {
                let base = self.resolve_type(elem, env_scope);
                self.ctx.type_registry.intern(TypeKind::Pointer(base))
            }
            ast::TypeKind::VolatilePtr { elem } => {
                let base = self.resolve_type(elem, env_scope);
                self.ctx.type_registry.intern(TypeKind::VolatilePtr(base))
            }
            ast::TypeKind::Slice { elem } => {
                let base = self.resolve_type(elem, env_scope);
                self.ctx.type_registry.intern(TypeKind::Slice(base))
            }
            ast::TypeKind::Array { elem, len } => {
                let base = self.resolve_type(elem, env_scope);
                if let ast::ExprKind::Infer = len.kind {
                    self.ctx.type_registry.intern(TypeKind::ArrayInfer(base))
                } else {
                    let mut evaluator = ConstEvaluator::new(self.ctx);
                    let length = match evaluator.eval_usize(len) {
                        Ok(l) => l,
                        Err(_) => 0, // 错误已经在 evaluator 内部 emit
                    };
                    self.ctx.type_registry.intern(TypeKind::Array {
                        elem: base,
                        len: length,
                    })
                }
            }
            ast::TypeKind::Function {
                params,
                ret,
                is_variadic,
            } => {
                let mut param_tys = Vec::with_capacity(params.len());
                for p in params {
                    param_tys.push(self.resolve_type(p, env_scope));
                }
                let ret_ty = match ret {
                    Some(r) => self.resolve_type(r, env_scope),
                    None => TypeId::VOID,
                };
                self.ctx.type_registry.intern(TypeKind::Function {
                    params: param_tys,
                    ret: ret_ty,
                    is_variadic: *is_variadic,
                })
            }
            ast::TypeKind::SelfType => {
                self.ctx.scopes.set_current_scope(env_scope);
                let self_sym = self.ctx.intern("Self");
                if let Some(info) = self.ctx.scopes.resolve(self_sym) {
                    info.type_id
                } else {
                    self.ctx.struct_error(ty_node.span, "the `Self` type is only valid inside `impl` blocks or `trait` definitions")
                        .with_hint("you are using it in a global or standard function context")
                        .emit();
                    TypeId::ERROR
                }
            }
            ast::TypeKind::Never => TypeId::NEVER,
            ast::TypeKind::Infer => {
                self.ctx.struct_error(ty_node.span, "type inference `_` is not allowed as a standalone type")
                    .with_hint("in Kern, the `_` placeholder is exclusively used for array length inference, e.g., `[_]u8.{ 1, 2, 3 }`")
                    .emit();
                TypeId::ERROR
            }
            // Struct/Enum/Union/Trait 在这里不会直接作为匿名类型出现 (已被 Collect 提取)
            _ => {
                self.ctx
                    .emit_error(ty_node.span, "Invalid or unsupported type construction");
                TypeId::ERROR
            }
        };

        self.ctx.node_types.insert(ty_node.id, ty_id);
        ty_id
    }

    // 递归查找并解析表达式内部的所有 TypeNode
    fn resolve_expr(&mut self, expr: &ast::Expr, scope: ScopeId) {
        match &expr.kind {
            ast::ExprKind::Let { init, .. } | ast::ExprKind::Static { init, .. } => {
                self.resolve_expr(init, scope);
            }
            ast::ExprKind::As { lhs, target } => {
                self.resolve_expr(lhs, scope);
                self.resolve_type(target, scope); // 捕获 TypeNode
            }
            ast::ExprKind::Block { stmts, result } => {
                for stmt in stmts {
                    match &stmt.kind {
                        ast::StmtKind::ExprStmt(e) | ast::StmtKind::ExprValue(e) => {
                            self.resolve_expr(e, scope);
                        }
                    }
                }
                if let Some(r) = result {
                    self.resolve_expr(r, scope);
                }
            }
            ast::ExprKind::If { cond, then_branch, else_branch } => {
                self.resolve_expr(cond, scope);
                self.resolve_expr(then_branch, scope);
                if let Some(e) = else_branch {
                    self.resolve_expr(e, scope);
                }
            }
            ast::ExprKind::Match { target, arms } => {
                self.resolve_expr(target, scope);
                for arm in arms {
                    // 捕获 Match 分支中可能携带的显式类型前缀 (e.g., Result[i32].Ok)
                    if let ast::MatchPattern::Variant { target_type: Some(ty), .. } = &arm.pattern {
                        self.resolve_type(ty, scope);
                    }
                    self.resolve_expr(&arm.body, scope);
                }
            }
            ast::ExprKind::Switch { target, cases, default_case } => {
                self.resolve_expr(target, scope);
                for case in cases {
                    // 遍历 Switch 的 Pattern (可能包含 Range 表达式)
                    for pat in &case.patterns {
                        match pat {
                            ast::SwitchPattern::Value(e) => self.resolve_expr(e, scope),
                            ast::SwitchPattern::Range { start, end, .. } => {
                                self.resolve_expr(start, scope);
                                self.resolve_expr(end, scope);
                            }
                        }
                    }
                    self.resolve_expr(&case.body, scope);
                }
                if let Some(d) = default_case {
                    self.resolve_expr(d, scope);
                }
            }
            ast::ExprKind::For { init, cond, post, body } => {
                if let Some(e) = init { self.resolve_expr(e, scope); }
                if let Some(e) = cond { self.resolve_expr(e, scope); }
                if let Some(e) = post { self.resolve_expr(e, scope); }
                self.resolve_expr(body, scope);
            }
            ast::ExprKind::Lambda { params, ret_type, body } => {
                // 捕获 Lambda 的参数类型和返回值类型
                for param in params {
                    self.resolve_type(&param.type_node, scope);
                }
                self.resolve_type(ret_type, scope);
                self.resolve_expr(body, scope);
            }
            ast::ExprKind::Binary { lhs, rhs, .. } | ast::ExprKind::Assign { lhs, rhs, .. } => {
                self.resolve_expr(lhs, scope);
                self.resolve_expr(rhs, scope);
            }
            ast::ExprKind::Unary { operand, .. } => {
                self.resolve_expr(operand, scope);
            }
            ast::ExprKind::FieldAccess { lhs, .. } => {
                self.resolve_expr(lhs, scope);
            }
            ast::ExprKind::IndexAccess { lhs, index } => {
                self.resolve_expr(lhs, scope);
                self.resolve_expr(index, scope);
            }
            ast::ExprKind::Call { callee, args } => {
                self.resolve_expr(callee, scope);
                for arg in args {
                    self.resolve_expr(arg, scope);
                }
            }
            ast::ExprKind::GenericInstantiation { target, types } => {
                self.resolve_expr(target, scope);
                // 捕获泛型实参
                for ty in types {
                    self.resolve_type(ty, scope);
                }
            }
            ast::ExprKind::DataInit { type_node, literal } => {
                // 捕获 Elided Initialization 的前缀类型
                if let Some(ty) = type_node {
                    self.resolve_type(ty, scope);
                }
                match literal {
                    ast::DataLiteralKind::Struct(fields) => {
                        for f in fields { self.resolve_expr(&f.value, scope); }
                    }
                    ast::DataLiteralKind::Array(elems) => {
                        for e in elems { self.resolve_expr(e, scope); }
                    }
                    ast::DataLiteralKind::Repeat { value, count } => {
                        self.resolve_expr(value, scope);
                        self.resolve_expr(count, scope);
                    }
                    ast::DataLiteralKind::Scalar(inner) => {
                        self.resolve_expr(inner, scope);
                    }
                }
            }
            ast::ExprKind::SliceOp { lhs, start, end, .. } => {
                self.resolve_expr(lhs, scope);
                if let Some(s) = start { self.resolve_expr(s, scope); }
                if let Some(e) = end { self.resolve_expr(e, scope); }
            }
            ast::ExprKind::Defer { expr: e } => self.resolve_expr(e, scope),
            ast::ExprKind::Return(Some(e)) => self.resolve_expr(e, scope),

            // 所有叶子节点 (Identifier, Int, EnumLiteral, Break 等) 直接忽略
            _ => {}
        }
    }

    /// 严格的路径类型解析 (支持 `module.submodule.Type[Generic]`)
    fn resolve_path_type(
        &mut self,
        segments: &[SymbolId],
        generics: &[ast::TypeNode],
        env_scope: ScopeId,
        span: crate::utils::Span,
    ) -> TypeId {
        if segments.is_empty() {
            return TypeId::ERROR;
        }

        let mut curr_scope = env_scope;
        let mut target_symbol = None;

        // 逐级解析路径
        for (i, &segment) in segments.iter().enumerate() {
            if i == 0 {
                // 第一段：如果只有一段，优先检查内置基础类型
                if segments.len() == 1 {
                    let name_str = self.ctx.resolve(segment);
                    if let Some(prim_id) = self.resolve_builtin_primitive(name_str) {
                        if !generics.is_empty() {
                            self.ctx
                                .emit_error(span, "Primitive types do not take generic arguments");
                        }
                        return prim_id;
                    }
                }

                // 沿着作用域树向上查找
                self.ctx.scopes.set_current_scope(curr_scope);
                target_symbol = self.ctx.scopes.resolve(segment).cloned();
            } else {
                // 后续段：严格只在前一个模块的内部作用域中查找
                target_symbol = self.ctx.scopes.resolve_in(curr_scope, segment).cloned();
            }

            let sym = match target_symbol.as_ref() {
                Some(s) => s,
                None => {
                    let name = self.ctx.resolve(segment).to_string();
                    if i == 0 {
                        self.ctx
                            .emit_error(span, format!("Cannot find type `{}` in this scope", name));
                    } else {
                        self.ctx.emit_error(
                            span,
                            format!("Cannot find `{}` in the target module", name),
                        );
                    }
                    return TypeId::ERROR;
                }
            };

            // 如果还没到最后一段，当前符号必须是个模块
            if i < segments.len() - 1 {
                if sym.kind == SymbolKind::Module {
                    let mod_def_id = sym.def_id.unwrap();
                    if let Def::Module(m) = &self.ctx.defs[mod_def_id.0 as usize] {
                        curr_scope = m.scope_id;
                    } else {
                        unreachable!()
                    }
                } else {
                    let name = self.ctx.resolve(segment).to_string();
                    self.ctx
                        .emit_error(span, format!("`{}` is not a module", name));
                    return TypeId::ERROR;
                }
            }
        }

        let final_sym = target_symbol.unwrap();

        // 解析附带的泛型参数 (在原始的作用域中解析)
        let mut resolved_generics = Vec::with_capacity(generics.len());
        for gen_ast in generics {
            resolved_generics.push(self.resolve_type(gen_ast, env_scope));
        }

        // 验证最终符号的类型
        match final_sym.kind {
            SymbolKind::Struct | SymbolKind::Union | SymbolKind::Enum => {
                let def_id = final_sym.def_id.unwrap();
                self.ctx
                    .type_registry
                    .intern(TypeKind::Def(def_id, resolved_generics))
            }
            SymbolKind::Adt => {
                let def_id = final_sym.def_id.unwrap();
                self.ctx
                    .type_registry
                    .intern(TypeKind::Adt(def_id, resolved_generics))
            }
            SymbolKind::Trait => {
                let def_id = final_sym.def_id.unwrap();
                self.ctx
                    .type_registry
                    .intern(TypeKind::TraitObject(def_id, resolved_generics))
            }
            SymbolKind::TypeParam => {
                if !resolved_generics.is_empty() {
                    self.ctx
                        .emit_error(span, "Type parameters cannot take generic arguments");
                }
                final_sym.type_id // 直接返回 Param(SymbolId)
            }
            SymbolKind::TypeAlias => {
                let def_id = final_sym.def_id.unwrap();
                let target_ty = final_sym.type_id;

                if resolved_generics.is_empty() {
                    // 没有传入泛型，直接穿透返回
                    target_ty
                } else {
                    // 获取别名的定义以提取泛型名字
                    if let Def::TypeAlias(t_def) = &self.ctx.defs[def_id.0 as usize] {
                        if t_def.generics.len() != resolved_generics.len() {
                            self.ctx.emit_error(span, format!("Type alias `{}` expects {} generic arguments, but {} were provided", self.ctx.resolve(*segments.last().unwrap()), t_def.generics.len(), resolved_generics.len()));
                            return TypeId::ERROR;
                        }

                        // 构造映射字典并执行替换
                        let mut map = std::collections::HashMap::new();
                        for (i, param) in t_def.generics.iter().enumerate() {
                            map.insert(param.name, resolved_generics[i]);
                        }
                        let mut subst = crate::sema::typeck::subst::Substituter::new(
                            &mut self.ctx.type_registry,
                            &map,
                        );
                        subst.substitute(target_ty)
                    } else {
                        unreachable!()
                    }
                }
            }
            _ => {
                let name = self.ctx.resolve(*segments.last().unwrap()).to_string();
                self.ctx.emit_error(
                    span,
                    format!(
                        "`{}` is a {}, not a type",
                        name,
                        self.kind_to_string(final_sym.kind)
                    ),
                );
                TypeId::ERROR
            }
        }
    }

    // ==========================================
    //               Helpers
    // ==========================================

    fn resolve_builtin_primitive(&self, name: &str) -> Option<TypeId> {
        match name {
            "void" => Some(TypeId::VOID),
            "bool" => Some(TypeId::BOOL),
            "i8" => Some(TypeId::I8),
            "i16" => Some(TypeId::I16),
            "i32" => Some(TypeId::I32),
            "i64" => Some(TypeId::I64),
            "i128" => Some(TypeId::I128),
            "isize" => Some(TypeId::ISIZE),
            "u8" => Some(TypeId::U8),
            "u16" => Some(TypeId::U16),
            "u32" => Some(TypeId::U32),
            "u64" => Some(TypeId::U64),
            "u128" => Some(TypeId::U128),
            "usize" => Some(TypeId::USIZE),
            "f32" => Some(TypeId::F32),
            "f64" => Some(TypeId::F64),
            "str" => Some(TypeId::STR),   
            "never" => Some(TypeId::NEVER), 
            _ => None,
        }
    }

    fn bind_generics(&mut self, generics: &[ast::GenericParam], scope: ScopeId) {
        self.ctx.scopes.set_current_scope(scope);
        for param in generics {
            let param_ty = self.ctx.type_registry.intern(TypeKind::Param(param.name));
            let info = SymbolInfo {
                kind: SymbolKind::TypeParam,
                node_id: self.ctx.next_node_id(),
                type_id: param_ty,
                def_id: None,
                span: param.span,
                is_pub: false,
            };
            let _ = self.ctx.scopes.define(param.name, info);
        }
    }

    fn bind_self_type(&mut self, target_ty: TypeId, scope: ScopeId, span: crate::utils::Span) {
        self.ctx.scopes.set_current_scope(scope);
        let self_sym = self.ctx.intern("Self");
        let info = SymbolInfo {
            kind: SymbolKind::TypeAlias,
            node_id: self.ctx.next_node_id(),
            type_id: target_ty,
            def_id: None,
            span,
            is_pub: false,
        };
        // 允许重复定义（覆盖外部可能存在的同名绑定）
        let _ = self.ctx.scopes.define(self_sym, info);
    }

    fn kind_to_string(&self, kind: SymbolKind) -> &'static str {
        match kind {
            SymbolKind::Var => "variable",
            SymbolKind::Const => "constant",
            SymbolKind::Static => "static variable",
            SymbolKind::Function => "function",
            SymbolKind::Module => "module",
            SymbolKind::Struct => "struct",
            SymbolKind::Enum => "enum",
            SymbolKind::Union => "union",
            SymbolKind::Adt => "algebraic data type",
            SymbolKind::Trait => "trait",
            SymbolKind::TypeAlias => "type alias",
            SymbolKind::TypeParam => "type parameter",
        }
    }
}
