use std::collections::HashMap;

use crate::{
    errors::TypeErrorKind,
    module::{ExportedSymbolKind, ModuleId},
    typechecker::ty::Type,
};

#[derive(Debug, Clone)]
pub enum TypeBinding {
    Local(Type),
    Const(Type),
    ImportedMember {
        ty: Box<Type>,
        kind: Box<ExportedSymbolKind>,
    },
    Module(ModuleId),
}

impl TypeBinding {
    pub fn value_type(&self) -> Option<Type> {
        match self {
            Self::Local(ty) | Self::Const(ty) => Some(ty.clone()),
            Self::ImportedMember { ty, .. } => Some((**ty).clone()),
            Self::Module(_) => None,
        }
    }
}

#[derive(Debug, Default, Clone)]
pub struct TypeEnv {
    scopes: Vec<HashMap<String, TypeBinding>>,
    types: HashMap<String, Type>,
}

impl TypeEnv {
    pub fn new() -> Self {
        Self {
            scopes: vec![HashMap::new()],
            types: HashMap::new(),
        }
    }
    pub fn push_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }
    pub fn pop_scope(&mut self) {
        self.scopes.pop().expect("shoulnt pop global scope");
    }

    pub fn define(&mut self, name: String, ty: Type) {
        self.define_binding(name, TypeBinding::Local(ty));
    }

    pub fn define_const(&mut self, name: String, ty: Type) {
        self.define_binding(name, TypeBinding::Const(ty));
    }

    pub fn define_binding(&mut self, name: String, binding: TypeBinding) {
        self.scopes
            .last_mut()
            .expect("should always have at least one scope")
            .insert(name, binding);
    }

    pub fn get_binding(&self, name: &str) -> Result<TypeBinding, TypeErrorKind> {
        for scope in self.scopes.iter().rev() {
            if let Some(value) = scope.get(name) {
                return Ok(value.clone());
            }
        }
        Err(TypeErrorKind::UndefinedVariable(name.to_string()))
    }

    pub fn get(&self, name: &str) -> Result<Type, TypeErrorKind> {
        self.get_binding(name)?
            .value_type()
            .ok_or_else(|| TypeErrorKind::NotAValue(name.to_string()))
    }

    pub fn get_current(&self, name: &str) -> Option<TypeBinding> {
        self.scopes
            .last()
            .and_then(|scope| scope.get(name))
            .cloned()
    }

    pub fn at_module_scope(&self) -> bool {
        self.scopes.len() == 1
    }

    pub fn define_type(&mut self, name: String, ty: Type) {
        self.types.insert(name, ty);
    }

    pub fn get_type(&self, name: &str) -> Option<Type> {
        self.types.get(name).cloned()
    }

    pub fn get_current_type(&self, name: &str) -> Option<Type> {
        self.types.get(name).cloned()
    }
}
