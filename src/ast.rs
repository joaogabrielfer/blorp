use std::fmt::Display;

use crate::{interpreter::values::Value, source::Span, typechecker::ty::Type};

#[derive(Debug, Clone)]
pub struct Program {
    pub items: Vec<Item>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Item {
    Import(ImportDecl),
    Type(TypeDecl),
    Const(ConstDecl),
    Function(FunctionDecl),
}

impl Item {
    pub fn span(&self) -> Span {
        match self {
            Self::Import(decl) => decl.span,
            Self::Type(decl) => decl.span,
            Self::Const(decl) => decl.span,
            Self::Function(decl) => decl.span,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct TypeDecl {
    pub public: bool,
    pub name: String,
    pub kind: TypeDeclKind,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TypeDeclKind {
    Struct(StructDecl),
    Enum(EnumDecl),
}

#[derive(Debug, Clone, PartialEq)]
pub struct StructDecl {
    pub fields: Vec<StructFieldDecl>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StructFieldDecl {
    pub name: String,
    pub ty: TypeExpr,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EnumDecl {
    pub variants: Vec<EnumVariantDecl>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EnumVariantDecl {
    pub name: String,
    pub payload: Option<EnumPayloadDecl>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum EnumPayloadDecl {
    Type(TypeExpr),
    InlineStruct(StructDecl),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ConstDecl {
    pub public: bool,
    pub name: String,
    pub type_annotation: TypeExpr,
    pub value: Expr,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FunctionDecl {
    pub public: bool,
    pub name: String,
    pub generics: Vec<String>,
    pub parameters: Vec<Parameter>,
    pub return_type: TypeAnnotation,
    pub body: Block,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ImportDecl {
    pub source: ImportSource,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Stmt {
    Expr(Expr),
    Return(Expr),
    Yield(Expr),

    Bind {
        mutable: bool,
        name: String,
        type_annotation: TypeAnnotation,
        value: Expr,
        span: Span,
    },

    Assignment {
        target: Expr,
        value: Expr,
    },

    Function(FunctionDecl),
}

impl Stmt {
    pub fn span(&self) -> Span {
        match self {
            Stmt::Expr(expr) => expr.span,
            Stmt::Return(expr) => expr.span,
            Stmt::Yield(expr) => expr.span,
            Stmt::Bind { span, .. } => *span,
            Stmt::Assignment { target, value } => Span {
                start: target.span.start,
                end: value.span.end,
            },
            Stmt::Function(decl) => decl.span,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ImportSource {
    Relative {
        path: String,
        mode: RelativeImportMode,
    },

    Module {
        tree: ImportTree,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum RelativeImportMode {
    Glob,

    Namespace { alias: String },
}

#[derive(Debug, Clone, PartialEq)]
pub enum ImportTree {
    Path {
        segments: Vec<String>,
        alias: Option<String>,
    },

    Group {
        module_path: Vec<String>,
        items: Vec<ImportItem>,
    },

    Glob {
        module_path: Vec<String>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct ImportItem {
    pub name: String,
    pub alias: Option<String>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TypeAnnotation {
    Inferred,
    Explicit(TypeExpr),
}

#[derive(Debug, Clone, PartialEq)]
pub enum TypeExpr {
    Path(Vec<String>),
    Apply {
        constructor: Vec<String>,
        arguments: Vec<TypeExpr>,
    },
    Tuple(Vec<TypeExpr>),
    Function {
        parameters: Vec<TypeExpr>,
        return_type: Box<TypeExpr>,
    },
    Unit,
}
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq)]
pub struct Parameter {
    pub name: String,
    pub t: TypeAnnotation,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Expr {
    pub kind: ExprKind,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ExprKind {
    Int(i64),
    Float(f64),
    String(String),
    Bool(bool),
    Ident(String),
    Path(Vec<String>),
    Unit,

    Block(Block),

    Tuple(Vec<Expr>),
    Array(Vec<Expr>),

    Unary {
        op: UnaryOp,
        operand: Box<Expr>,
    },

    Binary {
        lhs: Box<Expr>,
        op: BinaryOp,
        rhs: Box<Expr>,
    },

    Call {
        callee: Box<Expr>,
        args: Vec<ExprArgument>,
    },

    If {
        condition: Box<Expr>,
        then_branch: Box<Expr>,
        else_branch: Option<Box<Expr>>,
    },

    While {
        condition: Box<Expr>,
        block: Block,
    },

    For {
        binding: Box<Expr>,
        iterable: Box<Expr>,
        block: Block,
    },

    Lambda {
        parameters: Vec<Parameter>,
        body: Box<Expr>,
    },

    Index {
        target: Box<Expr>,
        index: Box<Expr>,
    },

    Field {
        target: Box<Expr>,
        name: String,
    },

    Match {
        value: Box<Expr>,
        arms: Vec<MatchArm>,
    },
}

impl Display for ExprKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExprKind::Int(i) => write!(f, "integer '{i}'"),
            ExprKind::Float(d) => write!(f, "float '{d}'"),
            ExprKind::String(s) => write!(f, "string '{s}'"),
            ExprKind::Bool(b) => write!(f, "bool '{b}'"),
            ExprKind::Ident(i) => write!(f, "identifier '{i}'"),
            ExprKind::Path(segments) => write!(f, "path '{}'", segments.join("::")),
            ExprKind::Unit => write!(f, "unit"),
            ExprKind::Block(_) => write!(f, "block"),
            ExprKind::Tuple(_) => write!(f, "tuple"),
            ExprKind::Array(_) => write!(f, "array"),
            ExprKind::Unary { .. } => write!(f, "unary operation"),
            ExprKind::Binary { .. } => write!(f, "binary operation"),
            ExprKind::Call { .. } => write!(f, "function calling"),
            ExprKind::If { .. } => write!(f, "if condition"),
            ExprKind::While { .. } => write!(f, "while loop"),
            ExprKind::For { .. } => write!(f, "for loop"),
            ExprKind::Lambda { .. } => write!(f, "lambda"),
            ExprKind::Index { .. } => write!(f, "index"),
            ExprKind::Field { .. } => write!(f, "field access"),
            ExprKind::Match { .. } => write!(f, "match"),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct MatchArm {
    pub pattern: Pattern,
    pub body: Block,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Pattern {
    Wildcard {
        span: Span,
    },
    EnumVariant {
        qualifier: Option<Vec<String>>,
        variant: String,
        binding: Option<String>,
        span: Span,
    },
}

pub type ExprArgument = CallArgument<Expr>;
pub type ValueArgument = CallArgument<Value>;
pub type TypeArgument = CallArgument<Type>;

#[derive(Debug, Clone, PartialEq)]
pub struct ArgumentName {
    pub value: String,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CallArgument<T> {
    pub name: Option<ArgumentName>,
    pub value: T,
    pub span: Span,
}

pub trait CallParameter {
    fn name(&self) -> &str;
}

impl CallParameter for Parameter {
    fn name(&self) -> &str {
        &self.name
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Block {
    pub statements: Vec<Stmt>,
}

impl Block {
    pub fn span(&self) -> Span {
        let start = self.statements.first();
        let end = self.statements.last();

        match (start, end) {
            (Some(s), Some(e)) => Span {
                start: s.span().start,
                end: e.span().end,
            },
            _ => Span::dummy(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BinaryOp {
    Add,
    Sub,
    Mul,
    Div,
    Eq,
    NotEq,
    Greater,
    GreaterEq,
    Less,
    LessEq,
    Combine,
    InclusiveRange,
    ExclusiveRange,
}

impl Display for BinaryOp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BinaryOp::Add => write!(f, "+"),
            BinaryOp::Sub => write!(f, "-"),
            BinaryOp::Mul => write!(f, "*"),
            BinaryOp::Div => write!(f, "/"),
            BinaryOp::Eq => write!(f, "="),
            BinaryOp::NotEq => write!(f, "!="),
            BinaryOp::Greater => write!(f, ">"),
            BinaryOp::GreaterEq => write!(f, ">="),
            BinaryOp::Less => write!(f, "<"),
            BinaryOp::LessEq => write!(f, "<="),
            BinaryOp::Combine => write!(f, "<>"),
            BinaryOp::InclusiveRange => write!(f, "..="),
            BinaryOp::ExclusiveRange => write!(f, "..<"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum UnaryOp {
    Negate,
    Not,
}

impl Display for UnaryOp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UnaryOp::Not => write!(f, "!"),
            UnaryOp::Negate => write!(f, "-"),
        }
    }
}
