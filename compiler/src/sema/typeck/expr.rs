use crate::driver::Context;
use crate::parser::ast::{self, BinaryOperator, Expr, ExprKind, StmtKind, UnaryOperator};
use crate::sema::def::Def;
use crate::sema::resolve_types::TypeResolver;
use crate::sema::scope::SymbolInfo;
use crate::sema::scope::SymbolKind;
use crate::sema::ty::{TypeId, TypeKind};
use crate::utils::Span;

pub struct ExprChecker<'a> {
    pub ctx: &'a mut Context,
    pub current_return_type: Option<TypeId>,
    pub has_returned: bool,
}

impl<'a> ExprChecker<'a> {
    pub fn new(ctx: &'a mut Context, current_return_type: Option<TypeId>) -> Self {
        Self {
            ctx,
            current_return_type,
            has_returned: false,
        }
    }

    /// 核心入口：检查表达式类型
    pub fn check_expr(&mut self, expr: &Expr, expected_ty: Option<TypeId>) -> TypeId {
        let ty = match &expr.kind {
            // === 1. 基础字面量 ===
            ExprKind::Integer(_) => expected_ty
                .unwrap_or_else(|| self.ctx.type_registry.intern(TypeKind::Mut(TypeId::USIZE))),
            ExprKind::Float(_) => expected_ty
                .unwrap_or_else(|| self.ctx.type_registry.intern(TypeKind::Mut(TypeId::F32))),
            ExprKind::Bool(_) => TypeId::BOOL,
            ExprKind::Char(_) => TypeId::U32,
            ExprKind::String(_) => self.ctx.type_registry.intern(TypeKind::Slice(TypeId::U8)),

            // === 2. 标识符与变量 ===
            ExprKind::Identifier(name) => self.check_identifier(*name, expr.span),
            ExprKind::SelfValue => self.check_self_value(expr.span),

            // === 3. 声明与绑定 ===
            ExprKind::Let { name, init } => {
                self.check_let_or_static(expr.id, *name, init, expected_ty, false, expr.span)
            }
            ExprKind::Static { name, init } => {
                self.check_let_or_static(expr.id, *name, init, expected_ty, true, expr.span)
            }

            // === 4. 运算与赋值 ===
            ExprKind::Binary { lhs, op, rhs } => self.check_binary(lhs, *op, rhs, expected_ty),
            ExprKind::Unary { op, operand } => {
                self.check_unary(*op, operand, expr.span, expected_ty)
            }
            ExprKind::Assign { lhs, rhs, .. } => self.check_assign(lhs, rhs, expr.span),

            // === 5. 转换 ===
            ExprKind::As { lhs, target } => self.check_as_expr(lhs, target),

            // === 6. 内存访问 (索引, 字段, 切片) ===
            ExprKind::IndexAccess { lhs, index } => self.check_index_access(lhs, index),
            ExprKind::FieldAccess { lhs, field } => self.check_field_access(lhs, *field, expr.span),
            ExprKind::SliceOp {
                lhs,
                start,
                end,
                is_inclusive,
            } => self.check_slice_op(
                lhs,
                start.as_deref(),
                end.as_deref(),
                *is_inclusive,
                expr.span,
            ),

            // === 7. 函数/宏调用 ===
            ExprKind::Call { callee, args } => self.check_call(callee, args, expr.span),

            // === 8. 复杂字面量 ===
            ExprKind::DataInit { type_node, literal } => {
                self.check_data_init_expr(type_node.as_deref(), literal, expected_ty, expr.span)
            }
            ExprKind::EnumLiteral(variant_name) => {
                self.check_enum_literal(*variant_name, expected_ty, expr.span)
            }
            ExprKind::Undef => self.check_undef(expected_ty, expr.span),

            // === 9. 控制流 ===
            ExprKind::Block { stmts, result } => {
                self.check_block(stmts, result.as_deref(), expected_ty)
            }
            ExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => self.check_if(cond, then_branch, else_branch.as_deref(), expected_ty),
            ExprKind::Switch {
                target,
                cases,
                default_case,
            } => self.check_switch_expr(
                target,
                cases,
                default_case.as_deref(),
                expected_ty,
                expr.span,
            ),
            ExprKind::For {
                init,
                cond,
                post,
                body,
            } => self.check_for(init.as_deref(), cond.as_deref(), post.as_deref(), body),
            ExprKind::Defer { expr: defer_expr } => self.check_defer(defer_expr),
            ExprKind::Break | ExprKind::Continue => TypeId::VOID,
            ExprKind::Return(val) => self.check_return(val.as_deref(), expr.span),

            // === 10. 泛型实例化 ===
            ExprKind::GenericInstantiation { target, types } => {
                self.check_generic_instantiation(target, types, expr.span)
            }
        };

        self.ctx.node_types.insert(expr.id, ty);
        ty
    }

    fn check_identifier(&mut self, name: crate::utils::SymbolId, span: Span) -> TypeId {
        if let Some(info) = self.ctx.scopes.resolve(name) {
            if info.kind == SymbolKind::Function {
                return self
                    .ctx
                    .type_registry
                    .intern(TypeKind::FnDef(info.def_id.unwrap(), vec![]));
            }
            info.type_id
        } else {
            let name_str = self.ctx.resolve(name).to_string();
            self.ctx
                .struct_error(span, format!("use of undeclared identifier `{}`", name_str))
                .with_hint("make sure the variable or function is defined before using it")
                .emit();
            TypeId::ERROR
        }
    }

    fn check_self_value(&mut self, span: Span) -> TypeId {
        let self_var = self.ctx.intern("self");
        let self_type = self.ctx.intern("Self");

        if let Some(info) = self.ctx.scopes.resolve(self_var) {
            info.type_id
        } else if let Some(info) = self.ctx.scopes.resolve(self_type) {
            info.type_id
        } else {
            self.ctx
                .struct_error(span, "`self` is not available in this context")
                .with_hint("the `self` keyword is only valid inside method implementations")
                .emit();
            TypeId::ERROR
        }
    }

    fn check_let_or_static(
        &mut self,
        node_id: ast::NodeId,
        name: crate::utils::SymbolId,
        init: &Expr,
        expected_ty: Option<TypeId>,
        is_static: bool,
        span: Span,
    ) -> TypeId {
        let init_ty = self.check_expr(init, expected_ty);
        let sym_kind = if is_static {
            SymbolKind::Static
        } else {
            SymbolKind::Var
        };

        let info = SymbolInfo {
            kind: sym_kind,
            node_id,
            type_id: init_ty,
            def_id: None,
            span,
            is_pub: false,
        };
        let _ = self.ctx.scopes.define(name, info);

        TypeId::VOID
    }

    fn check_as_expr(&mut self, lhs: &Expr, target: &ast::TypeNode) -> TypeId {
        let lhs_ty = self.check_expr(lhs, None);
        let mut resolver = TypeResolver::new(self.ctx);
        let scope = resolver.ctx.scopes.current_scope_id().unwrap();
        let target_ty = resolver.resolve_type(target, scope);

        self.check_cast(lhs.span, lhs_ty, target_ty);
        target_ty
    }

    fn check_index_access(&mut self, lhs: &Expr, index: &Expr) -> TypeId {
        let lhs_ty = self.check_expr(lhs, None);
        let idx_ty = self.check_expr(index, Some(TypeId::USIZE));

        let norm_idx = self.strip_mut(idx_ty);
        if !self.ctx.type_registry.is_integer(norm_idx) && norm_idx != TypeId::ERROR {
            let ty_str = self.ctx.ty_to_string(idx_ty);
            self.ctx
                .struct_error(index.span, "index must be an integer type")
                .with_hint(format!("found type `{}` instead", ty_str))
                .emit();
        }

        let norm_lhs = self.strip_mut(lhs_ty);
        match self.ctx.type_registry.get(norm_lhs) {
            TypeKind::Array { elem, .. } | TypeKind::Slice(elem) => {
                if self.is_mut_type(lhs_ty) {
                    self.ctx.type_registry.intern(TypeKind::Mut(*elem))
                } else {
                    *elem
                }
            }
            TypeKind::Error => TypeId::ERROR,
            _ => {
                let lhs_str = self.ctx.ty_to_string(lhs_ty);
                self.ctx
                    .struct_error(lhs.span, "cannot index into a non-array/non-slice type")
                    .with_hint(format!("target type is `{}`", lhs_str))
                    .emit();
                TypeId::ERROR
            }
        }
    }

