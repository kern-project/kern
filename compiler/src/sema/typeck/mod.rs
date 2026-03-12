pub mod const_eval;
pub mod expr;
pub mod subst;

use crate::driver::Context;
use crate::parser::ast;
use crate::sema::def::{Def, FunctionDef, GlobalDef, ImplDef};
use crate::sema::scope::{ScopeId, SymbolInfo, SymbolKind};
use crate::sema::ty::{TypeId, TypeKind};
use crate::sema::typeck::const_eval::ConstEvaluator;
use crate::sema::typeck::expr::ExprChecker;

/// 类型检查的主驱动器
pub struct TypeckDriver<'a> {
    pub ctx: &'a mut Context,
}

impl<'a> TypeckDriver<'a> {
    pub fn new(ctx: &'a mut Context) -> Self {
        Self { ctx }
    }

    /// 核心入口：按模块层级遍历，以保证顶级作用域正确
    pub fn check_all(&mut self) {
        let defs_clone = self.ctx.defs.clone();

        for def in defs_clone {
            if let Def::Module(m) = def {
                // 切换到模块的词法作用域
                self.ctx.scopes.set_current_scope(m.scope_id);

                for item_id in m.items {
                    self.check_item(item_id, m.scope_id);
                }
            }
        }
    }

    fn check_item(&mut self, id: crate::sema::ty::DefId, parent_scope: ScopeId) {
        let def = self.ctx.defs[id.0 as usize].clone();

        match def {
            Def::Function(f) => self.check_function(&f, parent_scope),
            Def::Global(g) => self.check_global(&g, parent_scope),
            Def::Impl(i) => self.check_impl(&i, parent_scope),
            _ => {} // 其他结构体/别名等仅作声明，没有可执行的代码体
        }
    }

    // ==========================================
    //          Item Checkers
    // ==========================================

    fn check_function(&mut self, f: &FunctionDef, parent_scope: ScopeId) {
        // 1. 验证 Extern 规则
        if !f.is_extern && f.body.is_none() {
            self.ctx
                .emit_error(f.span, "Non-extern functions must have a body");
            return;
        }

        let body_expr = match &f.body {
            Some(b) => b,
            None => return,
        };

        // 2. 提取解析好的函数签名
        let sig_ty = f.resolved_sig.unwrap_or(TypeId::ERROR);
        let (param_tys, ret_ty) = match self.ctx.type_registry.get(sig_ty).clone() {
            TypeKind::Function { params, ret, .. } => (params, ret),
            _ => (Vec::new(), TypeId::ERROR),
        };

        // === 3. 核心：作用域环境重建 ===
        self.ctx.scopes.set_current_scope(parent_scope);
        let _ = self.ctx.scopes.enter_scope();
        for (i, param_ast) in f.params.iter().enumerate() {
            if i < param_tys.len() {
                let info = SymbolInfo {
                    kind: SymbolKind::Var,
                    node_id: param_ast.type_node.id,
                    type_id: param_tys[i],
                    def_id: None,
                    span: param_ast.span,
                    is_pub: false,
                };
                let _ = self.ctx.scopes.define(param_ast.name, info);
            }
        }

        // 4. 启动表达式检查器
        let mut checker = ExprChecker::new(self.ctx, Some(ret_ty));

        let body_eval_ty = checker.check_expr(body_expr, Some(ret_ty));

        // 5. 校验函数的最终末尾表达式是否匹配签名
        if ret_ty != TypeId::ERROR && body_eval_ty != TypeId::ERROR {
            if ret_ty == body_eval_ty {
                // 类型完美匹配 (例如函数是 void，或者用户用了无分号的尾部表达式)
            } else if body_eval_ty == TypeId::VOID && checker.has_returned {
            } else {
                // 强制检查 Coercion（会打印 Type mismatch）
                if !checker.check_coercion(body_expr.span, ret_ty, body_eval_ty) {
                    self.ctx.emit_error(
                        body_expr.span,
                        "Function body evaluates to a type that does not match its signature. \
                        (Hint: Missing a return statement or a trailing semicolon?)",
                    );
                }
            }
        }

        self.ctx.scopes.exit_scope(); // 退出函数局部作用域
    }

    fn check_impl(&mut self, i: &ImplDef, parent_scope: ScopeId) {
        self.ctx.scopes.set_current_scope(parent_scope);
        let impl_scope = self.ctx.scopes.enter_scope();

        // 为 Impl 块注入 `Self` 类型
        let target_ty = self
            .ctx
            .node_types
            .get(&i.target_type.id)
            .copied()
            .unwrap_or(TypeId::ERROR);
        let self_sym = self.ctx.intern("Self");
        let _ = self.ctx.scopes.define(
            self_sym,
            SymbolInfo {
                kind: SymbolKind::TypeAlias,
                node_id: i.target_type.id,
                type_id: target_ty,
                def_id: None,
                span: i.span,
                is_pub: false,
            },
        );

        // 递归检查所有方法
        for &method_id in &i.methods {
            let method_def = self.ctx.defs[method_id.0 as usize].clone();
            if let Def::Function(f) = method_def {
                self.check_function(&f, impl_scope);
            }
        }

        self.ctx.scopes.exit_scope();
    }

    fn check_global(&mut self, g: &GlobalDef, parent_scope: ScopeId) {
        self.ctx.scopes.set_current_scope(parent_scope);
        let mut checker = ExprChecker::new(self.ctx, None);
        let init_ty = checker.check_expr(&g.value, None);
        if self.ctx.scopes.resolve_local(g.name).is_some() {
            self.ctx.scopes.update_type(g.name, init_ty);
        } else {
            // 如果走到这里，说明 Collector 没扫到这个全局变量，这是编译器本身的 Bug
            self.ctx.emit_ice(
                g.span,
                format!(
                    "global symbol `{}` was not collected during the collection pass",
                    self.ctx.resolve(g.name)
                ),
            );

            // 为了让编译器能继续跑完 Typeck 以发现更多错误，这里可以保留定义
            let info = SymbolInfo {
                kind: if g.is_static {
                    SymbolKind::Static
                } else {
                    SymbolKind::Const
                },
                node_id: g.value.id,
                type_id: init_ty,
                def_id: Some(g.id),
                span: g.span,
                is_pub: g.vis == crate::sema::def::Visibility::Public,
            };
            let _ = self.ctx.scopes.define(g.name, info);
        }

        // === 常量与 extern 规则约束 ===
        if !g.is_extern {
            if let ast::ExprKind::Undef = g.value.kind {
                self.ctx.emit_error(g.span, "Global variables cannot be initialized with `undef`. Must provide a constant value.");
            } else {
                let mut evaluator = ConstEvaluator::new(self.ctx);
                let _ = evaluator.eval_math(&g.value);
            }
        } else {
            // 如果是 extern，确保推导出了合法类型
            if init_ty == TypeId::ERROR {
                self.ctx.emit_error(
                    g.span,
                    "Extern statics must have a concrete type, e.g., `static X = i32.{undef};`",
                );
            } else if !matches!(g.value.kind, ast::ExprKind::DataInit { literal: ast::DataLiteralKind::Scalar(ref inner), .. } if matches!(inner.kind, ast::ExprKind::Undef))
            {
                // 确保 extern 不会被赋真实的值
                self.ctx.emit_error(g.span, "Extern statics must be initialized with `undef`, e.g., `static X = i32.{undef};`");
            }
        }
    }
}
