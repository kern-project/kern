#![allow(unused)]
use super::token::{Token, TokenType};
use crate::utils::{FileId, Span};
use std::str;

pub struct Lexer<'a> {
    source: &'a [u8],
    file_id: FileId,
    start: usize,   // 当前 Token 的起始位置
    current: usize, // 扫描探针的当前位置
}

impl<'a> Lexer<'a> {
    pub fn new(source: &'a str, file_id: FileId) -> Self {
        Self {
            source: source.as_bytes(),
            file_id,
            start: 0,
            current: 0,
        }
    }

    /// 获取下一个 Token
    pub fn next(&mut self) -> Token {
        self.skip_whitespace();

        self.start = self.current;

        let c = match self.advance() {
            Some(c) => c,
            None => return self.make_token(TokenType::Eof),
        };

        match c {
            // 标识符或关键字 (a-z, A-Z, _)
            b'a'..=b'z' | b'A'..=b'Z' | b'_' => self.scan_identifier(),

            // 数字 (0-9)
            b'0'..=b'9' => self.scan_number(),

            // 字符串
            b'"' => self.scan_string(),

            // 字符
            b'\'' => self.scan_char(),

            // 运算符和标点
            b'(' => self.make_token(TokenType::LParen),
            b')' => self.make_token(TokenType::RParen),
            b'{' => self.make_token(TokenType::LBrace),
            b'}' => self.make_token(TokenType::RBrace),
            b'[' => self.make_token(TokenType::LBracket),
            b']' => self.make_token(TokenType::RBracket),
            b',' => self.make_token(TokenType::Comma),
            b';' => self.make_token(TokenType::Semicolon),
            b':' => self.make_token(TokenType::Colon),
            b'#' => self.make_token(TokenType::Hash),
            b'@' => self.make_token(TokenType::At),

            // Dot 家族处理
            b'.' => {
                // 1. 检查 .. 开头的情况
                if self.match_char(b'.') {
                    // 检查是否是 ... (变长参数)
                    if self.match_char(b'.') {
                        return self.make_token(TokenType::Ellipsis);
                    }
                    // 检查是否是 ..= (闭区间范围)
                    else if self.match_char(b'=') {
                        return self.make_token(TokenType::DotDotEqual);
                    }
                    // 否则就是普通的 .. (半开范围)
                    return self.make_token(TokenType::DotDot);
                }
                // 2. 检查 .* (指针解引用)
                else if self.match_char(b'*') {
                    return self.make_token(TokenType::DotStar);
                }
                // 3. 检查 .& (取地址)
                else if self.match_char(b'&') {
                    return self.make_token(TokenType::DotAmpersand);
                }
                // 4. 检查 .[ (切片/数组索引)
                else if self.match_char(b'[') {
                    return self.make_token(TokenType::DotLBracket);
                }
                // 5. 检查 .{ (匿名初始化)
                else if self.match_char(b'{') {
                    return self.make_token(TokenType::DotLBrace);
                }
                // 6. 普通点 . (字段访问)
                else {
                    return self.make_token(TokenType::Dot);
                }
            }

            b'+' => self.match_assign(TokenType::Plus, TokenType::PlusAssign),
            b'-' => self.match_assign(TokenType::Minus, TokenType::MinusAssign),
            b'*' => self.match_assign(TokenType::Star, TokenType::StarAssign),
            b'%' => self.match_assign(TokenType::Percent, TokenType::PercentAssign),
            b'/' => {
                // 1. 检查是否是单行注释 //
                if self.match_char(b'/') {
                    self.skip_comment_line();
                    self.next() // 递归调用，寻找下一个有效 Token
                }
                // 2. 检查是否是多行注释 /*
                else if self.match_char(b'*') {
                    // 进入这里时，已经消耗了 "/*"
                    if !self.skip_comment_block() {
                        return self.make_token(TokenType::Illegal);
                    }
                    self.next() // 递归调用
                }
                // 3. 检查是否是除法赋值 /=
                else if self.match_char(b'=') {
                    self.make_token(TokenType::SlashAssign)
                }
                // 4. 普通除号 /
                else {
                    self.make_token(TokenType::Slash)
                }
            }

            b'=' => {
                if self.match_char(b'=') {
                    self.make_token(TokenType::EqualEqual)
                } else if self.match_char(b'>') {
                    self.make_token(TokenType::Arrow)
                } else {
                    self.make_token(TokenType::Assign)
                }
            }
            b'!' => {
                if self.match_char(b'=') {
                    self.make_token(TokenType::NotEqual)
                } else {
                    self.make_token(TokenType::Bang)
                }
            }
            b'<' => {
                // 检查是否是左移 <<
                if self.match_char(b'<') {
                    // 检查是否是左移赋值 <<=
                    if self.match_char(b'=') {
                        return self.make_token(TokenType::LShiftAssign);
                    }
                    return self.make_token(TokenType::LShift);
                }
                // 检查是否是小于等于 <=
                if self.match_char(b'=') {
                    return self.make_token(TokenType::LessEqual);
                }
                // 否则就是小于 <
                self.make_token(TokenType::LessThan)
            }
            b'>' => {
                // 检查是否是右移 >>
                if self.match_char(b'>') {
                    // 检查是否是右移赋值 >>=
                    if self.match_char(b'=') {
                        return self.make_token(TokenType::RShiftAssign);
                    }
                    return self.make_token(TokenType::RShift);
                }
                // 检查是否是大于等于 >=
                if self.match_char(b'=') {
                    return self.make_token(TokenType::GreaterEqual);
                }
                // 否则就是大于 >
                self.make_token(TokenType::GreaterThan)
            }

            // 位运算
            b'&' => self.match_assign(TokenType::Ampersand, TokenType::AmpersandAssign),
            b'|' => self.match_assign(TokenType::Pipe, TokenType::PipeAssign),
            b'^' => self.match_assign(TokenType::Caret, TokenType::CaretAssign),
            b'~' => self.make_token(TokenType::Tilde),

            _ => self.make_token(TokenType::Illegal),
        }
    }

