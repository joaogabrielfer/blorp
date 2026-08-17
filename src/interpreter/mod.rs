use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    rc::Rc,
};

use anyhow::Result;

use crate::{
    errors::{RuntimeError, RuntimeErrorKind},
    interpreter::env::Env,
    source::Span,
    typechecker::ty::{TypeContext, TypeDefinition, TypeId},
};

pub mod builtins;
pub mod env;
pub mod eval;
pub mod values;

pub struct Interpreter<W: std::io::Write> {
    env: Env,
    file_path: PathBuf,
    output: W,
    pub(crate) type_context: Rc<TypeContext>,
    pub(crate) type_definitions: HashMap<TypeId, TypeDefinition>,
}

impl<W: std::io::Write> Interpreter<W> {
    pub fn new(file_path: &Path, output: W) -> Self {
        let mut env = Env::default();
        env.load_builtins();
        Self {
            env,
            file_path: file_path.to_path_buf(),
            output,
            type_context: Rc::new(TypeContext::with_primitives()),
            type_definitions: HashMap::new(),
        }
    }
    pub fn new_with_env(file_path: &Path, env: Env, output: W) -> Self {
        Self {
            env,
            file_path: file_path.to_path_buf(),
            output,
            type_context: Rc::new(TypeContext::with_primitives()),
            type_definitions: HashMap::new(),
        }
    }
    fn _write_output(&mut self, value: impl std::fmt::Display) -> Result<(), Box<RuntimeError>> {
        write!(self.output, "{value}").map_err(|err| self.runtime_io_error(err))
    }
    fn writeln_output(&mut self, value: impl std::fmt::Display) -> Result<(), Box<RuntimeError>> {
        writeln!(self.output, "{value}").map_err(|err| self.runtime_io_error(err))
    }

    fn runtime_io_error(&self, err: std::io::Error) -> Box<RuntimeError> {
        self.error_at(Span::dummy(), RuntimeErrorKind::IoError(err.to_string()))
    }

    pub(crate) fn replace_context(&mut self, env: Env, file_path: PathBuf) -> (Env, PathBuf) {
        let previous_env = std::mem::replace(&mut self.env, env);
        let previous_path = std::mem::replace(&mut self.file_path, file_path);
        (previous_env, previous_path)
    }

    pub(crate) fn restore_context(&mut self, env: Env, file_path: PathBuf) {
        self.env = env;
        self.file_path = file_path;
    }

    pub(crate) fn install_type_metadata(
        &mut self,
        type_context: Rc<TypeContext>,
        definitions: &HashMap<TypeId, TypeDefinition>,
    ) {
        self.type_context = type_context;
        self.type_definitions.extend(definitions.clone());
    }
}

impl Interpreter<Vec<u8>> {
    pub fn new_buffered(file_path: &Path) -> Self {
        Self::new(file_path, Vec::new())
    }

    pub fn into_output_string(self) -> String {
        String::from_utf8(self.output).expect("interpreter output should be valid UTF-8")
    }

    pub fn take_output_string(&mut self) -> String {
        String::from_utf8(std::mem::take(&mut self.output))
            .expect("interpreter output should be valid UTF-8")
    }
}
