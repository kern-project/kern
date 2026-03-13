use crate::driver::Context;
use crate::parser::ast::*;

pub struct Pruner<'a> {
    pub ctx: &'a mut Context,
}

impl<'a> Pruner<'a> {
    pub fn new(ctx: &'a mut Context) -> Self {
        Self { ctx }
    }

    /// 遍历所有模块，进行条件剪枝
    pub fn prune_all(&mut self, asts: &mut Vec<(crate::sema::ty::DefId, Module)>) {
        for (_, module) in asts.iter_mut() {
            self.prune_module(module);
        }
    }

    fn prune_module(&mut self, module: &mut Module) {
        // 处理模块级属性 #![if(...)]
        if !self.eval_attributes(&module.attributes) {
            // 如果模块级别的条件为 false，直接清空整个模块的内容
            module.decls.clear();
            return;
        }

        // 过滤顶级声明
        module.decls.retain_mut(|decl| self.prune_decl(decl));
    }

    /// 如果该声明应该保留，返回 true；如果应该被剪枝，返回 false
    fn prune_decl(&mut self, decl: &mut Decl) -> bool {
        // 1. 评估当前声明的生存条件
        if !self.eval_attributes(&decl.attributes) {
            return false;
        }

        // 2. 如果保留下来了，且它是包含子项的块（Impl 或 Extern），递归剪枝内部项
        match &mut decl.kind {
            DeclKind::Impl { decls, .. } | DeclKind::ExternBlock { decls, .. } => {
                decls.retain_mut(|d| self.prune_decl(d));
            }
            DeclKind::Function { body: Some(b), .. } => {
                self.prune_expr(b);
            }
            _ => {}
        }
        true
    }

    fn prune_expr(&mut self, expr: &mut Expr) {
        match &mut expr.kind {
            // 核心：处理 Block 并过滤 Stmt
            ExprKind::Block { stmts, result } => {
                stmts.retain_mut(|stmt| {
                    if !self.eval_attributes(&stmt.attributes) {
                        false
                    } else {
                        match &mut stmt.kind {
                            StmtKind::ExprStmt(e) | StmtKind::ExprValue(e) => {
                                self.prune_expr(e);
                            }
                        }
                        true
                    }
                });
                if let Some(r) = result {
                    self.prune_expr(r);
                }
            }

            // 递归遍历所有包含子表达式的结构
            ExprKind::If { cond, then_branch, else_branch } => {
                self.prune_expr(cond);
                self.prune_expr(then_branch);
                if let Some(e) = else_branch {
                    self.prune_expr(e);
                }
            }
            ExprKind::Match { target, arms } => {
                self.prune_expr(target);
                for arm in arms {
                    self.prune_expr(&mut arm.body);
                }
            }
            ExprKind::Switch { target, cases, default_case } => {
                self.prune_expr(target);
                for case in cases {
                    self.prune_expr(&mut case.body);
                }
                if let Some(d) = default_case {
                    self.prune_expr(d);
                }
            }
            ExprKind::For { init, cond, post, body } => {
                if let Some(e) = init { self.prune_expr(e); }
                if let Some(e) = cond { self.prune_expr(e); }
                if let Some(e) = post { self.prune_expr(e); }
                self.prune_expr(body);
            }
            ExprKind::Lambda { body, .. } => self.prune_expr(body),
            ExprKind::Let { init, .. } | ExprKind::Static { init, .. } => self.prune_expr(init),
            ExprKind::Binary { lhs, rhs, .. } | ExprKind::Assign { lhs, rhs, .. } => {
                self.prune_expr(lhs);
                self.prune_expr(rhs);
            }
            ExprKind::Unary { operand, .. } => self.prune_expr(operand),
            ExprKind::FieldAccess { lhs, .. } | ExprKind::As { lhs, .. } => self.prune_expr(lhs),
            ExprKind::IndexAccess { lhs, index } => {
                self.prune_expr(lhs);
                self.prune_expr(index);
            }
            ExprKind::Call { callee, args } => {
                self.prune_expr(callee);
                for arg in args { self.prune_expr(arg); }
            }
            ExprKind::DataInit { literal, .. } => match literal {
                DataLiteralKind::Struct(fields) => {
                    for f in fields { self.prune_expr(&mut f.value); }
                }
                DataLiteralKind::Array(elems) => {
                    for e in elems { self.prune_expr(e); }
                }
                DataLiteralKind::Repeat { value, count } => {
                    self.prune_expr(value);
                    self.prune_expr(count);
                }
                DataLiteralKind::Scalar(val) => self.prune_expr(val),
            },
            ExprKind::SliceOp { lhs, start, end, .. } => {
                self.prune_expr(lhs);
                if let Some(s) = start { self.prune_expr(s); }
                if let Some(e) = end { self.prune_expr(e); }
            }
            ExprKind::Defer { expr: e } => self.prune_expr(e),
            ExprKind::Return(Some(e)) => self.prune_expr(e),
            ExprKind::GenericInstantiation { target, .. } => self.prune_expr(target),

            // 叶子节点 (如 Int, Float, Bool, Identifier, Break, Continue, Error 等) 无需递归
            _ => {}
        }
    }

