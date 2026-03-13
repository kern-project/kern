use crate::driver::Context;
use crate::parser::ast::{self, BinaryOperator, Expr, ExprKind, UnaryOperator};
use crate::sema::def::Def;
use crate::sema::scope::SymbolKind;
use crate::sema::ty::{TypeId, TypeKind};
use crate::utils::Span;

/// 编译期常量求值器 (Const Evaluator)
pub struct ConstEvaluator<'a> {
    pub ctx: &'a mut Context,
}

impl<'a> ConstEvaluator<'a> {
    pub fn new(ctx: &'a mut Context) -> Self {
        Self { ctx }
    }

    /// 计算并返回安全的 usize (例如用于数组长度)
    pub fn eval_usize(&mut self, expr: &Expr) -> Result<u64, ()> {
        match self.eval_math_inner(expr, 0) {
            Ok(val) => {
                if val < 0 {
                    self.ctx
                        .struct_error(
                            expr.span,
                            "constant expression cannot evaluate to a negative number here",
                        )
                        .with_hint("array lengths and similar contexts require positive integers")
                        .emit();
                    Err(())
                } else {
                    Ok(val as u64)
                }
            }
            Err(_) => {
                // eval_math 如果不是常量，应该已经报错了
                // 这里直接向上传递失败信号
                Err(())
            }
        }
    }

    pub fn eval_math(&mut self, expr: &Expr) -> Result<i128, ()> {
        self.eval_math_inner(expr, 0)
    }

    /// 核心递归求值引擎 (带有深度限制防死循环，中央分派器)
    fn eval_math_inner(&mut self, expr: &Expr, depth: usize) -> Result<i128, ()> {
        if depth > 100 {
            self.ctx
                .struct_error(
                    expr.span,
                    "constant evaluation exceeded maximum recursion depth",
                )
                .with_hint("check for circular references in your `const` declarations")
                .emit();
            return Err(());
        }

        match &expr.kind {
            // === 1. 基础字面量 ===
            ExprKind::Integer(val) => Ok(*val as i128),
            ExprKind::Char(c) => Ok(*c as i128),
            ExprKind::Bool(b) => Ok(if *b { 1 } else { 0 }),

            // === 2. 算术与逻辑运算 ===
            ExprKind::Binary { lhs, op, rhs } => self.eval_binary(lhs, *op, rhs, depth, expr.span),
            ExprKind::Unary { op, operand } => self.eval_unary(*op, operand, depth, expr.span),

            // === 3. 查表代入全局 Const 变量 ===
            ExprKind::Identifier(name) => self.eval_identifier(*name, depth, expr.span),

            // === 4. 内置常量函数调用 (Intrinsics) ===
            ExprKind::Call { callee, .. } => self.eval_intrinsic_call(callee, expr.span),

            // === 5. 枚举字面量求值 ===
            ExprKind::EnumLiteral(variant_name) => {
                self.eval_enum_literal(expr.id, *variant_name, depth, expr.span)
            }

            // === 6. 数据初始化 ===
            ExprKind::DataInit { literal, .. } => {
                match literal {
                    // 如果是一个纯量初始化（如 mut i32.{ 0 }），直接穿透求值
                    ast::DataLiteralKind::Scalar(inner) => self.eval_math_inner(inner, depth + 1),

                    // TODO: 如果是复杂的数组/结构体，由于当前 evaluator 只能返回 i128，我们暂时报错
                    // (后续如果需要支持 const arr = [3]i32.{1,2,3}，你需要重构 evaluator 返回 ConstantValue)
                    _ => {
                        self.ctx.struct_error(expr.span, "complex data initialization (arrays, structs) is not yet supported in simple constant math evaluation").emit();
                        Err(())
                    }
                }
            }

            // === 7. 不支持的表达式 ===
            ExprKind::GenericInstantiation { .. } => {
                self.ctx
                    .struct_error(
                        expr.span,
                        "generic instantiation cannot be evaluated as a standalone constant value",
                    )
                    .with_hint("did you mean to call it like `@sizeof[T]()`?")
                    .emit();
                Err(())
            }
            _ => {
                self.ctx
                    .struct_error(expr.span, "expected a constant expression")
                    .emit();
                Err(())
            }
        }
    }

