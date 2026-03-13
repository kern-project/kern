# Kern Programming Language

> **Status:** v0.3.8 (Experimental)  
> *High Abstraction, Low Policy.*

Kern is a systems-level programming language explicitly designed for operating system kernels, embedded firmware, and high-performance infrastructure. 

Kern is built on the observation that languages often trade off abstraction capabilities against policy constraints. Kern aims to occupy the "fourth quadrant": providing powerful, modern abstractions (like generics, ADTs, and module systems) while enforcing virtually **zero hidden runtime policies**. There are no implicit allocations, no garbage collection, no exceptions, and no closures with hidden captures.

## Core Philosophy

* **Clarity over Novelty:** What you write is what the machine executes. Kern fixes C's legacy warts (like spiral declarations and implicit array decays) while maintaining entirely predictable assembly generation.
* **Explicit over Implicit:** Mutability must be explicitly declared (`mut T`). Pointer mathematics is restricted. Return values cannot be silently ignored. No background magic.
* **Zero-Cost Abstractions:** Features like monomorphized Generics, Algebraic Data Types (ADT), and strictly Stateless Lambdas compile down to highly optimized, flat LLVM IR with zero runtime overhead.
* **Seamless Interoperability:** The C ABI is the universal language. Kern communicates natively with C and Assembly without complex bindings.

## A Taste of Kern

Kern elegantly combines low-level hardware control with high-level expression. The following example demonstrates explicit mutability, uniform Trait Object initialization, structured inline assembly, and exhaustive pattern matching.

```kern
use std.io;

// 1. Algebraic Data Types (ADT)
pub type Result[T, E] = adt {
    Ok: T,
    Err: E,
};

// 2. Trait VTables (Implicit 'self')
type HardwareDevice = trait {
    init: fn() Result[bool, i32],
};

type SerialPort = struct { port: u16 = 0x3F8 };

impl *mut SerialPort : HardwareDevice {
    pub fn init() Result[bool, i32] {
        let status = mut u8.{undef};
        
        // 3. Structured Inline Assembly (Compile-time MAST evaluation)
        @asm(.{
            asm: .{ "in al, dx" },
            outputs: .{ al: status.& },
            inputs: .{ dx: self.port },
            volatile: true
        });
        
        if (status == 0xFF) return .{ Err: -1 }; // Elided ADT Init
        return .{ Ok: true };
    }
}

// 4. AST Attributes for Linker/Memory Control
#[link_section(".text.boot")]
#[export_name("_start")]
extern fn start() ! {
    let serial = mut SerialPort.{ port: 0x3F8 };
    
    // 5. Trait Object Construction 
    let device = mut HardwareDevice.{serial.&};
    
    // 6. Exhaustive Pattern Matching & Explicit Discards
    let _ = match (device.init()) {
        .Ok: _ => io.print("Serial ready\n\0", .{}),
        .Err: code => @trap(), // Compiler intrinsic for llvm.trap
    };
    
    for (;;) {}
    @unreachable()
}

```

## Roadmap & Current Status

The compiler is currently in its `v0.3.x` series. It possesses a robust frontend pipeline, performs rigorous semantic analysis/typechecking, and generates optimized LLVM IR.

* **[Delivered] v0.1.x (Core Stabilization):** Core syntax parsing, module resolution, generics monomorphization, LLVM backend integration, and basic type checking.
* **[Delivered] v0.2.x (Advanced Types & Control Flow):** Algebraic Data Types (ADT), exhaustive pattern matching (`match`), and stateless anonymous functions (Lambdas). *(Note: The originally proposed `class` keyword was dropped in favor of orthogonal composition via Structs and Traits).*
* **[Delivered] v0.3.x (Meta-programming & Low-level Control):** Introduction of AST attribute markers (e.g., `#[packed]`, `#[link_section]`), compiler intrinsics (`@trap`, `@sizeOf`), and structured Inline Assembly (`@asm`).
* **[Current Focus] v0.4.x (Standard Library Maturation):** Full bootstrapping and stabilization of the `libcore` freestanding standard library, expanding compiler intrinsics for bitwise/memory operations.
* **v0.5.x (Ecosystem & Tooling):** CLI alias mappings for package management, enhanced error reporting, and language server (LSP) foundations.

## Building the Compiler

The Kern compiler (`kernc`) is written in Rust. To build it from source, ensure you have the latest stable Rust toolchain installed.

```bash
# Clone the repository
git clone https://github.com/softfault/kern.git
cd kern

# Build the compiler
cargo build --release

```

## Documentation

For a comprehensive dive into the language mechanics, type system, and memory rules, please read the [Kern Language Design Document](design.md).

## License

Kern is open-source software licensed under the [MIT License](LICENSE).