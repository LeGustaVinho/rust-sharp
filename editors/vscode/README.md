# R# VS Code Extension

Language support for **R# (`.rsharp`)** in Visual Studio Code, powered by the native Rust `rsharp lsp` language server.

## Features

- **C#-Aligned Syntax Highlighting**: TextMate grammar using VS Code's standard C# scope hierarchy, plus embedded Rust highlighting inside inline `rust { ... }` blocks and interpolated strings `$"...{expr}..."`.
- **Semantic Highlighting**: LSP semantic tokens distinguishing structs, enums, traits, type parameters, fields, methods, functions, parameters, immutable vs. mutable variables, attributes, macros, and prelude library symbols.
- **Live Syntax Checker**: Instant in-memory lex, parse, and codegen diagnostics as you type.
- **Save-Time Semantic & Borrow Checker**: Runs `rustc --emit=metadata --error-format=json` on save (`rsharp.checkOnSave`) to surface full Rust type and borrow-checker errors and warnings.
- **IDE Navigation & Assistance**: Document Outline (`Ctrl+Shift+O`), Breadcrumbs, Hover docs & signatures, Go to Definition (`F12`), context-aware Autocomplete (including dot-member completion), bracket/comment/doc-comment editing rules, and snippets.
- **Commands**:
  - `R#: Run Current File`
  - `R#: Check Current File`
  - `R#: Preview Generated Rust` (live side-by-side read-only Rust view)
  - `R#: Convert Rust File to R#`
  - `R#: Restart Language Server`

## Convert a Rust project

Open a trusted folder with a root `Cargo.toml`, Rust sources, and no R# sources. VS Code offers to convert the project even when no file is open. Close the notification to defer until you reopen the project, or select **Don't Ask Again Here** to remember that choice for this folder in the workspace.

Select **Convert to R#** to convert the saved sources with `rsharp from-rust --project`. The extension verifies each result by translating it back to Rust, then puts the `.rsharp` files in a new `rsharp-…` folder inside the project. Project conversion requires the `rsharp` CLI with project support and the `rust-analyzer` and `rust-src` Rust components.

## Settings

| Setting | Default | Description |
| :--- | :--- | :--- |
| `rsharp.executablePath` | `""` | Custom path to `rsharp` binary (auto-detects workspace `target/release`, `target/debug`, then `PATH`). |
| `rsharp.checkOnSave` | `true` | Enable `rustc` type & borrow-checker diagnostics on save. |
| `rsharp.trace.server` | `"off"` | LSP protocol trace verbosity (`off`, `messages`, `verbose`). |

## Local Development & Packaging

```bash
# 1. Build the native R# CLI + LSP server
cargo build --release -p rsharp-cli

# 2. Install extension dependencies and run tests (100% coverage)
cd editors/vscode
npm install
npm test

# 3. Package a local .vsix and install it into VS Code
npm run package
code --install-extension rsharp-vscode-0.1.1.vsix
```

Or open `editors/vscode` in VS Code and press **F5** to launch an Extension Development Host.
