use std::{
    fmt::{self, Display, Formatter},
    path::PathBuf,
};

use colored::Colorize;
use thiserror::Error;

use crate::{
    ast::{BinaryOp, UnaryOp},
    diagnostic::render_source_span,
    interpreter::values::Value,
    source::Span,
    typechecker::ty::{ParameterTypes, Type},
};

fn render_vec_tokens(tks: Vec<String>) -> String {
    let mut s = String::new();
    for (i, tk) in tks.iter().enumerate() {
        s = format!("{s}'");
        s = format!("{s}{tk}");
        s = format!("{s}'");
        if i < tks.len() - 1 {
            s = format!("{s} or ");
        }
    }
    s
}

pub fn render_types_from_value_vector(values: Vec<Value>) -> String {
    let mut s = String::new();
    s = format!("{s}(");
    for (i, v) in values.iter().enumerate() {
        s = format!("{s}{}", v.get_type());
        if i < values.len() - 1 {
            s = format!("{s}, ");
        }
    }
    s = format!("{s})");
    s
}

pub fn render_parameter_types(overloaded_parameters: Vec<ParameterTypes>) -> String {
    let mut s = String::new();
    s = format!("{s}(");
    for (i, parameters_types) in overloaded_parameters.iter().enumerate() {
        for (j, param) in parameters_types.iter().enumerate() {
            s = format!("{s}{}", param.ty);
            if j < parameters_types.len() - 1 {
                s = format!("{s}, ");
            }
        }
        if i < overloaded_parameters.len() - 1 {
            s = format!("{s}| ");
        }
    }
    s = format!("{s})");
    s
}

trait DiagnosticError {
    type Kind: Display;

    fn file_path(&self) -> &PathBuf;
    fn span(&self) -> Span;
    fn kind(&self) -> &Self::Kind;
}

fn fmt_diagnostic<E>(error: &E, f: &mut Formatter<'_>) -> fmt::Result
where
    E: DiagnosticError,
{
    match render_source_span(error.file_path(), error.span()) {
        Ok((line, col, snippet)) => {
            let spaces = (0..line.to_string().len()).map(|_| ' ').collect::<String>();
            let arrow = "-->".cyan();
            let error_red = "type error".red();
            write!(
                f,
                "{error_red}: {}\n{spaces}{arrow} {}:{line}:{col}:\n{snippet}",
                error.kind(),
                error.file_path().display(),
                // line.to_string().cyan(),
            )
        }
        Err(_) => {
            let error_red = "error".red();
            write!(f, "{error_red}: {}", error.kind())
        }
    }
}

#[derive(Error, Debug)]
pub struct LexerError {
    pub kind: LexerErrorKind,
    pub file_path: PathBuf,
    pub pos: Span,
}

impl DiagnosticError for LexerError {
    type Kind = LexerErrorKind;

    fn file_path(&self) -> &PathBuf {
        &self.file_path
    }
    fn span(&self) -> Span {
        self.pos
    }
    fn kind(&self) -> &Self::Kind {
        &self.kind
    }
}

impl Display for LexerError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        fmt_diagnostic(self, f)
    }
}

#[derive(Error, Debug)]
pub enum LexerErrorKind {
    #[error("unrecognized char '{0}'")]
    UnknownChar(char),
    #[error("unrecognized token '{0}'")]
    UnknownToken(String),
    #[error("invalid number '{0}'")]
    InvalidNumber(String),
    #[error("unterminated string")]
    UnterminatedString,
    #[error("expected chars {}, found {found}", render_vec_tokens(expected.clone()))]
    ExpectedChars {
        expected: Vec<String>,
        found: String,
    },
}

#[derive(Error, Debug)]
pub struct ParserError {
    pub kind: ParserErrorKind,
    pub file_path: PathBuf,
    pub pos: Span,
}

impl DiagnosticError for ParserError {
    type Kind = ParserErrorKind;

    fn file_path(&self) -> &PathBuf {
        &self.file_path
    }
    fn span(&self) -> Span {
        self.pos
    }
    fn kind(&self) -> &Self::Kind {
        &self.kind
    }
}

impl Display for ParserError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        fmt_diagnostic(self, f)
    }
}