    // === 核心扫描逻辑 ===

    fn scan_identifier(&mut self) -> Token {
        while is_alpha_numeric(self.peek()) {
            self.advance();
        }

        let text = &self.source[self.start..self.current];
        // 查表
        let tag = resolve_keyword(text);
        self.make_token(tag)
    }

    fn scan_number(&mut self) -> Token {
        // 1. 处理进制前缀 (0x, 0b, 0o)
        // source[start] 一定是数字，因为进入此函数前已经判断过
        if self.source[self.start] == b'0' {
            let next_char = self.peek();
            match next_char {
                b'x' | b'X' => {
                    self.advance(); // 吃掉 'x'
                    self.consume_digits(16);
                    return self.make_token(TokenType::IntLiteral);
                }
                b'b' | b'B' => {
                    self.advance(); // 吃掉 'b'
                    self.consume_digits(2);
                    return self.make_token(TokenType::IntLiteral);
                }
                b'o' | b'O' => {
                    self.advance(); // 吃掉 'o'
                    self.consume_digits(8);
                    return self.make_token(TokenType::IntLiteral);
                }
                _ => {
                    // 只是一个普通的 0，或者 0.xxxx，或者 0123
                    // 继续往下走，进入十进制逻辑
                }
            }
        }

        // 2. 扫描整数部分 (十进制)
        self.consume_digits(10);

        // 3. 处理小数部分 (Float)
        // 关键逻辑：如果是 '.'，必须确认 '.' 后面紧跟着数字，才算是浮点数。
        // 否则可能是 1.method() 或者是 1..10 (Range)
        if self.peek() == b'.' && is_digit(self.peek_next()) {
            self.advance(); // 吃掉 '.'
            self.consume_digits(10); // 扫描小数部分

            // 扫描完小数后，还可以继续跟指数部分，如 1.2e10
            self.try_scan_exponent();
            return self.make_token(TokenType::FloatLiteral);
        }

        // 4. 处理没有小数点的指数部分 (如 1e10)
        if self.try_scan_exponent() {
            return self.make_token(TokenType::FloatLiteral);
        }

        // 既没有小数点，也没有指数，就是普通的整数
        self.make_token(TokenType::IntLiteral)
    }

