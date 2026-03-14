use crate::driver::Context;
use crate::parser::ast::{self, Decl, DeclKind, TypeKind};
use crate::sema::def::*;
use crate::sema::scope::{SymbolInfo, SymbolKind};
use crate::sema::ty::{DefId, TypeId};
use crate::utils::SymbolId;

pub struct Collector<'a> {
    pub ctx: &'a mut Context,
    pub current_module: Option<DefId>,
}

impl<'a> Collector<'a> {
    pub fn new(ctx: &'a mut Context) -> Self {
        Self {
            ctx,
            current_module: None,
        }
    }

    /// 收集特定模块 AST 的内部成员
    pub fn collect_ast(&mut self, mod_id: DefId, module: &ast::Module) {
        let (scope_id, submodules) = if let Def::Module(m) = &self.ctx.defs[mod_id.0 as usize] {
            (m.scope_id, m.submodules.clone())
        } else {
            unreachable!()
        };

        let parent_module = self.current_module;
        self.current_module = Some(mod_id);

        let prev_scope = self.ctx.scopes.current_scope_id();
        self.ctx.scopes.set_current_scope(scope_id);

        let mut item_ids = Vec::new();
        let mut imports = Vec::new();

        // 将模块注册与常规收集融为一体
        for decl in &module.decls {
            match &decl.kind {
                DeclKind::Use { kind, path, target, is_reexport } => {
                    imports.push(ImportDef {
                        path_kind: *kind,
                        path: path.clone(),
                        target: target.clone(),
                        is_reexport: *is_reexport,
                        span: decl.span,
                    });
                }
                DeclKind::ModDecl { is_pub } => {
                    if let Some(&sub_id) = submodules.get(&decl.name) {
                        self.define_symbol(
                            decl.name,
                            SymbolKind::Module,
                            decl.id,
                            Some(sub_id),
                            decl.span,
                            *is_pub, // 精确提取可见性
                        );
                    }
                }
                DeclKind::ExternBlock { decls, .. } => {
                    for ext_decl in decls {
                        if let Some(def_id) = self.collect_decl(ext_decl, None, true, &[]) {
                            item_ids.push(def_id);
                        }
                    }
                }
                _ => {
                    if let Some(def_id) = self.collect_decl(decl, None, false, &[]) {
                        item_ids.push(def_id);
                    }
                }
            }
        }

        if let Def::Module(m) = &mut self.ctx.defs[mod_id.0 as usize] {
            m.items = item_ids;
            m.imports = imports;
        }

        if let Some(prev) = prev_scope {
            self.ctx.scopes.set_current_scope(prev);
        }
        self.current_module = parent_module;
    }

    /// 收集单个声明
    /// `parent_impl`: 如果当前声明位于 impl 块内，传入 impl 的 DefId
    /// `force_extern`: 如果当前声明位于 extern 块内，强制标记为 extern
    fn collect_decl(
        &mut self,
        decl: &Decl,
        parent_impl: Option<DefId>,
        force_extern: bool,
        impl_generics: &[ast::GenericParam],
    ) -> Option<DefId> {
        let vis = decl.is_pub.into();

        match &decl.kind {
            DeclKind::Function {
                generics,
                params,
                ret_type,
                body,
                is_extern,
                is_variadic,
            } => {
                // 合并 impl 块的泛型和函数自身的泛型
                let mut combined_generics = impl_generics.to_vec();
                combined_generics.extend_from_slice(generics);

                self.collect_function(
                    decl,
                    vis,
                    parent_impl,
                    force_extern || *is_extern,
                    &combined_generics,
                    params,
                    ret_type,
                    body,
                    *is_variadic,
                )
            }
            DeclKind::Var {
                value,
                is_static,
                is_extern,
            } => self.collect_global(decl, vis, force_extern || *is_extern, value, *is_static),
            DeclKind::TypeAlias {
                generics,
                target,
                is_extern,
                bounds,
            } => self.collect_type_alias_or_struct(
                decl,
                vis,
                bounds,
                force_extern || *is_extern,
                generics,
                target,
            ),
            DeclKind::Impl {
                generics,
                target_type,
                trait_type,
                decls,
            } => self.collect_impl(decl, generics, target_type, trait_type, decls),
            DeclKind::ExternBlock { .. } => {
                // Extern 块是一种特殊的顶层容器，必须在 collect_ast 级别被展开平铺。
                // 如果走到这里，说明出现了非法的嵌套（例如 impl 块内部嵌套了 extern 块）。
                self.ctx.emit_error(
                    decl.span,
                    "`extern` blocks are only allowed at the module top-level",
                );
                None
            }
            // 已在 collect_ast 处理
            DeclKind::Use { .. } => None,
            DeclKind::ModDecl { .. } => None,
        }
    }

