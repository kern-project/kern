use super::lexer::Lexer;
use super::token::{Token, TokenType};
use crate::utils::Span;
use std::collections::VecDeque;

pub struct TokenStream<'a> {
    lexer: Lexer<'a>,
    /// 缓冲区，用于支持无限 Lookahead
    buffer: VecDeque<Token>,
    /// 记录上一个被消费 Token 的 Span (用于错误报告)
    last_span: Span,
}

impl<'a> TokenStream<'a> {
    pub fn new(lexer: Lexer<'a>) -> Self {
        Self {
            lexer,
            buffer: VecDeque::new(),
            last_span: Span::default(),
        }
    }

    /// 填充缓冲区直到至少包含 n+1 个元素
    /// 或者直到遇到 EOF
    fn fill_buffer(&mut self, n: usize) {
        while self.buffer.len() <= n {
            let token = self.lexer.next();
            let is_eof = token.tag == TokenType::Eof;
            self.buffer.push_back(token);

            // 优化：一旦遇到 EOF，就没必要继续填充了
            // 后续的 peek 都会落入 EOF 处理逻辑
            if is_eof {
                break;
            }
        }
    }

    /// 查看第 N 个 Token (不消耗)
    /// n=0 是当前 Token，n=1 是下一个，以此类推
    pub fn peek_nth(&mut self, n: usize) -> Token {
        self.fill_buffer(n);

        // 如果请求的索引超出了缓冲区长度（说明中间遇到了 EOF）
        // 直接返回缓冲区最后一个元素（即 EOF）
        if n >= self.buffer.len() {
            return self.buffer.back().copied().unwrap_or_else(|| {
                // 理论上不可能进入这里，除非 fill_buffer 逻辑有误
                // 造一个假的 EOF
                Token {
                    tag: TokenType::Eof,
                    span: self.last_span,
                }
            });
        }

        self.buffer[n]
    }

    /// 查看当前 Token (Lookahead 0)
    pub fn peek(&mut self) -> Token {
        self.peek_nth(0)
    }

    /// 查看下一个 Token (Lookahead 1)
    pub fn peek_next(&mut self) -> Token {
        self.peek_nth(1)
    }

    /// 消耗并返回当前的 Token
    pub fn bump(&mut self) -> Token {
        // 确保缓冲区里至少有一个元素
        if self.buffer.is_empty() {
            let t = self.lexer.next();
            self.last_span = t.span;
            return t;
        }

        // 从队首弹出
        let token = self.buffer.pop_front().unwrap();
        self.last_span = token.span;
        token
    }

    /// 检查当前 Token 类型是否匹配，但不消耗
    pub fn check(&mut self, tag: TokenType) -> bool {
        self.peek().tag == tag
    }

    /// 如果当前 Token 匹配预期类型，则消耗它并返回 true；否则返回 false
    pub fn match_token(&mut self, tag: TokenType) -> bool {
        if self.check(tag) {
            self.bump();
            true
        } else {
            false
        }
    }

    /// 尝试消耗一个 Token，如果类型匹配则返回 Some(Token)，否则返回 None
    pub fn eat(&mut self, tag: TokenType) -> Option<Token> {
        if self.check(tag) {
            Some(self.bump())
        } else {
            None
        }
    }

    /// 获取上一个被消费 Token 的 Span
    /// 常用于：当 `expect` 失败或者解析到末尾时，需要知道“上一个有效位置”在哪
    pub fn prev_span(&self) -> Span {
        self.last_span
    }

    /// 判断是否到达文件末尾
    pub fn is_eof(&mut self) -> bool {
        self.peek().tag == TokenType::Eof
    }
}
