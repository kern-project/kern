# Contributing to Kern

First off, thank you for considering contributing to Kern! 

## Project Governance and Vision

Kern is fundamentally a personal project and founder-led. While community contributions, bug reports, and pull requests are incredibly valuable and deeply appreciated, the final decisions regarding language design, compiler architecture, and feature roadmaps rest entirely with the project founder. 

This model ensures a unified vision and prevents the language design from becoming fragmented. Before submitting a large Pull Request for a new feature, please open an Issue to discuss the design first to ensure it aligns with the project's direction.

## Development Setup

Kern is written in Rust. You will need the standard Rust toolchain installed.

1. Clone the repository:
```bash
git clone https://github.com/YOUR_USERNAME/kern.git
cd kern
```

2. Build the compiler:
```bash
cargo build
```

## Running Tests

Before submitting a Pull Request, please ensure all tests pass.

We use a data-driven testing approach for the compiler. Our tests are split into two parts:
1. **Unit Tests:** Located within the Rust source files in `compiler/src/`.
2. **Integration Tests:** Located in the root `tests/` directory as `.kn` source files.

To run all tests, simply execute:

```bash
cargo test

```

### Adding a New Test

When adding a new `.kn` test case, place it in `tests/pass/` for valid code, or `tests/fail/` for invalid code that the compiler must reject. You can use inline directives at the very top of the file:

- `// expected-stdout: <text>`: (For `pass` tests) Asserts that the executed program prints the exact text.
- `// expected-error: <text>`: (For `fail` tests) Asserts that the compiler fails and its stderr output contains the specified text.
- `// compile-flags: <flags>`: Passes specific command-line arguments to `kernc` (e.g., `--freestanding`).
- `// build-only`: Compiles the file but skips execution.

## Commit Guidelines

We use [Conventional Commits](https://www.conventionalcommits.org/). Please format your commit messages accordingly:

* `feat:` A new feature
* `fix:` A bug fix
* `docs:` Documentation only changes
* `refactor:` A code change that neither fixes a bug nor adds a feature
* `test:` Adding missing tests or correcting existing tests

Example:
`fix(lower): correct type inference for early returns`
