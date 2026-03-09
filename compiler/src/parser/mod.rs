pub mod ast;
pub mod lexer;
pub mod stream;
pub mod token;

use crate::driver::context::Context;
use crate::driver::diagnostic::DiagnosticLevel;
use crate::utils::FileId;
use crate::utils::{Span, SymbolId};
use ast::*;
use lexer::Lexer;
use stream::TokenStream;
use token::{Token, TokenType};

/// 解析结果类型
/// Err(()) 表示错误已经报告给 Context，调用者应该进行恢复或向上传播
pub type ParseResult<T> = Result<T, ()>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Precedence {
    Lowest,
    Assignment, // =
    LogicalOr,  // or
    LogicalAnd, // and
    Equality,   // == !=
    Comparison, // < > <= >=
    Term,       // + -
    Factor,     // * / %
    Unary,      // ! - * &
    Cast,       // as
    Call,       // () . []
}

impl Precedence {
    fn from_token(t: TokenType) -> Self {
        match t {
            TokenType::Dot
            | TokenType::DotLBracket
            | TokenType::DotLBrace
            | TokenType::DotStar
            | TokenType::LParen
            | TokenType::LBracket
            | TokenType::DotAmpersand
            | TokenType::Bang => Self::Call,

            TokenType::As => Self::Cast,

            TokenType::Star | TokenType::Slash | TokenType::Percent => Self::Factor,
            TokenType::Plus | TokenType::Minus | TokenType::Pipe | TokenType::Caret => Self::Term,

            TokenType::LessThan
            | TokenType::LessEqual
            | TokenType::GreaterThan
            | TokenType::GreaterEqual => Self::Comparison,
            TokenType::EqualEqual | TokenType::NotEqual => Self::Equality,

            TokenType::And => Self::LogicalAnd,
            TokenType::Or => Self::LogicalOr,

            TokenType::Assign
            | TokenType::PlusAssign
            | TokenType::MinusAssign
            | TokenType::StarAssign
            | TokenType::SlashAssign
            | TokenType::PercentAssign
            | TokenType::AmpersandAssign
            | TokenType::PipeAssign
            | TokenType::CaretAssign
            | TokenType::LShiftAssign
            | TokenType::RShiftAssign => Self::Assignment,

            _ => Self::Lowest,
        }
    }
}

pub struct Parser<'a> {
    stream: TokenStream<'a>,
    context: &'a mut Context,
    // 状态标记
    panic_mode: bool,
}

impl<'a> Parser<'a> {
    pub fn new(source: &'a str, file_id: FileId, context: &'a mut Context) -> Self {
        let lexer = Lexer::new(source, file_id);
        let stream = TokenStream::new(lexer);
        Self {
            stream,
            context,
            panic_mode: false,
        }
    }

    // ==========================================
    // Core Tools: AST Node Creation
    // ==========================================

    fn new_id(&mut self) -> NodeId {
        self.context.next_node_id()
    }

    // ==========================================
    // Core Tools: Token Consumption
    // ==========================================

    fn peek(&mut self) -> Token {
        self.stream.peek()
    }

    fn advance(&mut self) -> Token {
        self.stream.bump()
    }

    fn check(&mut self, tag: TokenType) -> bool {
        self.stream.check(tag)
    }

    fn match_token(&mut self, tags: &[TokenType]) -> bool {
        for &tag in tags {
            if self.check(tag) {
                self.advance();
                return true;
            }
        }
        false
    }

    /// 消费一个 Token，如果类型不对则报错 (Sync 入口)
    fn expect(&mut self, tag: TokenType) -> ParseResult<Token> {
        if self.check(tag) {
            Ok(self.advance())
        } else {
            let current = self.peek();
            let found_text = self
                .context
                .source_manager
                .slice_source(current.span)
                .to_string();

            let mut diag = self.context.struct_error(
                current.span,
                format!("expected `{:?}`, found `{}`", tag, found_text),
            );

            // 针对特定的缺失提供智能提示
            match tag {
                TokenType::Semicolon => diag = diag.with_hint("consider adding a `;` here"),
                TokenType::RBrace => diag = diag.with_hint("unclosed block"),
                TokenType::RParen => diag = diag.with_hint("unclosed parenthesis"),
                _ => {}
            }

            diag.emit();
            self.panic_mode = true;
            Err(())
        }
    }

    // ==========================================
    // Integration: String Interner & Unescape
    // ==========================================

    fn intern_token(&mut self, token: Token) -> SymbolId {
        let text = self.context.source_manager.slice_source(token.span);
        self.context.interner.intern(text)
    }

    fn parse_string_literal(&mut self, token: Token) -> ParseResult<SymbolId> {
        let raw = self
            .context
            .source_manager
            .slice_source(token.span)
            .to_string();

        // 1. 检查并去掉引号
        if raw.len() < 2 || !raw.starts_with('"') || !raw.ends_with('"') {
            self.context
                .struct_error(token.span, "invalid or unterminated string literal")
                .with_hint("ensure the string is properly enclosed in double quotes `\"`")
                .emit();
            return Err(());
        }

        let inner = &raw[1..raw.len() - 1];

        // 2. 转义处理
        let unescaped = self.unescape_string(inner, token.span)?;

        // 3. Intern
        Ok(self.context.intern(&unescaped))
    }

    fn unescape_string(&mut self, input: &str, span: Span) -> ParseResult<String> {
        let mut result = String::with_capacity(input.len());
        let mut chars = input.chars().peekable();

        while let Some(c) = chars.next() {
            if c == '\\' {
                match chars.next() {
                    Some('n') => result.push('\n'),
                    Some('r') => result.push('\r'),
                    Some('t') => result.push('\t'),
                    Some('\\') => result.push('\\'),
                    Some('\'') => result.push('\''),
                    Some('"') => result.push('"'),
                    Some('0') => result.push('\0'),

                    // Hex: \xNN
                    Some('x') => {
                        let hex: String = chars.by_ref().take(2).collect();
                        if hex.len() != 2 {
                            self.add_error(span, "Invalid hex escape sequence".to_string());
                            return Err(());
                        }
                        let byte = u8::from_str_radix(&hex, 16).map_err(|_| {
                            self.add_error(span, format!("Invalid hex escape: {}", hex));
                        })?;
                        result.push(byte as char);
                    }

                    // Unicode: \u{...}
                    Some('u') => {
                        if chars.next() != Some('{') {
                            self.add_error(span, "Expected '{' after \\u".to_string());
                            return Err(());
                        }

                        let mut hex_str = String::new();
                        let mut found_brace = false;
                        for ch in chars.by_ref() {
                            if ch == '}' {
                                found_brace = true;
                                break;
                            }
                            hex_str.push(ch);
                        }

                        if !found_brace {
                            self.add_error(span, "Unterminated unicode escape".to_string());
                            return Err(());
                        }

                        let code_point = u32::from_str_radix(&hex_str, 16).map_err(|_| {
                            self.add_error(span, format!("Invalid unicode scalar: {}", hex_str));
                        })?;

                        if let Some(c) = std::char::from_u32(code_point) {
                            result.push(c);
                        } else {
                            self.add_error(
                                span,
                                format!("Invalid unicode scalar value: {:x}", code_point),
                            );
                            return Err(());
                        }
                    }

                    Some(c) => {
                        self.context
                            .struct_error(span, format!("unknown escape sequence: `\\{}`", c))
                            .with_hint(format!("if you meant to write a backslash, use `\\\\`"))
                            .emit();
                        self.panic_mode = true;
                        return Err(());
                    }
                    None => {
                        self.add_error(span, "Unterminated escape sequence".to_string());
                        return Err(());
                    }
                }
            } else {
                result.push(c);
            }
        }
        Ok(result)
    }

    // ==========================================
    // Error Handling & Synchronization
    // ==========================================

    fn error_at_current(&mut self, msg: String) {
        let span = self.peek().span;
        self.add_error(span, msg);
    }

    fn add_error(&mut self, span: Span, msg: String) {
        if self.panic_mode {
            return;
        }
        self.panic_mode = true;
        self.context.report(span, DiagnosticLevel::Error, msg);
    }