    fn try_scan_exponent(&mut self) -> bool {
        let c = self.peek();
        if c == b'e' || c == b'E' {
            self.advance(); // 吃掉 'e'

            // 指数部分可以有正负号
            let next_c = self.peek();
            if next_c == b'+' || next_c == b'-' {
                self.advance();
            }

            self.consume_digits(10);
            return true;
        }
        false
    }

    fn consume_digits(&mut self, radix: u32) {
        loop {
            let c = self.peek();
            if c == b'_' {
                self.advance();
                continue;
            }

            let is_valid = match radix {
                2 => is_bin_digit(c),
                8 => is_oct_digit(c),
                10 => is_digit(c),
                16 => is_hex_digit(c),
                _ => false,
            };

            if is_valid {
                self.advance();
            } else {
                break;
            }
        }
    }

    fn scan_string(&mut self) -> Token {
        loop {
            let char = self.peek();
            match char {
                0 => return self.make_token(TokenType::Illegal), // 未闭合就结束
                b'"' => {
                    self.advance(); // 吞掉右引号
                    break;
                }
                b'\\' => {
                    self.advance(); // 跳过转义
                    self.advance();
                }
                _ => {
                    self.advance();
                }
            }
        }
        self.make_token(TokenType::StringLiteral)
    }

    fn scan_char(&mut self) -> Token {
        // 刚吃掉了左边的单引号 '，现在处于字符内容的第一个字节
        let c = self.peek();

        // 1. 处理转义字符 (以 \ 开头)
        if c == b'\\' {
            self.advance(); // 吃掉反斜杠 '\'

            let escaped = self.peek();
            match escaped {
                // 简单单字符转义
                b'n' | b'r' | b't' | b'\\' | b'\'' | b'\"' | b'0' => {
                    self.advance();
                }
                // 十六进制转义: \xNN
                b'x' => {
                    self.advance(); // 吃掉 'x'
                    if !self.consume_hex_digits(2) {
                        return self.make_token(TokenType::Illegal);
                    }
                }
                // Unicode 转义: \u{...}
                b'u' => {
                    self.advance(); // 吃掉 'u'
                    if self.peek() != b'{' {
                        return self.make_token(TokenType::Illegal);
                    }
                    self.advance(); // 吃掉 '{'

                    let mut length = 0;
                    while is_hex_digit(self.peek()) {
                        self.advance();
                        length += 1;
                        if length > 6 {
                            return self.make_token(TokenType::Illegal);
                        }
                    }

                    if self.peek() != b'}' {
                        return self.make_token(TokenType::Illegal);
                    }
                    self.advance(); // 吃掉 '}'
                }
                _ => return self.make_token(TokenType::Illegal),
            }
        }
        // 2. 处理普通字符 (包括 UTF-8 多字节字符)
        else if c != b'\'' && c != 0 {
            let len = utf8_byte_sequence_length(c);
            if len == 0 {
                return self.make_token(TokenType::Illegal);
            }

            for _ in 0..len {
                self.advance();
            }
        }
        // 3. 空字符 '' 或者直接遇到 EOF
        else {
            return self.make_token(TokenType::Illegal);
        }

        // 4. 必须以单引号闭合
        if self.match_char(b'\'') {
            self.make_token(TokenType::CharLiteral)
        } else {
            self.make_token(TokenType::Illegal)
        }
    }

    fn consume_hex_digits(&mut self, count: usize) -> bool {
        for _ in 0..count {
            if is_hex_digit(self.peek()) {
                self.advance();
            } else {
                return false;
            }
        }
        true
    }

    // === 辅助工具 ===

    fn advance(&mut self) -> Option<u8> {
        if self.current >= self.source.len() {
            return None;
        }
        let c = self.source[self.current];
        self.current += 1;
        Some(c)
    }

    fn peek(&self) -> u8 {
        if self.current >= self.source.len() {
            return 0;
        }
        self.source[self.current]
    }

    fn peek_next(&self) -> u8 {
        if self.current + 1 >= self.source.len() {
            return 0;
        }
        self.source[self.current + 1]
    }

    fn match_char(&mut self, expected: u8) -> bool {
        if self.current >= self.source.len() {
            return false;
        }
        if self.source[self.current] != expected {
            return false;
        }
        self.current += 1;
        true
    }

