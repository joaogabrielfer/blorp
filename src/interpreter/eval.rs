use std::{cell::RefCell, collections::HashMap, rc::Rc};

use anyhow::Result;

use crate::{
    ast::{
        BinaryOp, Block, Expr, ExprArgument, ExprKind, FunctionDecl, Item, Program,
        Stmt::{self},
        UnaryOp, ValueArgument,
    },
    errors::{ArgumentError, RuntimeError, RuntimeErrorKind},
    interpreter::{
        Interpreter,
        env::{BindingKind, Env, EnvFrame},
        values::{
            ConstructorRef, EnumValue, FunctionBody, FunctionValue, OverloadFunctionVariant,
            RangeValue, RuntimeIterator, StructValue, Value,
        },
    },
    source::{Span, SrcPos},
    typechecker::ty::{
        Type, TypeContext, TypeDefinition, TypeDefinitionKind, TypeId, VariantPayloadDefinition,
    },
};

pub enum EvalFlow {
    Continue(Value),
    Return { value: Value, span: Span },
    Yield { value: Value, span: Span },
}

#[derive(Debug, Clone, Copy)]
pub enum YieldMode {
    Bubble,
    Capture,
}

#[macro_export]
macro_rules! value_or_flow {
    ($flow:expr) => {
        match $flow {
            EvalFlow::Continue(value) => value,
            EvalFlow::Return { value, span } => return Ok(EvalFlow::Return { value, span }),
            EvalFlow::Yield { value, span } => return Ok(EvalFlow::Yield { value, span }),
        }
    };
}

impl<W: std::io::Write> Interpreter<W> {
    pub fn eval_program(&mut self, program: Program) -> Result<Value, Box<RuntimeError>> {
        let context = self.type_context.clone();
        self.load_module(
            &program,
            &std::collections::HashMap::new(),
            context,
            &std::collections::HashMap::new(),
        )?;
        self.run_main()
    }

    /// Evaluate an interactive submission without treating it as a module.
    ///
    /// The REPL is intentionally the one context that accepts expressions and
    /// local declarations outside a function body.
    pub fn eval_repl_statements(
        &mut self,
        statements: Vec<Stmt>,
    ) -> Result<Value, Box<RuntimeError>> {
        let mut last_value = Value::Unit;

        for statement in statements {
            match self.eval_statement(statement)? {
                EvalFlow::Continue(value) => last_value = value,
                EvalFlow::Yield { span, .. } => {
                    return Err(self.error_at(span, RuntimeErrorKind::YieldOutsideHandler));
                }
                EvalFlow::Return { value, .. } => return Ok(value),
            }
        }
        Ok(last_value)
    }

    pub fn load_module(
        &mut self,
        program: &Program,
        constants: &std::collections::HashMap<String, Value>,
        type_context: Rc<TypeContext>,
        type_definitions: &std::collections::HashMap<TypeId, TypeDefinition>,
    ) -> Result<(), Box<RuntimeError>> {
        self.install_type_metadata(type_context, type_definitions);
        for item in &program.items {
            if let Item::Type(decl) = item {
                let id = match self.type_context.names.get(&vec![decl.name.clone()]) {
                    Some(Type::Nominal(id)) => id.clone(),
                    _ => continue,
                };
                let definition = self
                    .type_definitions
                    .get(&id)
                    .expect("checked type definitions are available at runtime");
                let value = match definition.kind {
                    TypeDefinitionKind::Struct(_) => Value::Constructor(ConstructorRef::Struct(id)),
                    TypeDefinitionKind::Enum(_) => Value::EnumNamespace(id),
                };
                self.env.define(decl.name.clone(), false, value);
            }
        }
        for item in &program.items {
            if let Item::Const(decl) = item {
                let value = constants
                    .get(&decl.name)
                    .expect("checked module constants are evaluated")
                    .clone();
                self.env.define(decl.name.clone(), false, value);
            }
        }
        for item in &program.items {
            if let Item::Function(decl) = item {
                self.define_function(decl.clone())?;
            }
        }
        Ok(())
    }

    pub fn run_main(&mut self) -> Result<Value, Box<RuntimeError>> {
        let value = self
            .env
            .get("main")
            .map_err(|kind| self.error_at(Span::dummy(), kind))?;
        let Value::Function(function) = value else {
            return Err(self.error_at(
                Span::dummy(),
                RuntimeErrorKind::NotCallable(value.type_name()),
            ));
        };
        match self.call_function(function, vec![], Span::dummy())? {
            EvalFlow::Continue(value) | EvalFlow::Return { value, .. } => Ok(value),
            EvalFlow::Yield { span, .. } => {
                Err(self.error_at(span, RuntimeErrorKind::YieldOutsideHandler))
            }
        }
    }