    pub fn synchronize(&mut self) {
        self.panic_mode = false;
        if !self.check(TokenType::Eof) {
            self.advance();
        }

        while !self.check(TokenType::Eof) {
            // 如果碰到了分号，很可能上一个语句结束了
            if self.stream.peek_nth(0).tag == TokenType::Semicolon {
                self.advance();
                return;
            }

            match self.peek().tag {
                TokenType::Fn
                | TokenType::Let
                | TokenType::Const
                | TokenType::Struct
                | TokenType::Enum
                | TokenType::If
                | TokenType::For
                | TokenType::Return => return,
                _ => {
                    self.advance();
                }
            }
        }
    }
}

// ==========================================
//             Type Parsing
// ==========================================
impl<'a> Parser<'a> {
    pub fn parse_type(&mut self) -> ParseResult<TypeNode> {
        let start_token = self.peek();

        match start_token.tag {
            TokenType::Star => self.parse_pointer_type(),
            TokenType::Caret => self.parse_volatile_pointer_type(),
            TokenType::LBracket => self.parse_array_or_slice_type(),
            TokenType::Fn => self.parse_fn_type(),
            TokenType::Identifier => self.parse_path_type(),
            TokenType::Mut => self.parse_mut_type(),

            TokenType::Underscore => {
                self.advance();
                Ok(TypeNode {
                    id: self.new_id(),
                    span: start_token.span,
                    kind: TypeKind::Infer,
                })
            }
            TokenType::SelfType => {
                self.advance();
                Ok(TypeNode {
                    id: self.new_id(),
                    span: start_token.span,
                    kind: TypeKind::SelfType,
                })
            }

            TokenType::Struct => self.parse_struct_type(false),
            TokenType::Union => self.parse_struct_type(true),
            TokenType::Enum => self.parse_enum_type(),
            TokenType::Trait => self.parse_trait_type(),
            TokenType::Adt => self.parse_adt_type(),

            _ => {
                let token = self.peek();
                let found_text = self
                    .context
                    .source_manager
                    .slice_source(token.span)
                    .to_string();
                self.add_error(
                    token.span,
                    format!("Expected type definition, found '{}'", found_text),
                );
                Err(())
            }
        }
    }

    // --- Type Parsing Sub-Routines ---

    fn parse_pointer_type(&mut self) -> ParseResult<TypeNode> {
        let start_span = self.advance().span; // 消费 '*'
        let elem = self.parse_type()?;

        Ok(TypeNode {
            id: self.new_id(),
            span: start_span.to(elem.span),
            kind: TypeKind::Pointer {
                elem: Box::new(elem),
            },
        })
    }

    fn parse_volatile_pointer_type(&mut self) -> ParseResult<TypeNode> {
        let start_span = self.advance().span; // 消费 '^'
        let elem = self.parse_type()?;

        Ok(TypeNode {
            id: self.new_id(),
            span: start_span.to(elem.span),
            kind: TypeKind::VolatilePtr {
                elem: Box::new(elem),
            },
        })
    }

    fn parse_array_or_slice_type(&mut self) -> ParseResult<TypeNode> {
        let start_span = self.advance().span; // 消费 '['

        // A. 切片 []T
        if self.match_token(&[TokenType::RBracket]) {
            let elem = self.parse_type()?;
            Ok(TypeNode {
                id: self.new_id(),
                span: start_span.to(elem.span),
                kind: TypeKind::Slice {
                    elem: Box::new(elem),
                },
            })
        }
        // B. 数组 [expr]T
        else {
            let len_expr = self.parse_expression(Precedence::Lowest)?;
            self.expect(TokenType::RBracket)?;
            let elem = self.parse_type()?;

            Ok(TypeNode {
                id: self.new_id(),
                span: start_span.to(elem.span),
                kind: TypeKind::Array {
                    elem: Box::new(elem),
                    len: Box::new(len_expr),
                },
            })
        }
    }

    fn parse_fn_type(&mut self) -> ParseResult<TypeNode> {
        let start_span = self.advance().span; // 消费 'fn'
        self.expect(TokenType::LParen)?;

        let mut params = Vec::new();
        let mut is_variadic = false;

        if !self.check(TokenType::RParen) {
            loop {
                // 拦截可变参数 ...
                if self.match_token(&[TokenType::Ellipsis]) {
                    is_variadic = true;
                    break; // ... 必须是最后一个参数
                }

                params.push(self.parse_type()?);

                if !self.match_token(&[TokenType::Comma]) {
                    break;
                }
            }
        }
        self.expect(TokenType::RParen)?;

        let ret_type = self.parse_type()?;
        let end = ret_type.span;

        Ok(TypeNode {
            id: self.new_id(),
            span: start_span.to(end),
            kind: TypeKind::Function {
                params,
                ret: Some(Box::new(ret_type)),
                is_variadic,
            },
        })
    }

    fn parse_path_type(&mut self) -> ParseResult<TypeNode> {
        let start_token = self.advance(); // 消费第一个 ident
        let first_id = self.intern_token(start_token);
        let mut span = start_token.span;

        let mut segments = vec![first_id];

        while self.match_token(&[TokenType::Dot]) {
            let id_token = self.expect(TokenType::Identifier)?;
            segments.push(self.intern_token(id_token));
            span = span.to(id_token.span);
        }

        // 泛型参数 List[T]
        let mut generics = Vec::new();
        if self.check(TokenType::LBracket) {
            generics = self.parse_type_args()?;
            span = span.to(self.stream.prev_span());
        }

        Ok(TypeNode {
            id: self.new_id(),
            span,
            kind: TypeKind::Path { segments, generics },
        })
    }

    fn parse_mut_type(&mut self) -> ParseResult<TypeNode> {
        let start_span = self.advance().span; // 消费 'mut'
        let elem = self.parse_type()?;

        // 护城河：拦截 mut *, mut ^, mut []
        if matches!(
            elem.kind,
            TypeKind::Pointer { .. } | TypeKind::VolatilePtr { .. } | TypeKind::Slice { .. }
        ) {
            self.add_error(start_span.to(elem.span), "Forbidden: cannot apply 'mut' directly to pointers or slices (e.g., 'mut *T'). Pointer arithmetic is disabled.".to_string());
        }

        Ok(TypeNode {
            id: self.new_id(),
            span: start_span.to(elem.span),
            kind: TypeKind::Mut(Box::new(elem)),
        })
    }

    fn parse_type_args(&mut self) -> ParseResult<Vec<TypeNode>> {
        self.expect(TokenType::LBracket)?;
        let mut args = Vec::new();
        if !self.check(TokenType::RBracket) {
            loop {
                args.push(self.parse_type()?);
                if !self.match_token(&[TokenType::Comma]) {
                    break;
                }
                if self.check(TokenType::RBracket) {
                    break;
                }
            }
        }
        self.expect(TokenType::RBracket)?;
        Ok(args)
    }

    fn parse_struct_type(&mut self, is_union: bool) -> ParseResult<TypeNode> {
        let start_token = self.advance(); // struct / union
        self.expect(TokenType::LBrace)?;

        let mut fields = Vec::new();
        while !self.check(TokenType::RBrace) && !self.check(TokenType::Eof) {
            let name_token = self.expect(TokenType::Identifier)?;
            let name_id = self.intern_token(name_token);
            self.expect(TokenType::Colon)?;
            let field_type = self.parse_type()?;

            let mut default_value = None;
            if self.match_token(&[TokenType::Assign]) {
                default_value = Some(Box::new(self.parse_expression(Precedence::Lowest)?));
            }

            let span = name_token.span.to(if let Some(ref v) = default_value {
                v.span
            } else {
                field_type.span
            });

            fields.push(StructFieldDef {
                name: name_id,
                type_node: field_type,
                default_value,
                span,
            });

            if !self.match_token(&[TokenType::Comma]) {
                break;
            }
        }

        let end_token = self.expect(TokenType::RBrace)?;
        let kind = if is_union {
            TypeKind::Union { fields }
        } else {
            TypeKind::Struct { fields }
        };

        Ok(TypeNode {
            id: self.new_id(),
            span: start_token.span.to(end_token.span),
            kind,
        })
    }