    fn collect_function(
        &mut self,
        decl: &Decl,
        vis: Visibility,
        parent_impl: Option<DefId>,
        is_extern: bool,
        generics: &[ast::GenericParam],
        params: &[ast::FuncParam],
        ret_type: &ast::TypeNode,
        body: &Option<Box<ast::Expr>>,
        is_variadic: bool,
    ) -> Option<DefId> {
        let def_id = DefId(self.ctx.defs.len() as u32);

        let mut actual_params = params.to_vec();

        if parent_impl.is_some() {
            let self_sym = self.ctx.intern("self");
            let node_id = self.ctx.next_node_id();

            actual_params.insert(
                0,
                ast::FuncParam {
                    name: self_sym,
                    type_node: ast::TypeNode {
                        id: node_id,
                        span: decl.span,
                        kind: ast::TypeKind::SelfType,
                    },
                    span: decl.span,
                },
            );
        }

        let func_def = FunctionDef {
            id: def_id,
            name: decl.name,
            vis,
            parent: parent_impl.or(self.current_module),
            generics: generics.to_vec(),
            params: actual_params,
            ret_type: ret_type.clone(),
            body: body.clone(),
            is_extern,
            is_variadic,
            is_intrinsic: false,
            span: decl.span,
            resolved_sig: None,
            attributes: decl.attributes.clone(),
        };

        self.ctx.add_def(Def::Function(func_def));

        // 如果不是 impl 块中的方法，则将其注册到当前词法作用域
        if parent_impl.is_none() {
            let is_pub = vis == Visibility::Public;
            self.define_symbol(
                decl.name,
                SymbolKind::Function,
                decl.id,
                Some(def_id),
                decl.span,
                is_pub,
            );
        }

        Some(def_id)
    }

    fn collect_global(
        &mut self,
        decl: &Decl,
        vis: Visibility,
        is_extern: bool,
        value: &ast::Expr,
        is_static: bool,
    ) -> Option<DefId> {
        let def_id = DefId(self.ctx.defs.len() as u32);

        let global_def = GlobalDef {
            id: def_id,
            name: decl.name,
            vis,
            value: value.clone(),
            is_static,
            is_extern,
            span: decl.span,
            attributes: decl.attributes.clone(),
        };

        self.ctx.add_def(Def::Global(global_def));

        let sym_kind = if is_static {
            SymbolKind::Static
        } else {
            SymbolKind::Const
        };
        let is_pub = vis == Visibility::Public;
        self.define_symbol(
            decl.name,
            sym_kind,
            decl.id,
            Some(def_id),
            decl.span,
            is_pub,
        );

        Some(def_id)
    }

    /// 核心逻辑：将 `type Name = Target` 解包为对应的实体定义
    fn collect_type_alias_or_struct(
        &mut self,
        decl: &Decl,
        vis: Visibility,
        bounds: &[ast::TypeNode],
        is_extern: bool,
        generics: &[ast::GenericParam],
        target: &ast::TypeNode,
    ) -> Option<DefId> {
        let def_id = DefId(self.ctx.defs.len() as u32);
        let mut sym_kind = SymbolKind::TypeAlias;

        let def = match &target.kind {
            TypeKind::Struct { fields } => {
                sym_kind = SymbolKind::Struct;
                Def::Struct(StructDef {
                    id: def_id,
                    name: decl.name,
                    vis,
                    generics: generics.to_vec(),
                    fields: fields.clone(),
                    is_extern,
                    span: decl.span,
                    attributes: decl.attributes.clone(),
                })
            }
            TypeKind::Union { fields } => {
                sym_kind = SymbolKind::Union;
                Def::Union(UnionDef {
                    id: def_id,
                    name: decl.name,
                    vis,
                    generics: generics.to_vec(),
                    fields: fields.clone(),
                    span: decl.span,
                })
            }
            TypeKind::Enum {
                backing_type,
                variants,
            } => {
                sym_kind = SymbolKind::Enum;
                Def::Enum(EnumDef {
                    id: def_id,
                    name: decl.name,
                    vis,
                    generics: generics.to_vec(),
                    backing_type: backing_type.clone(),
                    variants: variants.clone(),
                    span: decl.span,
                })
            }
            TypeKind::Adt {
                backing_type,
                variants,
            } => {
                sym_kind = SymbolKind::Adt;
                Def::Adt(AdtDef {
                    id: def_id,
                    name: decl.name,
                    vis,
                    generics: generics.to_vec(),
                    backing_type: backing_type.clone(),
                    variants: variants.clone(),
                    span: decl.span,
                })
            }
            TypeKind::Trait { fields } => {
                sym_kind = SymbolKind::Trait;
                Def::Trait(TraitDef {
                    id: def_id,
                    name: decl.name,
                    vis,
                    generics: generics.to_vec(),
                    supertraits: bounds.to_vec(),
                    methods: fields.clone(),
                    resolved_methods: Vec::new(),
                    is_builtin: false,
                    span: decl.span,
                })
            }
            _ => {
                // 真正的类型别名，例如 `type MyInt = i32;`
                Def::TypeAlias(TypeAliasDef {
                    id: def_id,
                    name: decl.name,
                    vis,
                    generics: generics.to_vec(),
                    target: target.clone(),
                    span: decl.span,
                })
            }
        };

        self.ctx.add_def(def);
        let is_pub = vis == Visibility::Public;
        self.define_symbol(
            decl.name,
            sym_kind,
            decl.id,
            Some(def_id),
            decl.span,
            is_pub,
        );

        Some(def_id)
    }