    fn eval_statement(&mut self, statement: Stmt) -> Result<EvalFlow, Box<RuntimeError>> {
        match statement {
            Stmt::Expr(expr) => {
                let value = self.eval_expr(expr, YieldMode::Bubble)?;
                Ok(EvalFlow::Continue(value_or_flow!(value)))
            }
            Stmt::Return(expr) => {
                let span = expr.span;
                let value = self.eval_expr(expr, YieldMode::Capture)?;
                Ok(EvalFlow::Return {
                    value: value_or_flow!(value),
                    span,
                })
            }
            Stmt::Yield(expr) => {
                let span = expr.span;
                let value = self.eval_expr(expr, YieldMode::Capture)?;
                Ok(EvalFlow::Yield {
                    value: value_or_flow!(value),
                    span,
                })
            }
            Stmt::Bind {
                mutable,
                name,
                value,
                ..
            } => {
                let value = value_or_flow!(self.eval_expr(value, YieldMode::Capture)?);
                self.env.define(name, mutable, value);
                Ok(EvalFlow::Continue(Value::Unit))
            }
            Stmt::Assignment { target, value } => {
                let value_span = value.span;
                let value = self.eval_expr(value, YieldMode::Capture)?;
                match target.kind {
                    ExprKind::Ident(name) => {
                        self.env
                            .assign(&name, value_or_flow!(value))
                            .map_err(|kind| {
                                self.error_at(
                                    Span {
                                        start: target.span.start,
                                        end: value_span.end,
                                    },
                                    kind,
                                )
                            })?;
                        Ok(EvalFlow::Continue(Value::Unit))
                    }
                    ExprKind::Index { target, index } => {
                        let index_span = index.span;
                        let (name, mut indices) =
                            self.resolve_nested_index(*target.clone(), *index.clone(), vec![])?;

                        self.env
                            .mutate_binding(&name, |binding| {
                                if !binding.mutable {
                                    return Err(RuntimeErrorKind::CannotAssignImmutable(
                                        name.clone(),
                                    ));
                                }
                                let BindingKind::Value(Value::Array(mut array)) =
                                    binding.kind.clone()
                                else {
                                    return Err(RuntimeErrorKind::InvalidIndexingTarget(
                                        target.kind.to_string(),
                                    ));
                                };
                                indices.reverse();
                                if let Some((&last_idx, steps)) = indices.split_last() {
                                    for &idx in steps {
                                        if idx as usize >= array.len() || idx < 0 {
                                            return Err(RuntimeErrorKind::OutOfBounds(idx));
                                        }

                                        // eprintln!("array[idx] = {:?}", array[idx as usize]);
                                        // eprintln!("indices = {:?}", indices);
                                        if indices.is_empty() {
                                            let Value::Array(new_array) =
                                                array[idx as usize].clone()
                                            else {
                                                return Err(
                                                    RuntimeErrorKind::InvalidIndexingTarget(
                                                        target.kind.to_string(),
                                                    ),
                                                );
                                            };

                                            array = new_array;
                                        }
                                    }

                                    if last_idx as usize >= array.len() || last_idx < 0 {
                                        return Err(RuntimeErrorKind::OutOfBounds(last_idx));
                                    }

                                    array[last_idx as usize] = value_or_flow!(value);
                                    binding.kind = BindingKind::Value(Value::Array(array));
                                }

                                Ok(EvalFlow::Continue(Value::Unit))
                            })
                            .map_err(|kind| self.error_at(index_span, kind))
                    }
                    ExprKind::Path(segments) => Err(self.error_at(
                        target.span,
                        RuntimeErrorKind::ModuleMemberAssignment(segments.join("::")),
                    )),
                    ExprKind::Field { .. } => {
                        Err(self
                            .error_at(target.span, RuntimeErrorKind::FieldAssignmentUnsupported))
                    }
                    _ => Err(self.error_at(
                        target.span,
                        RuntimeErrorKind::InvalidAssignmentTarget(target.kind.to_string()),
                    )),
                }
            }
            Stmt::Function(decl) => {
                self.define_function(decl)?;
                Ok(EvalFlow::Continue(Value::Unit))
            }
        }
    }

    fn define_function(&mut self, decl: FunctionDecl) -> Result<(), Box<RuntimeError>> {
        let FunctionDecl {
            name,
            parameters,
            return_type,
            body,
            span,
            ..
        } = decl;
        let fun = if let Ok(Value::Function(mut fun)) = self.env.get(&name) {
            let return_type = self
                .type_context
                .resolve_annotation(&return_type)
                .unwrap_or(Type::Any);
            if fun.return_type == return_type {
                let mut repeated = false;
                for p in &fun.overload_variants {
                    if p.parameters == parameters {
                        repeated = true;
                    }
                }
                let overload_variants = if repeated {
                    fun.overload_variants
                } else {
                    fun.overload_variants.push(OverloadFunctionVariant {
                        parameters,
                        body: FunctionBody::Block(body),
                        captured_env: self.env.current_ref(),
                    });
                    fun.overload_variants
                };
                Value::Function(FunctionValue {
                    name: Some(name.clone()),
                    overload_variants,
                    return_type,
                    type_context: self.type_context.clone(),
                })
            } else {
                return Err(self.error_at(
                    span,
                    RuntimeErrorKind::MismatchedReturnTypes {
                        expected: fun.return_type.to_string(),
                        found: return_type.to_string(),
                    },
                ));
            }
        } else {
            let return_type = self
                .type_context
                .resolve_annotation(&return_type)
                .unwrap_or(Type::Any);
            Value::Function(FunctionValue {
                name: Some(name.clone()),
                overload_variants: vec![OverloadFunctionVariant {
                    parameters,
                    body: FunctionBody::Block(body),
                    captured_env: self.env.current_ref(),
                }],
                return_type,
                type_context: self.type_context.clone(),
            })
        };

        self.env.define(name, false, fun);
        Ok(())
    }

    pub fn eval_expr(
        &mut self,
        expr: Expr,
        yield_mode: YieldMode,
    ) -> Result<EvalFlow, Box<RuntimeError>> {
        match expr.kind {
            ExprKind::Int(val) => Ok(EvalFlow::Continue(Value::Int(val))),
            ExprKind::Float(val) => Ok(EvalFlow::Continue(Value::Float(val))),
            ExprKind::String(val) => Ok(EvalFlow::Continue(Value::String(val))),
            ExprKind::Bool(val) => Ok(EvalFlow::Continue(Value::Bool(val))),
            ExprKind::Ident(i) => self.eval_ident(i, expr.span),
            ExprKind::Path(segments) => self.eval_module_path(segments, expr.span),
            ExprKind::Unit => Ok(EvalFlow::Continue(Value::Unit)),
            ExprKind::Block(block) => self.eval_block(block, yield_mode),
            ExprKind::Tuple(exprs) => self.eval_tuple(exprs),
            ExprKind::Array(exprs) => self.eval_array(exprs),
            ExprKind::Unary { op, operand } => self.eval_unary(op, *operand, expr.span),
            ExprKind::Binary { lhs, op, rhs } => self.eval_binary(op, *lhs, *rhs, expr.span),
            ExprKind::Call { callee, args } => self.eval_call(*callee, args, expr.span),
            ExprKind::If {
                condition,
                then_branch,
                else_branch,
            } => self.eval_if(*condition, *then_branch, else_branch, yield_mode),
            ExprKind::While { condition, block } => self.eval_while(*condition, block, yield_mode),
            ExprKind::For {
                binding,
                iterable,
                block,
            } => self.eval_for(*binding, *iterable, block, yield_mode),
            ExprKind::Lambda { parameters, body } => {
                Ok(EvalFlow::Continue(Value::Function(FunctionValue {
                    name: None,
                    overload_variants: vec![OverloadFunctionVariant {
                        parameters,
                        body: FunctionBody::Expr(*body),
                        captured_env: self.env.current_ref(),
                    }],
                    return_type: Type::Any,
                    type_context: self.type_context.clone(),
                })))
            }
            ExprKind::Index { target, index } => {
                let target_span = target.span;
                let index_span = Span {
                    start: index.span.start,
                    end: SrcPos {
                        line: index.span.end.line,
                        col: index.span.end.col - 1,
                        idx: index.span.end.idx - 1,
                    },
                };
                let array = match value_or_flow!(self.eval_expr(*target, YieldMode::Capture)?) {
                    Value::Array(values) => values,
                    other => {
                        eprintln!("eval expr index");
                        return Err(self.error_at(
                            target_span,
                            RuntimeErrorKind::InvalidIndexingTarget(other.to_string()),
                        ));
                    }
                };
                let index = match value_or_flow!(self.eval_expr(*index, YieldMode::Capture)?) {
                    Value::Int(i) => i,
                    other => {
                        return Err(self.error_at(
                            target_span,
                            RuntimeErrorKind::InvalidIndex(other.to_string()),
                        ));
                    }
                };
                if index < 0 || index as usize >= array.len() {
                    return Err(self.error_at(index_span, RuntimeErrorKind::OutOfBounds(index)));
                }
                Ok(EvalFlow::Continue(array[index as usize].clone()))
            }
            ExprKind::Field { target, name } => self.eval_field(*target, name, expr.span),
            ExprKind::Match { value, arms } => self.eval_match(*value, arms, yield_mode, expr.span),
        }
    }

