#![allow(unused)]
use super::config::TargetMachine;
use super::CompileOptions;
use super::diagnostic::{Diagnostic, DiagnosticLevel};
use crate::parser::ast::NodeId;
use crate::sema::*;
use crate::utils::*;

use std::collections::HashMap;

pub struct Context {
    pub interner: Interner,
    pub source_manager: SourceManager,
    pub diagnostics: Vec<Diagnostic>,
    pub error_count: usize,
    pub type_registry: ty::TypeRegistry,
    pub defs: Vec<def::Def>,
    pub scopes: scope::SymbolTable,
    pub node_types: HashMap<NodeId, ty::TypeId>,
    pub target: TargetMachine,
    pub custom_defines: HashMap<String, String>,
    pub next_node_id: u32,
}

impl Context {
    pub fn new() -> Self {
        Self {
            interner: Interner::new(),
            source_manager: SourceManager::new(),
            diagnostics: Vec::new(),
            error_count: 0,
            type_registry: ty::TypeRegistry::new(),
            defs: Vec::new(),
            scopes: scope::SymbolTable::new(),
            node_types: HashMap::new(),
            target: TargetMachine::default(),
            custom_defines: HashMap::new(),
            next_node_id: 0,
        }
    }

    /// 便捷方法：将 Context 配置与命令行参数同步
    pub fn apply_options(&mut self, options: &CompileOptions) {
        self.target = options.target.clone();
        self.custom_defines = options.custom_defines.clone();
    }

    pub fn next_node_id(&mut self) -> NodeId {
        let id = self.next_node_id;
        self.next_node_id += 1;
        NodeId(id)
    }

    /// 核心方法：报告诊断信息
    pub fn report(&mut self, span: Span, level: DiagnosticLevel, msg: String) {
        if level == DiagnosticLevel::Error {
            self.error_count += 1;
        }

        self.diagnostics.push(Diagnostic::new(level, span, msg));
    }

    /// 快捷方法：报告 Warning
    pub fn emit_warning(&mut self, span: Span, msg: String) {
        self.report(span, DiagnosticLevel::Warning, msg);
    }

    /// 判断是否有致命错误
    pub fn has_errors(&self) -> bool {
        self.error_count > 0
    }

    /// 字符串驻留 (Interning)
    pub fn intern(&mut self, string: &str) -> SymbolId {
        self.interner.intern(string)
    }

    /// 解析驻留的字符串
    pub fn resolve(&self, sym: SymbolId) -> &str {
        self.interner.resolve(sym).unwrap_or("<unknown>")
    }

    /// 加载文件 (代理 SourceManager)
    pub fn load_file<P: AsRef<std::path::Path>>(&mut self, path: P) -> std::io::Result<FileId> {
        self.source_manager.load_file(path)
    }

    pub fn add_def(&mut self, def: def::Def) -> ty::DefId {
        let id = ty::DefId(self.defs.len() as u32);
        self.defs.push(def);
        id
    }

