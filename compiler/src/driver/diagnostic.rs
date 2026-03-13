use crate::utils::Span;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum DiagnosticLevel {
    Error,
    Warning,
    Note,
    Ice, // Internal Compiler Error
}

impl DiagnosticLevel {
    /// 纯文本名称，用于日志重定向
    pub fn name(&self) -> &'static str {
        match self {
            DiagnosticLevel::Error => "error",
            DiagnosticLevel::Warning => "warning",
            DiagnosticLevel::Note => "note",
            DiagnosticLevel::Ice => "ICE",
        }
    }

    /// ANSI 颜色控制码前缀
    pub fn color_prefix(&self) -> &'static str {
        match self {
            DiagnosticLevel::Error => "\x1b[31;1m",   // 红色粗体
            DiagnosticLevel::Warning => "\x1b[33;1m", // 黄色粗体
            DiagnosticLevel::Note => "\x1b[36;1m",    // 青色粗体
            DiagnosticLevel::Ice => "\x1b[35;1m",     // 紫色粗体
        }
    }
}

#[derive(Debug, Clone)]
pub struct Diagnostic {
    pub level: DiagnosticLevel,
    pub primary_span: Span,
    pub message: String,
    pub hints: Vec<String>,
    pub related_spans: Vec<(Span, String)>,
}

impl Diagnostic {
    pub fn new(level: DiagnosticLevel, span: Span, message: impl Into<String>) -> Self {
        Self {
            level,
            primary_span: span,
            message: message.into(),
            hints: Vec::new(),
            related_spans: Vec::new(),
        }
    }

    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hints.push(hint.into());
        self
    }
}