    fn eval_call(
        &mut self,
        callee: Expr,
        args: Vec<ExprArgument>,
        span: Span,
    ) -> Result<EvalFlow, Box<RuntimeError>> {
        let callee_span = callee.span;
        let callee = value_or_flow!(self.eval_expr(callee, YieldMode::Capture)?);

        match callee {
            Value::BuiltinFunction(builtin) => {
                if builtin.needs_raw_args() {
                    self.call_builtin_raw(builtin, args, span)
                } else {
                    let mut args_values = vec![];
                    for arg in args {
                        let value = value_or_flow!(self.eval_expr(arg.value, YieldMode::Capture)?);
                        let arg_value = ValueArgument {
                            name: arg.name,
                            value,
                            span: arg.span,
                        };

                        args_values.push(arg_value);
                    }
                    self.call_builtin(builtin, args_values, span)
                }
            }

            Value::Function(fun) => {
                let mut args_values = vec![];
                for arg in args {
                    let value = value_or_flow!(self.eval_expr(arg.value, YieldMode::Capture)?);
                    let arg_value = ValueArgument {
                        name: arg.name,
                        value,
                        span: arg.span,
                    };

                    args_values.push(arg_value);
                }
                self.call_function(fun, args_values, span)
            }

            Value::Constructor(constructor) => self.call_constructor(constructor, args, span),

            other => Err(self.error_at(
                callee_span,
                RuntimeErrorKind::NotCallable(other.type_name().to_string()),
            )),
        }
    }

    fn eval_block(
        &mut self,
        block: Block,
        yield_mode: YieldMode,
    ) -> Result<EvalFlow, Box<RuntimeError>> {
        let previous_env = self.env.clone();

        self.env.push_scope();

        let result = self.eval_block_inner(block, yield_mode);

        self.env = previous_env;

        result
    }

    fn eval_block_inner(
        &mut self,
        block: Block,
        yield_mode: YieldMode,
    ) -> Result<EvalFlow, Box<RuntimeError>> {
        let mut last_value = Value::Unit;

        for stmt in block.statements {
            match self.eval_statement(stmt)? {
                EvalFlow::Continue(value) => {
                    last_value = value;
                }

                EvalFlow::Return { value, span } => {
                    return Ok(EvalFlow::Return { value, span });
                }

                EvalFlow::Yield { value, span } => {
                    return match yield_mode {
                        YieldMode::Capture => Ok(EvalFlow::Continue(value)),
                        YieldMode::Bubble => Ok(EvalFlow::Yield { value, span }),
                    };
                }
            }
        }

        Ok(EvalFlow::Continue(last_value))
    }

    fn eval_if(
        &mut self,
        condition: Expr,
        then_branch: Expr,
        else_branch: Option<Box<Expr>>,
        yield_mode: YieldMode,
    ) -> Result<EvalFlow, Box<RuntimeError>> {
        let condition_span = condition.span;
        let condition = value_or_flow!(self.eval_expr(condition, YieldMode::Capture)?);

        let condition_bool = match condition {
            Value::Bool(b) => b,
            other => {
                return Err(self.error_at(
                    condition_span,
                    RuntimeErrorKind::ExpectedBool {
                        found: other.type_name(),
                    },
                ));
            }
        };

        let flow = if condition_bool {
            self.eval_expr(then_branch, yield_mode)?
        } else if let Some(else_branch) = else_branch {
            self.eval_expr(*else_branch, yield_mode)?
        } else if let YieldMode::Capture = yield_mode {
            return Err(self.error_at(
                Span {
                    start: condition_span.start,
                    end: then_branch.span.end,
                },
                RuntimeErrorKind::ElseBranchMissing,
            ));
        } else {
            EvalFlow::Continue(Value::Unit)
        };

        match (yield_mode, flow) {
            (YieldMode::Capture, EvalFlow::Yield { value, .. }) => Ok(EvalFlow::Continue(value)),
            (_, other) => Ok(other),
        }
    }