    fn collect_impl(
        &mut self,
        decl: &Decl,
        generics: &[ast::GenericParam],
        target_type: &ast::TypeNode,
        trait_type: &Option<ast::TypeNode>,
        decls: &[Decl],
    ) -> Option<DefId> {
        let impl_id = DefId(self.ctx.defs.len() as u32);
        let mut method_ids = Vec::new();

        // 1. 占位（此时 methods 为空）
        self.ctx.add_def(Def::Impl(ImplDef {
            id: impl_id,
            parent_module: self.current_module,
            generics: generics.to_vec(),
            target_type: target_type.clone(),
            trait_type: trait_type.clone(),
            methods: Vec::new(),
            span: decl.span,
        }));

        self.ctx.scopes.enter_scope();
        self.inject_generic_params(generics);

        for method_decl in decls {
            if matches!(method_decl.kind, DeclKind::Function { .. }) {
                if let Some(m_id) = self.collect_decl(method_decl, Some(impl_id), false, generics) {
                    method_ids.push(m_id);
                }
            } else {
                self.ctx.emit_error(
                    method_decl.span,
                    "Only functions are allowed inside `impl` blocks",
                );
            }
        }

        self.ctx.scopes.exit_scope();

        // 2. 🌟 绝对正确的原地更新：直接修改占位符，不新增任何元素！
        if let Def::Impl(i) = &mut self.ctx.defs[impl_id.0 as usize] {
            i.methods = method_ids;
        }

        Some(impl_id)
    }

    // ==========================================
    //               Helpers
    // ==========================================

    /// 向当前作用域注册符号，处理重定义错误并提供极其友好的诊断信息
    fn define_symbol(
        &mut self,
        name: SymbolId,
        kind: SymbolKind,
        node_id: ast::NodeId,
        def_id: Option<DefId>,
        span: crate::utils::Span,
        is_pub: bool,
    ) {
        // 如果是 `_`，直接忽略，不存入作用域
        if self.ctx.resolve(name) == "_" {
            return;
        }
        let info = SymbolInfo {
            kind,
            node_id,
            type_id: TypeId::ERROR, // Collect 阶段尚未推导类型
            def_id,
            span, // 记录符号的诞生位置
            is_pub,
        };

        // 利用 DiagnosticBuilder 提供多 Span 的关联报错
        if let Err(old_info) = self.ctx.scopes.define(name, info) {
            let name_str = self.ctx.resolve(name).to_string();

            self.ctx
                .struct_error(
                    span,
                    format!("the name `{}` is defined multiple times", name_str),
                )
                .with_hint(format!(
                    "`{}` must be defined only once in the same scope",
                    name_str
                ))
                .with_span_label(
                    old_info.span,
                    format!("previous definition of `{}` was here", name_str),
                )
                .emit();
        }
    }

    fn inject_generic_params(&mut self, generics: &[ast::GenericParam]) {
        for param in generics {
            let generic_node_id = self.ctx.next_node_id();

            self.define_symbol(
                param.name,
                SymbolKind::TypeParam,
                generic_node_id,
                None,
                param.span,
                false,
            );
        }
    }
}
