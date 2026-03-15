# Kern Programming Language

> **Status:** v0.4.0 (Experimental)  
> *High Abstraction, Low Policy.*

Kern is a systems-level programming language explicitly designed for operating system kernels, embedded firmware, and high-performance infrastructure. 

Kern is built on the observation that languages often trade off abstraction capabilities against policy constraints. Kern aims to occupy the "fourth quadrant": providing powerful, modern abstractions (like generics, ADTs, and explicit module systems) while enforcing virtually **zero hidden runtime policies**. There are no implicit allocations, no garbage collection, no exceptions, and no closures with hidden captures.

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

The compiler is currently in its **`v0.4.x`** series. It possesses a robust frontend pipeline, performs rigorous two-pass semantic analysis/typechecking, and generates optimized LLVM IR.

* **[Delivered] v0.1.x (Core Stabilization):** Core syntax parsing, basic module resolution, generics monomorphization, LLVM backend integration, and basic type checking.
* **[Delivered] v0.2.x (Advanced Types & Control Flow):** Algebraic Data Types (ADT), exhaustive pattern matching (`match`), and stateless anonymous functions (Lambdas).
* **[Delivered] v0.3.x (Meta-programming & Low-level Control):** AST attribute markers (e.g., `#[packed]`, `#[link_section]`), compiler intrinsics (`@trap`, `@sizeOf`), and structured Inline Assembly (`@asm`).
* **[Delivered] v0.4.x (Standard Library & Architecture Overhaul):** Implementation of an explicit module tree system (`mod` / `pub use`), cross-platform freestanding standard library (`std`), multi-pass type resolution, universal slicing, and CLI linker controls.
* **[Current Focus] v0.5.x (Ecosystem, Tooling & Optimization):** Package management foundations (CLI alias mappings), enhanced error reporting/diagnostics, LLVM optimization passes, and Language Server Protocol (LSP) foundations.

## Installation

The easiest way to install Kern is via our official installation script. This script automatically downloads and installs the pre-compiled compiler and the standard library toolchain to `~/.kern`.

**For Linux / macOS:**
```bash
curl -sSf https://raw.githubusercontent.com/softfault/kern/main/install.sh | bash

```

*(After installation, you may need to restart your terminal or `source ~/.bashrc` to update your PATH).*

## Building from Source

If you prefer to build the compiler from source or contribute to its development, ensure you have the latest stable Rust toolchain installed.

```bash
# Clone the repository
git clone https://github.com/softfault/kern.git
cd kern

# Build the compiler
cargo build --release

```

## Documentation

* **[Kern Language Design Document](design.md)**: A comprehensive dive into the language mechanics, memory rules, and syntax.
* **[The Kern Type System](kern_type.md)**: Essential reading for new users to understand Kern's Top-Down Bidirectional Type Checking model and how it differs from bottom-up inference languages like Rust or C++.

## Contributing

Contributions are welcome! Whether it's reporting a bug, improving documentation, or proposing new features. Please see our [CONTRIBUTING.md](CONTRIBUTING.md) for guidelines.

**Note on language design:** Kern is a founder-led project. To maintain a cohesive language architecture, major design changes and new syntax proposals will be evaluated strictly against the founder's vision for the language. Please open an issue to discuss significant changes before writing code.

## License

Kern is open-source software licensed under the [MIT License](LICENSE).
