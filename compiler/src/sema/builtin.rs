use crate::driver::Context;
use crate::parser::ast::{self, GenericParam, TypeNode};
use crate::sema::def::*;
use crate::sema::scope::{SymbolInfo, SymbolKind};
use crate::sema::ty::{DefId, TypeId, TypeKind};

pub struct BuiltinInjector<'a> {
    pub ctx: &'a mut Context,
}

impl<'a> BuiltinInjector<'a> {
    pub fn new(ctx: &'a mut Context) -> Self {
        Self { ctx }
    }

    pub fn inject(&mut self) {
        // 1. 注册内置 Traits: Integer, Float, Add 等
        let int_trait_id = self.inject_builtin_trait("Integer");
        let float_trait_id = self.inject_builtin_trait("Float");
        // let add_trait_id = self.inject_builtin_trait("Add");

        // 2. 为原始类型注入 Impl 块 (e.g., impl i32 : Integer)
        let int_types = [
            TypeId::I8,
            TypeId::I16,
            TypeId::I32,
            TypeId::I64,
            TypeId::I128,
            TypeId::ISIZE,
            TypeId::U8,
            TypeId::U16,
            TypeId::U32,
            TypeId::U64,
            TypeId::U128,
            TypeId::USIZE,
        ];
        for &ty in &int_types {
            self.inject_primitive_impl(ty, int_trait_id);
        }

        let float_types = [TypeId::F32, TypeId::F64];
        for &ty in &float_types {
            self.inject_primitive_impl(ty, float_trait_id);
        }

        // 3. 注册内置函数 (Intrinsics)
        self.inject_size_of();
        self.inject_align_of();
        self.inject_int_to_float(int_trait_id, float_trait_id);
        self.inject_float_cast(float_trait_id);
        self.inject_int_cast(int_trait_id);
        self.inject_float_to_int(float_trait_id, int_trait_id);
        self.inject_unreachable();
    }

    // ==========================================
    //          注入逻辑细节
    // ==========================================

    fn inject_builtin_trait(&mut self, name: &str) -> DefId {
        let name_id = self.ctx.intern(name);
        let def_id = DefId(self.ctx.defs.len() as u32);

        let trait_def = TraitDef {
            id: def_id,
            name: name_id,
            vis: Visibility::Public,
            generics: vec![],
            supertraits: vec![],
            methods: vec![], // 内置特征仅作约束，可以没有方法 (Marker Trait)
            resolved_methods: vec![],
            is_builtin: true,
            span: crate::utils::Span::default(),
        };

        self.ctx.add_def(Def::Trait(trait_def));

        let self_ty = self.ctx.type_registry.intern(TypeKind::Def(def_id, vec![]));
        let info = SymbolInfo {
            kind: SymbolKind::Trait,
            node_id: self.ctx.next_node_id(),
            type_id: self_ty,
            def_id: Some(def_id),
            span: Default::default(),
            is_pub: true,
        };
        let root_scope = crate::sema::scope::ScopeId(0);
        self.ctx.scopes.set_current_scope(root_scope);
        let _ = self.ctx.scopes.define(name_id, info);

        def_id
    }

    fn inject_primitive_impl(&mut self, target_ty_id: TypeId, trait_def_id: DefId) {
        let def_id = DefId(self.ctx.defs.len() as u32);

        // 伪造 AST 节点以适应现有的统一逻辑
        let target_id = self.ctx.next_node_id();
        let trait_id = self.ctx.next_node_id();

        let target_node = TypeNode {
            id: target_id,
            span: Default::default(),
            kind: ast::TypeKind::Infer,
        };
        let trait_node = TypeNode {
            id: trait_id,
            span: Default::default(),
            kind: ast::TypeKind::Infer,
        };

        // 直接在 node_types 缓存中写入它们真实的语义类型
        self.ctx.node_types.insert(target_node.id, target_ty_id);

        let trait_ty = self
            .ctx
            .type_registry
            .intern(TypeKind::Def(trait_def_id, vec![]));
        self.ctx.node_types.insert(trait_node.id, trait_ty);

        let impl_def = ImplDef {
            id: def_id,
            parent_module: None,
            generics: vec![],
            target_type: target_node,
            trait_type: Some(trait_node),
            methods: vec![],
            span: Default::default(),
        };
        self.ctx.add_def(Def::Impl(impl_def));
    }