    fn parse_enum_type(&mut self) -> ParseResult<TypeNode> {
        let start_token = self.advance();
        let mut backing_type = None;
        if self.match_token(&[TokenType::Colon]) {
            backing_type = Some(Box::new(self.parse_type()?));
        }

        self.expect(TokenType::LBrace)?;
        let mut variants = Vec::new();

        while !self.check(TokenType::RBrace) && !self.check(TokenType::Eof) {
            let name_token = self.expect(TokenType::Identifier)?;
            let name_id = self.intern_token(name_token);

            let mut value = None;
            if self.match_token(&[TokenType::Assign]) {
                value = Some(Box::new(self.parse_expression(Precedence::Lowest)?));
            }

            let span = name_token.span.to(if let Some(ref v) = value {
                v.span
            } else {
                name_token.span
            });

            variants.push(EnumVariant {
                name: name_id,
                value,
                span,
            });
            if !self.match_token(&[TokenType::Comma]) {
                break;
            }
        }

        let end_token = self.expect(TokenType::RBrace)?;

        Ok(TypeNode {
            id: self.new_id(),
            span: start_token.span.to(end_token.span),
            kind: TypeKind::Enum {
                backing_type,
                variants,
            },
        })
    }

    fn parse_trait_type(&mut self) -> ParseResult<TypeNode> {
        let start_token = self.advance();
        self.expect(TokenType::LBrace)?;

        let mut fields = Vec::new();
        while !self.check(TokenType::RBrace) && !self.check(TokenType::Eof) {
            let name_token = self.expect(TokenType::Identifier)?;
            let name_id = self.intern_token(name_token);
            self.expect(TokenType::Colon)?;
            // 1. 解析后面的签名，比如 fn() i32
            let mut method_type = self.parse_type()?;
            if let TypeKind::Function { ref mut params, .. } = method_type.kind {
                // 构造一个隐式的 Self 类型节点
                let implicit_self = TypeNode {
                    id: self.new_id(),
                    span: name_token.span, // 使用方法名的位置作为 span
                    kind: TypeKind::SelfType,
                };
                params.insert(0, implicit_self);
            } else {
                self.add_error(
                    method_type.span,
                    "Trait members must be function signatures (e.g., `fn() void`)".to_string(),
                );
            }

            if self.check(TokenType::Assign) {
                self.error_at_current(
                    "Trait methods cannot have default implementations here.".to_string(),
                );
                self.advance();
                let _ = self.parse_expression(Precedence::Lowest)?; // consume expr
            }

            fields.push(StructFieldDef {
                name: name_id,
                default_value: None,
                span: name_token.span.to(method_type.span),
                type_node: method_type,
            });

            if !self.match_token(&[TokenType::Comma]) {
                break;
            }
        }
        let end_token = self.expect(TokenType::RBrace)?;

        Ok(TypeNode {
            id: self.new_id(),
            span: start_token.span.to(end_token.span),
            kind: TypeKind::Trait { fields },
        })
    }

    fn parse_adt_type(&mut self) -> ParseResult<TypeNode> {
        let start_token = self.advance(); // 消费 'adt'
        self.expect(TokenType::LBrace)?;

        let mut variants = Vec::new();
        while !self.check(TokenType::RBrace) && !self.check(TokenType::Eof) {
            let name_token = self.expect(TokenType::Identifier)?;
            let name_id = self.intern_token(name_token);

            let mut payload_type = None;
            // 如果遇到冒号，说明有数据负载 (Payload)
            if self.match_token(&[TokenType::Colon]) {
                payload_type = Some(Box::new(self.parse_type()?));
            }

            let span = name_token.span.to(if let Some(ref p) = payload_type {
                p.span
            } else {
                name_token.span
            });

            variants.push(AdtVariant {
                name: name_id,
                payload_type,
                span,
            });

            if !self.match_token(&[TokenType::Comma]) {
                break;
            }
        }

        let end_token = self.expect(TokenType::RBrace)?;

        Ok(TypeNode {
            id: self.new_id(),
            span: start_token.span.to(end_token.span),
            kind: TypeKind::Adt { variants },
        })
    }
}

// ==========================================
//            Expression Parsing
// ==========================================
impl<'a> Parser<'a> {
    pub fn parse_expression(&mut self, precedence: Precedence) -> ParseResult<Expr> {
        let prefix_token = self.advance();
        let mut left = self.parse_prefix(prefix_token)?;

        while precedence < Precedence::from_token(self.peek().tag) {
            let op_token = self.advance();
            left = self.parse_infix(left, op_token)?;
        }
        Ok(left)
    }

    fn parse_prefix(&mut self, token: Token) -> ParseResult<Expr> {
        let span = token.span;
        match token.tag {
            // Literals
            TokenType::IntLiteral
            | TokenType::FloatLiteral
            | TokenType::StringLiteral
            | TokenType::CharLiteral => self.parse_literal_expr(token),

            TokenType::Identifier => {
                let name = self.intern_token(token);
                Ok(Expr {
                    id: self.new_id(),
                    span,
                    kind: ExprKind::Identifier(name),
                })
            }

            // Unary & Enums
            TokenType::DotLBrace => self.parse_data_init(None, span),
            TokenType::Dot => self.parse_enum_literal_expr(span),
            TokenType::Minus | TokenType::Bang | TokenType::Tilde | TokenType::Hash => {
                self.parse_unary_prefix_expr(token)
            }
            TokenType::LParen => self.parse_grouped_expr(span),

            // Control Flow & Blocks
            TokenType::If => self.parse_if_expr(span),
            TokenType::Switch => self.parse_switch_expr(span),
            TokenType::Match => self.parse_match_expr(span),
            TokenType::LBrace => self.parse_block_expr(span),
            TokenType::For => self.parse_for_expr(span),
            TokenType::Let | TokenType::Const | TokenType::Static => self.parse_decl_expr(token),

            // Jumps
            TokenType::Break => Ok(Expr {
                id: self.new_id(),
                span,
                kind: ExprKind::Break,
            }),
            TokenType::Continue => Ok(Expr {
                id: self.new_id(),
                span,
                kind: ExprKind::Continue,
            }),
            TokenType::Return => self.parse_return_expr(span),

            // Special / Intrinsics
            TokenType::Undef => Ok(Expr {
                id: self.new_id(),
                span,
                kind: ExprKind::Undef,
            }),
            TokenType::SelfValue => Ok(Expr {
                id: self.new_id(),
                span,
                kind: ExprKind::SelfValue,
            }),
            TokenType::At => self.parse_intrinsic_expr(token),

            // Explicitly Typed Initializations (e.g., mut T.{...}, [N]T.{...}, *T.{...})
            TokenType::Mut | TokenType::LBracket | TokenType::Star | TokenType::Caret => {
                self.parse_typed_data_init_prefix(token)
            }

            _ => {
                let text = self.context.source_manager.slice_source(span).to_string();
                self.add_error(span, format!("Expected expression, found '{}'", text));
                Err(())
            }
        }
    }

    fn parse_infix(&mut self, left: Expr, token: Token) -> ParseResult<Expr> {
        match token.tag {
            // Binary Operators
            TokenType::Plus
            | TokenType::Minus
            | TokenType::Star
            | TokenType::Slash
            | TokenType::EqualEqual
            | TokenType::NotEqual
            | TokenType::Percent
            | TokenType::LessThan
            | TokenType::LessEqual
            | TokenType::GreaterThan
            | TokenType::GreaterEqual
            | TokenType::And
            | TokenType::Or
            | TokenType::Pipe
            | TokenType::Ampersand
            | TokenType::Caret
            | TokenType::LShift
            | TokenType::RShift => self.parse_binary_expr(left, token),

            // Field & Method Access
            TokenType::Dot => self.parse_field_access_expr(left),
            TokenType::LParen => self.parse_call_expr(left),

            // Pointer Deref & AddressOf
            TokenType::DotStar => Ok(Expr {
                id: self.new_id(),
                span: left.span.to(token.span),
                kind: ExprKind::Unary {
                    op: UnaryOperator::PointerDeRef,
                    operand: Box::new(left),
                },
            }),
            TokenType::DotAmpersand => Ok(Expr {
                id: self.new_id(),
                span: left.span.to(token.span),
                kind: ExprKind::Unary {
                    op: UnaryOperator::AddressOf,
                    operand: Box::new(left),
                },
            }),

            // Assignments
            TokenType::Assign
            | TokenType::PlusAssign
            | TokenType::MinusAssign
            | TokenType::StarAssign
            | TokenType::SlashAssign
            | TokenType::PercentAssign
            | TokenType::AmpersandAssign
            | TokenType::PipeAssign
            | TokenType::CaretAssign
            | TokenType::LShiftAssign
            | TokenType::RShiftAssign => self.parse_assignment_expr(left, token),

            // Casts & Indexing/Slicing
            TokenType::As => self.parse_as_cast_expr(left),
            TokenType::DotLBracket => self.parse_slice_or_index_expr(left),
            TokenType::LBracket => self.parse_generic_instantiation_expr(left),

            // Type-affixed Data Init (Type.{...})
            TokenType::DotLBrace => {
                let type_node = self.expr_to_type(left)?;
                let span = type_node.span;
                self.parse_data_init(Some(Box::new(type_node)), span)
            }

            _ => {
                self.add_error(
                    token.span,
                    format!("Unexpected infix token {:?}", token.tag),
                );
                Err(())
            }
        }
    }