    fn check_slice_op(
        &mut self,
        lhs: &Expr,
        start: Option<&Expr>,
        end: Option<&Expr>,
        _is_inclusive: bool,
        _span: Span,
    ) -> TypeId {
        let lhs_ty = self.check_expr(lhs, None);

        if let Some(s) = start {
            let s_ty = self.check_expr(s, Some(TypeId::USIZE));
            if !self.ctx.type_registry.is_integer(self.strip_mut(s_ty)) {
                self.ctx
                    .struct_error(s.span, "slice start index must be an integer")
                    .emit();
            }
        }
        if let Some(e) = end {
            let e_ty = self.check_expr(e, Some(TypeId::USIZE));
            if !self.ctx.type_registry.is_integer(self.strip_mut(e_ty)) {
                self.ctx
                    .struct_error(e.span, "slice end index must be an integer")
                    .emit();
            }
        }

        let norm_lhs = self.strip_mut(lhs_ty);
        match self.ctx.type_registry.get(norm_lhs) {
            TypeKind::Array { elem, .. } | TypeKind::Slice(elem) => {
                let base_elem = *elem;
                let slice_elem = if self.is_mut_type(lhs_ty) {
                    self.ctx.type_registry.intern(TypeKind::Mut(base_elem))
                } else {
                    base_elem
                };
                self.ctx.type_registry.intern(TypeKind::Slice(slice_elem))
            }
            TypeKind::Error => TypeId::ERROR,
            _ => {
                let lhs_str = self.ctx.ty_to_string(lhs_ty);
                self.ctx
                    .struct_error(lhs.span, "cannot slice a non-array/non-slice type")
                    .with_hint(format!("target type is `{}`", lhs_str))
                    .emit();
                TypeId::ERROR
            }
        }
    }

    fn check_data_init_expr(
        &mut self,
        type_node: Option<&ast::TypeNode>,
        literal: &ast::DataLiteralKind,
        expected_ty: Option<TypeId>,
        span: Span,
    ) -> TypeId {
        let mut actual_expected = expected_ty;
        if let Some(ty_ast) = type_node {
            let mut resolver = TypeResolver::new(self.ctx);
            let scope = resolver.ctx.scopes.current_scope_id().unwrap();
            let prefix_ty = resolver.resolve_type(ty_ast, scope);
            actual_expected = Some(prefix_ty);
        }

        if let Some(exp_ty) = actual_expected {
            self.check_data_literal(literal, exp_ty, span)
        } else {
            if let ast::DataLiteralKind::Scalar(inner) = literal {
                self.check_expr(inner, None)
            } else {
                self.ctx.struct_error(span, "cannot infer type for anonymous initialization `.{...}`")
                    .with_hint("provide an explicit type context or prepend the type name, e.g., `MyStruct.{...}`")
                    .emit();
                TypeId::ERROR
            }
        }
    }

    fn check_enum_literal(
        &mut self,
        variant_name: crate::utils::SymbolId,
        expected_ty: Option<TypeId>,
        span: Span,
    ) -> TypeId {
        let mut res_ty = TypeId::ERROR;
        if let Some(exp_ty) = expected_ty {
            let norm_exp = self.strip_mut(exp_ty);
            if let TypeKind::Def(def_id, _) = self.ctx.type_registry.get(norm_exp) {
                if let Def::Enum(e) = &self.ctx.defs[def_id.0 as usize] {
                    if e.variants.iter().any(|v| v.name == variant_name) {
                        res_ty = exp_ty;
                    } else {
                        let v_str = self.ctx.resolve(variant_name).to_string();
                        let exp_str = self.ctx.ty_to_string(norm_exp);
                        let available_variants: Vec<String> = e
                            .variants
                            .iter()
                            .map(|v| format!(".{}", self.ctx.resolve(v.name)))
                            .collect();
                        let mut diag = self
                            .ctx
                            .struct_error(
                                span,
                                format!("variant `.{}` does not exist in the expected enum", v_str),
                            )
                            .with_hint(format!("expected enum type is `{}`", exp_str));

                        if !available_variants.is_empty() {
                            diag = diag.with_hint(format!(
                                "available variants: {}",
                                available_variants.join(", ")
                            ));
                        }
                        diag.emit();
                    }
                } else {
                    let exp_str = self.ctx.ty_to_string(norm_exp);
                    self.ctx
                        .struct_error(span, "expected an enum type for variant literal")
                        .with_hint(format!("but context expects `{}`", exp_str))
                        .emit();
                }
            } else if norm_exp != TypeId::ERROR {
                let exp_str = self.ctx.ty_to_string(norm_exp);
                self.ctx
                    .struct_error(span, "expected an enum type for variant literal")
                    .with_hint(format!("but context expects `{}`", exp_str))
                    .emit();
            }
        } else {
            self.ctx
                .struct_error(
                    span,
                    "cannot infer enum type for variant literal without context",
                )
                .with_hint("try prepending the enum type name, e.g., `Color.Red` instead of `.Red`")
                .emit();
        }
        res_ty
    }

    fn check_undef(&mut self, expected_ty: Option<TypeId>, span: Span) -> TypeId {
        if expected_ty.is_none() {
            self.ctx
                .struct_error(span, "`undef` must have a known expected type context")
                .emit();
            TypeId::ERROR
        } else {
            expected_ty.unwrap()
        }
    }

    fn check_block(
        &mut self,
        stmts: &[ast::Stmt],
        result: Option<&Expr>,
        expected_ty: Option<TypeId>,
    ) -> TypeId {
        self.ctx.scopes.enter_scope();
        for stmt in stmts {
            match &stmt.kind {
                StmtKind::ExprStmt(e) | StmtKind::ExprValue(e) => {
                    self.check_expr(e, None);
                }
            }
        }
        let ret_ty = if let Some(res) = result {
            self.check_expr(res, expected_ty)
        } else {
            TypeId::VOID
        };
        self.ctx.scopes.exit_scope();
        ret_ty
    }

    fn check_if(
        &mut self,
        cond: &Expr,
        then_branch: &Expr,
        else_branch: Option<&Expr>,
        expected_ty: Option<TypeId>,
    ) -> TypeId {
        let cond_ty = self.check_expr(cond, Some(TypeId::BOOL));
        self.check_coercion(cond.span, TypeId::BOOL, cond_ty);

        let then_ty = self.check_expr(then_branch, expected_ty);
        if let Some(else_expr) = else_branch {
            let else_ty = self.check_expr(else_expr, expected_ty);
            self.check_coercion(else_expr.span, then_ty, else_ty);
            then_ty
        } else {
            TypeId::VOID
        }
    }

    fn check_switch_expr(
        &mut self,
        target: &Expr,
        cases: &[ast::SwitchCase],
        default_case: Option<&Expr>,
        expected_ty: Option<TypeId>,
        span: Span,
    ) -> TypeId {
        let target_ty = self.check_expr(target, None);
        let mut common_ret_ty = expected_ty;

        for case in cases {
            for pat in &case.patterns {
                match pat {
                    ast::SwitchPattern::Value(v) => {
                        let v_ty = self.check_expr(v, Some(target_ty));
                        self.check_coercion(v.span, target_ty, v_ty);
                    }
                    ast::SwitchPattern::Range { start, end, .. } => {
                        let s_ty = self.check_expr(start, Some(target_ty));
                        let e_ty = self.check_expr(end, Some(target_ty));
                        self.check_coercion(start.span, target_ty, s_ty);
                        self.check_coercion(end.span, target_ty, e_ty);
                    }
                }
            }
            let body_ty = self.check_expr(&case.body, common_ret_ty);
            if common_ret_ty.is_none() {
                common_ret_ty = Some(body_ty);
            } else {
                self.check_coercion(case.body.span, common_ret_ty.unwrap(), body_ty);
            }
        }

        if let Some(def) = default_case {
            let def_ty = self.check_expr(def, common_ret_ty);
            if common_ret_ty.is_none() {
                common_ret_ty = Some(def_ty);
            } else {
                self.check_coercion(def.span, common_ret_ty.unwrap(), def_ty);
            }
        }
        self.check_switch_exhaustiveness(target_ty, cases, default_case.is_some(), span);
        common_ret_ty.unwrap_or(TypeId::VOID)
    }