    // ==========================================
    //            Const Eval Helpers
    // ==========================================

    fn eval_binary(
        &mut self,
        lhs: &Expr,
        op: BinaryOperator,
        rhs: &Expr,
        depth: usize,
        span: Span,
    ) -> Result<i128, ()> {
        let left = self.eval_math_inner(lhs, depth + 1)?;
        let right = self.eval_math_inner(rhs, depth + 1)?;

        match op {
            BinaryOperator::Add => Ok(left.wrapping_add(right)),
            BinaryOperator::Subtract => Ok(left.wrapping_sub(right)),
            BinaryOperator::Multiply => Ok(left.wrapping_mul(right)),
            BinaryOperator::Divide => {
                if right == 0 {
                    self.ctx
                        .struct_error(rhs.span, "division by zero in constant expression")
                        .emit();
                    Err(())
                } else {
                    Ok(left / right)
                }
            }
            BinaryOperator::Modulo => {
                if right == 0 {
                    self.ctx
                        .struct_error(rhs.span, "modulo by zero in constant expression")
                        .emit();
                    Err(())
                } else {
                    Ok(left % right)
                }
            }
            BinaryOperator::ShiftLeft => Ok(left << right),
            BinaryOperator::ShiftRight => Ok(left >> right),
            BinaryOperator::BitwiseAnd => Ok(left & right),
            BinaryOperator::BitwiseOr => Ok(left | right),
            BinaryOperator::BitwiseXor => Ok(left ^ right),
            _ => {
                self.ctx
                    .struct_error(span, "operator not supported in constant expression")
                    .emit();
                Err(())
            }
        }
    }

    fn eval_unary(
        &mut self,
        op: UnaryOperator,
        operand: &Expr,
        depth: usize,
        span: Span,
    ) -> Result<i128, ()> {
        let val = self.eval_math_inner(operand, depth + 1)?;
        match op {
            UnaryOperator::Negate => Ok(val.wrapping_neg()),
            UnaryOperator::BitwiseNot => Ok(!val),
            UnaryOperator::LogicalNot => Ok(if val == 0 { 1 } else { 0 }),
            _ => {
                self.ctx
                    .struct_error(span, "unary operator not supported in constant expression")
                    .emit();
                Err(())
            }
        }
    }

    fn eval_identifier(
        &mut self,
        name: crate::utils::SymbolId,
        depth: usize,
        span: Span,
    ) -> Result<i128, ()> {
        let sym_info = self.ctx.scopes.resolve(name).cloned();

        if let Some(info) = sym_info {
            if info.kind == SymbolKind::Const {
                if let Some(def_id) = info.def_id {
                    let const_expr = if let Def::Global(g) = &self.ctx.defs[def_id.0 as usize] {
                        g.value.clone()
                    } else {
                        return Err(());
                    };

                    return self.eval_math_inner(&const_expr, depth + 1);
                }
            } else {
                let name_str = self.ctx.resolve(name).to_string();
                self.ctx
                    .struct_error(
                        span,
                        format!(
                            "`{}` is a {}, not a compile-time constant",
                            name_str,
                            self.kind_to_string(info.kind)
                        ),
                    )
                    .with_hint("only `const` variables can be used in constant expressions")
                    .emit();
                return Err(());
            }
        }
        self.ctx
            .struct_error(span, "use of undeclared identifier in constant expression")
            .emit();
        Err(())
    }