    // --- Prefix Sub-Routines ---

    fn parse_literal_expr(&mut self, token: Token) -> ParseResult<Expr> {
        let span = token.span;
        match token.tag {
            TokenType::IntLiteral => {
                let text = self.context.source_manager.slice_source(span).to_string();
                let text_clean = text.replace("_", "");
                let (radix, num_str) = if text_clean.starts_with("0x") {
                    (16, &text_clean[2..])
                } else if text_clean.starts_with("0b") {
                    (2, &text_clean[2..])
                } else if text_clean.starts_with("0o") {
                    (8, &text_clean[2..])
                } else {
                    (10, text_clean.as_str())
                };

                let val = u128::from_str_radix(num_str, radix).map_err(|_| {
                    self.add_error(span, format!("Invalid integer literal: {}", text));
                })?;
                Ok(Expr {
                    id: self.new_id(),
                    span,
                    kind: ExprKind::Integer(val),
                })
            }
            TokenType::FloatLiteral => {
                let text = self
                    .context
                    .source_manager
                    .slice_source(span)
                    .replace("_", "");
                let val = text.parse::<f64>().map_err(|_| {
                    self.add_error(span, format!("Invalid float literal: {}", text));
                })?;
                Ok(Expr {
                    id: self.new_id(),
                    span,
                    kind: ExprKind::Float(val),
                })
            }
            TokenType::StringLiteral => {
                let sid = self.parse_string_literal(token)?;
                Ok(Expr {
                    id: self.new_id(),
                    span,
                    kind: ExprKind::String(self.context.resolve(sid).to_string()),
                })
            }
            TokenType::CharLiteral => {
                let raw = self.context.source_manager.slice_source(span);
                let inner = &raw[1..raw.len() - 1];
                let c = if inner.is_empty() {
                    self.add_error(span, "Empty character literal".to_string());
                    '\0' // Dummy value for recovery
                } else {
                    inner.chars().next().unwrap()
                };
                Ok(Expr {
                    id: self.new_id(),
                    span,
                    kind: ExprKind::Char(c),
                })
            }
            _ => unreachable!(),
        }
    }

    fn parse_enum_literal_expr(&mut self, start_span: Span) -> ParseResult<Expr> {
        if self.check(TokenType::Identifier) {
            let id_token = self.advance();
            let sid = self.intern_token(id_token);
            Ok(Expr {
                id: self.new_id(),
                span: start_span.to(id_token.span),
                kind: ExprKind::EnumLiteral(sid),
            })
        } else {
            self.add_error(
                start_span,
                "Unexpected '.' at start of expression".to_string(),
            );
            Err(())
        }
    }

    fn parse_unary_prefix_expr(&mut self, token: Token) -> ParseResult<Expr> {
        let op = match token.tag {
            TokenType::Minus => UnaryOperator::Negate,
            TokenType::Bang => UnaryOperator::LogicalNot,
            TokenType::Tilde => UnaryOperator::BitwiseNot,
            TokenType::Hash => UnaryOperator::LengthOf,
            _ => unreachable!(),
        };
        let operand = self.parse_expression(Precedence::Unary)?;
        Ok(Expr {
            id: self.new_id(),
            span: token.span.to(operand.span),
            kind: ExprKind::Unary {
                op,
                operand: Box::new(operand),
            },
        })
    }

    fn parse_grouped_expr(&mut self, start_span: Span) -> ParseResult<Expr> {
        let mut expr = self.parse_expression(Precedence::Lowest)?;
        let rparen = self.expect(TokenType::RParen)?;
        expr.span = start_span.to(rparen.span);
        Ok(expr)
    }

    fn parse_return_expr(&mut self, span: Span) -> ParseResult<Expr> {
        let mut val = None;
        let is_stopper = self.check(TokenType::Semicolon)
            || self.check(TokenType::RBrace)
            || self.check(TokenType::Else)
            || self.check(TokenType::RParen)
            || self.check(TokenType::RBracket)
            || self.check(TokenType::Comma)
            || self.check(TokenType::Eof);
        if !is_stopper {
            val = Some(Box::new(self.parse_expression(Precedence::Lowest)?));
        }
        Ok(Expr {
            id: self.new_id(),
            span,
            kind: ExprKind::Return(val),
        })
    }

    fn parse_intrinsic_expr(&mut self, at_token: Token) -> ParseResult<Expr> {
        let id_token = self.expect(TokenType::Identifier)?;
        let sym = self.intern_token(id_token);
        let name_str = format!("@{}", self.context.resolve(sym));
        let sym_id = self.context.intern(&name_str);
        Ok(Expr {
            id: self.new_id(),
            span: at_token.span.to(id_token.span),
            kind: ExprKind::Identifier(sym_id),
        })
    }

    fn parse_typed_data_init_prefix(&mut self, start_token: Token) -> ParseResult<Expr> {
        let span = start_token.span;

        // 我们利用了 parse_type 的递归结构，但是需要为第一步的特殊 token 手动桥接
        let type_node = match start_token.tag {
            TokenType::Mut => {
                let elem = self.parse_type()?;
                TypeNode {
                    id: self.new_id(),
                    span: span.to(elem.span),
                    kind: TypeKind::Mut(Box::new(elem)),
                }
            }
            TokenType::LBracket => {
                if self.match_token(&[TokenType::RBracket]) {
                    let elem = self.parse_type()?;
                    TypeNode {
                        id: self.new_id(),
                        span: span.to(elem.span),
                        kind: TypeKind::Slice {
                            elem: Box::new(elem),
                        },
                    }
                } else {
                    let len_expr = self.parse_expression(Precedence::Lowest)?;
                    self.expect(TokenType::RBracket)?;
                    let elem = self.parse_type()?;
                    TypeNode {
                        id: self.new_id(),
                        span: span.to(elem.span),
                        kind: TypeKind::Array {
                            elem: Box::new(elem),
                            len: Box::new(len_expr),
                        },
                    }
                }
            }
            TokenType::Star => {
                let elem = self.parse_type()?;
                TypeNode {
                    id: self.new_id(),
                    span: span.to(elem.span),
                    kind: TypeKind::Pointer {
                        elem: Box::new(elem),
                    },
                }
            }
            TokenType::Caret => {
                let elem = self.parse_type()?;
                TypeNode {
                    id: self.new_id(),
                    span: span.to(elem.span),
                    kind: TypeKind::VolatilePtr {
                        elem: Box::new(elem),
                    },
                }
            }
            _ => unreachable!(),
        };

        self.expect(TokenType::DotLBrace)?;
        self.parse_data_init(Some(Box::new(type_node)), span)
    }

    // --- Infix Sub-Routines ---

    fn parse_binary_expr(&mut self, left: Expr, token: Token) -> ParseResult<Expr> {
        let op = BinaryOperator::from_token(token.tag);
        let precedence = Precedence::from_token(token.tag);
        let right = self.parse_expression(precedence)?;
        Ok(Expr {
            id: self.new_id(),
            span: left.span.to(right.span),
            kind: ExprKind::Binary {
                lhs: Box::new(left),
                op,
                rhs: Box::new(right),
            },
        })
    }