#[derive(Error, Debug)]
pub enum ParserErrorKind {
    #[error("expected token '{expected}', found '{found}'")]
    ExpectedToken { expected: String, found: String },
    #[error("expected tokens {} but found '{found}'", render_vec_tokens(expected.clone()))]
    ExpectedTokens {
        expected: Vec<String>,
        found: String,
    },
    #[error("expected identifier, found '{found}'")]
    ExpectedIdentifier { found: String },
    #[error("expected expression, found '{found}'")]
    ExpectedExpression { found: String },
    #[error("unexpected token '{found}'")]
    UnexpectedToken { found: String },
    #[error("unexpected EOF")]
    UnexpectedEof,
    #[error("{expr} is not a valid assignment target")]
    InvalidAssignmentTarget { expr: String },
    #[error("{expr} is not a valid lamda parameter")]
    InvalidParameter { expr: String },
    #[error("expected type annotation, got nothing")]
    ExpectedTypeAnnotation,
    #[error("cannot chain more than one range operations")]
    ChainingRanges,
    #[error("{found} is not a valid import")]
    InvalidImport { found: String },
    #[error("{found} is not a valid part of a module path")]
    InvalidModulePath { found: String },
    #[error("expected a top-level item (`import`, `type`, `const`, or `fun`), found '{found}'")]
    ExpectedTopLevelItem { found: String },
    #[error("constant declarations require an explicit type")]
    ConstRequiresType,
}

#[derive(Debug, Error)]
pub struct RuntimeError {
    pub kind: RuntimeErrorKind,
    pub span: Span,
    pub file_path: PathBuf,
}

impl DiagnosticError for RuntimeError {
    type Kind = RuntimeErrorKind;

    fn file_path(&self) -> &PathBuf {
        &self.file_path
    }
    fn span(&self) -> Span {
        self.span
    }
    fn kind(&self) -> &Self::Kind {
        &self.kind
    }
}

impl Display for RuntimeError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        fmt_diagnostic(self, f)
    }
}

#[derive(Debug, Error)]
pub enum RuntimeErrorKind {
    #[error("'{0}' is not a struct value")]
    NotAStruct(String),
    #[error("unknown field '{field}' on '{ty}'")]
    UnknownField { ty: String, field: String },
    #[error("'{0}' is not an enum value")]
    NotAnEnum(String),
    #[error("unknown enum variant '{variant}' on '{ty}'")]
    UnknownEnumVariant { ty: String, variant: String },
    #[error("field assignment is not implemented; reconstruct the struct value instead")]
    FieldAssignmentUnsupported,
    #[error("undefined variable '{0}'")]
    UndefinedVariable(String),
    #[error("cannot assign to immutable variable '{0}'")]
    CannotAssignImmutable(String),
    #[error("cannot assign through imported member '{0}'")]
    CannotAssignImportedMember(String),
    #[error("unknown export '{member}' from module '{module}'")]
    UnknownModuleExport { module: String, member: String },
    #[error("cannot assign through module member path '{0}'")]
    ModuleMemberAssignment(String),
    #[error("{0} is not a valid assignment target")]
    InvalidAssignmentTarget(String),
    #[error("{0} is not a valid indexing target")]
    InvalidIndexingTarget(String),
    #[error("{0} is not a valid index")]
    InvalidIndex(String),
    #[error("invalid binary operation '{lhs} {op} {rhs}'")]
    InvalidBinaryOp {
        op: BinaryOp,
        lhs: String,
        rhs: String,
    },
    #[error("division by zero")]
    DivisionByZero,
    #[error("invalid unary operation '{op}{operand}'")]
    InvalidUnaryOp { op: UnaryOp, operand: String },
    #[error("invalid function parameter '{0}'")]
    InvalidFunctionParameter(String),
    #[error("invalid function parameters '{0}'")]
    InvalidFunctionParameters(String),
    #[error("expected boolean, found '{found}'")]
    ExpectedBool { found: String },
    #[error("'{0}' is not callable")]
    NotCallable(String),
    #[error("index {0} was outside of the bound of the array")]
    OutOfBounds(i64),
    #[error("expected signature of {expected}, but found {found}")]
    SignatureMismatch { expected: String, found: String },
    #[error("missing 'else' branch for 'if' condition that yields a value")]
    ElseBranchMissing,
    #[error("IO error: {0}")]
    IoError(String),
    #[error("not implemented")]
    NotImplemented,
    #[error("yield used outside of an effect handler")]
    YieldOutsideHandler,
    #[error("{found} is not an iterable value")]
    NotIterable { found: String },
    #[error("{found} is not a valid range step")]
    BadRangeStep { found: String },
    #[error("expected array, but found {found}")]
    ExpectedArray { found: String },
    #[error("mismatched returned types, expected '{expected}', but got '{found}'")]
    MismatchedReturnTypes { expected: String, found: String },
    #[error("argument error: {e}")]
    ArgumentError { e: Box<ArgumentError> },
}

#[derive(Debug, Error)]
pub struct TypeError {
    pub kind: TypeErrorKind,
    pub span: Span,
    pub file_path: PathBuf,
}

impl DiagnosticError for TypeError {
    type Kind = TypeErrorKind;

    fn file_path(&self) -> &PathBuf {
        &self.file_path
    }
    fn span(&self) -> Span {
        self.span
    }
    fn kind(&self) -> &Self::Kind {
        &self.kind
    }
}

impl Display for TypeError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        fmt_diagnostic(self, f)
    }
}