    /// 检查属性列表，如果有 #[if(expr)] 且求值为 false，则返回 false (应被剪枝)
    fn eval_attributes(&mut self, attributes: &[Attribute]) -> bool {
        for attr in attributes {
            if let AttributeKind::If(cond_expr) = &attr.kind {
                // 调用轻量级求值器
                let mut evaluator = ConditionEvaluator::new(self.ctx);
                match evaluator.eval(cond_expr) {
                    Ok(val) => {
                        if !val {
                            return false; // 只要有一个条件为 false，立刻短路剪枝
                        }
                    }
                    Err(_) => {
                        // 求值失败（比如遇到了未知的变量或类型不匹配），已经报过错了
                        // 为了防止产生更多级联错误，我们将其视为 false 剪掉
                        return false;
                    }
                }
            }
        }
        true
    }
}

// ==========================================
//    Lightweight Condition Evaluator
// ==========================================

/// 专门用于 `#[if(...)]` 的轻量级求值器。
/// 只能处理布尔逻辑、字符串比较和环境变量注入。完全不依赖 Sema 的类型系统。
struct ConditionEvaluator<'a> {
    ctx: &'a mut Context,
}

impl<'a> ConditionEvaluator<'a> {
    fn new(ctx: &'a mut Context) -> Self {
        Self { ctx }
    }

    fn eval(&mut self, expr: &Expr) -> Result<bool, ()> {
        let val = self.eval_inner(expr)?;
        match val {
            CondValue::Bool(b) => Ok(b),
            _ => {
                self.ctx
                    .struct_error(expr.span, "condition must evaluate to a boolean value")
                    .emit();
                Err(())
            }
        }
    }

    fn eval_inner(&mut self, expr: &Expr) -> Result<CondValue, ()> {
        match &expr.kind {
            ExprKind::Bool(b) => Ok(CondValue::Bool(*b)),
            ExprKind::String(s) => Ok(CondValue::String(s.clone())),

            // 核心：将标识符解析为环境变量
            ExprKind::Identifier(sym) => {
                let name = self.ctx.resolve(*sym);

                // 1. 尝试匹配内置平台变量
                if name == "os" {
                    // TargetMachine Triple 中的 OS 字段
                    let os_str = self.ctx.target.triple.operating_system.to_string();
                    return Ok(CondValue::String(os_str));
                }
                if name == "arch" {
                    let arch_str = self.ctx.target.triple.architecture.to_string();
                    return Ok(CondValue::String(arch_str));
                }

                // 2. 尝试从 CLI 传入的 -D 变量中读取
                if let Some(val) = self.ctx.custom_defines.get(name) {
                    if val == "true" {
                        return Ok(CondValue::Bool(true));
                    }
                    if val == "false" {
                        return Ok(CondValue::Bool(false));
                    }
                    return Ok(CondValue::String(val.clone()));
                }

                // 找不到变量
                self.ctx.struct_error(expr.span, format!("unknown environment variable `{}` in compilation condition", name))
                    .with_hint("available variables: `os`, `arch`, or custom defines via CLI (e.g., `-D feature=true`)")
                    .emit();
                Err(())
            }

            ExprKind::Unary {
                op: UnaryOperator::LogicalNot,
                operand,
            } => {
                let val = self.eval_inner(operand)?;
                if let CondValue::Bool(b) = val {
                    Ok(CondValue::Bool(!b))
                } else {
                    self.ctx
                        .struct_error(operand.span, "cannot apply `!` to a non-boolean value")
                        .emit();
                    Err(())
                }
            }

            ExprKind::Binary { lhs, op, rhs } => {
                let left = self.eval_inner(lhs)?;
                // 为了支持短路求值，如果是 And/Or，我们先不求右边
                if *op == BinaryOperator::LogicalAnd {
                    if let CondValue::Bool(false) = left {
                        return Ok(CondValue::Bool(false));
                    }
                    return self.eval_inner(rhs); // 左边是 true，结果取决于右边
                }
                if *op == BinaryOperator::LogicalOr {
                    if let CondValue::Bool(true) = left {
                        return Ok(CondValue::Bool(true));
                    }
                    return self.eval_inner(rhs); // 左边是 false，结果取决于右边
                }

                // 其他比较操作 (如 ==, !=)
                let right = self.eval_inner(rhs)?;
                match op {
                    BinaryOperator::Equal => Ok(CondValue::Bool(left == right)),
                    BinaryOperator::NotEqual => Ok(CondValue::Bool(left != right)),
                    _ => {
                        self.ctx
                            .struct_error(
                                expr.span,
                                "unsupported operator in compilation condition",
                            )
                            .emit();
                        Err(())
                    }
                }
            }

            _ => {
                self.ctx.struct_error(expr.span, "invalid expression in compilation condition")
                    .with_hint("conditions only support booleans, strings, `and`, `or`, `!`, `==`, and `!=`")
                    .emit();
                Err(())
            }
        }
    }
}

#[derive(PartialEq)]
enum CondValue {
    Bool(bool),
    String(String),
}
