use crate::{
    ast::{
        BinaryOp, Block, CallArgument, ConstDecl, EnumPayloadDecl, Expr, ExprKind, FunctionDecl,
        Item, Program, Stmt, TypeAnnotation, TypeDecl, TypeDeclKind, UnaryOp,
    },
    errors::{ArgumentError, ArgumentErrorKind, TypeError, TypeErrorKind},
    interpreter::values::{RangeValue, Value, normalize_arguments},
    module::{ExportedSymbol, ExportedSymbolKind, ExportedType, ModuleInterface, ResolvedImport},
    source::Span,
    typechecker::{
        CheckedModule, CheckedProgram, TypeChecker, TypeResult,
        env::TypeBinding,
        ty::{
            ConstructorSignature, EnumDefinition, FieldDefinition, ParameterType, StructDefinition,
            Type::{self},
            TypeDefinition, TypeDefinitionKind, TypeId, VariantDefinition,
            VariantPayloadDefinition,
        },
    },
};

#[derive(Debug, Clone)]
pub struct StmtCheck {
    pub normal_type: Type,
    pub yielded_type: Option<Type>,
    pub returned_type: Option<Type>,
}

impl StmtCheck {
    pub fn normal(ty: Type) -> Self {
        Self {
            normal_type: ty,
            yielded_type: None,
            returned_type: None,
        }
    }

    pub fn yields(ty: Type) -> Self {
        Self {
            normal_type: Type::Unit,
            yielded_type: Some(ty),
            returned_type: None,
        }
    }

    pub fn returns(ty: Type) -> Self {
        Self {
            normal_type: Type::Unit,
            yielded_type: None,
            returned_type: Some(ty),
        }
    }
}

#[derive(Debug, Clone)]
pub struct BlockCheck {
    pub ty: Type,
    pub returned_type: Option<Type>,
}

impl TypeChecker {
    pub fn check_program(&mut self, program: Program) -> TypeResult<CheckedProgram> {
        Ok(self
            .check_module(program, &[], &std::collections::HashMap::new())?
            .program)
    }

    /// Type-check an interactive submission without turning it into a module.
    /// The REPL owns its persistent local environment between submissions.
    pub fn check_repl_statements(&mut self, statements: &[Stmt]) -> TypeResult<()> {
        self.env.load_builtins();
        for statement in statements {
            let check = self.check_stmt(statement)?;
            if check.returned_type.is_some() {
                return Err(self.error_at(statement.span(), TypeErrorKind::ReturnOutsideFunction));
            }
            if check.yielded_type.is_some() {
                return Err(self.error_at(statement.span(), TypeErrorKind::YieldOutsideHandler));
            }
        }
        Ok(())
    }

    pub fn check_module(
        &mut self,
        program: Program,
        imports: &[ResolvedImport],
        module_interfaces: &std::collections::HashMap<crate::module::ModuleId, ModuleInterface>,
    ) -> TypeResult<CheckedModule> {
        self.env.load_builtins();
        self.module_interfaces = module_interfaces.clone();
        self.install_imports(imports)?;

        for item in &program.items {
            if let Item::Type(decl) = item {
                self.predeclare_type(decl)?;
            }
        }
        for item in &program.items {
            if let Item::Type(decl) = item {
                self.resolve_type_decl(decl)?;
            }
        }
        self.install_type_constructors()?;

        for item in &program.items {
            match item {
                Item::Function(decl) => self.declare_function(decl)?,
                Item::Const(decl) => self.declare_const(decl)?,
                Item::Import(_) | Item::Type(_) => {}
            }
        }

        for item in &program.items {
            match item {
                Item::Const(decl) => self.check_const_decl(decl)?,
                Item::Function(decl) => self.check_function_body(decl)?,
                Item::Import(_) | Item::Type(_) => {}
            }
        }

        let constants = self.evaluate_constants(&program)?;

        let mut exports = std::collections::HashMap::new();
        let mut types = std::collections::HashMap::new();
        for item in &program.items {
            match item {
                Item::Function(decl) if decl.public => {
                    let ty = self
                        .env
                        .get(&decl.name)
                        .map_err(|kind| self.error_at(decl.span, kind))?;
                    exports.insert(
                        decl.name.clone(),
                        ExportedSymbol {
                            ty,
                            kind: ExportedSymbolKind::Function,
                        },
                    );
                }
                Item::Const(decl) if decl.public => {
                    let ty = self.resolve_type_expr(&decl.type_annotation, decl.span)?;
                    let value = constants
                        .get(&decl.name)
                        .expect("every checked constant is evaluated")
                        .clone();
                    exports.insert(
                        decl.name.clone(),
                        ExportedSymbol {
                            ty,
                            kind: ExportedSymbolKind::Const { value },
                        },
                    );
                }
                Item::Type(decl) if decl.public => {
                    let id = self.type_id(std::slice::from_ref(&decl.name));
                    types.insert(decl.name.clone(), ExportedType { id: id.clone() });
                    let definition = self
                        .type_definitions
                        .get(&id)
                        .expect("predeclared public types are resolved");
                    match &definition.kind {
                        TypeDefinitionKind::Struct(structure) => {
                            exports.insert(
                                decl.name.clone(),
                                ExportedSymbol {
                                    ty: Type::Constructor(ConstructorSignature::Struct {
                                        type_id: id,
                                        fields: structure
                                            .fields
                                            .iter()
                                            .map(|field| ParameterType {
                                                name: field.name.clone(),
                                                ty: field.ty.clone(),
                                            })
                                            .collect(),
                                    }),
                                    kind: ExportedSymbolKind::Function,
                                },
                            );
                        }
                        TypeDefinitionKind::Enum(_) => {
                            exports.insert(
                                decl.name.clone(),
                                ExportedSymbol {
                                    ty: Type::Any,
                                    kind: ExportedSymbolKind::Function,
                                },
                            );
                        }
                    }
                }
                _ => {}
            }
        }

        Ok(CheckedModule {
            program: CheckedProgram {
                program,
                type_context: std::rc::Rc::new(self.type_context.clone()),
            },
            interface: ModuleInterface {
                exports,
                types,
                type_definitions: self.type_definitions.clone(),
            },
            constants,
            type_definitions: self.type_definitions.clone(),
        })
    }

    fn install_imports(&mut self, imports: &[ResolvedImport]) -> TypeResult<()> {
        for import in imports {
            let name = import.local_name();
            match import {
                ResolvedImport::Module { module, .. } => {
                    if self.env.get_current(name).is_some() {
                        return Err(self.error_at(
                            import.span(),
                            TypeErrorKind::NameCollision(name.to_string()),
                        ));
                    }
                    self.env
                        .define_binding(name.to_string(), TypeBinding::Module(module.clone()));
                    let interface = self
                        .module_interfaces
                        .get(module)
                        .expect("resolved module imports have an interface");
                    for (type_name, exported) in &interface.types {
                        self.type_context.names.insert(
                            vec![name.to_string(), type_name.clone()],
                            Type::Nominal(exported.id.clone()),
                        );
                    }
                }
                ResolvedImport::Member {
                    module,
                    export_name,
                    ..
                } => {
                    let interface = self
                        .module_interfaces
                        .get(module)
                        .expect("resolved imports must have a dependency interface");
                    let value_export = interface.exports.get(export_name);
                    let type_export = interface.types.get(export_name);
                    if value_export.is_none() && type_export.is_none() {
                        return Err(self.error_at(
                            import.span(),
                            TypeErrorKind::UnknownModuleExport {
                                module: module.to_string(),
                                member: export_name.clone(),
                            },
                        ));
                    }
                    if let Some(export) = value_export {
                        if self.env.get_current(name).is_some() {
                            return Err(self.error_at(
                                import.span(),
                                TypeErrorKind::NameCollision(name.to_string()),
                            ));
                        }
                        self.env.define_binding(
                            name.to_string(),
                            TypeBinding::ImportedMember {
                                ty: Box::new(export.ty.clone()),
                                kind: Box::new(export.kind.clone()),
                            },
                        );
                    }
                    if let Some(export) = type_export {
                        if self
                            .type_context
                            .names
                            .contains_key(&vec![name.to_string()])
                        {
                            return Err(self.error_at(
                                import.span(),
                                TypeErrorKind::TypeNameCollision(name.to_string()),
                            ));
                        }
                        let ty = Type::Nominal(export.id.clone());
                        self.type_context
                            .names
                            .insert(vec![name.to_string()], ty.clone());
                        self.env.define_type(name.to_string(), ty);
                    }
                }
            }
        }
        Ok(())
    }