#[derive(Debug, Error)]
pub enum TypeErrorKind {
    #[error("unknown type '{0}'")]
    UnknownType(String),
    #[error("type application for '{0}' is not supported yet")]
    UnsupportedTypeApplication(String),
    #[error("name '{0}' is already defined in the type namespace")]
    TypeNameCollision(String),
    #[error("duplicate field '{0}'")]
    DuplicateField(String),
    #[error("duplicate enum variant '{0}'")]
    DuplicateVariant(String),
    #[error("enum '{0}' must declare at least one variant")]
    EmptyEnum(String),
    #[error("'{0}' is not a struct type")]
    NotAStruct(String),
    #[error("unknown field '{field}' on '{ty}'")]
    UnknownField { ty: String, field: String },
    #[error("field assignment is not implemented; reconstruct the struct value instead")]
    FieldAssignmentUnsupported,
    #[error("'{0}' is not an enum type")]
    NotAnEnum(String),
    #[error("unknown variant '{variant}' for enum '{ty}'")]
    UnknownVariant { ty: String, variant: String },
    #[error("variant '{variant}' for enum '{ty}' has no payload")]
    VariantHasNoPayload { ty: String, variant: String },
    #[error("variant '{variant}' for enum '{ty}' requires a payload binding")]
    VariantRequiresPayload { ty: String, variant: String },
    #[error("duplicate match arm for variant '{0}'")]
    DuplicateMatchArm(String),
    #[error("non-exhaustive match; missing variants: {0}")]
    NonExhaustiveMatch(String),
    #[error("match arms after '_' are unreachable")]
    UnreachableMatchArm,
    #[error("{e}")]
    ArgumentError { e: Box<ArgumentError> },
    #[error("mismatched types, expected '{expected}' but got '{found}'")]
    MismatchedType { expected: String, found: String },
    #[error("mismatched array members types, expected '{expected} for array' but got '{found}'")]
    MismatchedArrayType { expected: String, found: String },
    #[error("mismatched yielded types, expected '{expected}', but got '{found}'")]
    MismatchedYieldTypes { expected: String, found: String },
    #[error("mismatched returned types, expected '{expected}', but got '{found}'")]
    MismatchedReturnTypes { expected: String, found: String },
    #[error("mismatched types on branches, expected '{expected}', but got '{found}'")]
    MismatchedBranchTypes { expected: String, found: String },
    #[error("'{lhs}' and '{rhs}' are not valid types for operation '{op}'")]
    MismatchedBinaryOpType {
        op: BinaryOp,
        lhs: String,
        rhs: String,
    },
    #[error("undefined variable '{0}'")]
    UndefinedVariable(String),
    #[error("'{0}' is a module namespace, not a value")]
    NotAValue(String),
    #[error("name '{0}' is already defined in this module scope")]
    NameCollision(String),
    #[error("unknown export '{member}' from module '{module}'")]
    UnknownModuleExport { module: String, member: String },
    #[error("cannot assign through module member path '{0}'")]
    ModuleMemberAssignment(String),
    #[error("{found} is not an iterable type")]
    NotIterable { found: String },
    #[error("{0} is not a valid assignment target")]
    InvalidAssignmentTarget(String),
    #[error("{0} is not a valid indexing target")]
    InvalidIndexingTarget(String),
    #[error("{0} is not a valid indexing type")]
    InvalidIndexType(String),
    #[error("cannot return outside a function")]
    ReturnOutsideFunction,
    #[error("yield used outside of an effect handler")]
    YieldOutsideHandler,
    #[error("'{0}' is not a compile-time constant")]
    NotAConstant(String),
    #[error("{0} is not allowed in a constant expression")]
    ConstExpressionNotAllowed(String),
    #[error("constant dependency cycle: {0}")]
    ConstantCycle(String),
    #[error("constant evaluation failed: {0}")]
    ConstantEvaluation(String),
}

#[derive(Debug, Error, Clone)]
#[error("{kind}")]
pub struct ArgumentError {
    pub kind: ArgumentErrorKind,
    pub span: Option<Span>,
}

#[derive(Debug, Error, Clone)]
pub enum ArgumentErrorKind {
    #[error("could not find parameter with name '{name}'")]
    UnknownName { name: String },

    #[error("duplicate parameter '{name}'")]
    Duplicate { name: String },

    #[error("missing parameter '{name}'")]
    Missing { name: String },

    #[error("too many arguments")]
    TooMany,

    #[error("a named argument cannot be followed by a positional one")]
    PositionalAfterNamed,

    #[error("this constructor requires named arguments")]
    NamedOnly,

    #[error("ambiguous function call")]
    Ambiguous,

    #[error("expected type '{expected}' for parameter '{parameter}', but found '{found}'")]
    TypeMismatch {
        parameter: String,
        expected: Type,
        found: Type,
    },
}
