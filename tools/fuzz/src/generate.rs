//! Random Kern source generator.

use rand::RngExt;
use rand_chacha::ChaCha8Rng;

const INDENT: &str = "    ";

pub struct Generator {
    rng: ChaCha8Rng,
}

impl Generator {
    pub fn new(rng: ChaCha8Rng) -> Self {
        Self { rng }
    }

    pub fn generate(&mut self) -> String {
        let mut buf = String::with_capacity(4096);
        let depth = self.rng.random_range(2..4);
        let item_count = self.rng.random_range(1..5);

        for _ in 0..item_count {
            self.gen_item(&mut buf, depth, 0);
        }

        self.gen_main_fn(&mut buf, depth);
        buf
    }

    fn gen_item(&mut self, buf: &mut String, depth: usize, indent: usize) {
        match self.rng.random_range(0..10) {
            0..=2 => self.gen_fn_def(buf, depth, indent),
            3 => self.gen_struct_def(buf, depth, indent),
            4 => self.gen_enum_def(buf, depth, indent),
            5 => self.gen_trait_def(buf, depth, indent),
            6 => self.gen_impl_def(buf, depth, indent),
            7 => self.gen_use_stmt(buf, indent),
            8 => self.gen_extern_decl(buf, indent),
            9 => self.gen_const_decl(buf, indent),
            _ => {}
        }
        buf.push('\n');
    }

    fn gen_use_stmt(&mut self, buf: &mut String, indent: usize) {
        let prefix = INDENT.repeat(indent);
        let path = self.random_use_path();
        buf.push_str(&format!("{prefix}use {path};\n"));
    }

