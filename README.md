# Kern Programming Language

> **Status:** v0.2.5 (Experimental)
> *High Abstraction, Low Policy.*

Kern is a systems-level programming language explicitly designed for operating system kernels, embedded firmware, and high-performance infrastructure.

Kern is built on the observation that languages often trade off abstraction capabilities against policy constraints. Kern aims to occupy the "fourth quadrant": providing powerful, modern abstractions (like generics, ADTs, and module systems) while enforcing virtually zero hidden runtime policies (no implicit allocations, no GC, no implicit exceptions, no closures with hidden captures).

## Core Philosophy

* **Clarity over Novelty:** What you write is what the machine executes. Kern fixes C's legacy warts (like spiral declarations and implicit array decays) while maintaining predictable assembly generation.
* **Explicit over Implicit:** Mutability must be explicitly declared (`mut T`). Pointer mathematics requires explicit casting. No implicit background magic.
* **Zero-Cost Abstractions:** Features like Generics, Algebraic Data Types (ADT), and Stateless Lambdas compile down to highly optimized, flat LLVM IR with zero runtime overhead.
* **Seamless Interoperability:** The C ABI is the universal language. Kern communicates natively with C and Assembly.

## Quick Look

Kern elegantly combines low-level memory control with high-level expression.

```kern
use std.io;

// 1. Algebraic Data Types (ADT)
pub type Result[T, E] = adt {
    Ok: T,
    Err: E,
};

// 2. Trait VTables
type Reader = trait {
    read: fn([]u8) usize,
};

type File = struct { fd: i32 };

impl *mut File : Reader {
    pub fn read(buf: []u8) usize {
        0 // Implementation here...
    }
}

pub extern fn _start() void {
    // 3. Explicit Mutability & Trait Objects
    let file = mut File.{ fd: 1 };
    let reader = file.& as mut Reader; 

    // 4. Exhaustive Pattern Matching
    let res = Result[i32, i32].{ Ok: 42 };
    let code = match (res) {
        .Ok: val => val,
        .Err: err => err,
    };

    // 5. Stateless Lambdas (compiles to pure C function pointers)
    let is_success = fn(c: i32) bool {
        return c == 42;
    };
}

```

## Roadmap & Current Status

The compiler is currently in its `v0.2.x` series. It possesses a robust frontend pipeline, performs rigorous semantic analysis/typechecking, and generates optimized LLVM IR.

* **[Delivered] v0.1.x (Core Stabilization):** Core syntax parsing, module resolution, generics monomorphization, LLVM backend integration, and basic type checking.
* **[Delivered] v0.2.x (Advanced Types & Control Flow):** Introduction of Algebraic Data Types (ADT), exhaustive pattern matching (`match`), and stateless anonymous functions (Lambdas).
* *Note: The originally proposed `class` keyword has been officially dropped. Kern embraces orthogonal composition via Structs, Traits, and Builder patterns, avoiding the hidden memory overhead of embedded VTables.*


* **[Current Focus] v0.3.x (Meta-programming & Attributes):** Introduction of AST attribute markers (e.g., `#[packed]`, `#[link_section]`, `#[no_mangle]`) to support robust conditional compilation and strict memory layout control.
* **v0.4.x (Low-level Control):** Support for Inline Assembly (`asm`) blocks, crucial for OS development.
* **v0.5.x (Standard Library Maturation):** Full bootstrapping and stabilization of the `libcore` standard library.

## Building the Compiler

The Kern compiler is written in Rust. To build it from source, ensure you have the latest stable Rust toolchain installed.

```bash
# Clone the repository
git clone https://github.com/softfault/kern.git
cd kern

# Build the compiler
cargo build --release

```

## Documentation

For a comprehensive dive into the language mechanics, type system, and memory rules, please read the [Kern Language Design Document](design.md).