    fn type_id(&self, path: &[String]) -> TypeId {
        TypeId {
            module: self.module_id.clone(),
            path: path.to_vec(),
        }
    }

    fn predeclare_type(&mut self, decl: &TypeDecl) -> TypeResult<()> {
        if self
            .type_context
            .names
            .contains_key(&vec![decl.name.clone()])
            || self.env.get_current_type(&decl.name).is_some()
        {
            return Err(self.error_at(
                decl.span,
                TypeErrorKind::TypeNameCollision(decl.name.clone()),
            ));
        }
        let id = self.type_id(std::slice::from_ref(&decl.name));
        self.type_context
            .names
            .insert(vec![decl.name.clone()], Type::Nominal(id.clone()));
        self.env
            .define_type(decl.name.clone(), Type::Nominal(id.clone()));
        let kind = match decl.kind {
            TypeDeclKind::Struct(_) => {
                TypeDefinitionKind::Struct(StructDefinition { fields: vec![] })
            }
            TypeDeclKind::Enum(_) => TypeDefinitionKind::Enum(EnumDefinition { variants: vec![] }),
        };
        self.type_definitions.insert(
            id.clone(),
            TypeDefinition {
                id,
                public: decl.public,
                kind,
            },
        );
        Ok(())
    }

    fn resolve_type_decl(&mut self, decl: &TypeDecl) -> TypeResult<()> {
        let id = self.type_id(std::slice::from_ref(&decl.name));
        let kind = match &decl.kind {
            TypeDeclKind::Struct(structure) => TypeDefinitionKind::Struct(StructDefinition {
                fields: self.resolve_struct_fields(&structure.fields)?,
            }),
            TypeDeclKind::Enum(enumeration) => {
                if enumeration.variants.is_empty() {
                    return Err(
                        self.error_at(decl.span, TypeErrorKind::EmptyEnum(decl.name.clone()))
                    );
                }
                for variant in &enumeration.variants {
                    if let Some(EnumPayloadDecl::InlineStruct(_)) = &variant.payload {
                        let payload_id = self.type_id(&[decl.name.clone(), variant.name.clone()]);
                        self.type_context.names.insert(
                            vec![decl.name.clone(), variant.name.clone()],
                            Type::Nominal(payload_id.clone()),
                        );
                        self.type_definitions.insert(
                            payload_id.clone(),
                            TypeDefinition {
                                id: payload_id,
                                public: decl.public,
                                kind: TypeDefinitionKind::Struct(StructDefinition {
                                    fields: vec![],
                                }),
                            },
                        );
                    }
                }
                let mut variants = Vec::new();
                let mut names = std::collections::HashSet::new();
                for (index, variant) in enumeration.variants.iter().enumerate() {
                    if !names.insert(variant.name.clone()) {
                        return Err(self.error_at(
                            variant.span,
                            TypeErrorKind::DuplicateVariant(variant.name.clone()),
                        ));
                    }
                    let payload = match &variant.payload {
                        None => VariantPayloadDefinition::Unit,
                        Some(EnumPayloadDecl::Type(ty)) => VariantPayloadDefinition::Value(
                            self.resolve_type_expr(ty, variant.span)?,
                        ),
                        Some(EnumPayloadDecl::InlineStruct(structure)) => {
                            let payload_id =
                                self.type_id(&[decl.name.clone(), variant.name.clone()]);
                            let definition = TypeDefinition {
                                id: payload_id.clone(),
                                public: decl.public,
                                kind: TypeDefinitionKind::Struct(StructDefinition {
                                    fields: self.resolve_struct_fields(&structure.fields)?,
                                }),
                            };
                            self.type_definitions.insert(payload_id.clone(), definition);
                            VariantPayloadDefinition::InlineStruct(payload_id)
                        }
                    };
                    variants.push(VariantDefinition {
                        name: variant.name.clone(),
                        payload,
                        index,
                        span: variant.span,
                    });
                }
                TypeDefinitionKind::Enum(EnumDefinition { variants })
            }
        };
        self.type_definitions.insert(
            id.clone(),
            TypeDefinition {
                id,
                public: decl.public,
                kind,
            },
        );
        Ok(())
    }

    fn resolve_struct_fields(
        &self,
        fields: &[crate::ast::StructFieldDecl],
    ) -> TypeResult<Vec<FieldDefinition>> {
        let mut names = std::collections::HashSet::new();
        fields
            .iter()
            .map(|field| {
                if !names.insert(field.name.clone()) {
                    return Err(self.error_at(
                        field.span,
                        TypeErrorKind::DuplicateField(field.name.clone()),
                    ));
                }
                Ok(FieldDefinition {
                    name: field.name.clone(),
                    ty: self.resolve_type_expr(&field.ty, field.span)?,
                    span: field.span,
                })
            })
            .collect()
    }

    fn resolve_type_expr(&self, expression: &crate::ast::TypeExpr, span: Span) -> TypeResult<Type> {
        self.type_context
            .resolve_type_expr(expression)
            .map_err(|kind| self.error_at(span, kind))
    }

    fn resolve_type_annotation(&self, annotation: &TypeAnnotation, span: Span) -> TypeResult<Type> {
        self.type_context
            .resolve_annotation(annotation)
            .map_err(|kind| self.error_at(span, kind))
    }

    fn install_type_constructors(&mut self) -> TypeResult<()> {
        for definition in self.type_definitions.values() {
            if definition.id.module != self.module_id || definition.id.path.len() != 1 {
                continue;
            }
            let name = definition.id.path[0].clone();
            match &definition.kind {
                TypeDefinitionKind::Struct(structure) => {
                    self.env.define(
                        name,
                        Type::Constructor(ConstructorSignature::Struct {
                            type_id: definition.id.clone(),
                            fields: structure
                                .fields
                                .iter()
                                .map(|field| ParameterType {
                                    name: field.name.clone(),
                                    ty: field.ty.clone(),
                                })
                                .collect(),
                        }),
                    );
                }
                TypeDefinitionKind::Enum(_) => {}
            }
        }
        Ok(())
    }