    fn eval_intrinsic_call(&mut self, callee: &Expr, span: Span) -> Result<i128, ()> {
        let callee_ty = self
            .ctx
            .node_types
            .get(&callee.id)
            .copied()
            .unwrap_or(TypeId::ERROR);
        let norm_callee = self.ctx.type_registry.normalize(callee_ty);

        if let TypeKind::FnDef(def_id, generic_args) =
            self.ctx.type_registry.get(norm_callee).clone()
        {
            if let Def::Function(f) = &self.ctx.defs[def_id.0 as usize] {
                if f.is_intrinsic {
                    let name_str = self.ctx.resolve(f.name);
                    match name_str {
                        "@sizeof" => {
                            if let Some(&target_ty) = generic_args.get(0) {
                                let size = self.compute_type_size(target_ty);
                                return Ok(size as i128);
                            }
                        }
                        "@alignof" => {
                            if let Some(&target_ty) = generic_args.get(0) {
                                let align = self.compute_type_align(target_ty);
                                return Ok(align as i128);
                            }
                        }
                        _ => {
                            self.ctx
                                .struct_error(
                                    span,
                                    format!(
                                        "intrinsic `{}` cannot be evaluated at compile time",
                                        name_str
                                    ),
                                )
                                .emit();
                            return Err(());
                        }
                    }
                }
            }
        }

        self.ctx
            .struct_error(
                span,
                "function calls are not allowed in constant expressions",
            )
            .with_hint(
                "only compile-time intrinsics like `@sizeof` or `@alignof` are permitted here",
            )
            .emit();
        Err(())
    }

    fn eval_enum_literal(
        &mut self,
        node_id: ast::NodeId,
        variant_name: crate::utils::SymbolId,
        depth: usize,
        span: Span,
    ) -> Result<i128, ()> {
        let ty = self
            .ctx
            .node_types
            .get(&node_id)
            .copied()
            .unwrap_or(TypeId::ERROR);
        let norm_ty = self.ctx.type_registry.normalize(ty);

        let def_id = if let TypeKind::Def(id, _) = self.ctx.type_registry.get(norm_ty) {
            *id
        } else {
            self.ctx
                .struct_error(
                    span,
                    "enum literal type could not be resolved during constant evaluation",
                )
                .emit();
            return Err(());
        };

        let enum_def = if let Def::Enum(e) = &self.ctx.defs[def_id.0 as usize] {
            e.clone()
        } else {
            return Err(());
        };

        let mut current_val: i128 = 0;
        for v in enum_def.variants {
            if let Some(v_expr) = v.value {
                current_val = self.eval_math_inner(&v_expr, depth + 1)?;
            }
            if v.name == variant_name {
                return Ok(current_val);
            }
            current_val += 1;
        }

        let v_str = self.ctx.resolve(variant_name).to_string();
        self.ctx
            .struct_error(span, format!("variant `.{}` not found in enum", v_str))
            .emit();
        Err(())
    }