    // 注入 @sizeOf[T]() -> usize
    fn inject_size_of(&mut self) {
        let name_id = self.ctx.intern("@sizeof");
        let def_id = DefId(self.ctx.defs.len() as u32);

        // 泛型参数 T (没有任何约束)
        let param_t = GenericParam {
            name: self.ctx.intern("T"),
            constraints: vec![],
            span: Default::default(),
        };

        // 构造类型签名: fn[T]() -> usize
        let ret_type_id = self.ctx.next_node_id();
        let sig_ty = {
            let _ = self.ctx.type_registry.intern(TypeKind::Param(param_t.name));
            self.ctx.node_types.insert(ret_type_id, TypeId::USIZE);
            self.ctx.type_registry.intern(TypeKind::Function {
                params: vec![],
                ret: TypeId::USIZE,
                is_variadic: false,
            })
        };

        let func_def = FunctionDef {
            id: def_id,
            name: name_id,
            vis: Visibility::Public,
            parent: None,
            generics: vec![param_t],
            params: vec![],
            ret_type: TypeNode {
                id: ret_type_id,
                span: Default::default(),
                kind: ast::TypeKind::Infer,
            },
            body: None,
            is_extern: false,
            is_variadic: false,
            is_intrinsic: true,
            resolved_sig: Some(sig_ty),
            span: Default::default(),
            attributes: vec![],
        };

        self.ctx.add_def(Def::Function(func_def));

        let root_scope = crate::sema::scope::ScopeId(0);
        self.ctx.scopes.set_current_scope(root_scope);
        let info = SymbolInfo {
            kind: SymbolKind::Function,
            node_id: self.ctx.next_node_id(),
            type_id: self
                .ctx
                .type_registry
                .intern(TypeKind::FnDef(def_id, vec![])),
            def_id: Some(def_id),
            span: crate::utils::Span::default(),
            is_pub: true, // 全局内置函数都是 Public
        };
        let _ = self.ctx.scopes.define(name_id, info);
    }

    // 注入 @alignOf[T]() -> usize
    fn inject_align_of(&mut self) {
        let name_id = self.ctx.intern("@alignOf");
        let def_id = DefId(self.ctx.defs.len() as u32);

        let param_t = GenericParam {
            name: self.ctx.intern("T"),
            constraints: vec![],
            span: Default::default(),
        };

        let ret_type_id = self.ctx.next_node_id();
        let sig_ty = {
            let _ = self.ctx.type_registry.intern(TypeKind::Param(param_t.name));
            self.ctx.node_types.insert(ret_type_id, TypeId::USIZE);
            self.ctx.type_registry.intern(TypeKind::Function {
                params: vec![],
                ret: TypeId::USIZE,
                is_variadic: false,
            })
        };

        let func_def = FunctionDef {
            id: def_id,
            name: name_id,
            vis: Visibility::Public,
            parent: None,
            generics: vec![param_t],
            params: vec![],
            ret_type: TypeNode {
                id: ret_type_id,
                span: Default::default(),
                kind: ast::TypeKind::Infer,
            },
            body: None,
            is_extern: false,
            is_variadic: false,
            is_intrinsic: true,
            resolved_sig: Some(sig_ty),
            span: Default::default(),
            attributes: vec![],
        };

        self.ctx.add_def(Def::Function(func_def));
        let root_scope = crate::sema::scope::ScopeId(0);
        self.ctx.scopes.set_current_scope(root_scope);
        let info = SymbolInfo {
            kind: SymbolKind::Function,
            node_id: self.ctx.next_node_id(),
            type_id: self
                .ctx
                .type_registry
                .intern(TypeKind::FnDef(def_id, vec![])),
            def_id: Some(def_id),
            span: Default::default(),
            is_pub: true,
        };
        let _ = self.ctx.scopes.define(name_id, info);
    }

