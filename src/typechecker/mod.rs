use std::path::PathBuf;

use crate::{
    ast::Program,
    errors::TypeError,
    module::{ModuleId, ModuleInterface},
    typechecker::{
        env::TypeEnv,
        ty::{Type, TypeContext, TypeDefinition},
    },
};

pub mod check;
pub mod env;
pub mod ty;

pub struct CheckedProgram {
    pub program: Program,
    pub type_context: std::rc::Rc<TypeContext>,
}

pub struct CheckedModule {
    pub program: CheckedProgram,
    pub interface: ModuleInterface,
    pub constants: std::collections::HashMap<String, crate::interpreter::values::Value>,
    pub type_definitions: std::collections::HashMap<ty::TypeId, TypeDefinition>,
}

#[derive(Clone)]
pub struct TypeChecker {
    file_path: PathBuf,
    env: TypeEnv,
    current_function_return: Option<Type>,
    module_interfaces: std::collections::HashMap<crate::module::ModuleId, ModuleInterface>,
    module_id: ModuleId,
    type_context: TypeContext,
    type_definitions: std::collections::HashMap<ty::TypeId, TypeDefinition>,
}

impl TypeChecker {
    pub fn new(file_path: PathBuf) -> Self {
        let module_id = ModuleId {
            origin: crate::module::ModuleOrigin::File(file_path.clone()),
        };
        Self::for_module(file_path, module_id)
    }

    pub fn for_module(file_path: PathBuf, module_id: ModuleId) -> Self {
        let type_context = TypeContext::with_primitives();
        Self {
            file_path,
            env: TypeEnv::new(),
            current_function_return: None,
            module_interfaces: std::collections::HashMap::new(),
            module_id,
            type_context,
            type_definitions: std::collections::HashMap::new(),
        }
    }
}

pub type TypeResult<A> = Result<A, Box<TypeError>>;