    fn kind_to_string(&self, kind: SymbolKind) -> &'static str {
        match kind {
            SymbolKind::Var => "variable (`let`)",
            SymbolKind::Static => "static variable",
            SymbolKind::Function => "function",
            SymbolKind::Struct => "struct",
            _ => "symbol",
        }
    }

    // ==========================================
    //          Memory Layout Engine
    // ==========================================

    pub fn compute_type_align(&mut self, ty: TypeId) -> u64 {
        self.compute_type_align_inner(ty, 0)
    }

    fn compute_type_align_inner(&mut self, ty: TypeId, depth: usize) -> u64 {
        if depth > 100 {
            return 1; /* 防止非法嵌套结构体导致死循环崩溃 */
        }

        let norm = self.ctx.type_registry.normalize(ty);
        let kind = self.ctx.type_registry.get(norm).clone();

        match kind {
            TypeKind::Pointer(_) | TypeKind::VolatilePtr(_) | TypeKind::Function { .. } => {
                self.ctx.target.pointer_size
            }
            TypeKind::Slice(_) | TypeKind::TraitObject(..) => self.ctx.target.pointer_size,
            TypeKind::Mut(inner) => self.compute_type_align_inner(inner, depth + 1),
            TypeKind::Array { elem, .. } => self.compute_type_align_inner(elem, depth + 1),
            
            TypeKind::Def(def_id, generic_args) => {
                self.compute_def_align(def_id, &generic_args, depth)
            }
            TypeKind::Primitive(crate::sema::ty::PrimitiveType::Never) | TypeKind::Error => 1,
            TypeKind::Primitive(p) => self.primitive_align(p),
            _ => 1,
        }
    }

    pub fn compute_type_size(&mut self, ty: TypeId) -> u64 {
        self.compute_type_size_inner(ty, 0)
    }

    fn compute_type_size_inner(&mut self, ty: TypeId, depth: usize) -> u64 {
        if depth > 100 {
            return 0; /* 防止非法嵌套结构体导致死循环崩溃 */
        }

        let norm = self.ctx.type_registry.normalize(ty);
        let kind = self.ctx.type_registry.get(norm).clone();

        match kind {
            TypeKind::Pointer(_) | TypeKind::VolatilePtr(_) | TypeKind::Function { .. } => {
                self.ctx.target.pointer_size
            }
            TypeKind::Slice(_) | TypeKind::TraitObject(..) => self.ctx.target.pointer_size * 2,
            TypeKind::Mut(inner) => self.compute_type_size_inner(inner, depth + 1),
            TypeKind::Array { elem, len } => self.compute_type_size_inner(elem, depth + 1) * len,
            
            TypeKind::Def(def_id, generic_args) => {
                self.compute_def_size(def_id, &generic_args, depth)
            }
            TypeKind::Error => {
                // 如果是求 Error 的大小，说明之前已经报过错了，直接返回 0 防止崩溃
                0
            }
            TypeKind::Primitive(crate::sema::ty::PrimitiveType::Never) => {
                // Never 类型 (如 !) 代表不可达，但在内存布局中它被视为零尺寸类型 (ZST)
                0
            }
            TypeKind::Primitive(p) => self.primitive_size(p),
            _ => 0,
        }
    }

    fn align_to(offset: u64, align: u64) -> u64 {
        (offset + align - 1) & !(align - 1)
    }

    // ==========================================
    //       Layout Helpers (Primitives)
    // ==========================================

    fn primitive_align(&self, p: crate::sema::ty::PrimitiveType) -> u64 {
        use crate::sema::ty::PrimitiveType::*;
        match p {
            I8 | U8 | Bool => 1,
            I16 | U16 => 2,
            I32 | U32 | F32 => 4,
            I64 | U64 | F64 => 8,
            // 动态读取目标机器的指针大小 (例如 32 位架构返回 4, 64 位架构返回 8)
            ISize | USize => self.ctx.target.pointer_size,
            I128 | U128 => 16,
            _ => 1,
        }
    }

    fn primitive_size(&self, p: crate::sema::ty::PrimitiveType) -> u64 {
        use crate::sema::ty::PrimitiveType::*;
        match p {
            I8 | U8 | Bool => 1,
            I16 | U16 => 2,
            I32 | U32 | F32 => 4,
            I64 | U64 | F64 => 8,
            // 动态读取目标机器的指针大小
            ISize | USize => self.ctx.target.pointer_size,
            I128 | U128 => 16,
            _ => 0,
        }
    }

    // ==========================================
    //       Layout Helpers (Complex Defs)
    // ==========================================

    fn compute_def_align(
        &mut self,
        def_id: crate::sema::ty::DefId,
        generic_args: &[TypeId],
        depth: usize,
    ) -> u64 {
        let def = self.ctx.defs[def_id.0 as usize].clone();
        match def {
            Def::Struct(s) => {
                let map = self.prepare_generic_subst(&s.generics, generic_args);
                let mut max_align = 1;
                for field in &s.fields {
                    let f_ty = self.resolve_field_type(&field.type_node, &map);
                    let align = self.compute_type_align_inner(f_ty, depth + 1);
                    if align > max_align {
                        max_align = align;
                    }
                }
                max_align
            }
            Def::Union(u) => {
                let map = self.prepare_generic_subst(&u.generics, generic_args);
                let mut max_align = 1;
                for field in &u.fields {
                    let f_ty = self.resolve_field_type(&field.type_node, &map);
                    let align = self.compute_type_align_inner(f_ty, depth + 1);
                    if align > max_align {
                        max_align = align;
                    }
                }
                max_align
            }
            Def::Enum(e) => {
                let back_ty = self.resolve_enum_backing_type(&e);
                self.compute_type_align_inner(back_ty, depth + 1)
            }
            _ => 1,
        }
    }

    fn compute_def_size(
        &mut self,
        def_id: crate::sema::ty::DefId,
        generic_args: &[TypeId],
        depth: usize,
    ) -> u64 {
        let def = self.ctx.defs[def_id.0 as usize].clone();
        match def {
            Def::Struct(s) => {
                let map = self.prepare_generic_subst(&s.generics, generic_args);
                let mut offset = 0;
                let mut max_align = 1;

                for field in &s.fields {
                    let f_ty = self.resolve_field_type(&field.type_node, &map);
                    let f_align = self.compute_type_align_inner(f_ty, depth + 1);
                    let f_size = self.compute_type_size_inner(f_ty, depth + 1);

                    if f_align > max_align {
                        max_align = f_align;
                    }
                    offset = Self::align_to(offset, f_align);
                    offset += f_size;
                }
                // 结构体总大小必须是其最大对齐要求的整数倍 (末尾填充)
                Self::align_to(offset, max_align)
            }
            Def::Union(u) => {
                let map = self.prepare_generic_subst(&u.generics, generic_args);
                let mut max_size = 0;
                let mut max_align = 1;

                for field in &u.fields {
                    let f_ty = self.resolve_field_type(&field.type_node, &map);
                    let f_align = self.compute_type_align_inner(f_ty, depth + 1);
                    let f_size = self.compute_type_size_inner(f_ty, depth + 1);

                    if f_align > max_align {
                        max_align = f_align;
                    }
                    if f_size > max_size {
                        max_size = f_size;
                    }
                }
                // 联合体总大小也必须对其最大对齐要求进行对齐
                Self::align_to(max_size, max_align)
            }
            Def::Enum(e) => {
                let back_ty = self.resolve_enum_backing_type(&e);
                self.compute_type_size_inner(back_ty, depth + 1)
            }
            _ => 0,
        }
    }

    /// 构建泛型替换映射表
    fn prepare_generic_subst(
        &self,
        generics: &[ast::GenericParam],
        args: &[TypeId],
    ) -> std::collections::HashMap<crate::utils::SymbolId, TypeId> {
        let mut map = std::collections::HashMap::new();
        if !generics.is_empty() && !args.is_empty() {
            for (i, param) in generics.iter().enumerate() {
                map.insert(param.name, args[i]);
            }
        }
        map
    }

    /// 获取字段的 AST 类型，并在需要时应用泛型替换
    fn resolve_field_type(
        &mut self,
        type_node: &ast::TypeNode,
        map: &std::collections::HashMap<crate::utils::SymbolId, TypeId>,
    ) -> TypeId {
        let mut f_ty = self
            .ctx
            .node_types
            .get(&type_node.id)
            .copied()
            .unwrap_or(TypeId::ERROR);
        if !map.is_empty() {
            let mut subst =
                crate::sema::typeck::subst::Substituter::new(&mut self.ctx.type_registry, map);
            f_ty = subst.substitute(f_ty);
        }
        f_ty
    }

    /// 获取枚举的底层表示类型
    fn resolve_enum_backing_type(&self, e: &crate::sema::def::EnumDef) -> TypeId {
        if let Some(bt) = &e.backing_type {
            self.ctx
                .node_types
                .get(&bt.id)
                .copied()
                .unwrap_or(TypeId::U32)
        } else {
            TypeId::U32
        }
    }
}