    // 注入 @intToFloat[T: Integer, U: Float](val: T) -> U
    fn inject_int_to_float(&mut self, int_trait_id: DefId, float_trait_id: DefId) {
        let name_id = self.ctx.intern("@intToFloat");
        let def_id = DefId(self.ctx.defs.len() as u32);

        // 伪造约束节点并提前填入类型缓存
        let c_int_id = self.ctx.next_node_id();
        let c_float_id = self.ctx.next_node_id();
        let c_int = TypeNode {
            id: c_int_id,
            span: Default::default(),
            kind: ast::TypeKind::Infer,
        };
        let c_float = TypeNode {
            id: c_float_id,
            span: Default::default(),
            kind: ast::TypeKind::Infer,
        };
        self.ctx.node_types.insert(
            c_int.id,
            self.ctx
                .type_registry
                .intern(TypeKind::Def(int_trait_id, vec![])),
        );
        self.ctx.node_types.insert(
            c_float.id,
            self.ctx
                .type_registry
                .intern(TypeKind::Def(float_trait_id, vec![])),
        );

        let param_t = GenericParam {
            name: self.ctx.intern("T"),
            constraints: vec![c_int],
            span: Default::default(),
        };
        let param_u = GenericParam {
            name: self.ctx.intern("U"),
            constraints: vec![c_float],
            span: Default::default(),
        };

        let val_param_id = self.ctx.next_node_id();
        let ret_id = self.ctx.next_node_id();
        let sig_ty = {
            let t_ty = self.ctx.type_registry.intern(TypeKind::Param(param_t.name));
            let u_ty = self.ctx.type_registry.intern(TypeKind::Param(param_u.name));
            self.ctx.node_types.insert(val_param_id, t_ty);
            self.ctx.node_types.insert(ret_id, u_ty);
            self.ctx.type_registry.intern(TypeKind::Function {
                params: vec![t_ty],
                ret: u_ty,
                is_variadic: false,
            })
        };

        let func_def = FunctionDef {
            id: def_id,
            name: name_id,
            vis: Visibility::Public,
            parent: None,
            generics: vec![param_t, param_u],
            // 伪造一个 FuncParam 供后续解构，虽然内置函数体为空
            params: vec![ast::FuncParam {
                name: self.ctx.intern("val"),
                type_node: TypeNode {
                    id: val_param_id,
                    span: Default::default(),
                    kind: ast::TypeKind::Infer,
                },
                span: Default::default(),
            }],
            ret_type: TypeNode {
                id: ret_id,
                span: Default::default(),
                kind: ast::TypeKind::Infer,
            },
            body: None,
            is_extern: false,
            is_variadic: false,
            is_intrinsic: true,
            resolved_sig: Some(sig_ty),
            span: Default::default(),
            attributes: vec![],
        };

        self.ctx.add_def(Def::Function(func_def));
        self.ctx
            .scopes
            .set_current_scope(crate::sema::scope::ScopeId(0));
        let info = SymbolInfo {
            kind: SymbolKind::Function,
            node_id: self.ctx.next_node_id(),
            type_id: self
                .ctx
                .type_registry
                .intern(TypeKind::FnDef(def_id, vec![])),
            def_id: Some(def_id),
            span: crate::utils::Span::default(),
            is_pub: true, // 全局内置函数都是 Public
        };
        let _ = self.ctx.scopes.define(name_id, info);
    }

    // 注入 @floatCast[T: Float, U: Float](val: T) -> U
    fn inject_float_cast(&mut self, float_trait_id: DefId) {
        let name_id = self.ctx.intern("@floatCast");
        let def_id = DefId(self.ctx.defs.len() as u32);

        // 两个参数都需要满足 Float Trait 约束
        let id1 = self.ctx.next_node_id();
        let id2 = self.ctx.next_node_id();

        let c_float1 = TypeNode {
            id: id1,
            span: Default::default(),
            kind: ast::TypeKind::Infer,
        };
        let c_float2 = TypeNode {
            id: id2,
            span: Default::default(),
            kind: ast::TypeKind::Infer,
        };
        self.ctx.node_types.insert(
            c_float1.id,
            self.ctx
                .type_registry
                .intern(TypeKind::Def(float_trait_id, vec![])),
        );
        self.ctx.node_types.insert(
            c_float2.id,
            self.ctx
                .type_registry
                .intern(TypeKind::Def(float_trait_id, vec![])),
        );

        let param_t = GenericParam {
            name: self.ctx.intern("T"),
            constraints: vec![c_float1],
            span: Default::default(),
        };
        let param_u = GenericParam {
            name: self.ctx.intern("U"),
            constraints: vec![c_float2],
            span: Default::default(),
        };
        let val_param_id = self.ctx.next_node_id();
        let ret_id = self.ctx.next_node_id();
        let sig_ty = {
            let t_ty = self.ctx.type_registry.intern(TypeKind::Param(param_t.name));
            let u_ty = self.ctx.type_registry.intern(TypeKind::Param(param_u.name));
            self.ctx.node_types.insert(val_param_id, t_ty);
            self.ctx.node_types.insert(ret_id, u_ty);
            self.ctx.type_registry.intern(TypeKind::Function {
                params: vec![t_ty],
                ret: u_ty,
                is_variadic: false,
            })
        };

        let func_def = FunctionDef {
            id: def_id,
            name: name_id,
            vis: Visibility::Public,
            parent: None,
            generics: vec![param_t, param_u],
            params: vec![ast::FuncParam {
                name: self.ctx.intern("val"),
                type_node: TypeNode {
                    id: val_param_id,
                    span: Default::default(),
                    kind: ast::TypeKind::Infer,
                },
                span: Default::default(),
            }],
            ret_type: TypeNode {
                id: ret_id,
                span: Default::default(),
                kind: ast::TypeKind::Infer,
            },
            body: None,
            is_extern: false,
            is_variadic: false,
            is_intrinsic: true, // 关键标记，后端会特殊处理
            resolved_sig: Some(sig_ty),
            span: Default::default(),
            attributes: vec![],
        };

        self.ctx.add_def(Def::Function(func_def));
        let root_scope = crate::sema::scope::ScopeId(0);
        self.ctx.scopes.set_current_scope(root_scope);
        let info = SymbolInfo {
            kind: SymbolKind::Function,
            node_id: self.ctx.next_node_id(),
            type_id: self
                .ctx
                .type_registry
                .intern(TypeKind::FnDef(def_id, vec![])),
            def_id: Some(def_id),
            span: crate::utils::Span::default(),
            is_pub: true,
        };
        let _ = self.ctx.scopes.define(name_id, info);
    }