    fn check_for(
        &mut self,
        init: Option<&Expr>,
        cond: Option<&Expr>,
        post: Option<&Expr>,
        body: &Expr,
    ) -> TypeId {
        self.ctx.scopes.enter_scope();
        if let Some(i) = init {
            self.check_expr(i, None);
        }
        if let Some(c) = cond {
            let c_ty = self.check_expr(c, Some(TypeId::BOOL));
            self.check_coercion(c.span, TypeId::BOOL, c_ty);
        }
        if let Some(p) = post {
            self.check_expr(p, None);
        }
        self.check_expr(body, None);
        self.ctx.scopes.exit_scope();
        TypeId::VOID
    }

    fn check_defer(&mut self, defer_expr: &Expr) -> TypeId {
        self.check_expr(defer_expr, None);
        TypeId::VOID
    }

    fn check_return(&mut self, val: Option<&Expr>, span: Span) -> TypeId {
        self.has_returned = true;
        let expected_ret = self.current_return_type.unwrap_or(TypeId::VOID);

        if let Some(v) = val {
            let v_ty = self.check_expr(v, Some(expected_ret));
            self.check_coercion(v.span, expected_ret, v_ty);
        } else {
            if expected_ret != TypeId::VOID && expected_ret != TypeId::ERROR {
                let ret_str = self.ctx.ty_to_string(expected_ret);
                self.ctx
                    .struct_error(span, "expected a return value, but found empty return")
                    .with_hint(format!("function is expected to return `{}`", ret_str))
                    .emit();
            }
        }
        TypeId::VOID
    }

    fn check_generic_instantiation(
        &mut self,
        target: &Expr,
        types: &[ast::TypeNode],
        span: Span,
    ) -> TypeId {
        let target_ty = self.check_expr(target, None);
        let target_norm = self.strip_mut(target_ty);

        if target_norm == TypeId::ERROR {
            return TypeId::ERROR;
        }

        let mut arg_tys = Vec::new();
        {
            let mut resolver = TypeResolver::new(self.ctx);
            let scope = resolver.ctx.scopes.current_scope_id().unwrap();
            for ty_node in types {
                arg_tys.push(resolver.resolve_type(ty_node, scope));
            }
        }

        let (def_id, _old_args) = match self.ctx.type_registry.get(target_norm) {
            TypeKind::FnDef(id, args) => (*id, args.clone()),
            TypeKind::Def(id, args) => (*id, args.clone()),
            _ => {
                self.ctx
                    .struct_error(
                        span,
                        "this expression does not support generic instantiation",
                    )
                    .emit();
                return TypeId::ERROR;
            }
        };

        let generics = {
            let def = &self.ctx.defs[def_id.0 as usize];
            match def {
                Def::Function(f) => f.generics.clone(),
                Def::Struct(s) => s.generics.clone(),
                Def::Union(u) => u.generics.clone(),
                Def::Enum(e) => e.generics.clone(),
                Def::TypeAlias(t) => t.generics.clone(),
                _ => unreachable!(),
            }
        };

        if generics.len() != arg_tys.len() {
            self.ctx
                .struct_error(
                    span,
                    format!(
                        "expected {} generic arguments, but {} were provided",
                        generics.len(),
                        arg_tys.len()
                    ),
                )
                .emit();
            return TypeId::ERROR;
        }

        self.check_generic_bounds(span, &generics, &arg_tys);

        if matches!(self.ctx.type_registry.get(target_norm), TypeKind::FnDef(..)) {
            self.ctx
                .type_registry
                .intern(TypeKind::FnDef(def_id, arg_tys))
        } else {
            self.ctx
                .type_registry
                .intern(TypeKind::Def(def_id, arg_tys))
        }
    }

    fn check_generic_bounds(
        &mut self,
        span: Span,
        generics: &[ast::GenericParam],
        arg_tys: &[TypeId],
    ) {
        for (i, param) in generics.iter().enumerate() {
            if i >= arg_tys.len() {
                break;
            }
            let act_ty = arg_tys[i];

            for constraint_node in &param.constraints {
                let constraint_ty = self
                    .ctx
                    .node_types
                    .get(&constraint_node.id)
                    .copied()
                    .unwrap_or(TypeId::ERROR);

                if constraint_ty != TypeId::ERROR {
                    if !self.check_trait_impl(act_ty, constraint_ty) {
                        let param_name = self.ctx.resolve(param.name);
                        let req_str = self.ctx.ty_to_string(constraint_ty);
                        let act_str = self.ctx.ty_to_string(act_ty);
                        self.ctx
                            .struct_error(
                                span,
                                format!(
                                    "type does not satisfy trait bounds for generic parameter `{}`",
                                    param_name
                                ),
                            )
                            .with_hint(format!("required trait: `{}`", req_str))
                            .with_hint(format!("provided type: `{}`", act_str))
                            .emit();
                    }
                }
            }
        }
    }

    // ==========================================
    //          Core Operations & Coercion
    // ==========================================

    fn check_binary(
        &mut self,
        lhs: &Expr,
        op: BinaryOperator,
        rhs: &Expr,
        expected_ty: Option<TypeId>,
    ) -> TypeId {
        let lhs_ty = self.check_expr(lhs, expected_ty);
        let rhs_ty = self.check_expr(rhs, Some(lhs_ty));

        let l_norm = self.strip_mut(lhs_ty);
        let r_norm = self.strip_mut(rhs_ty);

        if l_norm == TypeId::ERROR || r_norm == TypeId::ERROR {
            return TypeId::ERROR;
        }

        let is_l_ptr = matches!(
            self.ctx.type_registry.get(l_norm),
            TypeKind::Pointer(_) | TypeKind::VolatilePtr(_)
        );
        let is_r_ptr = matches!(
            self.ctx.type_registry.get(r_norm),
            TypeKind::Pointer(_) | TypeKind::VolatilePtr(_)
        );

        use BinaryOperator::*;
        match op {
            Add | Subtract | Multiply | Divide | Modulo => {
                if is_l_ptr || is_r_ptr {
                    self.ctx.struct_error(lhs.span, "implicit pointer arithmetic is strictly forbidden in Kern")
                        .with_hint("use explicit `as usize` casts to perform arithmetic, or use pointer methods")
                        .emit();
                    return TypeId::ERROR;
                }
                if !self.check_coercion(rhs.span, l_norm, r_norm) {
                    return TypeId::ERROR;
                }
                l_norm
            }
            Equal | NotEqual | LessThan | GreaterThan | LessOrEqual | GreaterOrEqual => {
                if !self.check_coercion(rhs.span, l_norm, r_norm) {
                    return TypeId::ERROR;
                }
                TypeId::BOOL
            }
            LogicalAnd | LogicalOr => {
                self.check_coercion(lhs.span, TypeId::BOOL, l_norm);
                self.check_coercion(rhs.span, TypeId::BOOL, r_norm);
                TypeId::BOOL
            }
            _ => {
                // Bitwise Ops
                if !self.ctx.type_registry.is_integer(l_norm) {
                    self.ctx
                        .struct_error(lhs.span, "bitwise operations require integer types")
                        .emit();
                }
                if !self.check_coercion(rhs.span, l_norm, r_norm) {
                    return TypeId::ERROR;
                }
                l_norm
            }
        }
    }

