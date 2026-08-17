use std::{collections::HashMap, fmt::Display};

use crate::{
    ast::{TypeAnnotation, TypeExpr},
    errors::TypeErrorKind,
    module::ModuleId,
    source::Span,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Type {
    Unit,
    Int,
    Float,
    Bool,
    String,

    Tuple(Vec<Type>),
    Array(Box<Type>),
    Range,
    Nominal(TypeId),

    Function {
        parameter_overloads: Vec<ParameterTypes>,
        return_type: Box<Type>,
    },

    Any,
    Unknown,
    TypeVar(String),
    Constructor(ConstructorSignature),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TypeId {
    pub module: ModuleId,
    pub path: Vec<String>,
}

impl Display for TypeId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.path.join("::"))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeDefinition {
    pub id: TypeId,
    pub public: bool,
    pub kind: TypeDefinitionKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeDefinitionKind {
    Struct(StructDefinition),
    Enum(EnumDefinition),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StructDefinition {
    pub fields: Vec<FieldDefinition>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldDefinition {
    pub name: String,
    pub ty: Type,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnumDefinition {
    pub variants: Vec<VariantDefinition>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VariantDefinition {
    pub name: String,
    pub payload: VariantPayloadDefinition,
    pub index: usize,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VariantPayloadDefinition {
    Unit,
    Value(Type),
    InlineStruct(TypeId),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConstructorSignature {
    Struct {
        type_id: TypeId,
        fields: Vec<ParameterType>,
    },
    EnumVariant {
        enum_id: TypeId,
        variant_index: usize,
        parameters: Vec<ParameterType>,
        named_only: bool,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TypeContext {
    pub names: HashMap<Vec<String>, Type>,
}

impl TypeContext {
    pub fn with_primitives() -> Self {
        let mut context = Self::default();
        for (name, ty) in [
            ("Int", Type::Int),
            ("Float", Type::Float),
            ("Bool", Type::Bool),
            ("String", Type::String),
            ("Unit", Type::Unit),
            ("Any", Type::Any),
            ("Range", Type::Range),
        ] {
            context.names.insert(vec![name.to_string()], ty);
        }
        context
    }

    pub fn resolve_annotation(&self, annotation: &TypeAnnotation) -> Result<Type, TypeErrorKind> {
        match annotation {
            TypeAnnotation::Inferred => Ok(Type::Any),
            TypeAnnotation::Explicit(expression) => self.resolve_type_expr(expression),
        }
    }

    pub fn resolve_type_expr(&self, expression: &TypeExpr) -> Result<Type, TypeErrorKind> {
        match expression {
            TypeExpr::Unit => Ok(Type::Unit),
            TypeExpr::Path(path) => self
                .names
                .get(path)
                .cloned()
                .ok_or_else(|| TypeErrorKind::UnknownType(path.join("::"))),
            TypeExpr::Apply {
                constructor,
                arguments,
            } if constructor.as_slice() == ["Arr"] && arguments.len() == 1 => Ok(Type::Array(
                Box::new(self.resolve_type_expr(&arguments[0])?),
            )),
            TypeExpr::Apply { constructor, .. } => Err(TypeErrorKind::UnsupportedTypeApplication(
                constructor.join("::"),
            )),
            TypeExpr::Tuple(items) => items
                .iter()
                .map(|item| self.resolve_type_expr(item))
                .collect::<Result<Vec<_>, _>>()
                .map(Type::Tuple),
            TypeExpr::Function {
                parameters,
                return_type,
            } => Ok(Type::Function {
                parameter_overloads: vec![
                    parameters
                        .iter()
                        .enumerate()
                        .map(|(index, parameter)| {
                            Ok(ParameterType {
                                name: format!("_{index}"),
                                ty: self.resolve_type_expr(parameter)?,
                            })
                        })
                        .collect::<Result<_, TypeErrorKind>>()?,
                ],
                return_type: Box::new(self.resolve_type_expr(return_type)?),
            }),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParameterType {
    pub name: String,
    pub ty: Type,
}

impl crate::ast::CallParameter for ParameterType {
    fn name(&self) -> &str {
        &self.name
    }
}

pub type ParameterTypes = Vec<ParameterType>;

impl Display for Type {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Type::Unit => write!(f, "Unit"),
            Type::Int => write!(f, "Int"),
            Type::Float => write!(f, "Float"),
            Type::Bool => write!(f, "Bool"),
            Type::String => write!(f, "String"),
            Type::Tuple(items) => {
                write!(f, "(")?;
                for (i, item) in items.iter().enumerate() {
                    write!(f, "{item}")?;
                    if i < items.len() - 1 {
                        write!(f, ", ")?;
                    }
                }
                write!(f, ")")?;
                Ok(())
            }
            Type::Array(t) => write!(f, "Arr<{t}>"),
            Type::Function {
                parameter_overloads: overloaded_parameters,
                return_type,
            } => {
                write!(f, "(")?;
                for (i, parameters_types) in overloaded_parameters.iter().enumerate() {
                    for (j, param) in parameters_types.iter().enumerate() {
                        write!(f, "{}", param.ty)?;
                        if j < parameters_types.len() - 1 {
                            write!(f, ", ")?;
                        }
                    }
                    if i < overloaded_parameters.len() - 1 {
                        write!(f, "| ")?;
                    }
                }
                write!(f, ") -> {return_type}")?;
                Ok(())
            }
            Type::Any => write!(f, "Any"),
            Type::Unknown => write!(f, "Unknown"),
            Type::Range => write!(f, "Range"),
            Type::Nominal(id) => write!(f, "{id}"),
            Type::TypeVar(t) => write!(f, "TypeVar[{t}]"),
            Type::Constructor(signature) => match signature {
                ConstructorSignature::Struct { type_id, fields } => {
                    write!(f, "constructor {type_id}(")?;
                    for (index, field) in fields.iter().enumerate() {
                        if index > 0 {
                            write!(f, ", ")?;
                        }
                        write!(f, "{}: {}", field.name, field.ty)?;
                    }
                    write!(f, ")")
                }
                ConstructorSignature::EnumVariant { enum_id, .. } => {
                    write!(f, "constructor {enum_id}")
                }
            },
        }
    }
}