    // 注入 @intCast[T: Integer, U: Integer](val: T) -> U
    fn inject_int_cast(&mut self, int_trait_id: DefId) {
        let name_id = self.ctx.intern("@intCast");
        let def_id = DefId(self.ctx.defs.len() as u32);

        // 两个参数都需要满足 Integer Trait 约束
        let id1 = self.ctx.next_node_id();
        let id2 = self.ctx.next_node_id();

        let c_int1 = TypeNode {
            id: id1,
            span: Default::default(),
            kind: ast::TypeKind::Infer,
        };
        let c_int2 = TypeNode {
            id: id2,
            span: Default::default(),
            kind: ast::TypeKind::Infer,
        };

        let trait_ty = self
            .ctx
            .type_registry
            .intern(TypeKind::Def(int_trait_id, vec![]));
        self.ctx.node_types.insert(c_int1.id, trait_ty);
        self.ctx.node_types.insert(c_int2.id, trait_ty);

        let param_t = GenericParam {
            name: self.ctx.intern("T"),
            constraints: vec![c_int1],
            span: Default::default(),
        };
        let param_u = GenericParam {
            name: self.ctx.intern("U"),
            constraints: vec![c_int2],
            span: Default::default(),
        };

        let val_param_id = self.ctx.next_node_id();
        let ret_id = self.ctx.next_node_id();
        let sig_ty = {
            let t_ty = self.ctx.type_registry.intern(TypeKind::Param(param_t.name));
            let u_ty = self.ctx.type_registry.intern(TypeKind::Param(param_u.name));
            self.ctx.node_types.insert(val_param_id, t_ty);
            self.ctx.node_types.insert(ret_id, u_ty);
            self.ctx.type_registry.intern(TypeKind::Function {
                params: vec![t_ty],
                ret: u_ty,
                is_variadic: false,
            })
        };

        let func_def = FunctionDef {
            id: def_id,
            name: name_id,
            vis: Visibility::Public,
            parent: None,
            generics: vec![param_t, param_u],
            params: vec![ast::FuncParam {
                name: self.ctx.intern("val"),
                type_node: TypeNode {
                    id: val_param_id,
                    span: Default::default(),
                    kind: ast::TypeKind::Infer,
                },
                span: Default::default(),
            }],
            ret_type: TypeNode {
                id: ret_id,
                span: Default::default(),
                kind: ast::TypeKind::Infer,
            },
            body: None,
            is_extern: false,
            is_variadic: false,
            is_intrinsic: true,
            resolved_sig: Some(sig_ty),
            span: Default::default(),
            attributes: vec![],
        };

        self.ctx.add_def(Def::Function(func_def));
        let root_scope = crate::sema::scope::ScopeId(0);
        self.ctx.scopes.set_current_scope(root_scope);
        let info = SymbolInfo {
            kind: SymbolKind::Function,
            node_id: self.ctx.next_node_id(),
            type_id: self
                .ctx
                .type_registry
                .intern(TypeKind::FnDef(def_id, vec![])),
            def_id: Some(def_id),
            span: Default::default(),
            is_pub: true,
        };
        let _ = self.ctx.scopes.define(name_id, info);
    }