    fn check_unary(
        &mut self,
        op: UnaryOperator,
        operand: &Expr,
        span: Span,
        expected_ty: Option<TypeId>,
    ) -> TypeId {
        let inner_expected = match op {
            UnaryOperator::Negate | UnaryOperator::BitwiseNot => expected_ty,
            UnaryOperator::AddressOf => {
                if let Some(exp) = expected_ty {
                    let norm = self.strip_mut(exp);
                    if let TypeKind::Pointer(inner) | TypeKind::VolatilePtr(inner) =
                        self.ctx.type_registry.get(norm)
                    {
                        Some(*inner)
                    } else {
                        None
                    }
                } else {
                    None
                }
            }
            _ => None,
        };

        let op_ty = self.check_expr(operand, inner_expected);
        if op_ty == TypeId::ERROR {
            return TypeId::ERROR;
        }

        match op {
            UnaryOperator::AddressOf => self.ctx.type_registry.intern(TypeKind::Pointer(op_ty)),
            UnaryOperator::PointerDeRef => {
                let norm = self.strip_mut(op_ty);
                match self.ctx.type_registry.get(norm) {
                    TypeKind::Pointer(inner) | TypeKind::VolatilePtr(inner) => *inner,
                    _ => {
                        let ty_str = self.ctx.ty_to_string(op_ty);
                        self.ctx
                            .struct_error(span, "cannot dereference a non-pointer type")
                            .with_hint(format!("type is `{}`", ty_str))
                            .emit();
                        TypeId::ERROR
                    }
                }
            }
            UnaryOperator::LengthOf => {
                let norm = self.strip_mut(op_ty);
                match self.ctx.type_registry.get(norm) {
                    TypeKind::Array { .. } | TypeKind::Slice(_) => TypeId::USIZE,
                    _ => {
                        self.ctx
                            .struct_error(
                                span,
                                "length operator `#` can only be applied to arrays and slices",
                            )
                            .emit();
                        TypeId::ERROR
                    }
                }
            }
            UnaryOperator::Negate => {
                if !self.ctx.type_registry.is_integer(self.strip_mut(op_ty))
                    && !self.ctx.type_registry.is_float(self.strip_mut(op_ty))
                {
                    self.ctx
                        .struct_error(span, "negation requires a numeric type")
                        .emit();
                }
                op_ty
            }
            UnaryOperator::LogicalNot => {
                self.check_coercion(span, TypeId::BOOL, op_ty);
                TypeId::BOOL
            }
            UnaryOperator::BitwiseNot => {
                if !self.ctx.type_registry.is_integer(self.strip_mut(op_ty)) {
                    self.ctx
                        .struct_error(span, "bitwise NOT requires an integer type")
                        .emit();
                }
                op_ty
            }
        }
    }

    fn check_assign(&mut self, lhs: &Expr, rhs: &Expr, span: Span) -> TypeId {
        let lhs_ty = self.check_expr(lhs, None);

        if !self.is_mut_type(lhs_ty) && lhs_ty != TypeId::ERROR {
            self.ctx
                .struct_error(
                    lhs.span,
                    "cannot assign to an immutable variable or location",
                )
                .with_hint("consider using the `mut` type modifier in the declaration")
                .emit();
        }

        let l_norm = self.strip_mut(lhs_ty);
        let rhs_ty = self.check_expr(rhs, Some(l_norm));

        if lhs_ty == TypeId::ERROR || rhs_ty == TypeId::ERROR {
            return TypeId::ERROR;
        }

        self.check_coercion(span, l_norm, self.strip_mut(rhs_ty));
        TypeId::VOID
    }

    // ==========================================
    //            Coercion & Mutability
    // ==========================================

    pub fn check_coercion(&mut self, span: Span, expected: TypeId, actual: TypeId) -> bool {
        let exp = self.strip_mut(expected);
        let act = self.strip_mut(actual);

        // 1. 快速通道：精确匹配或已有错误
        if exp == act || exp == TypeId::ERROR || act == TypeId::ERROR {
            return true;
        }

        let exp_kind = self.ctx.type_registry.get(exp).clone();
        let act_kind = self.ctx.type_registry.get(act).clone();

        // 2. 指针与易失指针安全降级 (*mut T -> *T, ^mut T -> ^T)
        if self.check_pointer_downgrade(&exp_kind, &act_kind) {
            return true;
        }

        // 3. 切片降级与数组退化 (Array Decay)
        if let TypeKind::Slice(exp_elem) = &exp_kind {
            // Downgrade: []mut T -> []T
            if self.check_slice_downgrade(*exp_elem, &act_kind) {
                return true;
            }

            // Decay: [N]mut T -> []mut T (or []T)
            match self.check_array_decay(*exp_elem, &act_kind, span) {
                Ok(true) => return true,
                Err(()) => return false, // 已经发射了精准的 Mut 错误，直接中止，防止报错冗余
                Ok(false) => {}          // 不匹配，继续往下走到 Fallback
            }
        }

        // 4. 兜底：无合适降级规则，报类型不匹配
        self.emit_mismatch_error(span, expected, actual);
        false
    }

    /// 核心守卫：判断是否是安全的 Mut 降级 (mut T -> T)
    /// 统一了 Pointer, VolatilePtr 和 Slice 的内部 Mut 校验逻辑！
    fn is_safe_mut_downgrade(&self, exp_inner: TypeId, act_inner: TypeId) -> bool {
        if self.strip_mut(exp_inner) == self.strip_mut(act_inner) {
            // 允许 `mut` 到非 `mut` 的单向降级
            self.is_mut_type(act_inner) && !self.is_mut_type(exp_inner)
        } else {
            false
        }
    }

    /// 助手 1：指针降级校验
    fn check_pointer_downgrade(&self, exp_kind: &TypeKind, act_kind: &TypeKind) -> bool {
        match (exp_kind, act_kind) {
            (TypeKind::Pointer(e_inner), TypeKind::Pointer(a_inner))
            | (TypeKind::VolatilePtr(e_inner), TypeKind::VolatilePtr(a_inner)) => {
                self.is_safe_mut_downgrade(*e_inner, *a_inner)
            }
            _ => false,
        }
    }

    /// 助手 2：切片降级校验
    fn check_slice_downgrade(&self, exp_elem: TypeId, act_kind: &TypeKind) -> bool {
        if let TypeKind::Slice(act_elem) = act_kind {
            self.is_safe_mut_downgrade(exp_elem, *act_elem)
        } else {
            false
        }
    }

