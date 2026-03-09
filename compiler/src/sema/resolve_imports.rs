#![allow(unused)]

use crate::driver::context::Context;
use crate::parser::ast::{UsePathKind, UseTarget};
use crate::sema::def::{Def, ImportDef, Visibility};
use crate::sema::scope::{ScopeId, SymbolInfo, SymbolKind};
use crate::sema::ty::DefId;
use crate::utils::SymbolId;

pub struct ImportResolver<'a> {
    pub ctx: &'a mut Context,
}

impl<'a> ImportResolver<'a> {
    pub fn new(ctx: &'a mut Context) -> Self {
        Self { ctx }
    }

    /// 执行完整的导入解析 Pass (支持不动点迭代)
    pub fn resolve_all(&mut self) {
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

        let mut pending_imports: Vec<(DefId, ImportDef)> = Vec::new();
        for mod_id in module_ids {
            if let Def::Module(m) = &self.ctx.defs[mod_id.0 as usize] {
                for imp in &m.imports {
                    pending_imports.push((mod_id, imp.clone()));
                }
            }
        }

        // 不动点迭代算法
        let mut progress = true;
        while progress && !pending_imports.is_empty() {
            progress = false;
            let mut unresolved = Vec::new();

            for (mod_id, import) in pending_imports {
                // 在迭代期间保持静默 (emit_errors = false)
                if self.resolve_single_import(mod_id, &import, false) {
                    progress = true; // 取得了进展！
                } else {
                    unresolved.push((mod_id, import)); // 没找到，等下一轮
                }
            }
            pending_imports = unresolved;
        }

        // 🌟 最终审判：如果还有没解析完的，开启报错开关进行最后一次解析！
        // 这样就完美去掉了 `report_unresolved_import` 的需求。
        for (mod_id, failed_import) in pending_imports {
            self.resolve_single_import(mod_id, &failed_import, true);
        }
    }

    fn resolve_single_import(
        &mut self,
        current_mod_id: DefId,
        import: &ImportDef,
        emit_errors: bool,
    ) -> bool {
        let current_scope = self.get_module_scope(current_mod_id);

        match &import.target {
            UseTarget::Module(alias) => {
                // 分离父路径和目标名
                // 对于 `use .utils.print_point;`，path 是 [utils, print_point]
                // parent_path 是 [utils]，target_name 是 print_point
                let (parent_path, last_segment) = import.path.split_at(import.path.len() - 1);
                let target_name = last_segment[0];

                let (parent_mod_id, parent_scope) = match self.resolve_path(
                    current_mod_id,
                    import.path_kind,
                    parent_path,
                    import.span,
                    emit_errors,
                ) {
                    Some(res) => res,
                    None => return false,
                };

                if let Some(symbol_info) = self.ctx.scopes.resolve_in(parent_scope, target_name) {
                    if !self.check_visibility(symbol_info.def_id, current_mod_id, parent_mod_id) {
                        if emit_errors {
                            let name_str = self.ctx.resolve(target_name).to_string();
                            self.ctx
                                .struct_error(
                                    import.span,
                                    format!("Symbol `{}` is private", name_str),
                                )
                                .emit();
                        }
                        return false;
                    }

                    let name_to_bind = alias.unwrap_or(target_name);
                    self.define_import(
                        current_scope,
                        name_to_bind,
                        symbol_info.clone(),
                        import.is_reexport,
                        import.span,
                        emit_errors,
                    );
                    true
                } else {
                    if emit_errors {
                        let name_str = self.ctx.resolve(target_name).to_string();
                        self.ctx
                            .struct_error(
                                import.span,
                                format!("Cannot find module or symbol `{}`", name_str),
                            )
                            .emit();
                    }
                    false
                }
            }
            UseTarget::Members(members) => {
                // 对于 `use .math.geometry.{Point};`，path 就是 [math, geometry]，直接整体作为模块解析
                let (target_mod_id, target_scope) = match self.resolve_path(
                    current_mod_id,
                    import.path_kind,
                    &import.path, // 👈 完整解析
                    import.span,
                    emit_errors,
                ) {
                    Some(res) => res,
                    None => return false,
                };

                let mut all_resolved = true;
                for member in members {
                    if let Some(symbol_info) = self.ctx.scopes.resolve_in(target_scope, member.name)
                    {
                        if !self.check_visibility(symbol_info.def_id, current_mod_id, target_mod_id)
                        {
                            if emit_errors {
                                let name_str = self.ctx.resolve(member.name).to_string();
                                self.ctx
                                    .struct_error(
                                        member.span,
                                        format!(
                                            "Symbol `{}` is private and cannot be imported",
                                            name_str
                                        ),
                                    )
                                    .emit();
                            }
                            all_resolved = false;
                            continue;
                        }

                        let name_to_bind = member.alias.unwrap_or(member.name);
                        self.define_import(
                            current_scope,
                            name_to_bind,
                            symbol_info.clone(),
                            import.is_reexport,
                            member.span,
                            emit_errors,
                        );
                    } else {
                        if emit_errors {
                            let name_str = self.ctx.resolve(member.name).to_string();
                            self.ctx
                                .struct_error(
                                    member.span,
                                    format!("Cannot find `{}` in the target module", name_str),
                                )
                                .emit();
                        }
                        all_resolved = false;
                    }
                }
                all_resolved
            }
        }
    }

    // ==========================================
    //               Core Resolution
    // ==========================================

