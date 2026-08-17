use std::{
    collections::{HashMap, HashSet},
    fs::read_to_string,
    path::{Path, PathBuf},
    rc::Rc,
};

use anyhow::{Context, Result, anyhow, bail};

use crate::{
    ast::{ImportSource, ImportTree, Item, Program, RelativeImportMode},
    errors::RuntimeError,
    interpreter::{
        Interpreter,
        env::Env,
        values::{ModuleValue, Value},
    },
    lexer::Lexer,
    parser::Parser,
    source::Span,
    typechecker::{
        CheckedModule, TypeChecker,
        ty::{Type, TypeContext, TypeDefinition, TypeId},
    },
};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ModuleId {
    pub origin: ModuleOrigin,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ModuleOrigin {
    File(PathBuf),
    Standard(Vec<String>),
}

impl std::fmt::Display for ModuleId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.origin {
            ModuleOrigin::File(path) => write!(f, "{}", path.display()),
            ModuleOrigin::Standard(path) => write!(f, "std::{}", path.join("::")),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ExportedSymbolKind {
    Function,
    Const { value: Value },
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExportedSymbol {
    pub ty: Type,
    pub kind: ExportedSymbolKind,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExportedType {
    pub id: TypeId,
}

#[derive(Debug, Clone, Default)]
pub struct ModuleInterface {
    pub exports: HashMap<String, ExportedSymbol>,
    pub types: HashMap<String, ExportedType>,
    pub type_definitions: HashMap<TypeId, TypeDefinition>,
}

#[derive(Debug, Clone)]
pub enum ResolvedImport {
    Module {
        module: ModuleId,
        local_name: String,
        span: Span,
    },
    Member {
        module: ModuleId,
        export_name: String,
        local_name: String,
        span: Span,
    },
}

impl ResolvedImport {
    pub fn local_name(&self) -> &str {
        match self {
            Self::Module { local_name, .. } | Self::Member { local_name, .. } => local_name,
        }
    }

    pub fn span(&self) -> Span {
        match self {
            Self::Module { span, .. } | Self::Member { span, .. } => *span,
        }
    }
}

#[derive(Debug, Clone)]
pub struct CompiledModule {
    pub id: ModuleId,
    pub path: PathBuf,
    pub program: Program,
    pub imports: Vec<ResolvedImport>,
    pub interface: ModuleInterface,
    pub constants: HashMap<String, Value>,
    pub type_definitions: HashMap<TypeId, TypeDefinition>,
    pub type_context: Rc<TypeContext>,
}

#[derive(Debug, Clone)]
pub struct Compilation {
    pub root: ModuleId,
    pub modules: HashMap<ModuleId, CompiledModule>,
}

pub struct CompilerSession {
    modules: HashMap<ModuleId, CompiledModule>,
    loading: HashSet<ModuleId>,
    loading_stack: Vec<ModuleId>,
    standard_root: PathBuf,
}

impl Default for CompilerSession {
    fn default() -> Self {
        Self::new(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("std"))
    }
}

impl CompilerSession {
    pub fn new(standard_root: PathBuf) -> Self {
        Self {
            modules: HashMap::new(),
            loading: HashSet::new(),
            loading_stack: Vec::new(),
            standard_root,
        }
    }

    pub fn compile_file(mut self, path: &Path) -> Result<Compilation> {
        let path = path
            .canonicalize()
            .with_context(|| format!("could not resolve root module '{}'", path.display()))?;
        let root = ModuleId {
            origin: ModuleOrigin::File(path.clone()),
        };
        self.load_module(root.clone(), path)?;
        self.validate_root_entry(&root)?;
        Ok(Compilation {
            root,
            modules: self.modules,
        })
    }

    fn validate_root_entry(&self, root: &ModuleId) -> Result<()> {
        let module = self
            .modules
            .get(root)
            .expect("the root module was loaded before entry validation");
        let mains = module
            .program
            .items
            .iter()
            .filter_map(|item| match item {
                Item::Function(decl) if decl.name == "main" => Some(decl),
                _ => None,
            })
            .collect::<Vec<_>>();
        let [main] = mains.as_slice() else {
            bail!(
                "root module '{}' must declare exactly one `fun main(): ()` entry point",
                module.path.display()
            );
        };
        if !main.generics.is_empty()
            || !main.parameters.is_empty()
            || !matches!(
                main.return_type,
                crate::ast::TypeAnnotation::Explicit(crate::ast::TypeExpr::Unit)
            )
        {
            bail!(
                "entry point `main` in '{}' must have the signature `fun main(): ()`",
                module.path.display()
            );
        }
        Ok(())
    }

    fn load_module(&mut self, id: ModuleId, path: PathBuf) -> Result<()> {
        if self.modules.contains_key(&id) {
            return Ok(());
        }

        if self.loading.contains(&id) {
            let first = self
                .loading_stack
                .iter()
                .position(|candidate| candidate == &id)
                .unwrap_or(0);
            let mut chain = self.loading_stack[first..].to_vec();
            chain.push(id);
            bail!(
                "circular module import: {}",
                chain
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(" -> ")
            );
        }

        self.loading.insert(id.clone());
        self.loading_stack.push(id.clone());

        let result = self.load_module_inner(id.clone(), path);

        self.loading_stack.pop();
        self.loading.remove(&id);

        result
    }

    fn load_module_inner(&mut self, id: ModuleId, path: PathBuf) -> Result<()> {
        let program = self.parse_module(&path)?;
        self.validate_import_placement(&program, &path)?;

        let mut imports = Vec::new();
        for item in &program.items {
            let Item::Import(import) = item else {
                continue;
            };
            imports.extend(
                self.resolve_import(&import.source, import.span, &path)
                    .with_context(|| format!("while resolving import in {}", path.display()))?,
            );
        }

        let interfaces = self
            .modules
            .iter()
            .map(|(module_id, module)| (module_id.clone(), module.interface.clone()))
            .collect();
        let CheckedModule {
            program,
            interface,
            constants,
            type_definitions,
        } = TypeChecker::for_module(path.clone(), id.clone()).check_module(
            program,
            &imports,
            &interfaces,
        )?;
        let crate::typechecker::CheckedProgram {
            program,
            type_context,
        } = program;

        self.modules.insert(
            id.clone(),
            CompiledModule {
                id,
                path,
                program,
                imports,
                interface,
                constants,
                type_definitions,
                type_context,
            },
        );
        Ok(())
    }

    fn parse_module(&self, path: &Path) -> Result<Program> {
        let path = path.to_path_buf();
        let source = read_to_string(&path)
            .with_context(|| format!("could not read module '{}'", path.display()))?;
        let tokens = Lexer::new(&path, &source).tokenize()?;
        Ok(Parser::new(tokens, &path).parse_program()?)
    }

    fn validate_import_placement(&self, program: &Program, path: &Path) -> Result<()> {
        let mut seen_non_import = false;
        for item in &program.items {
            match item {
                Item::Import(_) if !seen_non_import => {}
                Item::Import(import) => {
                    bail!(
                        "import at {}:{} must appear before declarations and statements in '{}'",
                        import.span.start.line,
                        import.span.start.col,
                        path.display()
                    );
                }
                _ => seen_non_import = true,
            }
        }
        Ok(())
    }

    fn resolve_import(
        &mut self,
        source: &ImportSource,
        span: Span,
        importer_path: &Path,
    ) -> Result<Vec<ResolvedImport>> {
        match source {
            ImportSource::Relative { path, mode } => {
                let module = self.resolve_relative_module(importer_path, path)?;
                self.load_module(module.clone(), self.path_for_module(&module)?)?;
                match mode {
                    RelativeImportMode::Namespace { alias } => Ok(vec![ResolvedImport::Module {
                        module,
                        local_name: alias.clone(),
                        span,
                    }]),
                    RelativeImportMode::Glob => self.resolve_glob(module, span),
                }
            }
            ImportSource::Module { tree } => match tree {
                ImportTree::Path { segments, alias } => {
                    let (module, member) = self.resolve_module_or_member(segments)?;
                    self.load_module(module.clone(), self.path_for_module(&module)?)?;
                    if let Some(export_name) = member {
                        self.resolve_member(module, &export_name, alias.as_deref(), span)
                    } else {
                        let local_name = alias.clone().unwrap_or_else(|| {
                            segments
                                .last()
                                .expect("module import has a final segment")
                                .clone()
                        });
                        Ok(vec![ResolvedImport::Module {
                            module,
                            local_name,
                            span,
                        }])
                    }
                }
                ImportTree::Group { module_path, items } => {
                    let module = self.resolve_exact_module(module_path)?;
                    self.load_module(module.clone(), self.path_for_module(&module)?)?;
                    let mut resolved = Vec::new();
                    for item in items {
                        resolved.extend(self.resolve_member(
                            module.clone(),
                            &item.name,
                            item.alias.as_deref(),
                            item.span,
                        )?);
                    }
                    Ok(resolved)
                }
                ImportTree::Glob { module_path } => {
                    let module = self.resolve_exact_module(module_path)?;
                    self.load_module(module.clone(), self.path_for_module(&module)?)?;
                    self.resolve_glob(module, span)
                }
            },
        }
    }

    fn resolve_relative_module(&self, importer_path: &Path, raw_path: &str) -> Result<ModuleId> {
        let parent = importer_path.parent().ok_or_else(|| {
            anyhow!(
                "module '{}' has no parent directory",
                importer_path.display()
            )
        })?;
        let path = parent.join(raw_path).canonicalize().with_context(|| {
            format!(
                "could not resolve relative module '{}' from '{}'",
                raw_path,
                importer_path.display()
            )
        })?;
        if path.extension().and_then(|extension| extension.to_str()) != Some("fob") {
            bail!(
                "relative module '{}' must have a .fob extension",
                path.display()
            );
        }
        Ok(ModuleId {
            origin: ModuleOrigin::File(path),
        })
    }

    fn resolve_module_or_member(&self, segments: &[String]) -> Result<(ModuleId, Option<String>)> {
        for length in (1..=segments.len()).rev() {
            if let Ok(module) = self.resolve_exact_module(&segments[..length]) {
                let remainder = &segments[length..];
                return match remainder {
                    [] => Ok((module, None)),
                    [member] => Ok((module, Some(member.clone()))),
                    _ => bail!(
                        "'{}' is not a valid module or member import",
                        segments.join("::")
                    ),
                };
            }
        }
        bail!("unknown module '{}'", segments.join("::"))
    }

    fn resolve_exact_module(&self, segments: &[String]) -> Result<ModuleId> {
        let Some((first, rest)) = segments.split_first() else {
            bail!("empty module path");
        };
        if first != "std" || rest.is_empty() {
            bail!("unknown module '{}'", segments.join("::"));
        }
        let mut path = self.standard_root.clone();
        for segment in rest {
            path.push(segment);
        }
        path.set_extension("fob");
        if !path.is_file() {
            bail!("unknown standard module '{}'", segments.join("::"));
        }
        Ok(ModuleId {
            origin: ModuleOrigin::Standard(rest.to_vec()),
        })
    }

    fn path_for_module(&self, id: &ModuleId) -> Result<PathBuf> {
        match &id.origin {
            ModuleOrigin::File(path) => Ok(path.clone()),
            ModuleOrigin::Standard(segments) => {
                let mut path = self.standard_root.clone();
                for segment in segments {
                    path.push(segment);
                }
                path.set_extension("fob");
                Ok(path)
            }
        }
    }

    fn resolve_member(
        &self,
        module: ModuleId,
        export_name: &str,
        alias: Option<&str>,
        span: Span,
    ) -> Result<Vec<ResolvedImport>> {
        let interface = &self
            .modules
            .get(&module)
            .expect("loaded module must have an interface")
            .interface;
        if !interface.exports.contains_key(export_name)
            && !interface.types.contains_key(export_name)
        {
            bail!("module '{}' does not export '{}'", module, export_name);
        }
        Ok(vec![ResolvedImport::Member {
            module,
            export_name: export_name.to_string(),
            local_name: alias.unwrap_or(export_name).to_string(),
            span,
        }])
    }

    fn resolve_glob(&self, module: ModuleId, span: Span) -> Result<Vec<ResolvedImport>> {
        let interface = &self
            .modules
            .get(&module)
            .expect("loaded module must have an interface")
            .interface;
        let mut export_names = interface
            .exports
            .keys()
            .chain(interface.types.keys())
            .cloned()
            .collect::<HashSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        export_names.sort();
        Ok(export_names
            .into_iter()
            .map(|export_name| ResolvedImport::Member {
                module: module.clone(),
                export_name: export_name.clone(),
                local_name: export_name,
                span,
            })
            .collect())
    }
}

pub struct RuntimeModules {
    root: ModuleId,
    modules: HashMap<ModuleId, CompiledModule>,
    instances: HashMap<ModuleId, Rc<ModuleValue>>,
    initializing: Vec<ModuleId>,
}

impl RuntimeModules {
    pub fn new(compilation: Compilation) -> Self {
        Self {
            root: compilation.root,
            modules: compilation.modules,
            instances: HashMap::new(),
            initializing: Vec::new(),
        }
    }

    pub fn execute_root<W: std::io::Write>(
        &mut self,
        interpreter: &mut Interpreter<W>,
    ) -> Result<Value> {
        let root = self.root.clone();
        let (instance, _) = self.initialize(&root, interpreter)?;
        let module = self
            .modules
            .get(&root)
            .expect("the root module remains in the compilation")
            .clone();
        let (previous_env, previous_path) =
            interpreter.replace_context(Env::from_ref(instance.env.clone()), module.path);
        let result = interpreter.run_main();
        interpreter.restore_context(previous_env, previous_path);
        result.map_err(Into::into)
    }

    fn initialize<W: std::io::Write>(
        &mut self,
        id: &ModuleId,
        interpreter: &mut Interpreter<W>,
    ) -> Result<(Rc<ModuleValue>, Value)> {
        if let Some(instance) = self.instances.get(id) {
            return Ok((instance.clone(), Value::Unit));
        }
        if let Some(first) = self.initializing.iter().position(|module| module == id) {
            let mut chain = self.initializing[first..].to_vec();
            chain.push(id.clone());
            bail!(
                "circular runtime module initialization: {}",
                chain
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(" -> ")
            );
        }

        let module = self
            .modules
            .get(id)
            .cloned()
            .expect("compiled dependencies must be present in the runtime module table");
        self.initializing.push(id.clone());

        let result = self.initialize_inner(module, interpreter);

        self.initializing.pop();
        result
    }

    fn initialize_inner<W: std::io::Write>(
        &mut self,
        module: CompiledModule,
        interpreter: &mut Interpreter<W>,
    ) -> Result<(Rc<ModuleValue>, Value)> {
        let dependency_ids = module
            .imports
            .iter()
            .map(|import| match import {
                ResolvedImport::Module { module, .. } | ResolvedImport::Member { module, .. } => {
                    module.clone()
                }
            })
            .collect::<HashSet<_>>();
        for dependency in dependency_ids {
            self.initialize(&dependency, interpreter).with_context(|| {
                format!(
                    "while initializing dependency '{}' for '{}'",
                    dependency, module.id
                )
            })?;
        }

        let mut env = Env::default();
        env.load_builtins();
        for import in &module.imports {
            match import {
                ResolvedImport::Module {
                    module: imported_module,
                    local_name,
                    ..
                } => {
                    let instance = self.instances.get(imported_module).expect(
                        "dependencies must be initialized before their import bindings are installed",
                    );
                    env.define_module(local_name.clone(), instance.clone());
                }
                ResolvedImport::Member {
                    module: imported_module,
                    export_name,
                    local_name,
                    ..
                } => {
                    let instance = self.instances.get(imported_module).expect(
                        "dependencies must be initialized before their import bindings are installed",
                    );
                    env.define_imported_member(
                        local_name.clone(),
                        instance.clone(),
                        export_name.clone(),
                    );
                }
            }
        }

        let instance = Rc::new(ModuleValue {
            id: module.id.clone(),
            env: env.current_ref(),
            exports: module.interface.exports.keys().cloned().collect(),
        });
        let (previous_env, previous_path) = interpreter.replace_context(env, module.path.clone());
        let eval_result = interpreter.load_module(
            &module.program,
            &module.constants,
            module.type_context.clone(),
            &module.type_definitions,
        );
        interpreter.restore_context(previous_env, previous_path);
        eval_result.map_err(|error: Box<RuntimeError>| anyhow!(error))?;

        self.instances.insert(module.id, instance.clone());
        Ok((instance, Value::Unit))
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{CompilerSession, RuntimeModules};
    use crate::interpreter::Interpreter;

    fn fixture(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures")
            .join(name)
    }

    #[test]
    fn supported_import_forms_compile_and_run() {
        for name in [
            "import.fob",
            "import_as.fob",
            "import_std.fob",
            "import_std_as.fob",
            "import_std_all.fob",
            "import_std_bind_as.fob",
            "import_std_many.fob",
            "import_std_many_bind_as.fob",
        ] {
            let path = fixture(name);
            let compilation = CompilerSession::default().compile_file(&path).unwrap();
            let mut interpreter = Interpreter::new_buffered(&path);
            RuntimeModules::new(compilation)
                .execute_root(&mut interpreter)
                .unwrap();
            assert!(interpreter.into_output_string().is_empty(), "{name}");
        }
    }

    #[test]
    fn module_instances_are_cached_and_share_declarative_constants() {
        let path = fixture("module_shared_state.fob");
        let compilation = CompilerSession::default().compile_file(&path).unwrap();
        assert_eq!(compilation.modules.len(), 4);

        let mut interpreter = Interpreter::new_buffered(&path);
        RuntimeModules::new(compilation)
            .execute_root(&mut interpreter)
            .unwrap();
        assert_eq!(interpreter.into_output_string(), "1\n1\n1\n");
    }

    #[test]
    fn invalid_module_programs_produce_specific_errors() {
        for (name, expected) in [
            ("module_private_error.fob", "unknown export 'HIDDEN'"),
            (
                "module_collision_error.fob",
                "name 'first' is already defined",
            ),
            ("module_cycle_error.fob", "circular module import: "),
            (
                "module_external_assignment_error.fob",
                "cannot assign through module member path 'counter::STEP'",
            ),
            ("module_late_import_error.fob", "expected a top-level item"),
            (
                "module_nested_import_error.fob",
                "unexpected token 'import'",
            ),
        ] {
            let path = fixture(name);
            let error = CompilerSession::default().compile_file(&path).unwrap_err();
            assert!(format!("{error:#}").contains(expected), "{name}: {error:#}");
        }
    }
}