    fn check_stmt(&mut self, stmt: &Stmt) -> TypeResult<StmtCheck> {
        match stmt {
            Stmt::Expr(expr) => Ok(StmtCheck::normal(self.infer_expr(expr)?)),
            Stmt::Yield(expr) => Ok(StmtCheck::yields(self.infer_expr(expr)?)),
            Stmt::Return(expr) => {
                let found = self.infer_expr(expr)?;
                let expected = self.current_function_return.clone().ok_or_else(|| {
                    self.error_at(stmt.span(), TypeErrorKind::ReturnOutsideFunction)
                })?;

                if TypeChecker::types_compatible(&expected, &found) {
                    Ok(StmtCheck::returns(found))
                } else {
                    Err(self.error_at(
                        stmt.span(),
                        TypeErrorKind::MismatchedType {
                            expected: expected.to_string(),
                            found: found.to_string(),
                        },
                    ))
                }
            }
            Stmt::Bind {
                name,
                type_annotation,
                value,
                ..
            } => {
                if self.env.at_module_scope() && self.env.get_current(name).is_some() {
                    return Err(
                        self.error_at(stmt.span(), TypeErrorKind::NameCollision(name.clone()))
                    );
                }
                let value_type = self.infer_expr(value)?;

                match type_annotation {
                    TypeAnnotation::Inferred => {
                        self.env.define(name.clone(), value_type);
                        Ok(StmtCheck::normal(Type::Unit))
                    }
                    TypeAnnotation::Explicit(bind_type) => {
                        let bind_type = self.resolve_type_expr(bind_type, stmt.span())?;
                        if TypeChecker::types_compatible(&bind_type, &value_type) {
                            self.env.define(name.clone(), bind_type);
                            Ok(StmtCheck::normal(Type::Unit))
                        } else {
                            Err(self.error_at(
                                stmt.span(),
                                TypeErrorKind::MismatchedType {
                                    expected: bind_type.to_string(),
                                    found: value_type.to_string(),
                                },
                            ))
                        }
                    }
                }
            }
            Stmt::Assignment { target, value } => {
                let value_type = self.infer_expr(value)?;

                match &target.kind {
                    ExprKind::Ident(name) => {
                        let target_type = self
                            .env
                            .get(name)
                            .map_err(|kind| self.error_at(target.span, kind))?;

                        if !TypeChecker::types_compatible(&target_type, &value_type) {
                            return Err(self.error_at(
                                value.span,
                                TypeErrorKind::MismatchedType {
                                    expected: target_type.to_string(),
                                    found: value_type.to_string(),
                                },
                            ));
                        }

                        Ok(StmtCheck::normal(Type::Unit))
                    }

                    ExprKind::Index { target, index } => {
                        // Optional if you support arr[i] = value already.
                        self.check_index_assignment(target, index)?;
                        Ok(StmtCheck::normal(Type::Unit))
                    }

                    ExprKind::Path(segments) => Err(self.error_at(
                        target.span,
                        TypeErrorKind::ModuleMemberAssignment(segments.join("::")),
                    )),

                    ExprKind::Field { .. } => {
                        Err(self.error_at(target.span, TypeErrorKind::FieldAssignmentUnsupported))
                    }

                    _ => Err(self.error_at(
                        target.span,
                        TypeErrorKind::InvalidAssignmentTarget(target.kind.to_string()),
                    )),
                }
            }
            Stmt::Function(decl) => {
                self.declare_function(decl)?;
                self.check_function_body(decl)?;
                Ok(StmtCheck::normal(Type::Unit))
            }
        }
    }

    fn declare_const(&mut self, decl: &ConstDecl) -> TypeResult<()> {
        if self.env.get_current(&decl.name).is_some() {
            return Err(self.error_at(decl.span, TypeErrorKind::NameCollision(decl.name.clone())));
        }
        self.env.define_const(
            decl.name.clone(),
            self.resolve_type_expr(&decl.type_annotation, decl.span)?,
        );
        Ok(())
    }

    fn check_const_decl(&mut self, decl: &ConstDecl) -> TypeResult<()> {
        self.validate_const_expr(&decl.value)?;
        let found = self.infer_expr(&decl.value)?;
        let expected = self.resolve_type_expr(&decl.type_annotation, decl.span)?;
        if TypeChecker::types_compatible(&expected, &found) {
            Ok(())
        } else {
            Err(self.error_at(
                decl.span,
                TypeErrorKind::MismatchedType {
                    expected: expected.to_string(),
                    found: found.to_string(),
                },
            ))
        }
    }

    fn validate_const_expr(&self, expr: &Expr) -> TypeResult<()> {
        match &expr.kind {
            ExprKind::Int(_)
            | ExprKind::Float(_)
            | ExprKind::String(_)
            | ExprKind::Bool(_)
            | ExprKind::Unit => Ok(()),
            ExprKind::Ident(name) => match self
                .env
                .get_binding(name)
                .map_err(|kind| self.error_at(expr.span, kind))?
            {
                TypeBinding::Const(_) => Ok(()),
                TypeBinding::ImportedMember { kind, .. }
                    if matches!(kind.as_ref(), ExportedSymbolKind::Const { .. }) =>
                {
                    Ok(())
                }
                _ => Err(self.error_at(expr.span, TypeErrorKind::NotAConstant(name.clone()))),
            },
            ExprKind::Path(segments) => {
                let Some((module_name, member_path)) = segments.split_first() else {
                    unreachable!("paths always have at least two segments")
                };
                let TypeBinding::Module(module) = self
                    .env
                    .get_binding(module_name)
                    .map_err(|kind| self.error_at(expr.span, kind))?
                else {
                    return Err(
                        self.error_at(expr.span, TypeErrorKind::NotAConstant(segments.join("::")))
                    );
                };
                let [member] = member_path else {
                    return Err(
                        self.error_at(expr.span, TypeErrorKind::NotAConstant(segments.join("::")))
                    );
                };
                let interface = self
                    .module_interfaces
                    .get(&module)
                    .expect("module bindings have an interface");
                match interface.exports.get(member) {
                    Some(ExportedSymbol {
                        kind: ExportedSymbolKind::Const { .. },
                        ..
                    }) => Ok(()),
                    Some(_) => {
                        Err(self
                            .error_at(expr.span, TypeErrorKind::NotAConstant(segments.join("::"))))
                    }
                    None => Err(self.error_at(
                        expr.span,
                        TypeErrorKind::UnknownModuleExport {
                            module: module.to_string(),
                            member: member.clone(),
                        },
                    )),
                }
            }
            ExprKind::Tuple(items) | ExprKind::Array(items) => {
                for item in items {
                    self.validate_const_expr(item)?;
                }
                Ok(())
            }
            ExprKind::Unary { operand, .. } => self.validate_const_expr(operand),
            ExprKind::Binary { lhs, rhs, .. } => {
                self.validate_const_expr(lhs)?;
                self.validate_const_expr(rhs)
            }
            ExprKind::If {
                condition,
                then_branch,
                else_branch,
            } => {
                self.validate_const_expr(condition)?;
                self.validate_const_expr(then_branch)?;
                if let Some(else_branch) = else_branch {
                    self.validate_const_expr(else_branch)?;
                }
                Ok(())
            }
            ExprKind::Index { target, index } => {
                self.validate_const_expr(target)?;
                self.validate_const_expr(index)
            }
            ExprKind::Block(_)
            | ExprKind::Call { .. }
            | ExprKind::While { .. }
            | ExprKind::For { .. }
            | ExprKind::Lambda { .. }
            | ExprKind::Field { .. }
            | ExprKind::Match { .. } => Err(self.error_at(
                expr.span,
                TypeErrorKind::ConstExpressionNotAllowed(expr.kind.to_string()),
            )),
        }
    }