    fn match_assign(&mut self, single: TokenType, double: TokenType) -> Token {
        if self.match_char(b'=') {
            self.make_token(double)
        } else {
            self.make_token(single)
        }
    }

    fn make_token(&self, tag: TokenType) -> Token {
        Token {
            tag,
            span: Span {
                file: self.file_id,
                start: self.start,
                end: self.current,
            },
        }
    }

    fn skip_whitespace(&mut self) {
        loop {
            let c = self.peek();
            match c {
                b' ' | b'\t' | b'\r' | b'\n' => {
                    self.advance();
                }
                _ => break,
            }
        }
    }

    fn skip_comment_line(&mut self) {
        while self.peek() != b'\n' && self.peek() != 0 {
            self.advance();
        }
    }

    fn skip_comment_block(&mut self) -> bool {
        let mut depth = 1;

        while depth > 0 {
            let c = self.peek();

            if c == 0 && self.current >= self.source.len() {
                return false;
            }

            // 嵌套开始 /*
            if c == b'/' && self.peek_next() == b'*' {
                self.advance();
                self.advance();
                depth += 1;
                continue;
            }

            // 嵌套结束 */
            if c == b'*' && self.peek_next() == b'/' {
                self.advance();
                self.advance();
                depth -= 1;
                continue;
            }

            self.advance();
        }
        true
    }
}

// === 字符判断辅助函数 ===

fn is_alpha_numeric(c: u8) -> bool {
    is_alpha(c) || is_digit(c)
}

fn is_alpha(c: u8) -> bool {
    (b'a'..=b'z').contains(&c) || (b'A'..=b'Z').contains(&c) || c == b'_'
}

fn is_digit(c: u8) -> bool {
    (b'0'..=b'9').contains(&c)
}

fn is_hex_digit(c: u8) -> bool {
    is_digit(c) || (b'a'..=b'f').contains(&c) || (b'A'..=b'F').contains(&c)
}

fn is_bin_digit(c: u8) -> bool {
    c == b'0' || c == b'1'
}

fn is_oct_digit(c: u8) -> bool {
    (b'0'..=b'7').contains(&c)
}

// 根据 UTF-8 首字节判断字符长度
fn utf8_byte_sequence_length(c: u8) -> usize {
    if c & 0x80 == 0 {
        1 // 0xxxxxxx
    } else if c & 0xE0 == 0xC0 {
        2 // 110xxxxx
    } else if c & 0xF0 == 0xE0 {
        3 // 1110xxxx
    } else if c & 0xF8 == 0xF0 {
        4 // 11110xxx
    } else {
        0 // Invalid
    }
}

// 关键字映射
fn resolve_keyword(text: &[u8]) -> TokenType {
    match text {
        b"fn" => TokenType::Fn,
        b"let" => TokenType::Let,
        b"mut" => TokenType::Mut,
        b"const" => TokenType::Const,
        b"static" => TokenType::Static,
        b"type" => TokenType::Type,
        b"struct" => TokenType::Struct,
        b"enum" => TokenType::Enum,
        b"union" => TokenType::Union,
        b"trait" => TokenType::Trait,
        b"if" => TokenType::If,
        b"else" => TokenType::Else,
        b"switch" => TokenType::Switch,
        b"for" => TokenType::For,
        b"break" => TokenType::Break,
        b"continue" => TokenType::Continue,
        b"return" => TokenType::Return,
        b"defer" => TokenType::Defer,
        b"pub" => TokenType::Pub,
        b"extern" => TokenType::Extern,
        b"use" => TokenType::Use,
        b"impl" => TokenType::Impl,
        b"true" => TokenType::True,
        b"false" => TokenType::False,
        b"undef" => TokenType::Undef,
        b"as" => TokenType::As,
        b"and" => TokenType::And,
        b"or" => TokenType::Or,
        b"_" => TokenType::Underscore,
        b"Self" => TokenType::SelfType,
        b"self" => TokenType::SelfValue,
        b"adt" => TokenType::Adt,
        b"match" => TokenType::Match,
        _ => TokenType::Identifier,
    }
}
