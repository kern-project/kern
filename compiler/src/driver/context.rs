#![allow(unused)]
use super::CompileOptions;
use super::config::TargetMachine;
use super::diagnostic::{Diagnostic, DiagnosticLevel};
use crate::parser::ast::NodeId;
use crate::sema::ty::TypeFormatter;
use crate::sema::*;
use crate::utils::*;

use std::collections::HashMap;
use std::io::IsTerminal;

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
    // 存储别名映射
    pub module_aliases: HashMap<String, String>,
    // 存储被加载进内存的外部包的 Root DefId
    pub alias_roots: HashMap<SymbolId, ty::DefId>,
    pub next_node_id: u32,
    pub use_color: bool, // 缓存 TTY 状态
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
            module_aliases: HashMap::new(),
            alias_roots: HashMap::new(),
            next_node_id: 0,
            use_color: std::io::stderr().is_terminal(), // 初始化时检测一次
        }
    }

    pub fn apply_options(&mut self, options: &CompileOptions) {
        self.target = options.target.clone();
        self.custom_defines = options.custom_defines.clone();
        self.module_aliases = options.module_aliases.clone();
    }

    pub fn next_node_id(&mut self) -> NodeId {
        let id = self.next_node_id;
        self.next_node_id += 1;
        NodeId(id)
    }

    pub fn report(&mut self, span: Span, level: DiagnosticLevel, msg: String) {
        if level == DiagnosticLevel::Error || level == DiagnosticLevel::Ice {
            self.error_count += 1;
        }
        self.diagnostics.push(Diagnostic::new(level, span, msg));
    }

    pub fn emit_warning(&mut self, span: Span, msg: String) {
        self.report(span, DiagnosticLevel::Warning, msg);
    }

    pub fn has_errors(&self) -> bool {
        self.error_count > 0
    }

    pub fn intern(&mut self, string: &str) -> SymbolId {
        self.interner.intern(string)
    }

    pub fn resolve(&self, sym: SymbolId) -> &str {
        self.interner.resolve(sym).unwrap_or("<unknown>")
    }

    pub fn load_file<P: AsRef<std::path::Path>>(&mut self, path: P) -> std::io::Result<FileId> {
        self.source_manager.load_file(path)
    }

    pub fn add_def(&mut self, def: def::Def) -> ty::DefId {
        let id = ty::DefId(self.defs.len() as u32);
        self.defs.push(def);
        id
    }

    pub fn struct_error(&mut self, span: Span, msg: impl Into<String>) -> DiagnosticBuilder<'_> {
        DiagnosticBuilder::new(self, DiagnosticLevel::Error, span, msg.into())
    }

    pub fn struct_warning(&mut self, span: Span, msg: impl Into<String>) -> DiagnosticBuilder<'_> {
        DiagnosticBuilder::new(self, DiagnosticLevel::Warning, span, msg.into())
    }

    pub fn emit_error(&mut self, span: Span, msg: impl Into<String>) {
        self.struct_error(span, msg).emit();
    }

    pub fn emit_ice(&mut self, span: Span, msg: impl Into<String>) {
        DiagnosticBuilder::new(self, DiagnosticLevel::Ice, span, msg.into())
            .with_hint("This is a bug in the Kern compiler. Please report it!")
            .emit(); // emit 内部会处理直接退出
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

        let (bold_start, reset, prefix, color_eq) = if self.use_color {
            (
                "\x1b[1m",
                "\x1b[0m",
                diag.level.color_prefix(),
                "\x1b[36;1m",
            )
        } else {
            ("", "", "", "")
        };

        eprintln!(
            "{}{}:{}:{}: {}{} {}{}",
            bold_start,
            filename,
            line,
            col,
            prefix,
            diag.level.name(),
            diag.message,
            reset
        );

        self.print_source_snippet(diag.primary_span, diag.level);

        for (rel_span, rel_label) in &diag.related_spans {
            eprintln!(
                "   {}={} {}note:{} {}",
                color_eq, reset, bold_start, reset, rel_label
            );
            self.print_source_snippet(*rel_span, DiagnosticLevel::Note);
        }

        for hint in &diag.hints {
            eprintln!(
                "   {}={} {}help:{} {}",
                color_eq, reset, bold_start, reset, hint
            );
        }
        eprintln!();
    }

    fn print_source_snippet(&self, span: Span, level: DiagnosticLevel) {
        if let Some(loc) = self.source_manager.lookup_location(span) {
            if let Some(line_text) = self.source_manager.get_line_text(loc.clone()) {
                let line_num_str = format!("{}", loc.line);
                let padding = " ".repeat(line_num_str.len());

                eprintln!(" {} |", padding);
                eprintln!(" {} | {}", line_num_str, line_text.trim_end());
                eprint!(" {} | ", padding);

                // 安全截断：波浪线长度不能超过当前行的物理剩余长度
                let text_len = line_text.trim_end().len();
                let col_offset = loc.col.saturating_sub(1);
                let max_possible_carets = text_len.saturating_sub(col_offset);

                let raw_span_len = span.end.saturating_sub(span.start);
                let print_len = std::cmp::max(1, std::cmp::min(raw_span_len, max_possible_carets));

                let carets = "^".repeat(print_len);

                if self.use_color {
                    eprintln!(
                        "{}{}{}\x1b[0m",
                        " ".repeat(col_offset),
                        level.color_prefix(),
                        carets
                    );
                } else {
                    eprintln!("{}{}", " ".repeat(col_offset), carets);
                }
            }
        }
    }

    /// 代理调用独立的格式化器
    pub fn ty_to_string(&self, ty: ty::TypeId) -> String {
        TypeFormatter { ctx: self }.format(ty)
    }
}

impl Default for Context {
    fn default() -> Self {
        Self::new()
    }
}

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

    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.diag = self.diag.with_hint(hint);
        self
    }

    pub fn with_span_label(mut self, span: Span, label: impl Into<String>) -> Self {
        self.diag.related_spans.push((span, label.into()));
        self
    }

    pub fn emit(self) {
        let is_ice = self.diag.level == DiagnosticLevel::Ice;
        if self.diag.level == DiagnosticLevel::Error || is_ice {
            self.ctx.error_count += 1;
        }
        self.ctx.diagnostics.push(self.diag.clone());

        // 遇到编译器崩溃，打印现场后立即熔断
        if is_ice {
            self.ctx.print_single_diagnostic(&self.diag);
            eprintln!("\nCompiler panicked due to an internal error. Aborting.");
            std::process::exit(101);
        }
    }
}