    fn eval_while(
        &mut self,
        condition: Expr,
        block: Block,
        yield_mode: YieldMode,
    ) -> Result<EvalFlow, Box<RuntimeError>> {
        loop {
            let condition_span = condition.span;
            let condition_value =
                value_or_flow!(self.eval_expr(condition.clone(), YieldMode::Capture)?);

            let condition_bool = match condition_value {
                Value::Bool(b) => b,
                other => {
                    return Err(self.error_at(
                        condition_span,
                        RuntimeErrorKind::ExpectedBool {
                            found: other.type_name(),
                        },
                    ));
                }
            };

            if !condition_bool {
                break;
            }

            let flow = self.eval_block(block.clone(), YieldMode::Bubble)?;

            match flow {
                EvalFlow::Continue(_) => {}

                EvalFlow::Return { value, span } => {
                    return Ok(EvalFlow::Return { value, span });
                }

                EvalFlow::Yield { value, span } => {
                    return match yield_mode {
                        YieldMode::Capture => Ok(EvalFlow::Continue(value)),
                        YieldMode::Bubble => Ok(EvalFlow::Yield { value, span }),
                    };
                }
            }
        }

        Ok(EvalFlow::Continue(Value::Unit))
    }

    fn eval_for(
        &mut self,
        binding: Expr,
        iterable: Expr,
        block: Block,
        yield_mode: YieldMode,
    ) -> Result<EvalFlow, Box<RuntimeError>> {
        let iterable_span = iterable.span;
        let binding_span = binding.span;
        let mut iterable = RuntimeIterator::from_value(value_or_flow!(
            self.eval_expr(iterable, YieldMode::Capture)?
        ))
        .map_err(|e| self.error_at(iterable_span, e))?;
        let ExprKind::Ident(binding) = binding.kind else {
            return Err(self.error_at(
                binding_span,
                RuntimeErrorKind::InvalidAssignmentTarget(binding.kind.to_string()),
            ));
        };
        while let Some(value) = iterable.next_value() {
            self.env.define(binding.clone(), false, value);
            let flow = self.eval_block(block.clone(), YieldMode::Bubble)?;

            match flow {
                EvalFlow::Continue(_) => {}

                EvalFlow::Return { value, span } => {
                    return Ok(EvalFlow::Return { value, span });
                }

                EvalFlow::Yield { value, span } => {
                    return match yield_mode {
                        YieldMode::Capture => Ok(EvalFlow::Continue(value)),
                        YieldMode::Bubble => Ok(EvalFlow::Yield { value, span }),
                    };
                }
            }
        }

        Ok(EvalFlow::Continue(Value::Unit))
    }
    fn eval_ident(&mut self, i: String, span: Span) -> Result<EvalFlow, Box<RuntimeError>> {
        // eprintln!("evaluating ident {i}");
        let value = self.env.get(&i).map_err(|kind| {
            Box::new(RuntimeError {
                kind,
                span,
                file_path: self.file_path.clone(),
            })
        })?;
        // eprintln!("has value of {value}");
        Ok(EvalFlow::Continue(value))
    }

    fn eval_module_path(
        &mut self,
        segments: Vec<String>,
        span: Span,
    ) -> Result<EvalFlow, Box<RuntimeError>> {
        let (head, tail) = segments
            .split_first()
            .expect("qualified paths always contain a head");
        let mut value = self
            .env
            .get(head)
            .map_err(|kind| self.error_at(span, kind))?;
        for member in tail {
            value = match value {
                Value::Module(module) => {
                    if !module.exports.contains(member) {
                        return Err(self.error_at(
                            span,
                            RuntimeErrorKind::UnknownModuleExport {
                                module: module.id.to_string(),
                                member: member.clone(),
                            },
                        ));
                    }
                    Env::from_ref(module.env.clone())
                        .get(member)
                        .map_err(|kind| self.error_at(span, kind))?
                }
                Value::EnumNamespace(enum_id) => self.enum_variant_value(&enum_id, member, span)?,
                other => {
                    return Err(self.error_at(
                        span,
                        RuntimeErrorKind::InvalidAssignmentTarget(other.type_name()),
                    ));
                }
            };
        }
        Ok(EvalFlow::Continue(value))
    }

    fn type_definition(&self, id: &TypeId) -> Option<&TypeDefinition> {
        self.type_definitions.get(id)
    }

    fn enum_variant_value(
        &self,
        enum_id: &TypeId,
        name: &str,
        span: Span,
    ) -> Result<Value, Box<RuntimeError>> {
        let Some(TypeDefinition {
            kind: TypeDefinitionKind::Enum(definition),
            ..
        }) = self.type_definition(enum_id)
        else {
            return Err(self.error_at(span, RuntimeErrorKind::NotAnEnum(enum_id.to_string())));
        };
        let variant = definition
            .variants
            .iter()
            .find(|variant| variant.name == name)
            .ok_or_else(|| {
                self.error_at(
                    span,
                    RuntimeErrorKind::UnknownEnumVariant {
                        ty: enum_id.to_string(),
                        variant: name.to_string(),
                    },
                )
            })?;
        match variant.payload {
            VariantPayloadDefinition::Unit => Ok(Value::Enum(EnumValue {
                type_id: enum_id.clone(),
                variant_index: variant.index,
                variant_name: variant.name.clone(),
                payload: None,
            })),
            _ => Ok(Value::Constructor(ConstructorRef::EnumVariant {
                enum_id: enum_id.clone(),
                variant_index: variant.index,
            })),
        }
    }