    fn parse_field_access_expr(&mut self, left: Expr) -> ParseResult<Expr> {
        let field_token = self.expect(TokenType::Identifier)?;
        let field_id = self.intern_token(field_token);
        Ok(Expr {
            id: self.new_id(),
            span: left.span.to(field_token.span),
            kind: ExprKind::FieldAccess {
                lhs: Box::new(left),
                field: field_id,
            },
        })
    }

    fn parse_call_expr(&mut self, left: Expr) -> ParseResult<Expr> {
        let mut args = Vec::new();
        if !self.check(TokenType::RParen) {
            loop {
                args.push(self.parse_expression(Precedence::Lowest)?);
                if !self.match_token(&[TokenType::Comma]) {
                    break;
                }
            }
        }
        let end = self.expect(TokenType::RParen)?;
        Ok(Expr {
            id: self.new_id(),
            span: left.span.to(end.span),
            kind: ExprKind::Call {
                callee: Box::new(left),
                args,
            },
        })
    }

    fn parse_assignment_expr(&mut self, left: Expr, token: Token) -> ParseResult<Expr> {
        let op = AssignmentOperator::from_token(token.tag);
        let right = self.parse_expression(Precedence::Lowest)?;
        Ok(Expr {
            id: self.new_id(),
            span: left.span.to(right.span),
            kind: ExprKind::Assign {
                lhs: Box::new(left),
                op,
                rhs: Box::new(right),
            },
        })
    }

    fn parse_as_cast_expr(&mut self, left: Expr) -> ParseResult<Expr> {
        let target = self.parse_type()?;
        Ok(Expr {
            id: self.new_id(),
            span: left.span.to(target.span),
            kind: ExprKind::As {
                lhs: Box::new(left),
                target: Box::new(target),
            },
        })
    }

    fn parse_slice_or_index_expr(&mut self, left: Expr) -> ParseResult<Expr> {
        let mut start = None;
        let mut end = None;
        let mut is_range = false;
        let mut is_inclusive = false;

        if self.match_token(&[TokenType::DotDot]) {
            is_range = true;
            if !self.check(TokenType::RBracket) {
                end = Some(Box::new(self.parse_expression(Precedence::Lowest)?));
            }
        } else {
            start = Some(Box::new(self.parse_expression(Precedence::Lowest)?));

            if self.match_token(&[TokenType::DotDot]) {
                is_range = true;
                if !self.check(TokenType::RBracket) {
                    end = Some(Box::new(self.parse_expression(Precedence::Lowest)?));
                }
            } else if self.match_token(&[TokenType::DotDotEqual]) {
                is_range = true;
                is_inclusive = true;
                end = Some(Box::new(self.parse_expression(Precedence::Lowest)?));
            }
        }

        let rbracket = self.expect(TokenType::RBracket)?;
        let span = left.span.to(rbracket.span);

        if is_range {
            Ok(Expr {
                id: self.new_id(),
                span,
                kind: ExprKind::SliceOp {
                    lhs: Box::new(left),
                    start,
                    end,
                    is_inclusive,
                },
            })
        } else {
            Ok(Expr {
                id: self.new_id(),
                span,
                kind: ExprKind::IndexAccess {
                    lhs: Box::new(left),
                    index: start.unwrap(),
                },
            })
        }
    }

    fn parse_generic_instantiation_expr(&mut self, left: Expr) -> ParseResult<Expr> {
        let mut types = Vec::new();
        if !self.check(TokenType::RBracket) {
            loop {
                types.push(self.parse_type()?);
                if !self.match_token(&[TokenType::Comma]) {
                    break;
                }
                if self.check(TokenType::RBracket) {
                    break;
                }
            }
        }
        let rb = self.expect(TokenType::RBracket)?;
        Ok(Expr {
            id: self.new_id(),
            span: left.span.to(rb.span),
            kind: ExprKind::GenericInstantiation {
                target: Box::new(left),
                types,
            },
        })
    }

    // ==========================================
    //            Specific Expressions
    // ==========================================

    fn parse_data_init(
        &mut self,
        type_node: Option<Box<TypeNode>>,
        start_span: Span,
    ) -> ParseResult<Expr> {
        // 空数组兜底
        if self.check(TokenType::RBrace) {
            let rb = self.advance();
            return Ok(Expr {
                id: self.new_id(),
                span: start_span.to(rb.span),
                kind: ExprKind::DataInit {
                    type_node,
                    literal: DataLiteralKind::Array(vec![]),
                },
            });
        }

        // 1. 判断是否是 Struct 模式 (包含 field: value)
        let mut is_struct_mode = false;
        if self.check(TokenType::Identifier) {
            if self.stream.peek_nth(1).tag == TokenType::Colon {
                is_struct_mode = true;
            }
        }

        if is_struct_mode {
            let mut fields = Vec::new();
            while !self.check(TokenType::RBrace) && !self.check(TokenType::Eof) {
                let name = self.expect(TokenType::Identifier)?;
                let name_id = self.intern_token(name);
                self.expect(TokenType::Colon)?;
                let val = self.parse_expression(Precedence::Lowest)?;
                fields.push(StructFieldInit {
                    name: name_id,
                    value: val,
                    span: name.span,
                });
                if !self.match_token(&[TokenType::Comma]) {
                    break;
                }
            }
            let rb = self.expect(TokenType::RBrace)?;
            Ok(Expr {
                id: self.new_id(),
                span: start_span.to(rb.span),
                kind: ExprKind::DataInit {
                    type_node,
                    literal: DataLiteralKind::Struct(fields),
                },
            })
        } else {
            // 2. 此时大括号内是一个普通的表达式
            let first = self.parse_expression(Precedence::Lowest)?;

            // 模式 A: [Repeat] .{ 0; 1024 }
            if self.match_token(&[TokenType::Semicolon]) {
                let count = self.parse_expression(Precedence::Lowest)?;
                let rb = self.expect(TokenType::RBrace)?;
                Ok(Expr {
                    id: self.new_id(),
                    span: start_span.to(rb.span),
                    kind: ExprKind::DataInit {
                        type_node,
                        literal: DataLiteralKind::Repeat {
                            value: Box::new(first),
                            count: Box::new(count),
                        },
                    },
                })
            }
            // 模式 B: [Array] .{ 1, 2, 3 } (只要遇到了逗号，就一定是数组)
            else if self.match_token(&[TokenType::Comma]) {
                let mut elems = vec![first];
                while !self.check(TokenType::RBrace) && !self.check(TokenType::Eof) {
                    elems.push(self.parse_expression(Precedence::Lowest)?);
                    if !self.match_token(&[TokenType::Comma]) {
                        break;
                    }
                }
                let rb = self.expect(TokenType::RBrace)?;
                Ok(Expr {
                    id: self.new_id(),
                    span: start_span.to(rb.span),
                    kind: ExprKind::DataInit {
                        type_node,
                        literal: DataLiteralKind::Array(elems),
                    },
                })
            }
            // 模式 C: [Scalar] .{ 10 } 或者 Type.{ 1 << 12 }
            else {
                // 既没有逗号，也没有分号，那就是唯一的一个单值！直接包装为 Scalar！
                let rb = self.expect(TokenType::RBrace)?;
                Ok(Expr {
                    id: self.new_id(),
                    span: start_span.to(rb.span),
                    kind: ExprKind::DataInit {
                        type_node,
                        literal: DataLiteralKind::Scalar(Box::new(first)),
                    },
                })
            }
        }
    }