    /// 返回一个 Builder，而不是直接报告。这样你可以加 Hint！
    pub fn struct_error(&mut self, span: Span, msg: impl Into<String>) -> DiagnosticBuilder<'_> {
        DiagnosticBuilder::new(self, DiagnosticLevel::Error, span, msg.into())
    }

    pub fn struct_warning(&mut self, span: Span, msg: impl Into<String>) -> DiagnosticBuilder<'_> {
        DiagnosticBuilder::new(self, DiagnosticLevel::Warning, span, msg.into())
    }

    /// 依然保留快捷方法，用于不需要复杂上下文的简单报错
    pub fn emit_error(&mut self, span: Span, msg: impl Into<String>) {
        self.struct_error(span, msg).emit();
    }

    /// ICE (Internal Compiler Error) 专用，遇到这个编译器应该考虑是否立即 panic，或者记录后退出
    pub fn emit_ice(&mut self, span: Span, msg: impl Into<String>) {
        DiagnosticBuilder::new(self, DiagnosticLevel::Ice, span, msg.into())
            .with_hint("This is a bug in the Kern compiler. Please report it!")
            .emit();
    }

    pub fn print_diagnostics(&self) {
        for diag in &self.diagnostics {
            self.print_single_diagnostic(diag);
        }
    }

    fn print_single_diagnostic(&self, diag: &Diagnostic) {
        let location = self.source_manager.lookup_location(diag.primary_span);

        let (filename, line, col) = match &location {
            Some(loc) => {
                let fname = self
                    .source_manager
                    .get_file_path(loc.file_id)
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "<unknown>".to_string());
                (fname, loc.line, loc.col)
            }
            None => ("<unknown>".to_string(), 0, 0),
        };

        // 1. 打印带颜色的 Header
        eprintln!(
            "\x1b[1m{}:{}:{}: {} \x1b[1m{}\x1b[0m",
            filename,
            line,
            col,
            diag.level.color_name(),
            diag.message
        );

        // 2. 打印主源码上下文 (红色波浪线)
        self.print_source_snippet(diag.primary_span, "\x1b[31;1m");

        // 🌟 3. 打印关联的源码上下文 (青色波浪线)
        for (rel_span, rel_label) in &diag.related_spans {
            eprintln!("   \x1b[36;1m=\x1b[0m \x1b[1mnote:\x1b[0m {}", rel_label);
            self.print_source_snippet(*rel_span, "\x1b[36;1m");
        }

        // 4. 打印纯文本提示信息 (Hints)
        for hint in &diag.hints {
            eprintln!("   \x1b[36;1m=\x1b[0m \x1b[1mhelp:\x1b[0m {}", hint);
        }

        eprintln!(); // 分隔空行
    }

    /// 辅助方法：提取出来的源码打印器
    fn print_source_snippet(&self, span: Span, caret_color: &str) {
        if let Some(loc) = self.source_manager.lookup_location(span) {
            if let Some(line_text) = self.source_manager.get_line_text(loc.clone()) {
                let line_num_str = format!("{}", loc.line);
                let padding = " ".repeat(line_num_str.len());

                eprintln!(" {} |", padding);
                eprintln!(" {} | {}", line_num_str, line_text.trim_end());
                eprint!(" {} | ", padding);

                let span_len = std::cmp::max(1, span.end.saturating_sub(span.start));
                let carets = "^".repeat(span_len);

                // 使用传入的颜色打印波浪线
                eprintln!(
                    "{}{}{}\x1b[0m",
                    " ".repeat(loc.col.saturating_sub(1)),
                    caret_color,
                    carets
                );
            }
        }
    }

    /// 将 TypeId 转换为人类可读的 Kern 语法字符串，专门用于报错和调试输出。
    /// 彻底告别 TypeId(20)！
    pub fn ty_to_string(&self, ty: ty::TypeId) -> String {
        let kind = self.type_registry.get(ty);
        match kind {
            ty::TypeKind::Primitive(p) => match p {
                ty::PrimitiveType::Void => "void".to_string(),
                ty::PrimitiveType::Bool => "bool".to_string(),
                ty::PrimitiveType::I8 => "i8".to_string(),
                ty::PrimitiveType::I16 => "i16".to_string(),
                ty::PrimitiveType::I32 => "i32".to_string(),
                ty::PrimitiveType::I64 => "i64".to_string(),
                ty::PrimitiveType::I128 => "i128".to_string(),
                ty::PrimitiveType::ISize => "isize".to_string(),
                ty::PrimitiveType::U8 => "u8".to_string(),
                ty::PrimitiveType::U16 => "u16".to_string(),
                ty::PrimitiveType::U32 => "u32".to_string(),
                ty::PrimitiveType::U64 => "u64".to_string(),
                ty::PrimitiveType::U128 => "u128".to_string(),
                ty::PrimitiveType::USize => "usize".to_string(),
                ty::PrimitiveType::F32 => "f32".to_string(),
                ty::PrimitiveType::F64 => "f64".to_string(),
                ty::PrimitiveType::Str => "str".to_string(),
            },

            // 组合类型：由于 Parser 的 AST 结构是 Mut(Elem)，
            // *mut T 会被解析为 Pointer(Mut(T))，这里打印出来刚好就是 "*mut T"
            ty::TypeKind::Pointer(elem) => format!("*{}", self.ty_to_string(*elem)),
            ty::TypeKind::VolatilePtr(elem) => format!("^{}", self.ty_to_string(*elem)),
            ty::TypeKind::Slice(elem) => format!("[]{}", self.ty_to_string(*elem)),
            ty::TypeKind::Array { elem, len } => format!("[{}]{}", len, self.ty_to_string(*elem)),
            ty::TypeKind::ArrayInfer(elem) => format!("[_]{}", self.ty_to_string(*elem)),
            ty::TypeKind::Mut(inner) => format!("mut {}", self.ty_to_string(*inner)),

            // 具名类型：去 defs 里查真名，并拼接泛型
            ty::TypeKind::Def(def_id, generics)
            | ty::TypeKind::TraitObject(def_id, generics)
            | ty::TypeKind::Adt(def_id, generics) => {
                let def = &self.defs[def_id.0 as usize];
                let name = def
                    .name()
                    .map(|sym| self.resolve(sym))
                    .unwrap_or("<anonymous>");

                if generics.is_empty() {
                    name.to_string()
                } else {
                    let gen_strs: Vec<String> =
                        generics.iter().map(|g| self.ty_to_string(*g)).collect();
                    format!("{}[{}]", name, gen_strs.join(", "))
                }
            }

            ty::TypeKind::AdtPayload(def_id, generics) => {
                let def = &self.defs[def_id.0 as usize];
                let name = def
                    .name()
                    .map(|sym| self.resolve(sym))
                    .unwrap_or("<anonymous>");
                if generics.is_empty() {
                    format!("{}::Payload", name)
                } else {
                    let gen_strs: Vec<String> =
                        generics.iter().map(|g| self.ty_to_string(*g)).collect();
                    format!("{}::Payload[{}]", name, gen_strs.join(", "))
                }
            }

            // 别名与占位符：打印名字，不展开底层目标（对用户更直观）
            ty::TypeKind::Alias(sym, _) => self.resolve(*sym).to_string(),
            ty::TypeKind::Param(sym) => self.resolve(*sym).to_string(),

            // 函数与方法签名
            ty::TypeKind::Function {
                params,
                ret,
                is_variadic,
            } => {
                let mut param_strs: Vec<String> =
                    params.iter().map(|p| self.ty_to_string(*p)).collect();
                if *is_variadic {
                    param_strs.push("...".to_string());
                }
                format!("fn({}) {}", param_strs.join(", "), self.ty_to_string(*ret))
            }

            // 特指某个具体的函数项 (比如传函数指针时报错)
            ty::TypeKind::FnDef(def_id, generics) => {
                let def = &self.defs[def_id.0 as usize];
                let name = def
                    .name()
                    .map(|sym| self.resolve(sym))
                    .unwrap_or("<anonymous fn>");
                if generics.is_empty() {
                    format!("fn item `{}`", name)
                } else {
                    let gen_strs: Vec<String> =
                        generics.iter().map(|g| self.ty_to_string(*g)).collect();
                    format!("fn item `{}[{}]`", name, gen_strs.join(", "))
                }
            }

            ty::TypeKind::Module(def_id) => {
                let def = &self.defs[def_id.0 as usize];
                let name = def
                    .name()
                    .map(|sym| self.resolve(sym))
                    .unwrap_or("<anonymous>");
                format!("module `{}`", name)
            }

            ty::TypeKind::Error => "{error}".to_string(),
        }
    }
}

// 允许默认初始化
impl Default for Context {
    fn default() -> Self {
        Self::new()
    }
}

/// 诊断信息构建器，允许链式调用以添加更多上下文信息
#[must_use = "Diagnostics must be emitted to take effect"]
pub struct DiagnosticBuilder<'a> {
    ctx: &'a mut Context,
    diag: Diagnostic,
}

impl<'a> DiagnosticBuilder<'a> {
    pub fn new(ctx: &'a mut Context, level: DiagnosticLevel, span: Span, msg: String) -> Self {
        Self {
            ctx,
            diag: Diagnostic::new(level, span, msg),
        }
    }

    /// 添加修改建议或提示
    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.diag = self.diag.with_hint(hint);
        self
    }

    /// 添加关联源码位置及其说明
    pub fn with_span_label(mut self, span: Span, label: impl Into<String>) -> Self {
        self.diag.related_spans.push((span, label.into()));
        self
    }

    /// 真正将错误提交到 Context 中
    pub fn emit(self) {
        if self.diag.level == DiagnosticLevel::Error || self.diag.level == DiagnosticLevel::Ice {
            self.ctx.error_count += 1;
        }
        self.ctx.diagnostics.push(self.diag);
    }
}