    fn call_constructor(
        &mut self,
        constructor: ConstructorRef,
        args: Vec<ExprArgument>,
        span: Span,
    ) -> Result<EvalFlow, Box<RuntimeError>> {
        let mut arguments = Vec::new();
        for argument in args {
            let value = value_or_flow!(self.eval_expr(argument.value, YieldMode::Capture)?);
            arguments.push((argument.name.map(|name| name.value), value));
        }

        match constructor {
            ConstructorRef::Struct(type_id) => {
                let Some(TypeDefinition {
                    kind: TypeDefinitionKind::Struct(definition),
                    ..
                }) = self.type_definition(&type_id)
                else {
                    return Err(
                        self.error_at(span, RuntimeErrorKind::NotAStruct(type_id.to_string()))
                    );
                };
                let fields =
                    self.normalize_constructor_fields(&definition.fields, arguments, true, span)?;
                Ok(EvalFlow::Continue(Value::Struct(StructValue {
                    type_id,
                    fields,
                })))
            }
            ConstructorRef::EnumVariant {
                enum_id,
                variant_index,
            } => {
                let Some(TypeDefinition {
                    kind: TypeDefinitionKind::Enum(definition),
                    ..
                }) = self.type_definition(&enum_id)
                else {
                    return Err(
                        self.error_at(span, RuntimeErrorKind::NotAnEnum(enum_id.to_string()))
                    );
                };
                let variant = definition
                    .variants
                    .get(variant_index)
                    .expect("checked variant indexes are valid");
                let payload = match &variant.payload {
                    VariantPayloadDefinition::Unit => {
                        return Err(self.error_at(
                            span,
                            RuntimeErrorKind::NotCallable("unit enum variant".to_string()),
                        ));
                    }
                    VariantPayloadDefinition::Value(_) => {
                        if arguments.len() != 1 || arguments[0].0.is_some() {
                            return Err(self.error_at(
                                span,
                                RuntimeErrorKind::InvalidFunctionParameters(
                                    "enum payload constructors take one positional argument"
                                        .to_string(),
                                ),
                            ));
                        }
                        Some(Box::new(arguments.pop().expect("checked length").1))
                    }
                    VariantPayloadDefinition::InlineStruct(payload_id) => {
                        let Some(TypeDefinition {
                            kind: TypeDefinitionKind::Struct(structure),
                            ..
                        }) = self.type_definition(payload_id)
                        else {
                            unreachable!("inline payload definitions are structs");
                        };
                        let fields = self.normalize_constructor_fields(
                            &structure.fields,
                            arguments,
                            true,
                            span,
                        )?;
                        Some(Box::new(Value::Struct(StructValue {
                            type_id: payload_id.clone(),
                            fields,
                        })))
                    }
                };
                Ok(EvalFlow::Continue(Value::Enum(EnumValue {
                    type_id: enum_id,
                    variant_index,
                    variant_name: variant.name.clone(),
                    payload,
                })))
            }
        }
    }

    fn normalize_constructor_fields(
        &self,
        fields: &[crate::typechecker::ty::FieldDefinition],
        arguments: Vec<(Option<String>, Value)>,
        named_only: bool,
        span: Span,
    ) -> Result<Vec<Value>, Box<RuntimeError>> {
        let mut bound = vec![None; fields.len()];
        for (name, value) in arguments {
            let Some(name) = name else {
                if named_only {
                    return Err(self.error_at(
                        span,
                        RuntimeErrorKind::InvalidFunctionParameters(
                            "struct constructors require named arguments".to_string(),
                        ),
                    ));
                }
                continue;
            };
            let index = fields
                .iter()
                .position(|field| field.name == name)
                .ok_or_else(|| {
                    self.error_at(
                        span,
                        RuntimeErrorKind::InvalidFunctionParameters(format!(
                            "unknown field '{name}'"
                        )),
                    )
                })?;
            if bound[index].is_some() {
                return Err(self.error_at(
                    span,
                    RuntimeErrorKind::InvalidFunctionParameters(format!(
                        "duplicate field '{name}'"
                    )),
                ));
            }
            bound[index] = Some(value);
        }
        fields
            .iter()
            .zip(bound)
            .map(|(field, value)| {
                value.ok_or_else(|| {
                    self.error_at(
                        span,
                        RuntimeErrorKind::InvalidFunctionParameters(format!(
                            "missing field '{}'",
                            field.name
                        )),
                    )
                })
            })
            .collect()
    }

    fn eval_field(
        &mut self,
        target: Expr,
        name: String,
        span: Span,
    ) -> Result<EvalFlow, Box<RuntimeError>> {
        let value = value_or_flow!(self.eval_expr(target, YieldMode::Capture)?);
        let Value::Struct(value) = value else {
            return Err(self.error_at(span, RuntimeErrorKind::NotAStruct(value.type_name())));
        };
        let Some(TypeDefinition {
            kind: TypeDefinitionKind::Struct(definition),
            ..
        }) = self.type_definition(&value.type_id)
        else {
            return Err(self.error_at(
                span,
                RuntimeErrorKind::NotAStruct(value.type_id.to_string()),
            ));
        };
        let index = definition
            .fields
            .iter()
            .position(|field| field.name == name)
            .ok_or_else(|| {
                self.error_at(
                    span,
                    RuntimeErrorKind::UnknownField {
                        ty: value.type_id.to_string(),
                        field: name,
                    },
                )
            })?;
        Ok(EvalFlow::Continue(value.fields[index].clone()))
    }

    fn eval_match(
        &mut self,
        value: Expr,
        arms: Vec<crate::ast::MatchArm>,
        yield_mode: YieldMode,
        span: Span,
    ) -> Result<EvalFlow, Box<RuntimeError>> {
        let value = value_or_flow!(self.eval_expr(value, YieldMode::Capture)?);
        let Value::Enum(enum_value) = value else {
            return Err(self.error_at(span, RuntimeErrorKind::NotAnEnum(value.type_name())));
        };
        for arm in arms {
            let matched = match &arm.pattern {
                crate::ast::Pattern::Wildcard { .. } => true,
                crate::ast::Pattern::EnumVariant { variant, .. } => self
                    .type_definition(&enum_value.type_id)
                    .and_then(|definition| match &definition.kind {
                        TypeDefinitionKind::Enum(definition) => {
                            definition.variants.get(enum_value.variant_index)
                        }
                        _ => None,
                    })
                    .is_some_and(|definition| definition.name == *variant),
            };
            if !matched {
                continue;
            }
            let previous = self.env.clone();
            self.env.push_scope();
            if let crate::ast::Pattern::EnumVariant {
                binding: Some(name),
                ..
            } = arm.pattern
            {
                self.env.define(
                    name,
                    false,
                    *enum_value
                        .payload
                        .clone()
                        .expect("checked payload binding has a payload"),
                );
            }
            let result = self.eval_block(arm.body, yield_mode);
            self.env = previous;
            return result;
        }
        Err(self.error_at(span, RuntimeErrorKind::NotImplemented))
    }

    fn eval_tuple(&mut self, exprs: Vec<Expr>) -> Result<EvalFlow, Box<RuntimeError>> {
        {
            let mut values = vec![];

            for expr in exprs {
                let value = value_or_flow!(self.eval_expr(expr, YieldMode::Capture)?);
                values.push(value);
            }

            Ok(EvalFlow::Continue(Value::Tuple(values)))
        }
    }