    fn random_use_path(&mut self) -> &'static str {
        const PATHS: &[&str] = &[
            "std.io",
            "std.mem",
            "std.fs",
            "base.mem",
            "base.coll",
            "base.test",
            "base.option",
            "base.result",
            "base.mem.alloc",
        ];
        PATHS[self.rng.random_range(0..PATHS.len())]
    }

    fn gen_extern_decl(&mut self, buf: &mut String, indent: usize) {
        let prefix = INDENT.repeat(indent);
        let mut kind = self.random_type_ref(0).to_string();
        if self.rng.random_bool(0.3) {
            kind.push(';');
        }
        buf.push_str(&format!("{prefix}extern {kind}\n"));
    }

    fn gen_const_decl(&mut self, buf: &mut String, indent: usize) {
        let prefix = INDENT.repeat(indent);
        let name = self.random_ident();
        let ty = self.random_type_ref(0);
        let val = self.gen_literal();
        buf.push_str(&format!("{prefix}const {name}: {ty} = {val};\n"));
    }

    fn gen_struct_def(&mut self, buf: &mut String, depth: usize, indent: usize) {
        if depth == 0 {
            return;
        }
        let prefix = INDENT.repeat(indent);
        let name = self.random_struct_name();
        buf.push_str(&format!("{prefix}struct {name} {{\n"));
        let field_cnt = self.rng.random_range(1..6);
        for _ in 0..field_cnt {
            let fname = self.random_ident();
            let ftype = self.random_type_ref(depth - 1);
            let fvis = if self.rng.random_bool(0.3) {
                "pub "
            } else {
                ""
            };
            buf.push_str(&format!("{prefix}{INDENT}{fvis}{fname}: {ftype},\n"));
        }
        buf.push_str(&format!("{prefix}}};\n"));
    }

    fn gen_enum_def(&mut self, buf: &mut String, depth: usize, indent: usize) {
        if depth == 0 {
            return;
        }
        let prefix = INDENT.repeat(indent);
        let name = self.random_enum_name();
        buf.push_str(&format!("{prefix}enum {name} {{\n"));
        let var_cnt = self.rng.random_range(1..5);
        for _ in 0..var_cnt {
            let vname = self.pascal_case_ident();
            if self.rng.random_bool(0.5) {
                let ptype = self.random_type_ref(depth - 1);
                buf.push_str(&format!("{prefix}{INDENT}{vname}: {ptype},\n"));
            } else {
                buf.push_str(&format!("{prefix}{INDENT}{vname},\n"));
            }
        }
        buf.push_str(&format!("{prefix}}};\n"));
    }

    fn gen_trait_def(&mut self, buf: &mut String, depth: usize, indent: usize) {
        if depth == 0 {
            return;
        }
        let prefix = INDENT.repeat(indent);
        let name = self.random_trait_name();
        buf.push_str(&format!("{prefix}trait {name} {{\n"));
        let method_cnt = self.rng.random_range(1..4);
        for _ in 0..method_cnt {
            let mname = self.random_ident();
            let ret = self.random_type_ref(depth - 1);
            buf.push_str(&format!("{prefix}{INDENT}fn {mname}() {ret};\n"));
        }
        buf.push_str(&format!("{prefix}}};\n"));
    }

    fn gen_impl_def(&mut self, buf: &mut String, depth: usize, indent: usize) {
        if depth == 0 {
            return;
        }
        let prefix = INDENT.repeat(indent);
        let self_ty = self.random_type_ref(depth - 1);

        if self.rng.random_bool(0.4) {
            let trait_ty = self.random_type_ref(depth - 1);
            buf.push_str(&format!("{prefix}impl {self_ty} : {trait_ty} {{\n"));
        } else {
            buf.push_str(&format!("{prefix}impl {self_ty} {{\n"));
        }

        let method_cnt = self.rng.random_range(1..4);
        for _ in 0..method_cnt {
            self.gen_fn_def(buf, depth - 1, indent + 1);
        }
        buf.push_str(&format!("{prefix}}}\n"));
    }

    fn gen_fn_def(&mut self, buf: &mut String, depth: usize, indent: usize) {
        if depth == 0 {
            return;
        }
        let prefix = INDENT.repeat(indent);
        let name = self.random_fn_ident();
        let ret = self.random_type_ref(depth - 1);

        let mut params = String::new();
        let param_cnt = self.rng.random_range(0..5);
        for i in 0..param_cnt {
            if i > 0 {
                params.push_str(", ");
            }
            let pname = self.random_ident();
            let ptype = self.random_type_ref(depth - 1);
            params.push_str(&format!("{pname}: {ptype}"));
        }

        buf.push_str(&format!("{prefix}fn {name}({params}) {ret} {{\n"));
        let stmt_cnt = self.rng.random_range(1..5);
        for _ in 0..stmt_cnt {
            self.gen_stmt(buf, depth.saturating_sub(1), indent + 1);
        }
        let ret_val = self.gen_expr(depth - 1);
        buf.push_str(&format!("{prefix}{INDENT}return {ret_val};\n"));
        buf.push_str(&format!("{prefix}}}\n"));
    }

    fn gen_main_fn(&mut self, buf: &mut String, depth: usize) {
        buf.push_str("fn main() i32 {\n");
        let stmt_cnt = self.rng.random_range(1..5);
        for _ in 0..stmt_cnt {
            self.gen_stmt(buf, depth, 1);
        }
        let ret_val = self.gen_int_expr(depth);
        buf.push_str(&format!("    return {ret_val};\n"));
        buf.push_str("}\n");
    }

    fn gen_stmt(&mut self, buf: &mut String, depth: usize, indent: usize) {
        let prefix = INDENT.repeat(indent);
        match self.rng.random_range(0..12) {
            0..=2 => {
                let val = self.gen_expr(depth);
                buf.push_str(&format!("{prefix}{val};\n"));
            }
            3 => {
                let name = self.random_ident();
                let init = self.gen_expr(depth);
                buf.push_str(&format!("{prefix}let {name} = {init};\n"));
            }
            4 => {
                let name = self.random_ident();
                let ty = self.random_type_ref(depth);
                let init = self.gen_expr(depth);
                buf.push_str(&format!("{prefix}let {name}: {ty} = {init};\n"));
            }
            5 => {
                let name = self.random_ident();
                let ty = self.random_type_ref(depth);
                let init = self.gen_expr(depth);
                buf.push_str(&format!("{prefix}let mut {name}: {ty} = {init};\n"));
            }
            6 => {
                let cond = self.gen_expr(depth);
                buf.push_str(&format!("{prefix}if ({cond}) {{\n"));
                let body_cnt = self.rng.random_range(1..4);
                for _ in 0..body_cnt {
                    self.gen_stmt(buf, depth.saturating_sub(1), indent + 1);
                }
                if self.rng.random_bool(0.6) {
                    buf.push_str(&format!("{prefix}}} else {{\n"));
                    let else_cnt = self.rng.random_range(1..4);
                    for _ in 0..else_cnt {
                        self.gen_stmt(buf, depth.saturating_sub(1), indent + 1);
                    }
                }
                buf.push_str(&format!("{prefix}}}\n"));
            }
            7 => {
                let val = self.gen_expr(depth);
                buf.push_str(&format!("{prefix}defer {val};\n"));
            }
            8 => {
                let cond = self.gen_expr(depth);
                buf.push_str(&format!("{prefix}while ({cond}) {{\n"));
                let body_cnt = self.rng.random_range(1..4);
                for _ in 0..body_cnt {
                    self.gen_stmt(buf, depth.saturating_sub(1), indent + 1);
                }
                buf.push_str(&format!("{prefix}}}\n"));
            }
            9 => {
                let val = self.gen_expr(depth);
                buf.push_str(&format!("{prefix}return {val};\n"));
            }
            10 => {
                let arg = self.gen_expr(depth);
                buf.push_str(&format!("{prefix}break {arg};\n"));
            }
            11 => {
                let val = self.gen_expr(depth);
                buf.push_str(&format!("{prefix}let _ = {val};\n"));
            }
            _ => {}
        }
    }

    fn gen_expr(&mut self, depth: usize) -> String {
        if depth == 0 {
            return self.gen_terminal_expr();
        }

        match self.rng.random_range(0..20) {
            0 => self.gen_literal(),
            1 => self.random_ident().to_string(),
            2 => {
                let lhs = self.gen_expr(depth - 1);
                let rhs = self.gen_expr(depth - 1);
                let op = self.random_binary_op();
                format!("{lhs} {op} {rhs}")
            }
            3 => {
                let inner = self.gen_expr(depth - 1);
                let op = self.random_unary_op();
                format!("{op}{inner}")
            }
            4 => {
                let inner = self.gen_expr(depth - 1);
                format!("({inner})")
            }
            5 => {
                let receiver = self.gen_expr(depth - 1);
                let method = self.random_ident();
                let args = self.gen_arg_list(depth - 1);
                format!("{receiver}.{method}({args})")
            }
            6 => {
                let name = self.random_fn_ident();
                let args = self.gen_arg_list(depth - 1);
                format!("{name}({args})")
            }
            7 => {
                let name = self.random_ident();
                let init = self.gen_expr(depth - 1);
                format!("let {name} = {init}")
            }
            8 => {
                let receiver = self.gen_expr(depth - 1);
                let field = self.random_ident();
                format!("{receiver}.{field}")
            }
            9 => {
                let receiver = self.gen_expr(depth - 1);
                format!("{receiver}..&")
            }
            10 => {
                let inner = self.gen_expr(depth - 1);
                format!("&{inner}")
            }
            11 => {
                let inner = self.gen_expr(depth - 1);
                format!("&mut {inner}")
            }
            12 => {
                let stmts = self.rng.random_range(1..4);
                let mut body = String::from("{\n");
                for _ in 0..stmts {
                    self.gen_stmt(&mut body, depth - 1, 1);
                }
                let last = self.gen_expr(depth - 1);
                body.push_str(&format!("    {last}\n"));
                body.push('}');
                body
            }
            13 => {
                let ty = self.random_type_ref(depth - 1);
                let field_cnt = self.rng.random_range(1..4);
                let mut fields = String::new();
                for i in 0..field_cnt {
                    if i > 0 {
                        fields.push_str(", ");
                    }
                    let fname = self.random_ident();
                    let fval = self.gen_expr(depth - 1);
                    fields.push_str(&format!("{fname}: {fval}"));
                }
                format!("{ty}.{{ {fields} }}")
            }
            14 => {
                let len = self.rng.random_range(1..6);
                let elem_ty = self.random_type_ref(depth - 1);
                let mut elems = String::new();
                for i in 0..len {
                    if i > 0 {
                        elems.push_str(", ");
                    }
                    elems.push_str(&self.gen_int_expr(depth - 1));
                }
                format!("[{len}]{elem_ty}.{{ {elems} }}")
            }
            15 => {
                let scrutinee = self.gen_expr(depth - 1);
                let arm_cnt = self.rng.random_range(1..4);
                let mut arms = String::new();
                for _ in 0..arm_cnt {
                    let pat = self.gen_pattern(depth - 1);
                    let body = self.gen_expr(depth - 1);
                    arms.push_str(&format!("        .{pat} => {body},\n"));
                }
                format!("match {scrutinee} {{\n{arms}    }}")
            }
            16 => {
                let closure_body = self.gen_expr(depth - 1);
                format!("[]() i32 {{ return {closure_body}; }}")
            }
            17 => {
                let receiver = self.gen_expr(depth - 1);
                let idx = self.gen_expr(depth - 1);
                format!("{receiver}[{idx}]")
            }
            18 => {
                let lhs = self.gen_expr(depth - 1);
                let rhs = self.gen_expr(depth - 1);
                format!("{lhs} == {rhs}")
            }
            _ => self.gen_terminal_expr(),
        }
    }

    fn gen_int_expr(&mut self, depth: usize) -> String {
        if depth == 0 {
            return self.gen_int_literal();
        }
        match self.rng.random_range(0..8) {
            0 => self.gen_int_literal(),
            1 => {
                let lhs = self.gen_int_expr(depth - 1);
                let rhs = self.gen_int_expr(depth - 1);
                let op = self.random_arith_op();
                format!("{lhs} {op} {rhs}")
            }
            2 => {
                let inner = self.gen_int_expr(depth - 1);
                let op = self.random_unary_arith_op();
                format!("{op}{inner}")
            }
            3 => {
                let inner = self.gen_int_expr(depth - 1);
                format!("({inner})")
            }
            4 => self.random_ident().to_string(),
            5 => {
                let fn_name = self.random_fn_ident();
                let args = self.gen_arg_list(depth - 1);
                format!("{fn_name}({args})")
            }
            6 => {
                let cond = self.random_bool_expr(depth - 1);
                let then_val = self.gen_int_expr(depth - 1);
                let else_val = self.gen_int_expr(depth - 1);
                format!("if ({cond}) {{ {then_val} }} else {{ {else_val} }}")
            }
            _ => "0".to_string(),
        }
    }

    fn random_bool_expr(&mut self, depth: usize) -> String {
        if depth == 0 {
            return self.random_bool_lit().to_string();
        }
        match self.rng.random_range(0..5) {
            0 => self.random_bool_lit().to_string(),
            1 => {
                let lhs = self.gen_int_expr(depth - 1);
                let rhs = self.gen_int_expr(depth - 1);
                let op = self.random_cmp_op();
                format!("{lhs} {op} {rhs}")
            }
            2 => {
                let lhs = self.random_bool_expr(depth - 1);
                let rhs = self.random_bool_expr(depth - 1);
                format!("{lhs} && {rhs}")
            }
            3 => {
                let lhs = self.random_bool_expr(depth - 1);
                let rhs = self.random_bool_expr(depth - 1);
                format!("{lhs} || {rhs}")
            }
            4 => {
                let inner = self.random_bool_expr(depth - 1);
                format!("!{inner}")
            }
            _ => "true".to_string(),
        }
    }

    fn gen_terminal_expr(&mut self) -> String {
        match self.rng.random_range(0..6) {
            0 => self.gen_literal(),
            1 => self.random_ident().to_string(),
            2 => "()".to_string(),
            3 => "true".to_string(),
            4 => "false".to_string(),
            5 => "void".to_string(),
            _ => "0".to_string(),
        }
    }

    fn gen_arg_list(&mut self, depth: usize) -> String {
        let cnt = self.rng.random_range(0..5);
        let mut args = String::new();
        for i in 0..cnt {
            if i > 0 {
                args.push_str(", ");
            }
            args.push_str(&self.gen_expr(depth));
        }
        args
    }

    fn gen_pattern(&mut self, depth: usize) -> String {
        if depth == 0 {
            return self.pascal_case_ident().to_string();
        }
        match self.rng.random_range(0..5) {
            0 => "_".to_string(),
            1 => self.random_ident().to_string(),
            2 => self.random_fn_ident().to_string(), // captures variant-like names
            3 => {
                let inner = self.gen_pattern(depth - 1);
                let field = self.random_ident();
                format!("{{ {field}: {inner} }}")
            }
            4 => self.gen_literal(),
            _ => "_".to_string(),
        }
    }

    fn random_type_ref(&mut self, depth: usize) -> &'static str {
        const TYPES: &[&str] = &[
            "i8", "i16", "i32", "i64", "u8", "u16", "u32", "u64", "usize", "bool", "void", "f32",
            "f64",
        ];
        if self.rng.random_bool(0.15) && depth > 0 {
            const COMPOUND: &[fn(usize, &mut ChaCha8Rng) -> String] = &[
                |_d, rng| format!("*{}", TYPES[rng.random_range(0..TYPES.len())]),
                |_d, rng| format!("&{}", TYPES[rng.random_range(0..TYPES.len())]),
                |_d, rng| format!("&mut {}", TYPES[rng.random_range(0..TYPES.len())]),
                |_d, rng| {
                    format!(
                        "[{}]{}",
                        rng.random_range(1..8),
                        TYPES[rng.random_range(0..TYPES.len())]
                    )
                },
            ];
            let idx = self.rng.random_range(0..COMPOUND.len());
            let s = COMPOUND[idx](depth, &mut self.rng);
            return s.leak();
        }
        TYPES[self.rng.random_range(0..TYPES.len())]
    }

    fn gen_literal(&mut self) -> String {
        match self.rng.random_range(0..8) {
            0 => self.gen_int_literal(),
            1 => format!("{}u8", self.rng.random_range(0..255u32)),
            2 => format!("{}u16", self.rng.random_range(0..65535u32)),
            3 => format!("{}u32", self.rng.random_range(0..1_000_000u64)),
            4 => format!("{}u64", self.rng.random_range(0..1_000_000u64)),
            5 => {
                let val = self.rng.random_range(0.0f64..1000.0);
                format!("{val:.2}f32")
            }
            6 => {
                let val = self.rng.random_range(0.0f64..1000.0);
                format!("{val:.2}f64")
            }
            7 => "true".to_string(),
            _ => "false".to_string(),
        }
    }

    fn gen_int_literal(&mut self) -> String {
        match self.rng.random_range(0..6) {
            0 => self.rng.random_range(-1000i32..1000).to_string(),
            1 => self.rng.random_range(0u32..1000).to_string(),
            2 => format!("0x{:x}", self.rng.random_range(0u32..0xFFFF)),
            3 => format!("0o{:o}", self.rng.random_range(0u32..0o7777)),
            4 => format!("0b{:b}", self.rng.random_range(0u32..256)),
            _ => "0".to_string(),
        }
    }

    fn random_bool_lit(&mut self) -> &'static str {
        if self.rng.random_bool(0.5) {
            "true"
        } else {
            "false"
        }
    }

    fn random_binary_op(&mut self) -> &'static str {
        const OPS: &[&str] = &[
            "+", "-", "*", "/", "%", "==", "!=", "<", ">", "<=", ">=", "&&", "||", "&", "|", "^",
            "<<", ">>",
        ];
        OPS[self.rng.random_range(0..OPS.len())]
    }

    fn random_arith_op(&mut self) -> &'static str {
        const OPS: &[&str] = &["+", "-", "*", "/", "%", "&", "|", "^", "<<", ">>"];
        OPS[self.rng.random_range(0..OPS.len())]
    }

    fn random_unary_op(&mut self) -> &'static str {
        const OPS: &[&str] = &["-", "!", "~", "*"];
        OPS[self.rng.random_range(0..OPS.len())]
    }

    fn random_unary_arith_op(&mut self) -> &'static str {
        const OPS: &[&str] = &["-", "~"];
        OPS[self.rng.random_range(0..OPS.len())]
    }

    fn random_cmp_op(&mut self) -> &'static str {
        const OPS: &[&str] = &["==", "!=", "<", ">", "<=", ">="];
        OPS[self.rng.random_range(0..OPS.len())]
    }

    fn random_ident(&mut self) -> &'static str {
        const WORDS: &[&str] = &[
            "x", "y", "z", "a", "b", "i", "j", "k", "n", "v", "val", "len", "idx", "ptr", "buf",
            "tmp", "res", "sum", "cnt", "src", "dst", "pos", "off", "num", "key", "val1", "val2",
            "arg", "acc", "r", "t", "p", "q", "w",
        ];
        WORDS[self.rng.random_range(0..WORDS.len())]
    }

    fn random_fn_ident(&mut self) -> &'static str {
        const NAMES: &[&str] = &[
            "do_stuff",
            "compute",
            "helper",
            "run",
            "process",
            "init",
            "cleanup",
            "transform",
            "foo",
            "bar",
            "baz",
            "step1",
            "step2",
            "apply",
            "map_val",
            "fold",
            "reduce",
            "make_thing",
            "get_value",
            "set_value",
        ];
        NAMES[self.rng.random_range(0..NAMES.len())]
    }

    fn random_struct_name(&mut self) -> &'static str {
        const NAMES: &[&str] = &[
            "Data", "Node", "Entry", "Pair", "Item", "Record", "Point", "Rect", "Config",
            "Context", "Handle", "Buffer", "Header", "Block", "Key",
        ];
        NAMES[self.rng.random_range(0..NAMES.len())]
    }

    fn random_enum_name(&mut self) -> &'static str {
        const NAMES: &[&str] = &[
            "Kind", "State", "Option", "Result", "Status", "Tag", "Variant", "Action", "Mode",
            "Error",
        ];
        NAMES[self.rng.random_range(0..NAMES.len())]
    }

    fn random_trait_name(&mut self) -> &'static str {
        const NAMES: &[&str] = &[
            "HasValue",
            "Printable",
            "Comparable",
            "Iterable",
            "Foldable",
            "Builder",
            "Validator",
            "Converter",
            "Accessor",
            "Factory",
        ];
        NAMES[self.rng.random_range(0..NAMES.len())]
    }

    fn pascal_case_ident(&mut self) -> &'static str {
        const NAMES: &[&str] = &[
            "Zero", "One", "Two", "Ok", "Err", "Some", "None", "Empty", "Full", "Open", "Close",
            "Start", "Stop", "Alpha", "Beta",
        ];
        NAMES[self.rng.random_range(0..NAMES.len())]
    }
}