    fn parse_block_expr(&mut self, start_span: Span) -> ParseResult<Expr> {
        let mut stmts = Vec::new();
        let mut result = None;

        while !self.check(TokenType::RBrace) && !self.check(TokenType::Eof) {
            if self.check(TokenType::Defer) {
                let defer_t = self.advance();
                let expr = self.parse_expression(Precedence::Lowest)?;
                self.expect(TokenType::Semicolon)?;

                let defer_expr = Expr {
                    id: self.new_id(),
                    span: defer_t.span.to(self.stream.prev_span()),
                    kind: ExprKind::Defer {
                        expr: Box::new(expr),
                    },
                };
                stmts.push(Stmt {
                    id: self.new_id(),
                    span: defer_expr.span,
                    kind: StmtKind::ExprStmt(defer_expr),
                });
                continue;
            }

            let expr = match self.parse_expression(Precedence::Lowest) {
                Ok(e) => e,
                Err(_) => {
                    // 如果块内的表达式解析失败，就在块内部同步，跳到下一个分号
                    self.synchronize();
                    continue;
                }
            };

            // 判断当前表达式是否是自带大括号的“块级表达式”
            let is_block_like = matches!(
                &expr.kind,
                ExprKind::If { .. }
                    | ExprKind::Block { .. }
                    | ExprKind::Switch { .. }
                    | ExprKind::For { .. }
            );

            if self.match_token(&[TokenType::Semicolon]) {
                stmts.push(Stmt {
                    id: self.new_id(),
                    span: expr.span,
                    kind: StmtKind::ExprStmt(expr),
                });
            } else if self.check(TokenType::RBrace) {
                // 如果紧跟着是 }，说明这是整个 Block 的返回值
                result = Some(Box::new(expr));
            } else if is_block_like {
                // 如果是块级表达式，没有分号也是合法的独立语句
                stmts.push(Stmt {
                    id: self.new_id(),
                    span: expr.span,
                    kind: StmtKind::ExprStmt(expr),
                });
            } else {
                self.error_at_current("Expected semicolon".to_string());
                stmts.push(Stmt {
                    id: self.new_id(),
                    span: expr.span,
                    kind: StmtKind::ExprStmt(expr),
                });
            }
        }
        let rb = self.expect(TokenType::RBrace)?;
        let end_span = rb.span;
        Ok(Expr {
            id: self.new_id(),
            span: start_span.to(end_span),
            kind: ExprKind::Block { stmts, result },
        })
    }

    fn parse_if_expr(&mut self, start_span: Span) -> ParseResult<Expr> {
        self.expect(TokenType::LParen)?;
        let cond = self.parse_expression(Precedence::Lowest)?;
        self.expect(TokenType::RParen)?;
        let then_branch = self.parse_expression(Precedence::Lowest)?;
        let mut else_branch = None;
        if self.match_token(&[TokenType::Else]) {
            else_branch = Some(Box::new(self.parse_expression(Precedence::Lowest)?));
        }
        let end = if let Some(ref e) = else_branch {
            e.span
        } else {
            then_branch.span
        };
        Ok(Expr {
            id: self.new_id(),
            span: start_span.to(end),
            kind: ExprKind::If {
                cond: Box::new(cond),
                then_branch: Box::new(then_branch),
                else_branch,
            },
        })
    }

    fn parse_for_expr(&mut self, start_span: Span) -> ParseResult<Expr> {
        self.expect(TokenType::LParen)?;
        let mut init = None;
        if !self.check(TokenType::Semicolon) {
            init = Some(Box::new(self.parse_expression(Precedence::Lowest)?));
        }
        self.expect(TokenType::Semicolon)?;

        let mut cond = None;
        if !self.check(TokenType::Semicolon) {
            cond = Some(Box::new(self.parse_expression(Precedence::Lowest)?));
        }
        self.expect(TokenType::Semicolon)?;

        let mut post = None;
        if !self.check(TokenType::RParen) {
            post = Some(Box::new(self.parse_expression(Precedence::Lowest)?));
        }
        self.expect(TokenType::RParen)?;

        let body = self.parse_expression(Precedence::Lowest)?;
        Ok(Expr {
            id: self.new_id(),
            span: start_span.to(body.span),
            kind: ExprKind::For {
                init,
                cond,
                post,
                body: Box::new(body),
            },
        })
    }

    fn parse_switch_expr(&mut self, start_span: Span) -> ParseResult<Expr> {
        self.expect(TokenType::LParen)?;
        let target = self.parse_expression(Precedence::Lowest)?;
        self.expect(TokenType::RParen)?;
        self.expect(TokenType::LBrace)?;

        let mut cases = Vec::new();
        let mut default_case = None;

        while !self.check(TokenType::RBrace) && !self.check(TokenType::Eof) {
            if self.match_token(&[TokenType::Else]) {
                self.expect(TokenType::Arrow)?;
                default_case = Some(Box::new(self.parse_switch_body()?));
                self.match_token(&[TokenType::Comma]);
                continue;
            }

            let mut patterns = Vec::new();
            loop {
                let start = self.parse_expression(Precedence::Lowest)?;
                if self.match_token(&[TokenType::DotDot]) {
                    let end = self.parse_expression(Precedence::Lowest)?;
                    patterns.push(SwitchPattern::Range {
                        start,
                        end,
                        inclusive: false,
                    });
                } else if self.match_token(&[TokenType::DotDotEqual]) {
                    let end = self.parse_expression(Precedence::Lowest)?;
                    patterns.push(SwitchPattern::Range {
                        start,
                        end,
                        inclusive: true,
                    });
                } else {
                    patterns.push(SwitchPattern::Value(start));
                }

                if !self.match_token(&[TokenType::Comma]) {
                    break;
                }
                if self.check(TokenType::Arrow) {
                    break;
                }
            }
            self.expect(TokenType::Arrow)?;
            let body = self.parse_switch_body()?;
            self.match_token(&[TokenType::Comma]);
            cases.push(SwitchCase {
                patterns,
                body,
                span: start_span, /* imprecise */
            });
        }
        let rb = self.expect(TokenType::RBrace)?;
        Ok(Expr {
            id: self.new_id(),
            span: start_span.to(rb.span),
            kind: ExprKind::Switch {
                target: Box::new(target),
                cases,
                default_case,
            },
        })
    }

    fn parse_switch_body(&mut self) -> ParseResult<Expr> {
        if self.check(TokenType::LBrace) {
            let t = self.advance();
            self.parse_block_expr(t.span)
        } else {
            self.parse_expression(Precedence::Lowest)
        }
    }

    fn parse_match_expr(&mut self, start_span: Span) -> ParseResult<Expr> {
        self.expect(TokenType::LParen)?;
        let target = self.parse_expression(Precedence::Lowest)?;
        self.expect(TokenType::RParen)?;
        self.expect(TokenType::LBrace)?;

        let mut arms = Vec::new();

        while !self.check(TokenType::RBrace) && !self.check(TokenType::Eof) {
            let arm_start = self.peek().span;

            let pattern = if self.match_token(&[TokenType::Else]) {
                MatchPattern::CatchAll(arm_start)
            } else {
                let mut target_type = None;
                let variant_name;

                // 语法1: 省略前缀 `.Ok`
                if self.match_token(&[TokenType::Dot]) {
                    let v_tok = self.expect(TokenType::Identifier)?;
                    variant_name = self.intern_token(v_tok);
                } else {
                    // 语法2: 带有显式前缀，尝试解析为 Type
                    let ty = self.parse_type()?;

                    if self.match_token(&[TokenType::Dot]) {
                        // 情况A: 带泛型的类型被截断，比如 `Result[i32, i32] . Ok`
                        let v_tok = self.expect(TokenType::Identifier)?;
                        variant_name = self.intern_token(v_tok);
                        target_type = Some(Box::new(ty));
                    } else if let TypeKind::Path {
                        mut segments,
                        generics,
                    } = ty.kind
                    {
                        // 情况B: 无泛型，parse_type 会把 `ASTNode.Boolean` 整体当作路径吃掉
                        if generics.is_empty() && segments.len() >= 2 {
                            // 把最后一段弹出来当做变体名
                            variant_name = segments.pop().unwrap();

                            // 剩下的重新组装回 Target Type
                            let remain_ty = TypeNode {
                                id: self.new_id(),
                                span: ty.span, // TODO: 近似
                                kind: TypeKind::Path {
                                    segments,
                                    generics: vec![],
                                },
                            };
                            target_type = Some(Box::new(remain_ty));
                        } else {
                            self.add_error(ty.span, "Invalid ADT match pattern".into());
                            return Err(());
                        }
                    } else {
                        self.add_error(ty.span, "Expected variant pattern".into());
                        return Err(());
                    }
                }

                // 判断是否有数据绑定: `: val`
                let mut binding = None;
                if self.match_token(&[TokenType::Colon]) {
                    let bind_token = self.expect(TokenType::Identifier)?;
                    binding = Some(self.intern_token(bind_token));
                }

                let pat_span = arm_start.to(self.stream.prev_span());
                MatchPattern::Variant {
                    target_type,
                    variant_name,
                    binding,
                    span: pat_span,
                }
            };

            self.expect(TokenType::Arrow)?;

            let body = self.parse_switch_body()?;
            self.match_token(&[TokenType::Comma]);

            arms.push(MatchArm {
                span: arm_start.to(body.span),
                pattern,
                body,
            });
        }

        let rb = self.expect(TokenType::RBrace)?;

        Ok(Expr {
            id: self.new_id(),
            span: start_span.to(rb.span),
            kind: ExprKind::Match {
                target: Box::new(target),
                arms,
            },
        })
    }

