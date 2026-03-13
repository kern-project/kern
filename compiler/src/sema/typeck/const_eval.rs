use crate::driver::Context;
use crate::parser::ast::{self, BinaryOperator, Expr, ExprKind, UnaryOperator};
use crate::sema::def::Def;
use crate::sema::scope::SymbolKind;
use crate::sema::ty::{PrimitiveType, TypeId, TypeKind};
use crate::utils::{Span, SymbolId};
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq)]
pub enum ConstValue {
    Int(i128),
    Float(f64),
    Bool(bool),
    String(String),
    Array(Vec<ConstValue>),
    Struct(HashMap<SymbolId, ConstValue>),
    Void,
}

pub struct ConstEvaluator<'a> {
    pub ctx: &'a mut Context,
}

impl<'a> ConstEvaluator<'a> {
    pub fn new(ctx: &'a mut Context) -> Self {
        Self { ctx }
    }

    /// 提取数组长度等所需的无符号整数
    pub fn eval_usize(&mut self, expr: &Expr) -> Result<u64, ()> {
        match self.eval_inner(expr, 0) {
            Ok(ConstValue::Int(val)) => {
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
            Ok(_) => {
                self.ctx
                    .struct_error(expr.span, "expected an integer constant")
                    .emit();
                Err(())
            }
            Err(_) => Err(()),
        }
    }

    /// 提取普通的有符号整数常量
    pub fn eval_math(&mut self, expr: &Expr) -> Result<i128, ()> {
        match self.eval_inner(expr, 0) {
            Ok(ConstValue::Int(val)) => Ok(val),
            Ok(_) => {
                self.ctx
                    .struct_error(expr.span, "expected an integer constant")
                    .emit();
                Err(())
            }
            Err(_) => Err(()),
        }
    }

    /// 核心递归求值引擎
    pub fn eval_inner(&mut self, expr: &Expr, depth: usize) -> Result<ConstValue, ()> {
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
            ExprKind::Integer(val) => Ok(ConstValue::Int(*val as i128)),
            ExprKind::Float(val) => Ok(ConstValue::Float(*val)),
            ExprKind::Bool(b) => Ok(ConstValue::Bool(*b)),
            ExprKind::Char(c) => Ok(ConstValue::Int(*c as u32 as i128)),
            ExprKind::String(s) => Ok(ConstValue::String(s.clone())),

            // === 2. 算术与逻辑运算 ===
            ExprKind::Binary { lhs, op, rhs } => self.eval_binary(lhs, *op, rhs, depth, expr.span),
            ExprKind::Unary { op, operand } => self.eval_unary(*op, operand, depth, expr.span),

            // === 3. 查表代入全局 Const 变量 ===
            ExprKind::Identifier(name) => self.eval_identifier(*name, depth, expr.span),

            // === 4. 内置常量函数调用 (Intrinsics) ===
            ExprKind::Call { callee, args } => {
                self.eval_intrinsic_call(callee, args, depth, expr.span)
            }

            // === 5. 枚举字面量求值 ===
            ExprKind::EnumLiteral(variant_name) => {
                self.eval_enum_literal(expr.id, *variant_name, depth, expr.span)
            }

            // === 6. 数据初始化 (支持嵌套 Array 和 Struct) ===
            ExprKind::DataInit { literal, .. } => match literal {
                ast::DataLiteralKind::Scalar(inner) => self.eval_inner(inner, depth + 1),
                ast::DataLiteralKind::Array(elems) => {
                    let mut arr = Vec::new();
                    for e in elems {
                        arr.push(self.eval_inner(e, depth + 1)?);
                    }
                    Ok(ConstValue::Array(arr))
                }
                ast::DataLiteralKind::Struct(fields) => {
                    let mut map = HashMap::new();
                    for f in fields {
                        map.insert(f.name, self.eval_inner(&f.value, depth + 1)?);
                    }
                    Ok(ConstValue::Struct(map))
                }
                ast::DataLiteralKind::Repeat { value, count } => {
                    let val = self.eval_inner(value, depth + 1)?;
                    let cnt = self.eval_usize(count)?;
                    Ok(ConstValue::Array(vec![val; cnt as usize]))
                }
            },

            // === 7. 常量聚合访问 (提取结构体字段和数组索引) ===
            ExprKind::FieldAccess { lhs, field } => {
                let base = self.eval_inner(lhs, depth + 1)?;
                if let ConstValue::Struct(map) = base {
                    if let Some(val) = map.get(field) {
                        Ok(val.clone())
                    } else {
                        let field_str = self.ctx.resolve(*field);
                        self.ctx
                            .struct_error(
                                expr.span,
                                format!("field `{}` not found in constant struct", field_str),
                            )
                            .emit();
                        Err(())
                    }
                } else {
                    self.ctx
                        .struct_error(expr.span, "attempted field access on a non-struct constant")
                        .emit();
                    Err(())
                }
            }

            ExprKind::IndexAccess { lhs, index } => {
                let base = self.eval_inner(lhs, depth + 1)?;
                let idx = self.eval_usize(index)?;
                if let ConstValue::Array(arr) = base {
                    if idx < arr.len() as u64 {
                        Ok(arr[idx as usize].clone())
                    } else {
                        self.ctx
                            .struct_error(expr.span, "constant array index out of bounds")
                            .emit();
                        Err(())
                    }
                } else {
                    self.ctx
                        .struct_error(expr.span, "attempted indexing into a non-array constant")
                        .emit();
                    Err(())
                }
            }

            // === 8. 不支持的表达式 ===
            ExprKind::GenericInstantiation { .. } => {
                self.ctx
                    .struct_error(
                        expr.span,
                        "generic instantiation cannot be evaluated directly as a value",
                    )
                    .emit();
                Err(())
            }
            _ => {
                self.ctx
                    .struct_error(expr.span, "expected a valid constant expression")
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
    ) -> Result<ConstValue, ()> {
        let left = self.eval_inner(lhs, depth + 1)?;
        let right = self.eval_inner(rhs, depth + 1)?;

        match (left, right) {
            (ConstValue::Int(l), ConstValue::Int(r)) => {
                use BinaryOperator::*;
                match op {
                    Add => Ok(ConstValue::Int(l.wrapping_add(r))),
                    Subtract => Ok(ConstValue::Int(l.wrapping_sub(r))),
                    Multiply => Ok(ConstValue::Int(l.wrapping_mul(r))),
                    Divide => {
                        if r == 0 {
                            self.ctx
                                .struct_error(span, "division by zero in constant expression")
                                .emit();
                            Err(())
                        } else {
                            Ok(ConstValue::Int(l / r))
                        }
                    }
                    Modulo => {
                        if r == 0 {
                            self.ctx
                                .struct_error(span, "modulo by zero in constant expression")
                                .emit();
                            Err(())
                        } else {
                            Ok(ConstValue::Int(l % r))
                        }
                    }
                    ShiftLeft => Ok(ConstValue::Int(l << r)),
                    ShiftRight => Ok(ConstValue::Int(l >> r)),
                    BitwiseAnd => Ok(ConstValue::Int(l & r)),
                    BitwiseOr => Ok(ConstValue::Int(l | r)),
                    BitwiseXor => Ok(ConstValue::Int(l ^ r)),
                    Equal => Ok(ConstValue::Bool(l == r)),
                    NotEqual => Ok(ConstValue::Bool(l != r)),
                    LessThan => Ok(ConstValue::Bool(l < r)),
                    LessOrEqual => Ok(ConstValue::Bool(l <= r)),
                    GreaterThan => Ok(ConstValue::Bool(l > r)),
                    GreaterOrEqual => Ok(ConstValue::Bool(l >= r)),
                    _ => {
                        self.ctx
                            .struct_error(span, "unsupported operator for constant integers")
                            .emit();
                        Err(())
                    }
                }
            }
            (ConstValue::Float(l), ConstValue::Float(r)) => {
                use BinaryOperator::*;
                match op {
                    Add => Ok(ConstValue::Float(l + r)),
                    Subtract => Ok(ConstValue::Float(l - r)),
                    Multiply => Ok(ConstValue::Float(l * r)),
                    Divide => Ok(ConstValue::Float(l / r)),
                    Equal => Ok(ConstValue::Bool(l == r)),
                    NotEqual => Ok(ConstValue::Bool(l != r)),
                    LessThan => Ok(ConstValue::Bool(l < r)),
                    LessOrEqual => Ok(ConstValue::Bool(l <= r)),
                    GreaterThan => Ok(ConstValue::Bool(l > r)),
                    GreaterOrEqual => Ok(ConstValue::Bool(l >= r)),
                    _ => {
                        self.ctx
                            .struct_error(span, "unsupported operator for constant floats")
                            .emit();
                        Err(())
                    }
                }
            }
            (ConstValue::Bool(l), ConstValue::Bool(r)) => {
                use BinaryOperator::*;
                match op {
                    LogicalAnd => Ok(ConstValue::Bool(l && r)),
                    LogicalOr => Ok(ConstValue::Bool(l || r)),
                    Equal => Ok(ConstValue::Bool(l == r)),
                    NotEqual => Ok(ConstValue::Bool(l != r)),
                    _ => {
                        self.ctx
                            .struct_error(span, "unsupported operator for constant booleans")
                            .emit();
                        Err(())
                    }
                }
            }
            _ => {
                self.ctx
                    .struct_error(
                        span,
                        "type mismatch or unsupported types in constant binary expression",
                    )
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
    ) -> Result<ConstValue, ()> {
        let val = self.eval_inner(operand, depth + 1)?;
        match (op, val) {
            (UnaryOperator::Negate, ConstValue::Int(v)) => Ok(ConstValue::Int(v.wrapping_neg())),
            (UnaryOperator::Negate, ConstValue::Float(v)) => Ok(ConstValue::Float(-v)),
            (UnaryOperator::BitwiseNot, ConstValue::Int(v)) => Ok(ConstValue::Int(!v)),
            (UnaryOperator::LogicalNot, ConstValue::Bool(v)) => Ok(ConstValue::Bool(!v)),
            _ => {
                self.ctx
                    .struct_error(span, "invalid unary operator for the given constant type")
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
    ) -> Result<ConstValue, ()> {
        let sym_info = self.ctx.scopes.resolve(name).cloned();

        if let Some(info) = sym_info {
            if info.kind == SymbolKind::Const {
                if let Some(def_id) = info.def_id {
                    let const_expr = if let Def::Global(g) = &self.ctx.defs[def_id.0 as usize] {
                        g.value.clone()
                    } else {
                        return Err(());
                    };

                    return self.eval_inner(&const_expr, depth + 1);
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

    fn eval_intrinsic_call(&mut self, callee: &Expr, args: &[Expr], depth: usize, span: Span) -> Result<ConstValue, ()> {
        let callee_ty = self.ctx.node_types.get(&callee.id).copied().unwrap_or(TypeId::ERROR);
        let norm_callee = self.ctx.type_registry.normalize(callee_ty);

        let (def_id, generic_args) = match self.ctx.type_registry.get(norm_callee).clone() {
            TypeKind::FnDef(id, args) => (id, args),
            _ => {
                self.ctx.struct_error(span, "function calls are not allowed in constant expressions").emit();
                return Err(());
            }
        };

        let (is_intrinsic, fn_name_id, generics_len) = if let Def::Function(f) = &self.ctx.defs[def_id.0 as usize] {
            (f.is_intrinsic, f.name, f.generics.len())
        } else {
            return Err(());
        };

        if is_intrinsic {
            let name_str = self.ctx.resolve(fn_name_id).to_string();

            // 在 const 阶段，强制要求泛型参数被显式填满
            if generic_args.len() != generics_len {
                self.ctx.struct_error(span, format!("intrinsic `{}` requires explicit generic arguments in constant evaluation", name_str))
                    .with_hint(format!("example: `{}[u32](...)`", name_str))
                    .emit();
                return Err(());
            }

            match name_str.as_str() {
                "@sizeOf" => {
                    if let Some(&target_ty) = generic_args.get(0) {
                        let size = self.compute_type_size(target_ty);
                        return Ok(ConstValue::Int(size as i128));
                    }
                }
                "@alignOf" => {
                    if let Some(&target_ty) = generic_args.get(0) {
                        let align = self.compute_type_align(target_ty);
                        return Ok(ConstValue::Int(align as i128));
                    }
                }
                "@popCount" | "@clz" | "@ctz" => {
                    if let Ok(ConstValue::Int(val)) = self.eval_inner(&args[0], depth + 1) {
                        let target_ty = generic_args[0];
                        let bit_width = self.compute_type_size(target_ty) * 8;
                        let mask = if bit_width == 128 { u128::MAX } else { (1 << bit_width) - 1 };
                        let u_val = (val as u128) & mask;

                        let res = match name_str.as_str() {
                            "@popCount" => u_val.count_ones() as i128,
                            "@clz" => (u_val.leading_zeros() as i128) - (128 - bit_width as i128),
                            "@ctz" => if u_val == 0 { bit_width as i128 } else { u_val.trailing_zeros() as i128 },
                            _ => unreachable!()
                        };
                        return Ok(ConstValue::Int(res));
                    }
                }
                "@intCast" => {
                    if let Ok(ConstValue::Int(val)) = self.eval_inner(&args[0], depth + 1) {
                        let target_ty = generic_args[1];
                        let bit_width = self.compute_type_size(target_ty) * 8;
                        let mask = if bit_width == 128 { u128::MAX } else { (1 << bit_width) - 1 };
                        let mut u_val = (val as u128) & mask;
                        
                        let is_signed = matches!(self.ctx.type_registry.get(target_ty), TypeKind::Primitive(PrimitiveType::I8 | PrimitiveType::I16 | PrimitiveType::I32 | PrimitiveType::I64 | PrimitiveType::I128 | PrimitiveType::ISize));
                        if is_signed && bit_width < 128 && (u_val & (1 << (bit_width - 1))) != 0 {
                            u_val |= u128::MAX << bit_width;
                        }
                        return Ok(ConstValue::Int(u_val as i128));
                    }
                }
                "@bswap" => {
                    if let Ok(ConstValue::Int(val)) = self.eval_inner(&args[0], depth + 1) {
                        let target_ty = generic_args[0];
                        let bit_width = self.compute_type_size(target_ty) * 8;
                        let mask = if bit_width == 128 { u128::MAX } else { (1 << bit_width) - 1 };
                        let u_val = (val as u128) & mask;

                        let res = match bit_width {
                            16 => (u_val as u16).swap_bytes() as i128,
                            32 => (u_val as u32).swap_bytes() as i128,
                            64 => (u_val as u64).swap_bytes() as i128,
                            128 => u_val.swap_bytes() as i128,
                            _ => u_val as i128,
                        };
                        return Ok(ConstValue::Int(res));
                    }
                }
                "@memcpy" | "@memset" => {
                    self.ctx.struct_error(span, format!("memory intrinsic `{}` cannot be evaluated at compile time", name_str)).emit();
                    return Err(());
                }
                _ => {
                    self.ctx.struct_error(span, format!("intrinsic `{}` cannot be evaluated at compile time", name_str)).emit();
                    return Err(());
                }
            }
        }

        self.ctx
            .struct_error(span, "function calls are not allowed in constant expressions")
            .with_hint("only compile-time intrinsics like `@sizeOf` or `@clz` are permitted here")
            .emit();
        Err(())
    }

    fn eval_enum_literal(
        &mut self,
        node_id: ast::NodeId,
        variant_name: crate::utils::SymbolId,
        depth: usize,
        span: Span,
    ) -> Result<ConstValue, ()> {
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
                if let Ok(ConstValue::Int(val)) = self.eval_inner(&v_expr, depth + 1) {
                    current_val = val;
                }
            }
            if v.name == variant_name {
                return Ok(ConstValue::Int(current_val));
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
            return 1;
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

            TypeKind::Def(def_id, generic_args) | TypeKind::Adt(def_id, generic_args) => {
                self.compute_def_align(def_id, &generic_args, depth)
            }
            TypeKind::Primitive(PrimitiveType::Never) | TypeKind::Error => 1,
            TypeKind::Primitive(p) => self.primitive_align(p),
            _ => 1,
        }
    }

    pub fn compute_type_size(&mut self, ty: TypeId) -> u64 {
        self.compute_type_size_inner(ty, 0)
    }

    fn compute_type_size_inner(&mut self, ty: TypeId, depth: usize) -> u64 {
        if depth > 100 {
            return 0;
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

            TypeKind::Def(def_id, generic_args) | TypeKind::Adt(def_id, generic_args) => {
                self.compute_def_size(def_id, &generic_args, depth)
            }
            TypeKind::Error | TypeKind::Primitive(PrimitiveType::Never) => 0,
            TypeKind::Primitive(p) => self.primitive_size(p),
            _ => 0,
        }
    }

    fn align_to(offset: u64, align: u64) -> u64 {
        (offset + align - 1) & !(align - 1)
    }

    fn primitive_align(&self, p: PrimitiveType) -> u64 {
        use PrimitiveType::*;
        match p {
            I8 | U8 | Bool => 1,
            I16 | U16 => 2,
            I32 | U32 | F32 => 4,
            I64 | U64 | F64 => 8,
            ISize | USize => self.ctx.target.pointer_size,
            I128 | U128 => 16,
            _ => 1,
        }
    }

    fn primitive_size(&self, p: PrimitiveType) -> u64 {
        use PrimitiveType::*;
        match p {
            I8 | U8 | Bool => 1,
            I16 | U16 => 2,
            I32 | U32 | F32 => 4,
            I64 | U64 | F64 => 8,
            ISize | USize => self.ctx.target.pointer_size,
            I128 | U128 => 16,
            _ => 0,
        }
    }

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
            Def::Adt(a) => {
                let tag_ty = a.backing_type.as_ref().map_or(TypeId::U32, |bt| {
                    self.ctx
                        .node_types
                        .get(&bt.id)
                        .copied()
                        .unwrap_or(TypeId::U32)
                });
                let mut max_align = self.compute_type_align_inner(tag_ty, depth + 1);

                let map = self.prepare_generic_subst(&a.generics, generic_args);
                for v in &a.variants {
                    if let Some(payload) = &v.payload_type {
                        let p_ty = self.resolve_field_type(payload, &map);
                        let align = self.compute_type_align_inner(p_ty, depth + 1);
                        if align > max_align {
                            max_align = align;
                        }
                    }
                }
                max_align
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
                Self::align_to(max_size, max_align)
            }
            Def::Enum(e) => {
                let back_ty = self.resolve_enum_backing_type(&e);
                self.compute_type_size_inner(back_ty, depth + 1)
            }
            Def::Adt(a) => {
                // ADT Size = align_to(TagSize, MaxAlign) + align_to(MaxPayloadSize, MaxAlign)
                // (简化版的 C 布局，实际以 target data_layout 为准)
                let tag_ty = a.backing_type.as_ref().map_or(TypeId::U32, |bt| {
                    self.ctx
                        .node_types
                        .get(&bt.id)
                        .copied()
                        .unwrap_or(TypeId::U32)
                });
                let mut max_align = self.compute_type_align_inner(tag_ty, depth + 1);
                let tag_size = self.compute_type_size_inner(tag_ty, depth + 1);

                let map = self.prepare_generic_subst(&a.generics, generic_args);
                let mut max_payload_size = 0;

                for v in &a.variants {
                    if let Some(payload) = &v.payload_type {
                        let p_ty = self.resolve_field_type(payload, &map);
                        let align = self.compute_type_align_inner(p_ty, depth + 1);
                        let size = self.compute_type_size_inner(p_ty, depth + 1);
                        if align > max_align {
                            max_align = align;
                        }
                        if size > max_payload_size {
                            max_payload_size = size;
                        }
                    }
                }

                let mut offset = tag_size;
                offset = Self::align_to(offset, max_align);
                offset += max_payload_size;
                Self::align_to(offset, max_align)
            }
            _ => 0,
        }
    }

    fn prepare_generic_subst(
        &self,
        generics: &[ast::GenericParam],
        args: &[TypeId],
    ) -> HashMap<SymbolId, TypeId> {
        let mut map = HashMap::new();
        if !generics.is_empty() && !args.is_empty() {
            for (i, param) in generics.iter().enumerate() {
                map.insert(param.name, args[i]);
            }
        }
        map
    }

    fn resolve_field_type(
        &mut self,
        type_node: &ast::TypeNode,
        map: &HashMap<SymbolId, TypeId>,
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