    fn evaluate_constants(
        &self,
        program: &Program,
    ) -> TypeResult<std::collections::HashMap<String, Value>> {
        let names = program
            .items
            .iter()
            .filter_map(|item| match item {
                Item::Const(decl) => Some(decl.name.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>();
        let declarations = program
            .items
            .iter()
            .filter_map(|item| match item {
                Item::Const(decl) => Some((decl.name.clone(), decl)),
                _ => None,
            })
            .collect::<std::collections::HashMap<_, _>>();
        let mut values = std::collections::HashMap::new();
        let mut visiting = Vec::new();
        for name in names {
            self.evaluate_const_by_name(name, &declarations, &mut values, &mut visiting)?;
        }
        Ok(values)
    }

    fn evaluate_const_by_name(
        &self,
        name: &str,
        declarations: &std::collections::HashMap<String, &ConstDecl>,
        values: &mut std::collections::HashMap<String, Value>,
        visiting: &mut Vec<String>,
    ) -> TypeResult<Value> {
        if let Some(value) = values.get(name) {
            return Ok(value.clone());
        }
        if let Some(index) = visiting.iter().position(|item| item == name) {
            let mut cycle = visiting[index..].to_vec();
            cycle.push(name.to_string());
            let span = declarations
                .get(name)
                .expect("the cycle consists of local constants")
                .span;
            return Err(self.error_at(span, TypeErrorKind::ConstantCycle(cycle.join(" -> "))));
        }
        let decl = declarations
            .get(name)
            .expect("only declared constants are evaluated");
        visiting.push(name.to_string());
        let value = self.evaluate_const_expr(&decl.value, declarations, values, visiting);
        visiting.pop();
        let value = value?;
        values.insert(name.to_string(), value.clone());
        Ok(value)
    }

    fn evaluate_const_expr(
        &self,
        expr: &Expr,
        declarations: &std::collections::HashMap<String, &ConstDecl>,
        values: &mut std::collections::HashMap<String, Value>,
        visiting: &mut Vec<String>,
    ) -> TypeResult<Value> {
        let invalid =
            |message: String| self.error_at(expr.span, TypeErrorKind::ConstantEvaluation(message));
        match &expr.kind {
            ExprKind::Int(value) => Ok(Value::Int(*value)),
            ExprKind::Float(value) => Ok(Value::Float(*value)),
            ExprKind::String(value) => Ok(Value::String(value.clone())),
            ExprKind::Bool(value) => Ok(Value::Bool(*value)),
            ExprKind::Unit => Ok(Value::Unit),
            ExprKind::Ident(name) => {
                if declarations.contains_key(name) {
                    self.evaluate_const_by_name(name, declarations, values, visiting)
                } else {
                    match self
                        .env
                        .get_binding(name)
                        .map_err(|kind| self.error_at(expr.span, kind))?
                    {
                        TypeBinding::ImportedMember { kind, .. } => match *kind {
                            ExportedSymbolKind::Const { value } => Ok(value),
                            _ => {
                                Err(self
                                    .error_at(expr.span, TypeErrorKind::NotAConstant(name.clone())))
                            }
                        },
                        _ => {
                            Err(self.error_at(expr.span, TypeErrorKind::NotAConstant(name.clone())))
                        }
                    }
                }
            }
            ExprKind::Path(segments) => {
                let (module_name, member_path) =
                    segments.split_first().expect("paths are non-empty");
                let TypeBinding::Module(module) = self
                    .env
                    .get_binding(module_name)
                    .map_err(|kind| self.error_at(expr.span, kind))?
                else {
                    return Err(
                        self.error_at(expr.span, TypeErrorKind::NotAConstant(segments.join("::")))
                    );
                };
                let [member] = member_path else {
                    return Err(
                        self.error_at(expr.span, TypeErrorKind::NotAConstant(segments.join("::")))
                    );
                };
                let interface = self
                    .module_interfaces
                    .get(&module)
                    .expect("module bindings have an interface");
                match interface.exports.get(member) {
                    Some(ExportedSymbol {
                        kind: ExportedSymbolKind::Const { value },
                        ..
                    }) => Ok(value.clone()),
                    _ => {
                        Err(self
                            .error_at(expr.span, TypeErrorKind::NotAConstant(segments.join("::"))))
                    }
                }
            }
            ExprKind::Tuple(items) => items
                .iter()
                .map(|item| self.evaluate_const_expr(item, declarations, values, visiting))
                .collect::<TypeResult<Vec<_>>>()
                .map(Value::Tuple),
            ExprKind::Array(items) => items
                .iter()
                .map(|item| self.evaluate_const_expr(item, declarations, values, visiting))
                .collect::<TypeResult<Vec<_>>>()
                .map(Value::Array),
            ExprKind::Unary { op, operand } => {
                let value = self.evaluate_const_expr(operand, declarations, values, visiting)?;
                match (op, value) {
                    (UnaryOp::Negate, Value::Int(value)) => Ok(Value::Int(-value)),
                    (UnaryOp::Negate, Value::Float(value)) => Ok(Value::Float(-value)),
                    (UnaryOp::Not, Value::Bool(value)) => Ok(Value::Bool(!value)),
                    (_, value) => Err(invalid(format!(
                        "invalid unary operation on {}",
                        value.type_name()
                    ))),
                }
            }
            ExprKind::Binary { lhs, op, rhs } => {
                let lhs = self.evaluate_const_expr(lhs, declarations, values, visiting)?;
                let rhs = self.evaluate_const_expr(rhs, declarations, values, visiting)?;
                self.evaluate_const_binary(*op, lhs, rhs, expr.span)
            }
            ExprKind::If {
                condition,
                then_branch,
                else_branch,
            } => {
                let condition =
                    self.evaluate_const_expr(condition, declarations, values, visiting)?;
                match condition {
                    Value::Bool(true) => {
                        self.evaluate_const_expr(then_branch, declarations, values, visiting)
                    }
                    Value::Bool(false) => match else_branch {
                        Some(branch) => {
                            self.evaluate_const_expr(branch, declarations, values, visiting)
                        }
                        None => Ok(Value::Unit),
                    },
                    value => Err(invalid(format!(
                        "if condition must be Bool, found {}",
                        value.type_name()
                    ))),
                }
            }
            ExprKind::Index { target, index } => {
                let target = self.evaluate_const_expr(target, declarations, values, visiting)?;
                let index = self.evaluate_const_expr(index, declarations, values, visiting)?;
                match (target, index) {
                    (Value::Array(items), Value::Int(index)) if index >= 0 => items
                        .get(index as usize)
                        .cloned()
                        .ok_or_else(|| invalid(format!("index {index} is out of bounds"))),
                    (Value::Array(_), Value::Int(index)) => {
                        Err(invalid(format!("index {index} is out of bounds")))
                    }
                    (Value::Array(_), value) => Err(invalid(format!(
                        "array index must be Int, found {}",
                        value.type_name()
                    ))),
                    (value, _) => Err(invalid(format!("{} is not indexable", value.type_name()))),
                }
            }
            _ => Err(self.error_at(
                expr.span,
                TypeErrorKind::ConstExpressionNotAllowed(expr.kind.to_string()),
            )),
        }
    }

    fn evaluate_const_binary(
        &self,
        op: BinaryOp,
        lhs: Value,
        rhs: Value,
        span: Span,
    ) -> TypeResult<Value> {
        let invalid =
            |message: String| self.error_at(span, TypeErrorKind::ConstantEvaluation(message));
        match op {
            BinaryOp::Add => match (lhs, rhs) {
                (Value::Int(a), Value::Int(b)) => a
                    .checked_add(b)
                    .map(Value::Int)
                    .ok_or_else(|| invalid("integer overflow".to_string())),
                (Value::Float(a), Value::Float(b)) => Ok(Value::Float(a + b)),
                (a, b) => Err(invalid(format!(
                    "cannot add {} and {}",
                    a.type_name(),
                    b.type_name()
                ))),
            },
            BinaryOp::Sub => match (lhs, rhs) {
                (Value::Int(a), Value::Int(b)) => a
                    .checked_sub(b)
                    .map(Value::Int)
                    .ok_or_else(|| invalid("integer overflow".to_string())),
                (Value::Float(a), Value::Float(b)) => Ok(Value::Float(a - b)),
                (a, b) => Err(invalid(format!(
                    "cannot subtract {} and {}",
                    a.type_name(),
                    b.type_name()
                ))),
            },
            BinaryOp::Mul => match (lhs, rhs) {
                (Value::Int(a), Value::Int(b)) => a
                    .checked_mul(b)
                    .map(Value::Int)
                    .ok_or_else(|| invalid("integer overflow".to_string())),
                (Value::Float(a), Value::Float(b)) => Ok(Value::Float(a * b)),
                (a, b) => Err(invalid(format!(
                    "cannot multiply {} and {}",
                    a.type_name(),
                    b.type_name()
                ))),
            },
            BinaryOp::Div => match (lhs, rhs) {
                (Value::Int(_), Value::Int(0)) => Err(invalid("division by zero".to_string())),
                (Value::Int(a), Value::Int(b)) => a
                    .checked_div(b)
                    .map(Value::Int)
                    .ok_or_else(|| invalid("integer overflow".to_string())),
                (Value::Float(a), Value::Float(b)) => Ok(Value::Float(a / b)),
                (a, b) => Err(invalid(format!(
                    "cannot divide {} and {}",
                    a.type_name(),
                    b.type_name()
                ))),
            },
            BinaryOp::Eq => Ok(Value::Bool(lhs == rhs)),
            BinaryOp::NotEq => Ok(Value::Bool(lhs != rhs)),
            BinaryOp::Greater => match (lhs, rhs) {
                (Value::Int(a), Value::Int(b)) => Ok(Value::Bool(a > b)),
                (Value::Float(a), Value::Float(b)) => Ok(Value::Bool(a > b)),
                (a, b) => Err(invalid(format!(
                    "cannot compare {} and {}",
                    a.type_name(),
                    b.type_name()
                ))),
            },
            BinaryOp::GreaterEq => match (lhs, rhs) {
                (Value::Int(a), Value::Int(b)) => Ok(Value::Bool(a >= b)),
                (Value::Float(a), Value::Float(b)) => Ok(Value::Bool(a >= b)),
                (a, b) => Err(invalid(format!(
                    "cannot compare {} and {}",
                    a.type_name(),
                    b.type_name()
                ))),
            },
            BinaryOp::Less => match (lhs, rhs) {
                (Value::Int(a), Value::Int(b)) => Ok(Value::Bool(a < b)),
                (Value::Float(a), Value::Float(b)) => Ok(Value::Bool(a < b)),
                (a, b) => Err(invalid(format!(
                    "cannot compare {} and {}",
                    a.type_name(),
                    b.type_name()
                ))),
            },
            BinaryOp::LessEq => match (lhs, rhs) {
                (Value::Int(a), Value::Int(b)) => Ok(Value::Bool(a <= b)),
                (Value::Float(a), Value::Float(b)) => Ok(Value::Bool(a <= b)),
                (a, b) => Err(invalid(format!(
                    "cannot compare {} and {}",
                    a.type_name(),
                    b.type_name()
                ))),
            },
            BinaryOp::Combine => Ok(Value::String(format!("{lhs}{rhs}"))),
            BinaryOp::InclusiveRange => match (lhs, rhs) {
                (Value::Int(start), Value::Int(end)) => Ok(Value::Range(RangeValue {
                    start,
                    end,
                    inclusive: true,
                    step: 1,
                })),
                (a, b) => Err(invalid(format!(
                    "cannot build a range from {} and {}",
                    a.type_name(),
                    b.type_name()
                ))),
            },
            BinaryOp::ExclusiveRange => match (lhs, rhs) {
                (Value::Int(start), Value::Int(end)) => Ok(Value::Range(RangeValue {
                    start,
                    end,
                    inclusive: false,
                    step: 1,
                })),
                (a, b) => Err(invalid(format!(
                    "cannot build a range from {} and {}",
                    a.type_name(),
                    b.type_name()
                ))),
            },
        }
    }

    fn infer_expr(&mut self, expr: &Expr) -> TypeResult<Type> {
        let span = expr.span;
        match &expr.kind {
            ExprKind::Int(_) => Ok(Type::Int),
            ExprKind::Float(_) => Ok(Type::Float),
            ExprKind::String(_) => Ok(Type::String),
            ExprKind::Bool(_) => Ok(Type::Bool),
            ExprKind::Ident(value) => {
                let value = self
                    .env
                    .get(value)
                    .map_err(|kind| self.error_at(span, kind))?;
                Ok(value)
            }
            ExprKind::Path(segments) => self.infer_path(segments, span),
            ExprKind::Unit => Ok(Type::Unit),
            ExprKind::Block(block) => self.check_block(block).map(|b| b.ty),
            ExprKind::Tuple(exprs) => {
                let mut tys = vec![];
                for expr in exprs {
                    tys.push(self.infer_expr(expr)?);
                }
                Ok(Type::Tuple(tys))
            }
            ExprKind::Array(exprs) => {
                let Some(fst) = exprs.first() else {
                    return Ok(Type::Array(Box::new(Type::Any)));
                };
                let t = self.infer_expr(fst)?;
                for expr in exprs {
                    let span = expr.span;
                    let current_t = self.infer_expr(expr)?;
                    if current_t != t {
                        return Err(self.error_at(
                            span,
                            TypeErrorKind::MismatchedArrayType {
                                expected: t.to_string(),
                                found: current_t.to_string(),
                            },
                        ));
                    }
                }
                Ok(Type::Array(Box::new(t)))
            }
            ExprKind::Unary { op, operand } => match op {
                UnaryOp::Negate => self.infer_expr(operand),
                UnaryOp::Not => self.infer_expr(operand),
            },
            ExprKind::Binary { lhs, op, rhs } => {
                let (lhs_span, rhs_span) = (lhs.span, rhs.span);
                let (lhs, rhs) = (self.infer_expr(lhs)?, self.infer_expr(rhs)?);
                match (op, lhs, rhs) {
                    (
                        BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul | BinaryOp::Div,
                        Type::Int,
                        Type::Int,
                    ) => Ok(Type::Int),
                    (
                        BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul | BinaryOp::Div,
                        Type::Float,
                        Type::Float,
                    ) => Ok(Type::Float),
                    (
                        BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul | BinaryOp::Div,
                        Type::Float | Type::Int | Type::Any,
                        Type::Any,
                    ) => Ok(Type::Any),
                    (
                        BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul | BinaryOp::Div,
                        Type::Any,
                        Type::Float | Type::Int,
                    ) => Ok(Type::Any),
                    (BinaryOp::Eq | BinaryOp::NotEq, t1, t2) if t1 == t2 => Ok(Type::Bool),
                    (BinaryOp::Eq | BinaryOp::NotEq, _, Type::Any) => Ok(Type::Bool),
                    (BinaryOp::Eq | BinaryOp::NotEq, Type::Any, _) => Ok(Type::Bool),
                    (
                        BinaryOp::Greater | BinaryOp::GreaterEq | BinaryOp::Less | BinaryOp::LessEq,
                        Type::Int,
                        Type::Int,
                    ) => Ok(Type::Bool),
                    (
                        BinaryOp::Greater | BinaryOp::GreaterEq | BinaryOp::Less | BinaryOp::LessEq,
                        Type::Float,
                        Type::Float,
                    ) => Ok(Type::Bool),
                    (
                        BinaryOp::Greater | BinaryOp::GreaterEq | BinaryOp::Less | BinaryOp::LessEq,
                        Type::Float | Type::Int | Type::Any,
                        Type::Any,
                    ) => Ok(Type::Bool),
                    (
                        BinaryOp::Greater | BinaryOp::GreaterEq | BinaryOp::Less | BinaryOp::LessEq,
                        Type::Any,
                        Type::Float | Type::Int,
                    ) => Ok(Type::Bool),
                    (BinaryOp::Combine, Type::String, _) => Ok(Type::String),
                    (BinaryOp::Combine, _, Type::String | Type::Any) => Ok(Type::String),
                    (
                        BinaryOp::InclusiveRange | BinaryOp::ExclusiveRange,
                        Type::Int | Type::Any,
                        Type::Int | Type::Any,
                    ) => Ok(Type::Range),
                    (other_op, other_lhs, other_rhs) => Err(self.error_at(
                        Span {
                            start: lhs_span.start,
                            end: rhs_span.end,
                        },
                        TypeErrorKind::MismatchedBinaryOpType {
                            op: *other_op,
                            lhs: other_lhs.to_string(),
                            rhs: other_rhs.to_string(),
                        },
                    )),
                }
            }
            ExprKind::Call { callee, args } => {
                let callee_type = self.infer_expr(callee)?;
                if let Type::Constructor(signature) = callee_type {
                    return self.check_constructor_call(signature, args, callee.span);
                }
                let Type::Function {
                    parameter_overloads,
                    return_type,
                } = callee_type
                else {
                    return Err(self.error_at(
                        callee.span,
                        TypeErrorKind::MismatchedType {
                            expected: "a callable value".to_string(),
                            found: callee_type.to_string(),
                        },
                    ));
                };

                let argument_types = args
                    .iter()
                    .map(|argument| {
                        Ok(CallArgument {
                            name: argument.name.clone(),
                            value: self.infer_expr(&argument.value)?,
                            span: argument.span,
                        })
                    })
                    .collect::<TypeResult<Vec<_>>>()?;

                let mut matches = 0;
                let mut last_argument_error = None;
                let mut last_type_mismatch = None;
                for parameters in &parameter_overloads {
                    let normalized = match normalize_arguments(parameters, &argument_types) {
                        Ok(arguments) => arguments,
                        Err(error) => {
                            last_argument_error = Some(error);
                            continue;
                        }
                    };

                    let mismatch =
                        parameters
                            .iter()
                            .zip(normalized)
                            .find(|(parameter, argument)| {
                                !TypeChecker::types_compatible(&parameter.ty, argument)
                            });

                    if let Some((parameter, argument)) = mismatch {
                        last_type_mismatch = Some((parameter.ty.clone(), argument));
                    } else {
                        matches += 1;
                    }
                }

                if matches == 0 {
                    if let Some((expected, found)) = last_type_mismatch {
                        return Err(self.error_at(
                            callee.span,
                            TypeErrorKind::MismatchedType {
                                expected: expected.to_string(),
                                found: found.to_string(),
                            },
                        ));
                    }

                    if let Some(error) = last_argument_error {
                        let span = error.span.unwrap_or(callee.span);
                        return Err(self.error_at(span, TypeErrorKind::ArgumentError { e: error }));
                    }

                    return Err(self.error_at(
                        callee.span,
                        TypeErrorKind::MismatchedType {
                            expected: "a matching function signature".to_string(),
                            found: "the provided argument types".to_string(),
                        },
                    ));
                }

                if matches > 1 {
                    return Err(self.error_at(
                        callee.span,
                        TypeErrorKind::ArgumentError {
                            e: Box::new(ArgumentError {
                                kind: ArgumentErrorKind::Ambiguous,
                                span: None,
                            }),
                        },
                    ));
                }

                Ok(*return_type)
            }
            ExprKind::If {
                condition: _,
                then_branch,
                else_branch,
            } => {
                let then_span = then_branch.span;
                let then_type = self.infer_expr(then_branch)?;
                match else_branch {
                    Some(eb) => {
                        let else_span = eb.span;
                        let else_type = self.infer_expr(eb)?;
                        if else_type != then_type {
                            Err(self.error_at(
                                Span {
                                    start: then_span.start,
                                    end: else_span.end,
                                },
                                TypeErrorKind::MismatchedBranchTypes {
                                    expected: then_type.to_string(),
                                    found: else_type.to_string(),
                                },
                            ))
                        } else {
                            Ok(then_type)
                        }
                    }
                    None => Ok(then_type),
                }
            }
            ExprKind::While { condition, block } => {
                let condition_span = condition.span;
                let t = self.infer_expr(condition)?;
                if !TypeChecker::types_compatible(&Type::Bool, &t) {
                    return Err(self.error_at(
                        condition_span,
                        TypeErrorKind::MismatchedType {
                            expected: "Bool".to_string(),
                            found: t.to_string(),
                        },
                    ));
                }

                self.check_block(block).map(|b| b.ty)
            }
            ExprKind::For {
                binding,
                iterable,
                block,
            } => {
                let iterable_span = iterable.span;
                let binding_span = binding.span;
                let iterable = self.infer_expr(iterable)?;

                let binding_name = match binding.kind.clone() {
                    ExprKind::Ident(i) => i,
                    other => {
                        return Err(self.error_at(
                            binding_span,
                            TypeErrorKind::InvalidAssignmentTarget(other.to_string()),
                        ));
                    }
                };

                let binding_type = match iterable {
                    Type::Range => Type::Int,

                    Type::Array(inner) => *inner,

                    other => {
                        return Err(self.error_at(
                            iterable_span,
                            TypeErrorKind::NotIterable {
                                found: other.to_string(),
                            },
                        ));
                    }
                };

                self.env.push_scope();
                self.env.define(binding_name, binding_type);

                let block_check = self.check_block(block)?;

                self.env.pop_scope();

                Ok(block_check.ty)
            }
            ExprKind::Lambda { parameters, body } => {
                self.env.push_scope();

                let previous_return = self.current_function_return.clone();
                self.current_function_return = Some(Type::Any);

                let result = (|| {
                    let mut parameters_types = vec![];

                    for parameter in parameters {
                        parameters_types.push(ParameterType {
                            name: parameter.name.clone(),
                            ty: Type::Any,
                        });
                        self.env.define(parameter.name.clone(), Type::Any);
                    }

                    let ret_type = match &body.kind {
                        ExprKind::Block(block) => {
                            let block_check = self.check_block(block)?;

                            match block_check.returned_type {
                                Some(returned_type) => returned_type,
                                None => block_check.ty,
                            }
                        }

                        _ => self.infer_expr(body)?,
                    };

                    Ok(Type::Function {
                        parameter_overloads: vec![parameters_types],
                        return_type: Box::new(ret_type),
                    })
                })();

                self.current_function_return = previous_return;
                self.env.pop_scope();

                result
            }
            ExprKind::Index { target, index } => self.check_index_assignment(target, index),
            ExprKind::Field { target, name } => self.infer_field(target, name, span),
            ExprKind::Match { value, arms } => self.check_match(value, arms, span),
        }
    }

    fn check_block(&mut self, block: &Block) -> TypeResult<BlockCheck> {
        self.env.push_scope();

        let mut yielded_type: Option<Type> = None;
        let mut returned_type: Option<Type> = None;

        for stmt in &block.statements {
            let stmt_check = self.check_stmt(stmt)?;

            if let Some(stmt_yielded_type) = stmt_check.yielded_type {
                match &yielded_type {
                    Some(existing) if existing != &stmt_yielded_type => {
                        return Err(self.error_at(
                            Span::dummy(), // TODO: add span to statements
                            TypeErrorKind::MismatchedYieldTypes {
                                expected: existing.clone().to_string(),
                                found: stmt_yielded_type.to_string(),
                            },
                        ));
                    }

                    Some(_) => {}

                    None => {
                        yielded_type = Some(stmt_yielded_type);
                    }
                }
            }

            if let Some(stmt_returned_type) = stmt_check.returned_type {
                match &returned_type {
                    Some(existing) if existing != &stmt_returned_type => {
                        return Err(self.error_at(
                            Span::dummy(), // TODO: add span to statements
                            TypeErrorKind::MismatchedReturnTypes {
                                expected: existing.clone().to_string(),
                                found: stmt_returned_type.to_string(),
                            },
                        ));
                    }

                    Some(_) => {}

                    None => {
                        returned_type = Some(stmt_returned_type);
                    }
                }
            }
        }

        self.env.pop_scope();

        Ok(BlockCheck {
            ty: yielded_type.unwrap_or(Type::Unit),
            returned_type,
        })
    }

    fn check_index_assignment(&mut self, target: &Expr, index: &Expr) -> TypeResult<Type> {
        let target_span = target.span;
        let target = self.infer_expr(target)?;
        let array_t = match target {
            Type::Array(t) => *t,
            other => {
                return Err(self.error_at(
                    target_span,
                    TypeErrorKind::InvalidIndexingTarget(other.to_string()),
                ));
            }
        };

        let index_span = index.span;
        let index = self.infer_expr(index)?;
        match index {
            Type::Int => Ok(array_t),
            other => Err(self.error_at(
                index_span,
                TypeErrorKind::InvalidIndexType(other.to_string()),
            )),
        }
    }

    fn declare_function(&mut self, decl: &FunctionDecl) -> TypeResult<()> {
        let FunctionDecl {
            name,
            parameters,
            return_type,
            ..
        } = decl;

        let mut parameters_types = Vec::new();

        for param in parameters {
            parameters_types.push(ParameterType {
                name: param.name.clone(),
                ty: self.resolve_type_annotation(&param.t, decl.span)?,
            });
        }

        let return_type = Box::new(self.resolve_type_annotation(return_type, decl.span)?);

        let fun = match self.env.get_current(name) {
            Some(TypeBinding::Local(Type::Function {
                mut parameter_overloads,
                return_type: defined_return_type,
            })) => {
                if defined_return_type != return_type {
                    return Err(self.error_at(
                        decl.span,
                        TypeErrorKind::MismatchedReturnTypes {
                            expected: defined_return_type.to_string(),
                            found: return_type.to_string(),
                        },
                    ));
                }
                parameter_overloads.push(parameters_types);
                Type::Function {
                    parameter_overloads,
                    return_type,
                }
            }
            Some(_) => {
                return Err(self.error_at(decl.span, TypeErrorKind::NameCollision(name.clone())));
            }
            None => Type::Function {
                parameter_overloads: vec![parameters_types],
                return_type,
            },
        };

        self.env.define(name.clone(), fun);

        Ok(())
    }

    fn definition(&self, id: &TypeId) -> Option<&TypeDefinition> {
        self.type_definitions.get(id).or_else(|| {
            self.module_interfaces
                .get(&id.module)
                .and_then(|interface| interface.type_definitions.get(id))
        })
    }

    fn constructor_for_variant(
        &self,
        enum_id: &TypeId,
        variant_name: &str,
        span: Span,
    ) -> TypeResult<Type> {
        let Some(TypeDefinition {
            kind: TypeDefinitionKind::Enum(definition),
            ..
        }) = self.definition(enum_id)
        else {
            return Err(self.error_at(span, TypeErrorKind::NotAnEnum(enum_id.to_string())));
        };
        let variant = definition
            .variants
            .iter()
            .find(|variant| variant.name == variant_name)
            .ok_or_else(|| {
                self.error_at(
                    span,
                    TypeErrorKind::UnknownVariant {
                        ty: enum_id.to_string(),
                        variant: variant_name.to_string(),
                    },
                )
            })?;
        match &variant.payload {
            VariantPayloadDefinition::Unit => Ok(Type::Nominal(enum_id.clone())),
            VariantPayloadDefinition::Value(ty) => {
                Ok(Type::Constructor(ConstructorSignature::EnumVariant {
                    enum_id: enum_id.clone(),
                    variant_index: variant.index,
                    parameters: vec![ParameterType {
                        name: "value".to_string(),
                        ty: ty.clone(),
                    }],
                    named_only: false,
                }))
            }
            VariantPayloadDefinition::InlineStruct(payload_id) => {
                let Some(TypeDefinition {
                    kind: TypeDefinitionKind::Struct(structure),
                    ..
                }) = self.definition(payload_id)
                else {
                    unreachable!("inline payload IDs always refer to structs");
                };
                Ok(Type::Constructor(ConstructorSignature::EnumVariant {
                    enum_id: enum_id.clone(),
                    variant_index: variant.index,
                    parameters: structure
                        .fields
                        .iter()
                        .map(|field| ParameterType {
                            name: field.name.clone(),
                            ty: field.ty.clone(),
                        })
                        .collect(),
                    named_only: true,
                }))
            }
        }
    }

    fn infer_path(&self, segments: &[String], span: Span) -> TypeResult<Type> {
        let (head, tail) = segments.split_first().expect("paths have a head");
        if let Ok(TypeBinding::Module(module)) = self.env.get_binding(head) {
            let interface = self
                .module_interfaces
                .get(&module)
                .expect("module namespace bindings must refer to a dependency interface");
            if let [member] = tail
                && let Some(symbol) = interface.exports.get(member)
            {
                return Ok(symbol.ty.clone());
            }
            if let [type_name, variant] = tail
                && let Some(exported) = interface.types.get(type_name)
            {
                return self.constructor_for_variant(&exported.id, variant, span);
            }
            return Err(self.error_at(
                span,
                TypeErrorKind::UnknownModuleExport {
                    module: module.to_string(),
                    member: tail.join("::"),
                },
            ));
        }

        let enum_ty = self
            .type_context
            .names
            .get(&vec![head.clone()])
            .cloned()
            .ok_or_else(|| self.error_at(span, TypeErrorKind::NotAValue(head.clone())))?;
        let Type::Nominal(enum_id) = enum_ty else {
            return Err(self.error_at(span, TypeErrorKind::NotAValue(head.clone())));
        };
        let [variant] = tail else {
            return Err(self.error_at(
                span,
                TypeErrorKind::UnknownVariant {
                    ty: enum_id.to_string(),
                    variant: tail.join("::"),
                },
            ));
        };
        self.constructor_for_variant(&enum_id, variant, span)
    }

    fn check_constructor_call(
        &mut self,
        signature: ConstructorSignature,
        arguments: &[CallArgument<Expr>],
        span: Span,
    ) -> TypeResult<Type> {
        let (parameters, result, named_only) = match signature {
            ConstructorSignature::Struct { type_id, fields } => {
                (fields, Type::Nominal(type_id), true)
            }
            ConstructorSignature::EnumVariant {
                enum_id,
                parameters,
                named_only,
                ..
            } => (parameters, Type::Nominal(enum_id), named_only),
        };
        if named_only && arguments.iter().any(|argument| argument.name.is_none()) {
            return Err(self.error_at(
                span,
                TypeErrorKind::ArgumentError {
                    e: Box::new(ArgumentError {
                        kind: ArgumentErrorKind::NamedOnly,
                        span: None,
                    }),
                },
            ));
        }
        let argument_types = arguments
            .iter()
            .map(|argument| {
                Ok(CallArgument {
                    name: argument.name.clone(),
                    value: self.infer_expr(&argument.value)?,
                    span: argument.span,
                })
            })
            .collect::<TypeResult<Vec<_>>>()?;
        let normalized = normalize_arguments(&parameters, &argument_types)
            .map_err(|error| self.error_at(span, TypeErrorKind::ArgumentError { e: error }))?;
        for (parameter, found) in parameters.iter().zip(normalized) {
            if !TypeChecker::types_compatible(&parameter.ty, &found) {
                return Err(self.error_at(
                    span,
                    TypeErrorKind::MismatchedType {
                        expected: parameter.ty.to_string(),
                        found: found.to_string(),
                    },
                ));
            }
        }
        Ok(result)
    }

    fn infer_field(&mut self, target: &Expr, name: &str, span: Span) -> TypeResult<Type> {
        let target_ty = self.infer_expr(target)?;
        let Type::Nominal(id) = target_ty else {
            return Err(self.error_at(span, TypeErrorKind::NotAStruct(target_ty.to_string())));
        };
        let Some(TypeDefinition {
            kind: TypeDefinitionKind::Struct(definition),
            ..
        }) = self.definition(&id)
        else {
            return Err(self.error_at(span, TypeErrorKind::NotAStruct(id.to_string())));
        };
        definition
            .fields
            .iter()
            .find(|field| field.name == name)
            .map(|field| field.ty.clone())
            .ok_or_else(|| {
                self.error_at(
                    span,
                    TypeErrorKind::UnknownField {
                        ty: id.to_string(),
                        field: name.to_string(),
                    },
                )
            })
    }

    fn check_match(
        &mut self,
        value: &Expr,
        arms: &[crate::ast::MatchArm],
        span: Span,
    ) -> TypeResult<Type> {
        let value_ty = self.infer_expr(value)?;
        let Type::Nominal(enum_id) = value_ty else {
            return Err(self.error_at(span, TypeErrorKind::NotAnEnum(value_ty.to_string())));
        };
        let Some(TypeDefinition {
            kind: TypeDefinitionKind::Enum(definition),
            ..
        }) = self.definition(&enum_id)
        else {
            return Err(self.error_at(span, TypeErrorKind::NotAnEnum(enum_id.to_string())));
        };
        let variants = definition.variants.clone();
        let mut seen = std::collections::HashSet::new();
        let mut wildcard = false;
        let mut yielded = None;
        for arm in arms {
            if wildcard {
                return Err(self.error_at(arm.span, TypeErrorKind::UnreachableMatchArm));
            }
            self.env.push_scope();
            match &arm.pattern {
                crate::ast::Pattern::Wildcard { .. } => wildcard = true,
                crate::ast::Pattern::EnumVariant {
                    qualifier,
                    variant,
                    binding,
                    ..
                } => {
                    if let Some(qualifier) = qualifier {
                        let expected = qualifier.join("::");
                        if self.type_context.names.get(qualifier)
                            != Some(&Type::Nominal(enum_id.clone()))
                        {
                            self.env.pop_scope();
                            return Err(self.error_at(arm.span, TypeErrorKind::NotAnEnum(expected)));
                        }
                    }
                    let variant_definition = variants
                        .iter()
                        .find(|item| item.name == *variant)
                        .ok_or_else(|| {
                            self.error_at(
                                arm.span,
                                TypeErrorKind::UnknownVariant {
                                    ty: enum_id.to_string(),
                                    variant: variant.clone(),
                                },
                            )
                        })?;
                    if !seen.insert(variant.clone()) {
                        self.env.pop_scope();
                        return Err(self.error_at(
                            arm.span,
                            TypeErrorKind::DuplicateMatchArm(variant.clone()),
                        ));
                    }
                    match (&variant_definition.payload, binding) {
                        (VariantPayloadDefinition::Unit, Some(_)) => {
                            self.env.pop_scope();
                            return Err(self.error_at(
                                arm.span,
                                TypeErrorKind::VariantHasNoPayload {
                                    ty: enum_id.to_string(),
                                    variant: variant.clone(),
                                },
                            ));
                        }
                        (VariantPayloadDefinition::Unit, None) => {}
                        (VariantPayloadDefinition::Value(ty), Some(name)) => {
                            self.env.define(name.clone(), ty.clone())
                        }
                        (VariantPayloadDefinition::InlineStruct(id), Some(name)) => {
                            self.env.define(name.clone(), Type::Nominal(id.clone()))
                        }
                        (_, None) => {
                            self.env.pop_scope();
                            return Err(self.error_at(
                                arm.span,
                                TypeErrorKind::VariantRequiresPayload {
                                    ty: enum_id.to_string(),
                                    variant: variant.clone(),
                                },
                            ));
                        }
                    }
                }
            }
            let check = self.check_block(&arm.body)?;
            self.env.pop_scope();
            if let Some(ty) = check.ty.ne(&Type::Unit).then_some(check.ty) {
                if let Some(existing) = &yielded {
                    if !TypeChecker::types_compatible(existing, &ty) {
                        return Err(self.error_at(
                            arm.span,
                            TypeErrorKind::MismatchedBranchTypes {
                                expected: existing.to_string(),
                                found: ty.to_string(),
                            },
                        ));
                    }
                } else {
                    yielded = Some(ty);
                }
            }
        }
        if !wildcard {
            let missing = variants
                .iter()
                .filter(|variant| !seen.contains(&variant.name))
                .map(|variant| variant.name.as_str())
                .collect::<Vec<_>>();
            if !missing.is_empty() {
                return Err(
                    self.error_at(span, TypeErrorKind::NonExhaustiveMatch(missing.join(", ")))
                );
            }
        }
        Ok(yielded.unwrap_or(Type::Unit))
    }

    fn check_function_body(&mut self, decl: &FunctionDecl) -> TypeResult<()> {
        let FunctionDecl {
            parameters,
            return_type,
            body,
            ..
        } = decl;

        let expected_return = self.resolve_type_annotation(return_type, decl.span)?;
        let previous_return = self.current_function_return.clone();
        self.current_function_return = Some(expected_return.clone());

        self.env.push_scope();

        for param in parameters {
            let param_type = self.resolve_type_annotation(&param.t, decl.span)?;
            self.env.define(param.name.clone(), param_type);
        }

        let body_check = self.check_block(body)?;

        self.env.pop_scope();

        self.current_function_return = previous_return;

        if let Some(returned) = body_check.returned_type
            && !TypeChecker::types_compatible(&expected_return, &returned)
        {
            return Err(self.error_at(
                body.span(),
                TypeErrorKind::MismatchedType {
                    expected: expected_return.to_string(),
                    found: returned.to_string(),
                },
            ));
        }

        Ok(())
    }

    pub fn types_compatible(expected: &Type, found: &Type) -> bool {
        match (expected, found) {
            (Type::Any, _) | (_, Type::Any) => true,
            (
                Type::Function {
                    parameter_overloads: expected_overloads,
                    return_type: expected_return,
                },
                Type::Function {
                    parameter_overloads: found_overloads,
                    return_type: found_return,
                },
            ) => {
                TypeChecker::types_compatible(expected_return, found_return)
                    && expected_overloads.iter().all(|expected_parameters| {
                        found_overloads.iter().any(|found_parameters| {
                            expected_parameters.len() == found_parameters.len()
                                && expected_parameters.iter().zip(found_parameters).all(
                                    |(expected, found)| {
                                        TypeChecker::types_compatible(&expected.ty, &found.ty)
                                    },
                                )
                        })
                    })
            }
            (Type::Array(i1), other) => {
                if let Type::Any = **i1 {
                    true
                } else if let Type::Array(i2) = other {
                    if let Type::Any = **i2 {
                        true
                    } else {
                        *expected == *found
                    }
                } else {
                    *expected == *found
                }
            }
            _ => *expected == *found,
        }
    }

    fn error_at(&self, span: Span, kind: TypeErrorKind) -> Box<TypeError> {
        Box::new(TypeError {
            kind,
            span,
            file_path: self.file_path.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::TypeChecker;
    use crate::typechecker::ty::{ParameterType, Type};

    fn unary_function(parameter_name: &str, parameter_type: Type, return_type: Type) -> Type {
        Type::Function {
            parameter_overloads: vec![vec![ParameterType {
                name: parameter_name.to_string(),
                ty: parameter_type,
            }]],
            return_type: Box::new(return_type),
        }
    }

    #[test]
    fn function_type_compatibility_ignores_internal_parameter_names() {
        let annotated = unary_function("_0", Type::Any, Type::Any);
        let lambda = unary_function("x", Type::Any, Type::Any);

        assert!(TypeChecker::types_compatible(&annotated, &lambda));
    }

    #[test]
    fn function_type_compatibility_still_checks_parameter_types() {
        let expected = unary_function("_0", Type::Int, Type::Int);
        let found = unary_function("x", Type::String, Type::Int);

        assert!(!TypeChecker::types_compatible(&expected, &found));
    }
}