    fn parse_decl_expr(&mut self, start_token: Token) -> ParseResult<Expr> {
        let tag = start_token.tag;

        let name = self.expect(TokenType::Identifier)?;
        let name_id = self.intern_token(name);

        if self.match_token(&[TokenType::Colon]) {
            let err_span = self.stream.prev_span();
            // 假装解析掉类型，防止后续连锁报错
            let _ = self.parse_type();

            self.context.struct_error(err_span, "type annotations on the left side of declarations are strictly forbidden in Kern")
                .with_hint("Kern uses explicit constructor syntax on the right side")
                .with_hint("try rewriting this as: `let x = mut Type.{ ... };`")
                .emit();
            self.panic_mode = true;
        }

        self.expect(TokenType::Assign)?;
        let init = self.parse_expression(Precedence::Lowest)?;
        let span = start_token.span.to(init.span);

        match tag {
            TokenType::Static => Ok(Expr {
                id: self.new_id(),
                span,
                kind: ExprKind::Static {
                    name: name_id,
                    init: Box::new(init),
                },
            }),
            TokenType::Let | TokenType::Const => Ok(Expr {
                id: self.new_id(),
                span,
                kind: ExprKind::Let {
                    name: name_id,
                    init: Box::new(init),
                },
            }),
            _ => unreachable!(),
        }
    }
}

// ==========================================
//               Declarations
// ==========================================
impl<'a> Parser<'a> {
    fn parse_generic_params(&mut self) -> ParseResult<Vec<GenericParam>> {
        if !self.match_token(&[TokenType::LBracket]) {
            return Ok(Vec::new());
        }
        let mut params = Vec::new();
        while !self.check(TokenType::RBracket) && !self.check(TokenType::Eof) {
            let name = self.expect(TokenType::Identifier)?;
            let name_id = self.intern_token(name);
            let mut span = name.span;
            let mut constraints = Vec::new();

            if self.match_token(&[TokenType::Colon]) {
                loop {
                    let con = self.parse_type()?;
                    span = span.to(con.span);
                    constraints.push(con);
                    if !self.match_token(&[TokenType::Plus]) {
                        break;
                    }
                }
            }
            params.push(GenericParam {
                name: name_id,
                constraints,
                span,
            });
            if !self.match_token(&[TokenType::Comma]) {
                break;
            }
        }
        self.expect(TokenType::RBracket)?;
        Ok(params)
    }

    fn parse_func_params(&mut self) -> ParseResult<(Vec<FuncParam>, bool)> {
        self.expect(TokenType::LParen)?;
        let mut params = Vec::new();
        let mut is_variadic = false;

        while !self.check(TokenType::RParen) && !self.check(TokenType::Eof) {
            if self.match_token(&[TokenType::Ellipsis]) {
                is_variadic = true;
                break;
            }

            let name = self.expect(TokenType::Identifier)?;
            let name_id = self.intern_token(name);
            self.expect(TokenType::Colon)?;
            let type_node = self.parse_type()?;

            params.push(FuncParam {
                name: name_id,
                span: name.span.to(type_node.span),
                type_node,
            });

            if !self.match_token(&[TokenType::Comma]) {
                break;
            }
        }
        self.expect(TokenType::RParen)?;
        Ok((params, is_variadic))
    }

    // Top Level
    pub fn parse_module(&mut self) -> ParseResult<Module> {
        let mut decls = Vec::new();
        while !self.check(TokenType::Eof) {
            match self.parse_decl() {
                Ok(Some(decl)) => decls.push(decl),
                Ok(None) => {} // Skipped
                Err(_) => {
                    // Error already reported, sync and continue
                    self.synchronize();
                }
            }
        }
        Ok(Module {
            path: "test.kn".to_string(),
            decls,
        })
    }

    fn parse_decl(&mut self) -> ParseResult<Option<Decl>> {
        let is_pub = self.match_token(&[TokenType::Pub]);
        let start_span = if is_pub {
            self.stream.prev_span()
        } else {
            self.peek().span
        };
        let is_extern = self.match_token(&[TokenType::Extern]);

        if is_extern {
            if self.check(TokenType::LBrace) || self.check(TokenType::StringLiteral) {
                if is_pub {
                    self.add_error(start_span, "Extern blocks cannot be pub".to_string());
                }
                return Ok(Some(self.parse_extern_block(start_span)?));
            }
        }

        let token = self.peek();
        match token.tag {
            TokenType::Fn => Ok(Some(self.parse_fn_decl(start_span, is_pub, is_extern)?)),
            TokenType::Type => Ok(Some(
                self.parse_type_alias_decl(start_span, is_pub, is_extern)?,
            )),
            TokenType::Const | TokenType::Static => Ok(Some(
                self.parse_global_var_decl(start_span, is_pub, is_extern)?,
            )),
            TokenType::Use => Ok(Some(self.parse_use_decl(start_span, is_pub)?)),
            TokenType::Impl => {
                if is_pub {
                    self.add_error(start_span, "impl blocks cannot be pub".to_string());
                }
                Ok(Some(self.parse_impl_decl(start_span)?))
            }
            TokenType::Semicolon => {
                self.advance();
                Ok(None)
            }
            TokenType::Eof => Ok(None),
            _ => {
                let txt = self
                    .context
                    .source_manager
                    .slice_source(token.span)
                    .to_string();
                self.add_error(token.span, format!("Expected declaration, found '{}'", txt));
                Err(())
            }
        }
    }

    fn parse_fn_decl(&mut self, start: Span, is_pub: bool, is_extern: bool) -> ParseResult<Decl> {
        self.advance(); // fn
        let name = self.expect(TokenType::Identifier)?;
        let name_id = self.intern_token(name);

        let generics = self.parse_generic_params()?;
        let (params, is_variadic) = self.parse_func_params()?;

        if is_variadic && !is_extern {
            self.add_error(start, "Variadic args only allowed in extern".to_string());
        }

        let ret_type = self.parse_type()?;

        let body = if is_extern {
            self.expect(TokenType::Semicolon)?;
            None
        } else {
            let brace = self.expect(TokenType::LBrace)?;
            Some(Box::new(self.parse_block_expr(brace.span)?))
        };

        let end = if let Some(ref b) = body {
            b.span
        } else {
            self.stream.prev_span()
        };

        Ok(Decl {
            id: self.new_id(),
            span: start.to(end),
            name: name_id,
            is_pub,
            kind: DeclKind::Function {
                generics,
                params,
                ret_type,
                body,
                is_extern,
                is_variadic,
            },
        })
    }

    fn parse_extern_block(&mut self, start: Span) -> ParseResult<Decl> {
        let mut abi = None;
        if self.check(TokenType::StringLiteral) {
            let t = self.advance();
            let sid = self.parse_string_literal(t)?;
            abi = Some(self.context.resolve(sid).to_string());
        }
        self.expect(TokenType::LBrace)?;

        let mut decls = Vec::new();
        while !self.check(TokenType::RBrace) && !self.check(TokenType::Eof) {
            let is_pub = self.match_token(&[TokenType::Pub]);
            let d_start = if is_pub {
                self.stream.prev_span()
            } else {
                self.peek().span
            };

            if self.check(TokenType::Fn) {
                decls.push(self.parse_fn_decl(d_start, is_pub, true)?);
            } else if self.check(TokenType::Static) {
                decls.push(self.parse_global_var_decl(d_start, is_pub, true)?);
            } else {
                self.error_at_current("Only fn and static allowed in extern".to_string());
                self.synchronize();
            }
        }
        let end = self.expect(TokenType::RBrace)?;
        let name = self.context.intern("extern_block");
        Ok(Decl {
            id: self.new_id(),
            span: start.to(end.span),
            name,
            is_pub: false,
            kind: DeclKind::ExternBlock { abi, decls },
        })
    }