    fn eval_array(&mut self, exprs: Vec<Expr>) -> Result<EvalFlow, Box<RuntimeError>> {
        {
            let mut values = vec![];

            for expr in exprs {
                let value = value_or_flow!(self.eval_expr(expr, YieldMode::Capture)?);
                values.push(value);
            }

            Ok(EvalFlow::Continue(Value::Array(values)))
        }
    }

    fn eval_unary(
        &mut self,
        op: UnaryOp,
        operand: Expr,
        span: Span,
    ) -> Result<EvalFlow, Box<RuntimeError>> {
        let value = value_or_flow!(self.eval_expr(operand, YieldMode::Capture)?);
        match op {
            crate::ast::UnaryOp::Negate => match value {
                Value::Int(val) => Ok(EvalFlow::Continue(Value::Int(-val))),
                Value::Float(val) => Ok(EvalFlow::Continue(Value::Float(-val))),
                other => Err(self.error_at(
                    span,
                    RuntimeErrorKind::InvalidUnaryOp {
                        op,
                        operand: other.type_name(),
                    },
                )),
            },
            crate::ast::UnaryOp::Not => match value {
                Value::Bool(val) => Ok(EvalFlow::Continue(Value::Bool(!val))),
                other => Err(self.error_at(
                    span,
                    RuntimeErrorKind::InvalidUnaryOp {
                        op,
                        operand: other.type_name(),
                    },
                )),
            },
        }
    }