    /// 解析路径，返回目标模块的 (DefId, ScopeId)
    fn resolve_path(
        &mut self,
        current_mod_id: DefId,
        kind: UsePathKind,
        path: &[SymbolId],
        span: crate::utils::Span,
        emit_errors: bool, // 🌟 新增静默开关
    ) -> Option<(DefId, ScopeId)> {
        let (mut curr_mod_id, mut curr_scope) = match kind {
            UsePathKind::Absolute => (DefId(0), ScopeId(0)),
            UsePathKind::Relative => (current_mod_id, self.get_module_scope(current_mod_id)),
            UsePathKind::Super => {
                if let Def::Module(m) = &self.ctx.defs[current_mod_id.0 as usize] {
                    if let Some(parent_id) = m.parent {
                        if self.is_init_module(parent_id) {
                            if emit_errors {
                                self.ctx.struct_error(span, "Child modules are strictly forbidden from importing `init.kn` contents").emit();
                            }
                            return None;
                        }
                        (parent_id, self.get_module_scope(parent_id))
                    } else {
                        if emit_errors {
                            self.ctx.struct_error(span, "Cannot use `..` because the current module is a top-level module").emit();
                        }
                        return None;
                    }
                } else {
                    unreachable!()
                }
            }
        };

        if path.is_empty() {
            return Some((curr_mod_id, curr_scope));
        }

        for &segment in path {
            if let Some(symbol) = self.ctx.scopes.resolve_in(curr_scope, segment) {
                if symbol.kind == SymbolKind::Module {
                    if let Some(target_def_id) = symbol.def_id {
                        curr_mod_id = target_def_id;
                        curr_scope = self.get_module_scope(target_def_id);
                        continue;
                    }
                }

                if emit_errors {
                    let name = self.ctx.resolve(segment).to_string();
                    self.ctx.struct_error(span, format!("`{}` is not a module", name))
                        .with_hint("only modules can be used in the intermediate segments of an import path")
                        .emit();
                }
                return None;
            } else {
                if emit_errors {
                    let name = self.ctx.resolve(segment).to_string();
                    self.ctx
                        .struct_error(
                            span,
                            format!("Unresolved import: cannot find module `{}`", name),
                        )
                        .emit();
                }
                return None;
            }
        }

        Some((curr_mod_id, curr_scope))
    }

    /// 检查符号是否对当前模块可见
    fn check_visibility(
        &self,
        symbol_def_id: Option<DefId>,
        current_mod: DefId,
        target_mod: DefId,
    ) -> bool {
        // 如果是从当前模块自身导入，永远可见
        if current_mod == target_mod {
            return true;
        }

        let def_id = match symbol_def_id {
            Some(id) => id,
            None => return true, // 如果没有 DefId（例如内置虚假符号），假定可见
        };

        let def = &self.ctx.defs[def_id.0 as usize];

        // 提取定义本身的可见性
        let vis = match def {
            Def::Function(d) => d.vis,
            Def::Struct(d) => d.vis,
            Def::Union(d) => d.vis,
            Def::Enum(d) => d.vis,
            Def::Trait(d) => d.vis,
            Def::Global(d) => d.vis,
            Def::TypeAlias(d) => d.vis,
            Def::Adt(d) => d.vis,
            Def::Module(_) => Visibility::Public, // 模块本身的可见性通常由其导出方式决定，默认为 Pub
            Def::Impl(_) => return true,          // Impl 块没有直接的可见性概念
        };

        match vis {
            Visibility::Public => true,
            Visibility::Private => {
                // 私有成员仅对同模块可见。
                false
            }
        }
    }

    // ==========================================
    //               Helpers
    // ==========================================

    fn get_module_scope(&self, mod_id: DefId) -> ScopeId {
        if let Def::Module(m) = &self.ctx.defs[mod_id.0 as usize] {
            m.scope_id
        } else {
            panic!("DefId {:?} is not a module", mod_id)
        }
    }

    /// 判断模块是否名为 `init`，用于检查严格层级约束
    fn is_init_module(&self, mod_id: DefId) -> bool {
        if let Def::Module(m) = &self.ctx.defs[mod_id.0 as usize] {
            let name_str = self.ctx.resolve(m.name);
            name_str == "init"
        } else {
            false
        }
    }

    /// 将解析好的符号注入到当前模块的作用域
    fn define_import(
        &mut self,
        target_scope: ScopeId,
        name: SymbolId,
        mut info: SymbolInfo,
        is_reexport: bool,
        span: crate::utils::Span,
        emit_errors: bool,
    ) {
        info.is_pub = is_reexport;
        info.span = span;

        let prev_scope = self.ctx.scopes.current_scope_id();
        self.ctx.scopes.set_current_scope(target_scope);

        if let Err(old_info) = self.ctx.scopes.define(name, info.clone()) {
            if emit_errors && old_info.span != span {
                // 容忍特判：如果是导入同一个东西（比如显式的 use 和隐式的子模块同名），直接放行
                if old_info.def_id == info.def_id && old_info.kind == info.kind {
                    // 一切正常，它本来就在这里
                } else {
                    let name_str = self.ctx.resolve(name).to_string();
                    self.ctx
                        .struct_error(
                            span,
                            format!("the name `{}` is defined multiple times", name_str),
                        )
                        .with_hint(format!(
                            "`{}` was already imported or defined in this module",
                            name_str
                        ))
                        .with_span_label(old_info.span, "previous definition was here")
                        .emit();
                }
            }
        }

        if let Some(prev) = prev_scope {
            self.ctx.scopes.set_current_scope(prev);
        }
    }
}