    fn parse_impl_decl(&mut self, start: Span) -> ParseResult<Decl> {
        self.advance(); // impl
        let generics = self.parse_generic_params()?;
        let target_type = self.parse_type()?;
        let mut trait_type = None;
        if self.match_token(&[TokenType::Colon]) {
            trait_type = Some(self.parse_type()?);
        }
        self.expect(TokenType::LBrace)?;

        let mut decls = Vec::new();
        while !self.check(TokenType::RBrace) && !self.check(TokenType::Eof) {
            let is_pub = self.match_token(&[TokenType::Pub]);
            let d_start = if is_pub {
                self.stream.prev_span()
            } else {
                self.peek().span
            };
            if self.check(TokenType::Fn) {
                decls.push(self.parse_fn_decl(d_start, is_pub, false)?);
            } else {
                self.error_at_current("Only fn allowed in impl".to_string());
                self.synchronize();
            }
        }
        let end = self.expect(TokenType::RBrace)?;
        let name = self.context.intern("impl");
        Ok(Decl {
            id: self.new_id(),
            span: start.to(end.span),
            name,
            is_pub: false,
            kind: DeclKind::Impl {
                generics,
                target_type,
                trait_type,
                decls,
            },
        })
    }

    fn parse_global_var_decl(
        &mut self,
        start: Span,
        is_pub: bool,
        is_extern: bool,
    ) -> ParseResult<Decl> {
        let kw = self.advance();
        let is_static = kw.tag == TokenType::Static;

        let name = self.expect(TokenType::Identifier)?;
        let name_id = self.intern_token(name);

        // 🌟 护城河：全局变量同样拦截左侧冒号
        if self.match_token(&[TokenType::Colon]) {
            let err_span = self.stream.prev_span();
            self.add_error(err_span, "Global variables no longer support left-side type annotations. Use explicit constructors: `static X = Type.{ value };`".to_string());
            let _ = self.parse_type();
        }

        // Init
        let value;
        if self.match_token(&[TokenType::Assign]) {
            value = self.parse_expression(Precedence::Lowest)?;
        } else {
            // 不再自动补齐，无论是 extern 还是普通全局变量，都必须带 =
            self.add_error(
                start,
                "Global/extern vars must be initialized (use `= Type.{undef};` for externs)"
                    .to_string(),
            );
            return Err(());
        }
        self.expect(TokenType::Semicolon)?;
        let end = self.stream.prev_span();

        Ok(Decl {
            id: self.new_id(),
            span: start.to(end),
            name: name_id,
            is_pub,
            // ✅ 瘦身后的 Var
            kind: DeclKind::Var {
                value,
                is_static,
                is_extern,
            },
        })
    }

    fn parse_type_alias_decl(
        &mut self,
        start: Span,
        is_pub: bool,
        is_extern: bool,
    ) -> ParseResult<Decl> {
        self.advance(); // 消费 `type`
        let name = self.expect(TokenType::Identifier)?;
        let name_id = self.intern_token(name);

        // 解析泛型参数 [T]
        let generics = self.parse_generic_params()?;

        // 解析约束界限 `: Reader + Writer`
        let mut bounds = Vec::new();
        if self.match_token(&[TokenType::Colon]) {
            loop {
                bounds.push(self.parse_type()?);
                if !self.match_token(&[TokenType::Plus]) {
                    break;
                }
            }
        }

        self.expect(TokenType::Assign)?;
        let target = self.parse_type()?;
        self.expect(TokenType::Semicolon)?;

        let end = self.stream.prev_span();

        Ok(Decl {
            id: self.new_id(),
            span: start.to(end),
            name: name_id,
            is_pub,
            kind: DeclKind::TypeAlias {
                generics,
                bounds,
                target,
                is_extern,
            },
        })
    }

    fn parse_use_decl(&mut self, start: Span, is_pub: bool) -> ParseResult<Decl> {
        self.advance(); // 消费 `use`

        let mut kind = UsePathKind::Absolute;
        if self.match_token(&[TokenType::Dot]) {
            kind = UsePathKind::Relative;
        } else if self.match_token(&[TokenType::DotDot]) {
            kind = UsePathKind::Super;
        }

        let mut path = Vec::new();
        let target: UseTarget;
        loop {
            // 情况 1: 遇到纯粹的 `{` (比如 `use .{ ... }` 或者路径已经被处理完)
            if self.match_token(&[TokenType::LBrace]) {
                target = self.parse_use_members()?;
                break;
            }
            // 情况 2: 核心修复，处理被 Lexer 捏合在一起的 `.{` Token
            if self.match_token(&[TokenType::DotLBrace]) {
                target = self.parse_use_members()?;
                break;
            }

            // 正常读取路径段
            let id = self.expect(TokenType::Identifier)?;
            path.push(self.intern_token(id));

            // 在路径段之后再次检查是否直接跟着 `.{`
            if self.match_token(&[TokenType::DotLBrace]) {
                target = self.parse_use_members()?;
                break;
            } else if self.match_token(&[TokenType::Dot]) {
                // 如果后面跟着普通的点号，继续循环读取下一个段
                continue;
            } else {
                // 没有任何后缀，跳出循环，按普通模块导入处理
                let mut alias = None;
                if self.match_token(&[TokenType::As]) {
                    let a = self.expect(TokenType::Identifier)?;
                    alias = Some(self.intern_token(a));
                }
                target = UseTarget::Module(alias);
                break;
            }
        }

        self.expect(TokenType::Semicolon)?;

        let name = if let Some(&last) = path.last() {
            last
        } else {
            self.context.intern("root")
        };

        Ok(Decl {
            id: self.new_id(),
            span: start.to(self.stream.prev_span()),
            name,
            is_pub,
            kind: DeclKind::Use {
                kind,
                path,
                target,
                is_reexport: is_pub,
            },
        })
    }

    // 辅助方法：专门解析大括号里的重导出成员 `{ Point, new_point as np }`
    fn parse_use_members(&mut self) -> ParseResult<UseTarget> {
        let mut members = Vec::new();
        while !self.check(TokenType::RBrace) && !self.check(TokenType::Eof) {
            let m_tok = self.expect(TokenType::Identifier)?;
            let m_id = self.intern_token(m_tok);
            let mut alias = None;
            if self.match_token(&[TokenType::As]) {
                let a_tok = self.expect(TokenType::Identifier)?;
                alias = Some(self.intern_token(a_tok));
            }
            members.push(UseMember {
                name: m_id,
                alias,
                span: m_tok.span,
            });
            if !self.match_token(&[TokenType::Comma]) {
                break;
            }
        }
        self.expect(TokenType::RBrace)?;
        Ok(UseTarget::Members(members))
    }

    /// 将一个路径表达式强制转换为 TypeNode（用于处理 Type.{...} 的左侧）
    fn expr_to_type(&mut self, expr: Expr) -> ParseResult<TypeNode> {
        match expr.kind {
            ExprKind::Identifier(id) => Ok(TypeNode {
                id: self.new_id(),
                span: expr.span,
                kind: TypeKind::Path {
                    segments: vec![id],
                    generics: Vec::new(),
                },
            }),
            ExprKind::FieldAccess { lhs, field } => {
                let mut base = self.expr_to_type(*lhs)?;
                if let TypeKind::Path {
                    ref mut segments, ..
                } = base.kind
                {
                    segments.push(field);
                    base.span = base.span.to(expr.span);
                    Ok(base)
                } else {
                    self.add_error(expr.span, "Invalid path used as type".to_string());
                    Err(())
                }
            }
            ExprKind::GenericInstantiation { target, types } => {
                let mut base = self.expr_to_type(*target)?;
                if let TypeKind::Path {
                    ref mut generics, ..
                } = base.kind
                {
                    *generics = types;
                    base.span = base.span.to(expr.span);
                    Ok(base)
                } else {
                    self.add_error(expr.span, "Invalid generic type target".to_string());
                    Err(())
                }
            }
            _ => {
                self.add_error(
                    expr.span,
                    "Invalid expression used as a type prefix".to_string(),
                );
                Err(())
            }
        }
    }
}