    /// 助手 3：数组到切片的退化 (Array Decay)
    fn check_array_decay(
        &mut self,
        exp_elem: TypeId,
        act_kind: &TypeKind,
        span: Span,
    ) -> Result<bool, ()> {
        if let TypeKind::Array { elem: act_elem, .. } = act_kind {
            let exp_base = self.strip_mut(exp_elem);
            let act_base = self.strip_mut(*act_elem);

            if exp_base == act_base {
                let exp_is_mut = self.is_mut_type(exp_elem);
                let act_is_mut = self.is_mut_type(*act_elem);

                // 拦截非法的 Mut 提升 (Immutable Array -> Mutable Slice)
                if exp_is_mut && !act_is_mut {
                    self.ctx.struct_error(span, "cannot implicitly convert an immutable array to a mutable slice `[]mut T`")
                        .with_hint("the slice must be immutable `[]T` to match the array's mutability")
                        .emit();
                    return Err(()); // 告诉主控：我已经精准拦截并报错了，请直接返回 false
                }
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// 助手 4：格式化并输出类型不匹配错误
    fn emit_mismatch_error(&mut self, span: Span, expected: TypeId, actual: TypeId) {
        let exp_str = self.ctx.ty_to_string(expected);
        let act_str = self.ctx.ty_to_string(actual);

        self.ctx
            .struct_error(span, "mismatched types")
            .with_hint(format!("expected `{}`", exp_str))
            .with_hint(format!("   found `{}`", act_str))
            .emit();
    }

    fn check_cast(&mut self, span: Span, from: TypeId, to: TypeId) {
        if from == to || from == TypeId::ERROR || to == TypeId::ERROR {
            return;
        }

        let f_norm = self.strip_mut(from);
        let t_norm = self.strip_mut(to);

        // === 1. Trait Object 强转 ===
        if let TypeKind::TraitObject(to_def_id, _) = self.ctx.type_registry.get(t_norm) {
            let is_to_mut = self.is_mut_type(to);
            let is_from_mut_ptr = self.is_mutable_pointer(from);

            if is_to_mut && !is_from_mut_ptr {
                self.ctx
                    .struct_error(
                        span,
                        "cannot cast a read-only pointer to a mutable trait object `mut Trait`",
                    )
                    .with_hint("ensure the source pointer is `*mut T`")
                    .emit();
                return;
            }

            // 🌟 修复: 检查 from 是否已经是 TraitObject
            let is_from_ptr = matches!(
                self.ctx.type_registry.get(f_norm),
                TypeKind::Pointer(_) | TypeKind::VolatilePtr(_)
            );
            let is_from_trait_obj = matches!(
                self.ctx.type_registry.get(f_norm),
                TypeKind::TraitObject(..)
            );

            if !is_from_ptr && !is_from_trait_obj {
                let from_str = self.ctx.ty_to_string(from);
                self.ctx
                    .struct_error(
                        span,
                        "only pointers or trait objects can be cast to a trait object",
                    )
                    .with_hint(format!("source type is `{}`", from_str))
                    .emit();
                return;
            }

            let sym = self.ctx.defs[to_def_id.0 as usize].name().unwrap();
            if !self.check_trait_impl(from, t_norm) {
                let trait_name = self.ctx.resolve(sym);
                self.ctx
                    .struct_error(
                        span,
                        format!("the source type does not implement trait `{}`", trait_name),
                    )
                    .emit();
            }
            return;
        }

        // === 2. 常规的 Bit-Pattern 强转 ===
        let is_f_int = self.ctx.type_registry.is_integer(f_norm);
        let is_t_int = self.ctx.type_registry.is_integer(t_norm);
        let is_f_ptr = matches!(
            self.ctx.type_registry.get(f_norm),
            TypeKind::Pointer(_) | TypeKind::VolatilePtr(_)
        );
        let is_t_ptr = matches!(
            self.ctx.type_registry.get(t_norm),
            TypeKind::Pointer(_) | TypeKind::VolatilePtr(_)
        );
        let is_f_slice = matches!(self.ctx.type_registry.get(f_norm), TypeKind::Slice(_));

        if is_f_slice && is_t_ptr {
            return;
        }
        if (is_f_int && is_t_int)
            || (is_f_ptr && is_t_ptr)
            || (is_f_int && is_t_ptr)
            || (is_f_ptr && is_t_int)
        {
            return;
        }

        let from_str = self.ctx.ty_to_string(from);
        let to_str = self.ctx.ty_to_string(to);
        self.ctx.struct_error(span, "invalid `as` cast")
            .with_hint("`as` only supports bit-pattern preservation (e.g., int to ptr) or Trait Object construction")
            .with_hint(format!("attempted to cast from `{}` to `{}`", from_str, to_str))
            .emit();
    }

    fn is_mutable_pointer(&self, ty: TypeId) -> bool {
        let norm = self.ctx.type_registry.normalize(ty);
        match self.ctx.type_registry.get(norm) {
            TypeKind::Pointer(inner) | TypeKind::VolatilePtr(inner) => self.is_mut_type(*inner),
            _ => false,
        }
    }

    // ==========================================
    //            Field & Method Access
    // ==========================================

    fn check_field_access(
        &mut self,
        lhs: &Expr,
        field: crate::utils::SymbolId,
        span: Span,
    ) -> TypeId {
        if let ExprKind::Identifier(name) = &lhs.kind {
            if let Some(info) = self.ctx.scopes.resolve(*name) {
                if info.kind == SymbolKind::Module {
                    let mod_def_id = info.def_id.unwrap();
                    let mod_scope = if let Def::Module(m) = &self.ctx.defs[mod_def_id.0 as usize] {
                        m.scope_id
                    } else {
                        unreachable!()
                    };

                    // 去那个模块的作用域里寻找真实的符号
                    if let Some(target_info) = self.ctx.scopes.resolve_in(mod_scope, field) {
                        let real_ty = if target_info.kind == SymbolKind::Function {
                            self.ctx
                                .type_registry
                                .intern(TypeKind::FnDef(target_info.def_id.unwrap(), vec![]))
                        } else {
                            target_info.type_id
                        };
                        let mod_ty = self.ctx.type_registry.intern(TypeKind::Module(mod_def_id));
                        self.ctx.node_types.insert(lhs.id, mod_ty);
                        return real_ty;
                    } else {
                        let mod_name = self.ctx.resolve(*name);
                        let field_name = self.ctx.resolve(field);
                        self.ctx
                            .struct_error(
                                span,
                                format!(
                                    "module `{}` has no public member `{}`",
                                    mod_name, field_name
                                ),
                            )
                            .emit();
                        return TypeId::ERROR;
                    }
                }
            }
        }

        let lhs_ty = self.check_expr(lhs, None);
        if lhs_ty == TypeId::ERROR {
            return TypeId::ERROR;
        }

        // 1. 获取底层规范类型以及访问路径的 Mut 属性 (自动解引用逻辑)
        let (current_norm, is_target_mut) = self.get_base_type_and_mut(lhs_ty);

        // 2. 如果是 Trait Object，走虚表方法解析路径
        if let TypeKind::TraitObject(trait_def_id, trait_args) =
            self.ctx.type_registry.get(current_norm).clone()
        {
            return self.resolve_trait_object_method(trait_def_id, &trait_args, field, span);
        }

        // 3. 如果是具名类型 (Struct/Union/Enum)，查找字段或变体
        if let TypeKind::Def(def_id, generic_args) =
            self.ctx.type_registry.get(current_norm).clone()
        {
            if let Some(field_ty) =
                self.resolve_def_field(def_id, &generic_args, field, is_target_mut)
            {
                return field_ty;
            }
        }

        // 4. 作为最后的 fallback，去全局的 impl 块中查找方法 (处理形如 `10.to_string()` 或普通结构体方法)
        if let Some(method_ty) = self.resolve_impl_method(lhs_ty, field) {
            return method_ty;
        }

        // 5. 全部失败，抛出详细诊断
        let field_str = self.ctx.resolve(field);
        let lhs_str = self.ctx.ty_to_string(lhs_ty);

        self.ctx
            .struct_error(
                span,
                format!(
                    "no field or method named `{}` found on type `{}`",
                    field_str, lhs_str
                ),
            )
            .with_hint(
                "if this is a method, ensure the trait defining it is imported and implemented",
            )
            .with_hint("if this is a struct field, check for typos")
            .emit();

        TypeId::ERROR
    }

    /// 辅助方法 1：剥离 Pointer/Mut 获取真实的底层数据类型，并推导字段级别的 Mut 可见性
    fn get_base_type_and_mut(&self, mut base_ty: TypeId) -> (TypeId, bool) {
        let mut is_target_mut = false;

        loop {
            let norm = self.ctx.type_registry.normalize(base_ty);
            match self.ctx.type_registry.get(norm) {
                TypeKind::Mut(inner) => {
                    is_target_mut = true;
                    base_ty = *inner;
                }
                TypeKind::Pointer(inner) | TypeKind::VolatilePtr(inner) => {
                    // 指针解引用会重置 Mut 追踪：`*T` 不能改内部，`*mut T` 由 inner 自己决定
                    is_target_mut = false;
                    base_ty = *inner;
                }
                _ => return (norm, is_target_mut),
            }
        }
    }

    /// 辅助方法 2：解析 Trait Object 的接口方法
    fn resolve_trait_object_method(
        &mut self,
        trait_def_id: crate::sema::ty::DefId,
        trait_args: &[TypeId],
        field: crate::utils::SymbolId,
        span: Span,
    ) -> TypeId {
        let trait_def = match &self.ctx.defs[trait_def_id.0 as usize] {
            Def::Trait(t) => t.clone(),
            _ => unreachable!(),
        };

        if let Some(&(_, mut method_ty)) = trait_def
            .resolved_methods
            .iter()
            .find(|(m_name, _)| *m_name == field)
        {
            // 泛型实例化替换
            if !trait_def.generics.is_empty() && !trait_args.is_empty() {
                let mut map = std::collections::HashMap::new();
                for (i, param) in trait_def.generics.iter().enumerate() {
                    map.insert(param.name, trait_args[i]);
                }
                let mut subst =
                    crate::sema::typeck::subst::Substituter::new(&mut self.ctx.type_registry, &map);
                method_ty = subst.substitute(method_ty);
            }
            return method_ty;
        }

        let field_str = self.ctx.resolve(field);
        self.ctx
            .struct_error(
                span,
                format!("method `{}` not found in trait object", field_str),
            )
            .with_hint("ensure the method is explicitly declared in the trait's contract")
            .emit();
        TypeId::ERROR
    }

    /// 辅助方法 3：解析 Struct/Union 字段或 Enum 变体
    fn resolve_def_field(
        &mut self,
        def_id: crate::sema::ty::DefId,
        generic_args: &[TypeId],
        field: crate::utils::SymbolId,
        is_target_mut: bool,
    ) -> Option<TypeId> {
        let def = self.ctx.defs[def_id.0 as usize].clone();

        match &def {
            Def::Struct(s) => {
                if let Some(f) = s.fields.iter().find(|f| f.name == field) {
                    return Some(self.apply_generics_and_mut(
                        &s.generics,
                        generic_args,
                        f.type_node.id,
                        is_target_mut,
                    ));
                }
            }
            Def::Union(u) => {
                if let Some(f) = u.fields.iter().find(|f| f.name == field) {
                    return Some(self.apply_generics_and_mut(
                        &u.generics,
                        generic_args,
                        f.type_node.id,
                        is_target_mut,
                    ));
                }
            }
            Def::Enum(e) => {
                if e.variants.iter().any(|v| v.name == field) {
                    // 访问 Enum 变体（如 `Status.Ok`），返回 Enum 本身的类型
                    return Some(
                        self.ctx
                            .type_registry
                            .intern(TypeKind::Def(def_id, generic_args.to_vec())),
                    );
                }
            }
            _ => {}
        }
        None
    }

    /// 辅助方法 3.1：处理字段提取后的泛型替换与 Mut 传染
    fn apply_generics_and_mut(
        &mut self,
        generics: &[ast::GenericParam],
        args: &[TypeId],
        node_id: ast::NodeId,
        is_target_mut: bool,
    ) -> TypeId {
        let mut field_ty = self
            .ctx
            .node_types
            .get(&node_id)
            .copied()
            .unwrap_or(TypeId::ERROR);

        if !generics.is_empty() && !args.is_empty() {
            let mut map = std::collections::HashMap::new();
            for (i, param) in generics.iter().enumerate() {
                map.insert(param.name, args[i]);
            }
            let mut subst =
                crate::sema::typeck::subst::Substituter::new(&mut self.ctx.type_registry, &map);
            field_ty = subst.substitute(field_ty);
        }

        // 🌟 Mut 传染机制：如果外层实例是 mut 的，那么它的字段默认也被标记为 mut
        if is_target_mut {
            field_ty = self.ctx.type_registry.intern(TypeKind::Mut(field_ty));
        }
        field_ty
    }

    /// 辅助方法 4：通过全局 Impl 块进行方法分发 (Method Dispatch)
    fn resolve_impl_method(
        &mut self,
        lhs_ty: TypeId,
        field: crate::utils::SymbolId,
    ) -> Option<TypeId> {
        let mut found_method_id = None;
        let mut resolved_impl_args = Vec::new();

        // 注意：遍历全局 Impl 块是 O(N) 的，未来可以考虑在 Sema 收集阶段将方法缓存在 Context 中
        for global_def in &self.ctx.defs {
            if let Def::Impl(impl_def) = global_def {
                let impl_target_ty = self
                    .ctx
                    .node_types
                    .get(&impl_def.target_type.id)
                    .copied()
                    .unwrap_or(TypeId::ERROR);
                let mut map = std::collections::HashMap::new();

                if self.unify(impl_target_ty, lhs_ty, &mut map) {
                    // 将 Impl 块捕获的泛型参数提取出来
                    for param in &impl_def.generics {
                        resolved_impl_args
                            .push(map.get(&param.name).copied().unwrap_or(TypeId::ERROR));
                    }
                    // 在匹配的 Impl 块内寻找目标函数
                    for &method_id in &impl_def.methods {
                        if let Def::Function(func_def) = &self.ctx.defs[method_id.0 as usize] {
                            if func_def.name == field {
                                found_method_id = Some(method_id);
                                break;
                            }
                        }
                    }
                }
            }
            if found_method_id.is_some() {
                break;
            }
        }

        found_method_id.map(|method_id| {
            self.ctx
                .type_registry
                .intern(TypeKind::FnDef(method_id, resolved_impl_args))
        })
    }

    // ==========================================
    //            Function & Method Calls
    // ==========================================

    fn check_call(&mut self, callee: &Expr, args: &[Expr], span: Span) -> TypeId {
        let callee_ty = self.check_expr(callee, None);
        let norm_callee = self.strip_mut(callee_ty);

        if norm_callee == TypeId::ERROR {
            // 确保每个实参节点都能被推导并注册到 ctx.node_types 中，防止产生 AST 空洞。
            for arg in args {
                self.check_expr(arg, None);
            }
            return TypeId::ERROR;
        }

        // 1. 获取准确的函数签名 (处理泛型单态化替换)
        let sig_ty = self.resolve_callee_signature(norm_callee);

        // 2. 校验签名并执行分发
        if let TypeKind::Function {
            params,
            ret,
            is_variadic,
        } = self.ctx.type_registry.get(sig_ty).clone()
        {
            // 3. 探查是否是方法调用，提取接收者 (Receiver) 信息
            let (is_method, receiver_ty) = self.resolve_method_context(callee);

            // 4. 校验参数数量 (Arity)
            self.check_call_arity(args.len(), params.len(), is_method, is_variadic, span);

            // 5. 校验方法接收者上下文 (隐式 Self 匹配)
            if is_method && !params.is_empty() {
                self.check_method_receiver(params[0], receiver_ty, callee.span);
            }

            // 6. 逐个校验传入参数 (包含 C ABI 可变参数规则)
            self.check_call_arguments(args, &params, is_method, is_variadic);

            return ret;
        }

        // 7. 兜底报错：该类型不可调用
        let callee_str = self.ctx.ty_to_string(callee_ty);
        self.ctx
            .struct_error(callee.span, "expression is not callable")
            .with_hint(format!("type is `{}`", callee_str))
            .emit();
        TypeId::ERROR
    }

    /// 助手 1：解析函数的实际签名。如果是带有泛型实参的 FnDef，立即执行类型替换。
    fn resolve_callee_signature(&mut self, norm_callee: TypeId) -> TypeId {
        if let TypeKind::FnDef(def_id, generic_args) =
            self.ctx.type_registry.get(norm_callee).clone()
        {
            let f = match &self.ctx.defs[def_id.0 as usize] {
                Def::Function(func) => func,
                _ => unreachable!(),
            };

            let raw_sig = f.resolved_sig.expect("Function signature missing");

            // 如果存在泛型，执行查表和替换
            if !f.generics.is_empty() && !generic_args.is_empty() {
                let mut map = std::collections::HashMap::new();
                for (i, param) in f.generics.iter().enumerate() {
                    map.insert(param.name, generic_args[i]);
                }
                let mut subst =
                    crate::sema::typeck::subst::Substituter::new(&mut self.ctx.type_registry, &map);
                return subst.substitute(raw_sig);
            }
            return raw_sig;
        }

        // 普通函数指针
        norm_callee
    }

    /// 助手 2：判断这是否是一个方法调用，如果是，提取它的 Receiver 类型 (LHS)
    fn resolve_method_context(&self, callee: &Expr) -> (bool, TypeId) {
        if let ExprKind::FieldAccess { lhs, .. } = &callee.kind {
            // 拦截：如果 lhs 是一个模块，则绝对不是运行时的方法调用
            if let ExprKind::Identifier(name) = &lhs.kind {
                if let Some(info) = self.ctx.scopes.resolve(*name) {
                    if info.kind == SymbolKind::Module {
                        return (false, TypeId::ERROR);
                    }
                }
            }

            let callee_node_ty = self
                .ctx
                .node_types
                .get(&callee.id)
                .copied()
                .unwrap_or(TypeId::ERROR);
            let norm_node_ty = self.ctx.type_registry.normalize(callee_node_ty);

            if matches!(
                self.ctx.type_registry.get(norm_node_ty),
                TypeKind::FnDef(..) | TypeKind::Function { .. }
            ) {
                let receiver_ty = self
                    .ctx
                    .node_types
                    .get(&lhs.id)
                    .copied()
                    .unwrap_or(TypeId::ERROR);
                return (true, receiver_ty);
            }
        }
        (false, TypeId::ERROR)
    }

    /// 助手 3：校验参数个数 (Arity)
    fn check_call_arity(
        &mut self,
        arg_count: usize,
        param_count: usize,
        is_method: bool,
        is_variadic: bool,
        span: Span,
    ) {
        let expected_arg_count = if is_method {
            param_count.saturating_sub(1)
        } else {
            param_count
        };

        if is_variadic {
            if arg_count < expected_arg_count {
                self.ctx
                    .struct_error(
                        span,
                        format!(
                            "function expects at least {} arguments, but {} were provided",
                            expected_arg_count, arg_count
                        ),
                    )
                    .emit();
            }
        } else {
            if arg_count != expected_arg_count {
                self.ctx
                    .struct_error(
                        span,
                        format!(
                            "function expects exactly {} arguments, but {} were provided",
                            expected_arg_count, arg_count
                        ),
                    )
                    .emit();
            }
        }
    }

    /// 助手 4：Kern 专属校验 - 方法调用的接收者类型匹配
    fn check_method_receiver(&mut self, expected_self: TypeId, receiver_ty: TypeId, span: Span) {
        if !self.check_coercion(span, expected_self, receiver_ty) {
            let norm_expected = self.strip_mut(expected_self);
            let is_exp_ptr = matches!(
                self.ctx.type_registry.get(norm_expected),
                TypeKind::Pointer(_) | TypeKind::VolatilePtr(_)
            );

            if is_exp_ptr {
                self.ctx.struct_error(span, "method receiver type mismatch")
                    .with_hint("the method expects a pointer receiver")
                    .with_hint("Kern does not implicitly take addresses for method calls. Try using `(&obj).method()` or `obj.&.method()`")
                    .emit();
            }
        }
    }

    /// 助手 5：逐一检查参数的类型转换，并处理 C ABI 可变参数 (Varargs) 的类型提升规则
    fn check_call_arguments(
        &mut self,
        args: &[Expr],
        params: &[TypeId],
        is_method: bool,
        _is_variadic: bool,
    ) {
        let param_offset = if is_method { 1 } else { 0 };

        for (i, arg) in args.iter().enumerate() {
            let sig_param_idx = i + param_offset;

            if sig_param_idx < params.len() {
                // 1. 常规参数校验
                let arg_ty = self.check_expr(arg, Some(params[sig_param_idx]));
                self.check_coercion(arg.span, params[sig_param_idx], arg_ty);
            } else {
                // 2. Variadic 额外参数校验 (C ABI Rules)
                let arg_ty = self.check_expr(arg, None);
                let norm_arg = self.strip_mut(arg_ty);

                if norm_arg == TypeId::ERROR {
                    continue;
                }

                // C ABI 整型提升规则：传入可变参数的整型不能小于 32位
                let is_small_int = matches!(
                    norm_arg,
                    TypeId::I8 | TypeId::I16 | TypeId::U8 | TypeId::U16
                );

                if is_small_int {
                    self.ctx.struct_error(arg.span, "C ABI requires integer arguments passed to `...` to be at least 32-bit")
                        .with_hint("please cast it explicitly (e.g., `as i32` or `as u32`)")
                        .emit();
                } else if norm_arg == TypeId::F32 {
                    // C ABI 浮点型提升规则：传入可变参数的浮点数必须被提升为 64位 (double)
                    self.ctx
                        .struct_error(
                            arg.span,
                            "C ABI requires float arguments passed to `...` to be 64-bit",
                        )
                        .with_hint("please cast it explicitly (e.g., `as f64`)")
                        .emit();
                }
            }
        }
    }

    // ==========================================
    //            Data Literal Checking
    // ==========================================

    fn check_data_literal(
        &mut self,
        kind: &ast::DataLiteralKind,
        expected: TypeId,
        span: Span,
    ) -> TypeId {
        let exp_norm = self.strip_mut(expected);

        match kind {
            ast::DataLiteralKind::Array(elems) => {
                self.check_array_literal(elems, expected, exp_norm, span)
            }
            ast::DataLiteralKind::Repeat { value, count } => {
                self.check_repeat_literal(value, count, expected, exp_norm, span)
            }
            ast::DataLiteralKind::Struct(init_fields) => {
                self.check_struct_or_union_literal(init_fields, expected, exp_norm, span)
            }
            ast::DataLiteralKind::Scalar(inner) => self.check_scalar_literal(inner, expected),
        }
    }

    /// 辅助方法 1：校验普通数组字面量 `.{ 1, 2, 3 }`
    fn check_array_literal(
        &mut self,
        elems: &[Expr],
        expected: TypeId,
        exp_norm: TypeId,
        span: Span,
    ) -> TypeId {
        if let TypeKind::Array {
            elem: exp_elem,
            len,
        } = self.ctx.type_registry.get(exp_norm)
        {
            let exp_elem_ty = *exp_elem;

            if elems.len() as u64 != *len {
                self.ctx
                    .struct_error(
                        span,
                        format!(
                            "array literal length ({}) does not match expected length ({})",
                            elems.len(),
                            len
                        ),
                    )
                    .emit();
            }

            for e in elems {
                let act_ty = self.check_expr(e, Some(exp_elem_ty));
                self.check_coercion(e.span, exp_elem_ty, act_ty);
            }
            expected
        } else {
            let ty_str = self.ctx.ty_to_string(expected);
            self.ctx
                .struct_error(span, "expected an array type for array literal `.{ ... }`")
                .with_hint(format!("context expects `{}`", ty_str))
                .emit();
            TypeId::ERROR
        }
    }

    /// 辅助方法 2：校验重复数组字面量 `.{ 0; 1024 }`
    fn check_repeat_literal(
        &mut self,
        value: &Expr,
        count: &Expr,
        expected: TypeId,
        exp_norm: TypeId,
        span: Span,
    ) -> TypeId {
        if let TypeKind::Array { elem: exp_elem, .. } = self.ctx.type_registry.get(exp_norm) {
            let exp_elem_ty = *exp_elem;

            let val_ty = self.check_expr(value, Some(exp_elem_ty));
            self.check_coercion(value.span, exp_elem_ty, val_ty);

            let c_ty = self.check_expr(count, Some(TypeId::USIZE));
            if !self.ctx.type_registry.is_integer(self.strip_mut(c_ty)) {
                self.ctx
                    .struct_error(count.span, "repeat count must be an integer")
                    .emit();
            }
            expected
        } else {
            let ty_str = self.ctx.ty_to_string(expected);
            self.ctx
                .struct_error(
                    span,
                    "expected an array type for repeat literal `.{ v; N }`",
                )
                .with_hint(format!("context expects `{}`", ty_str))
                .emit();
            TypeId::ERROR
        }
    }

    /// 辅助方法 3：校验结构体或联合体初始化 `.{ x: 10, y: 20 }` 或 Union `.{ as_int: 123 }`
    fn check_struct_or_union_literal(
        &mut self,
        init_fields: &[ast::StructFieldInit],
        expected: TypeId,
        exp_norm: TypeId,
        span: Span,
    ) -> TypeId {
        // 1. 提取定义信息与泛型实参，同时识别是 Struct 还是 Union
        let (def_fields, def_name, def_generics, generic_args, is_union) =
            if let TypeKind::Def(def_id, args) = self.ctx.type_registry.get(exp_norm) {
                match &self.ctx.defs[def_id.0 as usize] {
                    Def::Struct(s) => (
                        s.fields.clone(),
                        self.ctx.resolve(s.name).to_string(),
                        s.generics.clone(),
                        args.clone(),
                        false,
                    ),
                    Def::Union(u) => (
                        u.fields.clone(),
                        self.ctx.resolve(u.name).to_string(),
                        u.generics.clone(),
                        args.clone(),
                        true,
                    ),
                    _ => {
                        self.ctx
                            .struct_error(
                                span,
                                "expected a struct or union type for literal initialization",
                            )
                            .emit();
                        return TypeId::ERROR;
                    }
                }
            } else {
                self.ctx
                    .struct_error(
                        span,
                        "expected a struct or union type for literal initialization",
                    )
                    .emit();
                return TypeId::ERROR;
            };

        let mut initialized = std::collections::HashSet::new();

        // 2. 校验用户提供的初始化字段的类型
        for init_f in init_fields {
            if let Some(def_f) = def_fields.iter().find(|f| f.name == init_f.name) {
                let mut f_ty = self
                    .ctx
                    .node_types
                    .get(&def_f.type_node.id)
                    .copied()
                    .unwrap_or(TypeId::ERROR);

                // 处理泛型字段类型替换
                if !def_generics.is_empty() && !generic_args.is_empty() {
                    let mut map = std::collections::HashMap::new();
                    for (i, param) in def_generics.iter().enumerate() {
                        map.insert(param.name, generic_args[i]);
                    }
                    let mut subst = crate::sema::typeck::subst::Substituter::new(
                        &mut self.ctx.type_registry,
                        &map,
                    );
                    f_ty = subst.substitute(f_ty);
                }

                let val_ty = self.check_expr(&init_f.value, Some(f_ty));
                self.check_coercion(init_f.span, f_ty, val_ty);

                // 检查重复初始化的字段（对 struct 和 union 都有效）
                if !initialized.insert(init_f.name) {
                    let name_str = self.ctx.resolve(init_f.name);
                    self.ctx
                        .struct_error(
                            init_f.span,
                            format!("field `{}` is initialized more than once", name_str),
                        )
                        .emit();
                }
            } else {
                let name_str = self.ctx.resolve(init_f.name);
                self.ctx
                    .struct_error(
                        init_f.span,
                        format!("field `{}` does not exist in `{}`", name_str, def_name),
                    )
                    .emit();
            }
        }

        // 3. 校验 Kern 核心规则：针对 Struct 和 Union 分别处理
        if is_union {
            // Kern Union 规则：必须且只能初始化 1 个字段
            if initialized.len() != 1 {
                self.ctx
                    .struct_error(
                        span,
                        format!(
                            "union `{}` must be initialized with exactly one field",
                            def_name
                        ),
                    )
                    .with_hint(format!("you provided {} fields", initialized.len()))
                    .with_hint(
                        "unions share memory across fields, so multiple initializers are ambiguous",
                    )
                    .emit();
            }
        } else {
            // Kern Struct 规则：无隐式零初始化。漏掉字段必须显式使用 undef 或具有默认值
            for def_f in &def_fields {
                if !initialized.contains(&def_f.name) && def_f.default_value.is_none() {
                    let name_str = self.ctx.resolve(def_f.name).to_string();
                    self.ctx.struct_error(span, format!("field `{}` is missing and has no default value", name_str))
                        .with_hint("Kern structs do not zero-initialize implicitly.")
                        .with_hint(format!("use `{}: type.{{undef}}` if you intentionally want to leave memory uninitialized", name_str))
                        .emit();
                }
            }
        }

        expected
    }

    /// 辅助方法 4：校验标量构造 `.{ 10 }`
    fn check_scalar_literal(&mut self, inner: &Expr, expected: TypeId) -> TypeId {
        let inner_ty = self.check_expr(inner, Some(expected));
        self.check_coercion(inner.span, expected, inner_ty);
        expected
    }

    pub fn strip_mut(&self, ty: TypeId) -> TypeId {
        let norm = self.ctx.type_registry.normalize(ty);
        if let TypeKind::Mut(inner) = self.ctx.type_registry.get(norm) {
            *inner
        } else {
            norm
        }
    }

    pub fn is_mut_type(&self, ty: TypeId) -> bool {
        let norm = self.ctx.type_registry.normalize(ty);
        matches!(self.ctx.type_registry.get(norm), TypeKind::Mut(_))
    }

    fn check_trait_impl(&mut self, source_ty: TypeId, target_trait_ty: TypeId) -> bool {
        let mut visited = std::collections::HashSet::new();
        self.check_trait_impl_inner(source_ty, target_trait_ty, &mut visited)
    }

    fn check_trait_impl_inner(
        &mut self,
        source_ty: TypeId,
        target_trait_ty: TypeId,
        visited: &mut std::collections::HashSet<crate::sema::ty::DefId>,
    ) -> bool {
        let mut impl_blocks = Vec::new();
        for def in &self.ctx.defs {
            if let Def::Impl(impl_def) = def {
                impl_blocks.push(impl_def.clone());
            }
        }

        for impl_def in impl_blocks {
            if let Some(trait_ast) = &impl_def.trait_type {
                let impl_target_ty = self
                    .ctx
                    .node_types
                    .get(&impl_def.target_type.id)
                    .copied()
                    .unwrap_or(TypeId::ERROR);
                let impl_trait_ty = self
                    .ctx
                    .node_types
                    .get(&trait_ast.id)
                    .copied()
                    .unwrap_or(TypeId::ERROR);

                if impl_target_ty == TypeId::ERROR || impl_trait_ty == TypeId::ERROR {
                    continue;
                }

                let mut map = std::collections::HashMap::new();

                if self.unify(impl_target_ty, source_ty, &mut map) {
                    let instantiated_trait_ty = {
                        let mut subst = crate::sema::typeck::subst::Substituter::new(
                            &mut self.ctx.type_registry,
                            &map,
                        );
                        subst.substitute(impl_trait_ty)
                    };

                    let inst_norm = self.strip_mut(instantiated_trait_ty);
                    let target_norm = self.strip_mut(target_trait_ty);

                    if inst_norm == target_norm || instantiated_trait_ty == target_trait_ty {
                        return true;
                    }

                    let inst_norm = self.strip_mut(instantiated_trait_ty);
                    if let TypeKind::TraitObject(inst_def_id, _) =
                        self.ctx.type_registry.get(inst_norm)
                    {
                        if visited.insert(*inst_def_id) {
                            if let Def::Trait(trait_def) =
                                self.ctx.defs[inst_def_id.0 as usize].clone()
                            {
                                for supertrait_ast in &trait_def.supertraits {
                                    let super_ty = self
                                        .ctx
                                        .node_types
                                        .get(&supertrait_ast.id)
                                        .copied()
                                        .unwrap_or(TypeId::ERROR);
                                    let inst_super_ty = {
                                        let mut subst =
                                            crate::sema::typeck::subst::Substituter::new(
                                                &mut self.ctx.type_registry,
                                                &map,
                                            );
                                        subst.substitute(super_ty)
                                    };

                                    if inst_super_ty == target_trait_ty
                                        || self.check_trait_impl_inner(
                                            source_ty,
                                            inst_super_ty,
                                            visited,
                                        )
                                    {
                                        return true;
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        false
    }

    fn check_switch_exhaustiveness(
        &mut self,
        target_ty: TypeId,
        cases: &[ast::SwitchCase],
        has_default: bool,
        span: Span,
    ) {
        if has_default {
            return;
        }

        let norm_target = self.strip_mut(target_ty);

        if let TypeKind::Def(def_id, _) = self.ctx.type_registry.get(norm_target) {
            if let Def::Enum(e) = &self.ctx.defs[def_id.0 as usize] {
                let mut unhandled_variants: std::collections::HashSet<crate::utils::SymbolId> =
                    e.variants.iter().map(|v| v.name).collect();

                for case in cases {
                    for pat in &case.patterns {
                        if let ast::SwitchPattern::Value(v_expr) = pat {
                            if let ExprKind::EnumLiteral(name) = &v_expr.kind {
                                unhandled_variants.remove(name);
                            }
                        }
                    }
                }

                if !unhandled_variants.is_empty() {
                    let missing: Vec<String> = unhandled_variants
                        .into_iter()
                        .map(|id| self.ctx.resolve(id).to_string())
                        .collect();
                    self.ctx
                        .struct_error(span, "switch expression is not exhaustive")
                        .with_hint(format!("missing enum variants: {}", missing.join(", ")))
                        .emit();
                }
                return;
            }
        }

        self.ctx
            .struct_error(span, "switch expression must be exhaustive")
            .with_hint("consider adding an `else =>` catch-all branch")
            .emit();
    }

    fn unify(
        &self,
        generic_ty: TypeId,
        concrete_ty: TypeId,
        map: &mut std::collections::HashMap<crate::utils::SymbolId, TypeId>,
    ) -> bool {
        let gen_norm = self.strip_mut(generic_ty);
        let con_norm = self.strip_mut(concrete_ty);

        let gen_kind = self.ctx.type_registry.get(gen_norm).clone();
        let con_kind = self.ctx.type_registry.get(con_norm).clone();

        match (gen_kind, con_kind) {
            (TypeKind::Param(name), _) => {
                if let Some(&existing_ty) = map.get(&name) {
                    existing_ty == concrete_ty
                } else {
                    map.insert(name, concrete_ty);
                    true
                }
            }
            (TypeKind::Pointer(g), TypeKind::Pointer(c)) => self.unify(g, c, map),
            (TypeKind::VolatilePtr(g), TypeKind::VolatilePtr(c)) => self.unify(g, c, map),
            (TypeKind::Mut(g), TypeKind::Mut(c)) => self.unify(g, c, map),
            (TypeKind::Slice(g), TypeKind::Slice(c)) => self.unify(g, c, map),
            (TypeKind::Array { elem: ge, len: gl }, TypeKind::Array { elem: ce, len: cl }) => {
                gl == cl && self.unify(ge, ce, map)
            }
            (TypeKind::Def(g_id, g_args), TypeKind::Def(c_id, c_args)) if g_id == c_id => {
                if g_args.len() != c_args.len() {
                    return false;
                }
                for (ga, ca) in g_args.iter().zip(c_args.iter()) {
                    if !self.unify(*ga, *ca, map) {
                        return false;
                    }
                }
                true
            }
            (TypeKind::TraitObject(g_id, g_args), TypeKind::TraitObject(c_id, c_args))
                if g_id == c_id =>
            {
                if g_args.len() != c_args.len() {
                    return false;
                }
                for (ga, ca) in g_args.iter().zip(c_args.iter()) {
                    if !self.unify(*ga, *ca, map) {
                        return false;
                    }
                }
                true
            }
            _ => gen_norm == con_norm,
        }
    }
}