    fn eval_binary(
        &mut self,
        op: BinaryOp,
        lhs: Expr,
        rhs: Expr,
        span: Span,
    ) -> Result<EvalFlow, Box<RuntimeError>> {
        {
            let lhs = value_or_flow!(self.eval_expr(lhs, YieldMode::Capture)?);
            let rhs = value_or_flow!(self.eval_expr(rhs, YieldMode::Capture)?);
            match op {
                BinaryOp::Add => match (lhs, rhs) {
                    (Value::Int(i1), Value::Int(i2)) => Ok(EvalFlow::Continue(Value::Int(i1 + i2))),
                    (Value::Float(f1), Value::Float(f2)) => {
                        Ok(EvalFlow::Continue(Value::Float(f1 + f2)))
                    }
                    (other_lhs, other_rhs) => Err(self.error_at(
                        span,
                        RuntimeErrorKind::InvalidBinaryOp {
                            op,
                            lhs: other_lhs.type_name(),
                            rhs: other_rhs.type_name(),
                        },
                    )),
                },
                BinaryOp::Sub => match (lhs, rhs) {
                    (Value::Int(i1), Value::Int(i2)) => Ok(EvalFlow::Continue(Value::Int(i1 - i2))),
                    (Value::Float(f1), Value::Float(f2)) => {
                        Ok(EvalFlow::Continue(Value::Float(f1 - f2)))
                    }
                    (other_lhs, other_rhs) => Err(self.error_at(
                        span,
                        RuntimeErrorKind::InvalidBinaryOp {
                            op,
                            lhs: other_lhs.type_name(),
                            rhs: other_rhs.type_name(),
                        },
                    )),
                },
                BinaryOp::Mul => match (lhs, rhs) {
                    (Value::Int(i1), Value::Int(i2)) => Ok(EvalFlow::Continue(Value::Int(i1 * i2))),
                    (Value::Float(f1), Value::Float(f2)) => {
                        Ok(EvalFlow::Continue(Value::Float(f1 * f2)))
                    }
                    (other_lhs, other_rhs) => Err(self.error_at(
                        span,
                        RuntimeErrorKind::InvalidBinaryOp {
                            op,
                            lhs: other_lhs.type_name(),
                            rhs: other_rhs.type_name(),
                        },
                    )),
                },
                BinaryOp::Div => match (lhs, rhs) {
                    (Value::Int(_), Value::Int(0)) | (Value::Float(_), Value::Float(0.0)) => {
                        Err(self.error_at(span, RuntimeErrorKind::DivisionByZero))
                    }
                    (Value::Int(i1), Value::Int(i2)) => Ok(EvalFlow::Continue(Value::Int(i1 / i2))),
                    (Value::Float(f1), Value::Float(f2)) => {
                        Ok(EvalFlow::Continue(Value::Float(f1 / f2)))
                    }
                    (other_lhs, other_rhs) => Err(self.error_at(
                        span,
                        RuntimeErrorKind::InvalidBinaryOp {
                            op,
                            lhs: other_lhs.type_name(),
                            rhs: other_rhs.type_name(),
                        },
                    )),
                },
                BinaryOp::Eq => match (lhs, rhs) {
                    (Value::Float(a), Value::Float(b)) => {
                        Ok(EvalFlow::Continue(Value::Bool(a == b)))
                    }
                    (Value::Int(a), Value::Int(b)) => Ok(EvalFlow::Continue(Value::Bool(a == b))),
                    (Value::Bool(a), Value::Bool(b)) => Ok(EvalFlow::Continue(Value::Bool(a == b))),
                    (Value::Tuple(a), Value::Tuple(b)) => {
                        Ok(EvalFlow::Continue(Value::Bool(a == b)))
                    }
                    (Value::String(a), Value::String(b)) => {
                        Ok(EvalFlow::Continue(Value::Bool(a == b)))
                    }
                    (Value::Unit, Value::Unit) => Ok(EvalFlow::Continue(Value::Bool(true))),
                    (Value::Array(a), Value::Array(b)) => {
                        Ok(EvalFlow::Continue(Value::Bool(a == b)))
                    }
                    (Value::BuiltinFunction(a), Value::BuiltinFunction(b)) => {
                        Ok(EvalFlow::Continue(Value::Bool(a == b)))
                    }
                    (Value::Range(a), Value::Range(b)) => {
                        Ok(EvalFlow::Continue(Value::Bool(a == b)))
                    }
                    (Value::Function(a), Value::Function(b)) => {
                        Ok(EvalFlow::Continue(Value::Bool(a == b)))
                    }
                    (Value::Struct(a), Value::Struct(b)) => {
                        Ok(EvalFlow::Continue(Value::Bool(a == b)))
                    }
                    (Value::Enum(a), Value::Enum(b)) => Ok(EvalFlow::Continue(Value::Bool(a == b))),
                    (other_lhs, other_rhs) => Err(self.error_at(
                        span,
                        RuntimeErrorKind::InvalidBinaryOp {
                            op,
                            lhs: other_lhs.type_name(),
                            rhs: other_rhs.type_name(),
                        },
                    )),
                },
                BinaryOp::NotEq => match (lhs, rhs) {
                    (Value::Float(a), Value::Float(b)) => {
                        Ok(EvalFlow::Continue(Value::Bool(a != b)))
                    }
                    (Value::Int(a), Value::Int(b)) => Ok(EvalFlow::Continue(Value::Bool(a != b))),
                    (Value::Bool(a), Value::Bool(b)) => Ok(EvalFlow::Continue(Value::Bool(a != b))),
                    (Value::Tuple(a), Value::Tuple(b)) => {
                        Ok(EvalFlow::Continue(Value::Bool(a != b)))
                    }
                    (Value::String(a), Value::String(b)) => {
                        Ok(EvalFlow::Continue(Value::Bool(a != b)))
                    }
                    (Value::Unit, Value::Unit) => Ok(EvalFlow::Continue(Value::Bool(false))),
                    (Value::Array(a), Value::Array(b)) => {
                        Ok(EvalFlow::Continue(Value::Bool(a != b)))
                    }
                    (Value::BuiltinFunction(a), Value::BuiltinFunction(b)) => {
                        Ok(EvalFlow::Continue(Value::Bool(a != b)))
                    }
                    (Value::Range(a), Value::Range(b)) => {
                        Ok(EvalFlow::Continue(Value::Bool(a != b)))
                    }
                    (Value::Function(a), Value::Function(b)) => {
                        Ok(EvalFlow::Continue(Value::Bool(a != b)))
                    }
                    (Value::Struct(a), Value::Struct(b)) => {
                        Ok(EvalFlow::Continue(Value::Bool(a != b)))
                    }
                    (Value::Enum(a), Value::Enum(b)) => Ok(EvalFlow::Continue(Value::Bool(a != b))),
                    (other_lhs, other_rhs) => Err(self.error_at(
                        span,
                        RuntimeErrorKind::InvalidBinaryOp {
                            op,
                            lhs: other_lhs.type_name(),
                            rhs: other_rhs.type_name(),
                        },
                    )),
                },
                BinaryOp::Greater => match (lhs, rhs) {
                    (Value::Float(a), Value::Float(b)) => {
                        Ok(EvalFlow::Continue(Value::Bool(a > b)))
                    }
                    (Value::Int(a), Value::Int(b)) => Ok(EvalFlow::Continue(Value::Bool(a > b))),
                    (other_lhs, other_rhs) => Err(self.error_at(
                        span,
                        RuntimeErrorKind::InvalidBinaryOp {
                            op,
                            lhs: other_lhs.type_name(),
                            rhs: other_rhs.type_name(),
                        },
                    )),
                },
                BinaryOp::GreaterEq => match (lhs, rhs) {
                    (Value::Float(a), Value::Float(b)) => {
                        Ok(EvalFlow::Continue(Value::Bool(a >= b)))
                    }
                    (Value::Int(a), Value::Int(b)) => Ok(EvalFlow::Continue(Value::Bool(a >= b))),
                    (other_lhs, other_rhs) => Err(self.error_at(
                        span,
                        RuntimeErrorKind::InvalidBinaryOp {
                            op,
                            lhs: other_lhs.type_name(),
                            rhs: other_rhs.type_name(),
                        },
                    )),
                },
                BinaryOp::Less => match (lhs, rhs) {
                    (Value::Float(a), Value::Float(b)) => {
                        Ok(EvalFlow::Continue(Value::Bool(a < b)))
                    }
                    (Value::Int(a), Value::Int(b)) => Ok(EvalFlow::Continue(Value::Bool(a < b))),
                    (other_lhs, other_rhs) => Err(self.error_at(
                        span,
                        RuntimeErrorKind::InvalidBinaryOp {
                            op,
                            lhs: other_lhs.type_name(),
                            rhs: other_rhs.type_name(),
                        },
                    )),
                },
                BinaryOp::LessEq => match (lhs, rhs) {
                    (Value::Float(a), Value::Float(b)) => {
                        Ok(EvalFlow::Continue(Value::Bool(a <= b)))
                    }
                    (Value::Int(a), Value::Int(b)) => Ok(EvalFlow::Continue(Value::Bool(a <= b))),
                    (other_lhs, other_rhs) => Err(self.error_at(
                        span,
                        RuntimeErrorKind::InvalidBinaryOp {
                            op,
                            lhs: other_lhs.type_name(),
                            rhs: other_rhs.type_name(),
                        },
                    )),
                },
                BinaryOp::Combine => match (lhs, rhs) {
                    (Value::String(s1), s2) => Ok(EvalFlow::Continue(Value::String(
                        format!("{s1}{s2}").to_string(),
                    ))),
                    (s1, Value::String(s2)) => Ok(EvalFlow::Continue(Value::String(
                        format!("{s1}{s2}").to_string(),
                    ))),
                    (other_lhs, other_rhs) => Err(self.error_at(
                        span,
                        RuntimeErrorKind::InvalidBinaryOp {
                            op,
                            lhs: other_lhs.type_name(),
                            rhs: other_rhs.type_name(),
                        },
                    )),
                },
                BinaryOp::InclusiveRange => {
                    let Value::Int(start) = lhs else {
                        return Err(self.error_at(
                            span,
                            RuntimeErrorKind::InvalidFunctionParameter(lhs.to_string()),
                        ));
                    };
                    let Value::Int(end) = rhs else {
                        return Err(self.error_at(
                            span,
                            RuntimeErrorKind::InvalidFunctionParameter(rhs.to_string()),
                        ));
                    };
                    Ok(EvalFlow::Continue(Value::Range(RangeValue {
                        start,
                        end,
                        inclusive: true,
                        step: 1,
                    })))
                }
                BinaryOp::ExclusiveRange => {
                    let Value::Int(start) = lhs else {
                        return Err(self.error_at(
                            span,
                            RuntimeErrorKind::InvalidFunctionParameter(lhs.to_string()),
                        ));
                    };
                    let Value::Int(end) = rhs else {
                        return Err(self.error_at(
                            span,
                            RuntimeErrorKind::InvalidFunctionParameter(rhs.to_string()),
                        ));
                    };
                    Ok(EvalFlow::Continue(Value::Range(RangeValue {
                        start,
                        end,
                        inclusive: false,
                        step: 1,
                    })))
                }
            }
        }
    }

