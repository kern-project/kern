#![allow(unused)]
use crate::utils::Span;

#[derive(Default, Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Token {
    pub tag: TokenType,
    pub span: Span,
}

#[derive(Default, Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum TokenType {
    // === 标识符与字面量 ===
    Identifier,    // abc, my_var
    IntLiteral,    // 123, 0xFF, 10u8
    FloatLiteral,  // 3.14
    StringLiteral, // "hello"
    CharLiteral,   // 'a'

    // === 关键字 ===
    Fn,
    Let,
    Mut,
    Const,
    Static,
    Type,
    Struct,
    Enum,
    Union,
    Trait,
    If,
    Else,
    Switch,
    For,
    Break,
    Continue,
    Return,
    Defer,
    Pub,
    Extern,
    Use,
    Impl,
    True,
    False,
    Undef,
    As,
    And,
    Or,
    Underscore,
    SelfType,
    SelfValue,
    Adt,   // adt
    Match, // match

    // === 运算符 ===

    // 算术: + - * / %
    Plus,
    Minus,
    Star, // 同时用于指针解引用/类型 *T
    Slash,
    Percent,

    // 特殊前缀
    Hash,  // #arr (长度)
    At,    // @intToFloat (内置函数)
    Caret, // ^T (易失性指针) / Bitwise XOR

    // 逻辑/位运算
    Bang,      // ! (宏调用 / 逻辑非)
    Ampersand, // & (取地址 / 按位与)
    Pipe,      // | (按位或)
    Tilde,     // ~ (按位取反)

    // 比较: == != < <= > >=
    EqualEqual,
    NotEqual,
    LessThan,
    LessEqual,
    GreaterThan,
    GreaterEqual,

    // 移位: << >>
    LShift,
    RShift,

    // 赋值: = += -= ...
    Assign,
    PlusAssign,
    MinusAssign,
    StarAssign,
    SlashAssign,
    PercentAssign,
    AmpersandAssign,
    PipeAssign,
    CaretAssign,
    LShiftAssign,
    RShiftAssign,

    // === 标点 ===
    Dot,          // . (字段访问)
    DotDot,       // .. (Range)
    DotDotEqual,  // ..= (Range Inclusive)
    DotAmpersand, // .&
    DotStar,      // .* (指针解引用)

    DotLBracket, // .[ (切片/数组索引)
    DotLBrace,   // .{ (匿名结构体初始化)

    Ellipsis, // ...

    Comma,     // ,
    Colon,     // :
    Semicolon, // ;

    // ( )
    LParen,
    RParen,

    // { }
    LBrace,
    RBrace,

    // [ ]
    LBracket,
    RBracket,

    // =>
    Arrow,

    // === 特殊 ===
    Eof,
    #[default]
    Illegal,
}
