use std::fmt;

/// Source byte/line/column location span.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Span {
    pub line: usize,
    pub col: usize,
    pub start: usize,
    pub end: usize,
}

impl Span {
    pub const fn new(line: usize, col: usize, start: usize, end: usize) -> Self {
        Self {
            line,
            col,
            start,
            end,
        }
    }
}

/// Unified error type for R# lexing, parsing, transpilation, conversion, and execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RSharpError {
    Lex { span: Span, message: String },
    Parse { span: Span, message: String },
    Codegen { message: String },
    FromRust { message: String },
    UnresolvedType { binding: String, message: String },
    Compile { message: String },
    Io { message: String },
}

impl RSharpError {
    pub fn lex(span: Span, message: impl Into<String>) -> Self {
        Self::Lex {
            span,
            message: message.into(),
        }
    }

    pub fn parse(span: Span, message: impl Into<String>) -> Self {
        Self::Parse {
            span,
            message: message.into(),
        }
    }

    pub fn codegen(message: impl Into<String>) -> Self {
        Self::Codegen {
            message: message.into(),
        }
    }

    pub fn from_rust(message: impl Into<String>) -> Self {
        Self::FromRust {
            message: message.into(),
        }
    }

    pub fn unresolved_type(binding: impl Into<String>, message: impl Into<String>) -> Self {
        Self::UnresolvedType {
            binding: binding.into(),
            message: message.into(),
        }
    }

    pub fn compile(message: impl Into<String>) -> Self {
        Self::Compile {
            message: message.into(),
        }
    }

    pub fn io(message: impl Into<String>) -> Self {
        Self::Io {
            message: message.into(),
        }
    }
}

impl fmt::Display for RSharpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Lex { span, message } => {
                write!(f, "Lex error at {}:{}: {}", span.line, span.col, message)
            }
            Self::Parse { span, message } => {
                write!(f, "Parse error at {}:{}: {}", span.line, span.col, message)
            }
            Self::Codegen { message } => write!(f, "Codegen error: {}", message),
            Self::FromRust { message } => write!(f, "Rust->R# error: {}", message),
            Self::UnresolvedType { binding, message } => {
                write!(
                    f,
                    "Unresolved type for '{}': {} (R# requires explicit typing)",
                    binding, message
                )
            }
            Self::Compile { message } => write!(f, "Compiler error: {}", message),
            Self::Io { message } => write!(f, "IO error: {}", message),
        }
    }
}

impl std::error::Error for RSharpError {}

pub type Result<T> = std::result::Result<T, RSharpError>;