    // 注入 @floatToInt[T: Float, U: Integer](val: T) -> U
    fn inject_float_to_int(&mut self, float_trait_id: DefId, int_trait_id: DefId) {
        let name_id = self.ctx.intern("@floatToInt");
        let def_id = DefId(self.ctx.defs.len() as u32);

        let id1 = self.ctx.next_node_id();
        let id2 = self.ctx.next_node_id();

        let c_float = TypeNode {
            id: id1,
            span: Default::default(),
            kind: ast::TypeKind::Infer,
        };
        let c_int = TypeNode {
            id: id2,
            span: Default::default(),
            kind: ast::TypeKind::Infer,
        };

        self.ctx.node_types.insert(
            c_float.id,
            self.ctx
                .type_registry
                .intern(TypeKind::Def(float_trait_id, vec![])),
        );
        self.ctx.node_types.insert(
            c_int.id,
            self.ctx
                .type_registry
                .intern(TypeKind::Def(int_trait_id, vec![])),
        );

        let param_t = GenericParam {
            name: self.ctx.intern("T"),
            constraints: vec![c_float],
            span: Default::default(),
        };
        let param_u = GenericParam {
            name: self.ctx.intern("U"),
            constraints: vec![c_int],
            span: Default::default(),
        };

        let val_param_id = self.ctx.next_node_id();
        let ret_id = self.ctx.next_node_id();
        let sig_ty = {
            let t_ty = self.ctx.type_registry.intern(TypeKind::Param(param_t.name));
            let u_ty = self.ctx.type_registry.intern(TypeKind::Param(param_u.name));
            self.ctx.node_types.insert(val_param_id, t_ty);
            self.ctx.node_types.insert(ret_id, u_ty);
            self.ctx.type_registry.intern(TypeKind::Function {
                params: vec![t_ty],
                ret: u_ty,
                is_variadic: false,
            })
        };

        let func_def = FunctionDef {
            id: def_id,
            name: name_id,
            vis: Visibility::Public,
            parent: None,
            generics: vec![param_t, param_u],
            params: vec![ast::FuncParam {
                name: self.ctx.intern("val"),
                type_node: TypeNode {
                    id: val_param_id,
                    span: Default::default(),
                    kind: ast::TypeKind::Infer,
                },
                span: Default::default(),
            }],
            ret_type: TypeNode {
                id: ret_id,
                span: Default::default(),
                kind: ast::TypeKind::Infer,
            },
            body: None,
            is_extern: false,
            is_variadic: false,
            is_intrinsic: true,
            resolved_sig: Some(sig_ty),
            span: Default::default(),
            attributes: vec![],
        };

        self.ctx.add_def(Def::Function(func_def));
        let root_scope = crate::sema::scope::ScopeId(0);
        self.ctx.scopes.set_current_scope(root_scope);
        let info = SymbolInfo {
            kind: SymbolKind::Function,
            node_id: self.ctx.next_node_id(),
            type_id: self
                .ctx
                .type_registry
                .intern(TypeKind::FnDef(def_id, vec![])),
            def_id: Some(def_id),
            span: Default::default(),
            is_pub: true,
        };
        let _ = self.ctx.scopes.define(name_id, info);
    }

    // 注入 @unreachable() -> !
    fn inject_unreachable(&mut self) {
        let name_id = self.ctx.intern("@unreachable");
        let def_id = DefId(self.ctx.defs.len() as u32);

        let ret_id = self.ctx.next_node_id();
        let sig_ty = {
            self.ctx.node_types.insert(ret_id, TypeId::NEVER);
            self.ctx.type_registry.intern(TypeKind::Function {
                params: vec![],
                ret: TypeId::NEVER,
                is_variadic: false,
            })
        };

        let func_def = FunctionDef {
            id: def_id,
            name: name_id,
            vis: Visibility::Public,
            parent: None,
            generics: vec![],
            params: vec![],
            ret_type: TypeNode {
                id: ret_id,
                span: Default::default(),
                kind: ast::TypeKind::Never, // 直接映射到 Never 类型
            },
            body: None,
            is_extern: false,
            is_variadic: false,
            is_intrinsic: true,
            resolved_sig: Some(sig_ty),
            span: Default::default(),
            attributes: vec![],
        };

        self.ctx.add_def(Def::Function(func_def));
        let root_scope = crate::sema::scope::ScopeId(0);
        self.ctx.scopes.set_current_scope(root_scope);
        let info = SymbolInfo {
            kind: SymbolKind::Function,
            node_id: self.ctx.next_node_id(),
            type_id: self
                .ctx
                .type_registry
                .intern(TypeKind::FnDef(def_id, vec![])),
            def_id: Some(def_id),
            span: Default::default(),
            is_pub: true,
        };
        let _ = self.ctx.scopes.define(name_id, info);
    }
}