    pub fn resolve_nested_index(
        &mut self,
        target: Expr,
        index: Expr,
        current_indices: Vec<i64>,
    ) -> Result<(String, Vec<i64>), Box<RuntimeError>> {
        let ExprKind::Int(new_idx) = index.kind else {
            return Err(self.error_at(
                target.span,
                RuntimeErrorKind::InvalidIndex(index.kind.to_string()),
            ));
        };
        let mut new_indices = current_indices;
        new_indices.push(new_idx);
        let (name, new_indices) = match target.kind {
            ExprKind::Ident(i) => (i, new_indices),
            ExprKind::Index { target, index } => {
                self.resolve_nested_index(*target, *index, new_indices)?
            }
            _ => {
                return Err(self.error_at(
                    target.span,
                    RuntimeErrorKind::InvalidIndexingTarget(target.kind.to_string()),
                ));
            }
        };
        Ok((name, new_indices))
    }

    fn call_function(
        &mut self,
        fun: FunctionValue,
        args: Vec<ValueArgument>,
        span: Span,
    ) -> Result<EvalFlow, Box<RuntimeError>> {
        let matched = fun
            .match_variant(args)
            .map_err(|e| self.argument_error(span, *e))?;

        let previous_env = self.env.clone();
        let previous_type_context = self.type_context.clone();
        self.type_context = fun.type_context.clone();
        self.env = Env::from_ref(Rc::new(RefCell::new(EnvFrame {
            frame: HashMap::new(),
            parent: Some(
                fun.overload_variants[matched.variant_index]
                    .captured_env
                    .clone(),
            ),
        })));

        let variant = &fun.overload_variants[matched.variant_index];

        for (parameter, argument) in variant.parameters.iter().zip(matched.arguments) {
            self.env.define(parameter.name.clone(), false, argument);
        }

        let result = match variant.body.clone() {
            FunctionBody::Block(block) => self.eval_block(block, YieldMode::Bubble),

            FunctionBody::Expr(expr) => self.eval_expr(expr, YieldMode::Capture),
        };

        self.env = previous_env;
        self.type_context = previous_type_context;

        match result? {
            EvalFlow::Continue(value) => Ok(EvalFlow::Continue(value)),
            EvalFlow::Return { value, .. } => Ok(EvalFlow::Continue(value)),
            EvalFlow::Yield { .. } => {
                Err(self.error_at(span, RuntimeErrorKind::YieldOutsideHandler))
            }
        }
    }

    pub fn error_at(&self, span: Span, kind: RuntimeErrorKind) -> Box<RuntimeError> {
        Box::new(RuntimeError {
            kind,
            span,
            file_path: self.file_path.clone(),
        })
    }

    pub fn argument_error(&self, span: Span, e: ArgumentError) -> Box<RuntimeError> {
        let span = e.span.unwrap_or(span);
        Box::new(RuntimeError {
            kind: RuntimeErrorKind::ArgumentError {
                e: Box::new(e.clone()),
            },
            span,
            file_path: self.file_path.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::lexer::Lexer;
    use std::{
        ffi::OsStr,
        fs::{read_dir, read_to_string},
    };

    use crate::{
        dump::{create_expected_by_ext, normalize_snapshot_paths},
        interpreter::Interpreter,
        module::{CompilerSession, RuntimeModules},
        parser,
    };

    #[test]
    fn validate_expected_eval() {
        let cargo_dir = env!("CARGO_MANIFEST_DIR");
        let entries = read_dir(format!("{cargo_dir}/fixtures")).unwrap();

        for entry in entries {
            let current_file_path = entry.unwrap().path();

            if current_file_path.is_file()
                && current_file_path.extension() == Some(OsStr::new("fob"))
            {
                let eval_expected_path =
                    create_expected_by_ext(&current_file_path, ".eval").unwrap();

                let expected_eval = match read_to_string(eval_expected_path.clone()) {
                    Ok(s) => normalize_snapshot_paths(&s),
                    Err(_) => {
                        eprintln!(
                            "Expected eval file {eval_expected_path:?} not found. Skipping it"
                        );
                        continue;
                    }
                };

                let content = read_to_string(&current_file_path).unwrap();
                let tokens = Lexer::new(&current_file_path, &content).tokenize();

                let eval_str = match tokens {
                    Ok(tokens) => {
                        let ast = parser::Parser::new(tokens, &current_file_path).parse_program();

                        match ast {
                            Ok(_) => {
                                let mut interpreter = Interpreter::new_buffered(&current_file_path);
                                let eval_result = CompilerSession::default()
                                    .compile_file(&current_file_path)
                                    .and_then(|compilation| {
                                        RuntimeModules::new(compilation)
                                            .execute_root(&mut interpreter)
                                    });
                                let output = interpreter.into_output_string();

                                match eval_result {
                                    Ok(value) => {
                                        if output.is_empty() {
                                            format!("result:\n{value:#?}\n")
                                        } else {
                                            format!("output:\n{output}\nresult:\n{value:#?}\n")
                                        }
                                    }
                                    Err(e) => normalize_snapshot_paths(&format!("{e:#?}")),
                                }
                            }
                            Err(e) => normalize_snapshot_paths(&format!("{e:#?}")),
                        }
                    }
                    Err(e) => normalize_snapshot_paths(&format!("{e:#?}")),
                };

                for (ev, ex) in eval_str
                    .trim_end()
                    .lines()
                    .zip(expected_eval.trim_end().lines())
                {
                    assert_eq!(
                        snapshot_line(ev),
                        snapshot_line(ex),
                        "failed to match ast output in file {eval_expected_path:?}"
                    );
                }
            }
        }
    }

    fn snapshot_line(line: &str) -> &str {
        if line.trim_start().starts_with("file_path:") {
            "file_path: <fixture path>"
        } else {
            line
        }
    }
}
