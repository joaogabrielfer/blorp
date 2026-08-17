use crate::{
    ast::{
        self, BinaryOp, EnumDecl, EnumPayloadDecl, EnumVariantDecl, Expr, ExprArgument, ExprKind,
        ImportItem, ImportSource, ImportTree, Parameter, RelativeImportMode, StructDecl,
        StructFieldDecl, TypeAnnotation, TypeDecl, TypeDeclKind, TypeExpr,
    },
    errors::{ParserError, ParserErrorKind},
    lexer::{
        Token, TokenKind,
        TokenTag::{self, RParen},
    },
    source::Span,
};
use anyhow::Result;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct Parser<'a> {
    tokens: Vec<Token<'a>>,
    index: usize,
    file_path: &'a PathBuf,
}

impl<'a> Parser<'a> {
    pub fn new(tokens: Vec<Token<'a>>, file_path: &'a PathBuf) -> Self {
        Self {
            tokens,
            file_path,
            index: 0,
        }
    }

    pub fn parse_program(&mut self) -> Result<ast::Program, Box<ParserError>> {
        let mut items = vec![];
        self.consume_newlines();

        while !self.is_at_end() {
            items.push(self.parse_item()?);
            self.expect(TokenTag::NewLine)?;
            self.consume_newlines();
        }
        Ok(ast::Program { items })
    }

    /// Parse a REPL submission. This deliberately stays separate from module
    /// parsing: source files never admit top-level statements.
    pub fn parse_repl_submission(&mut self) -> Result<Vec<ast::Stmt>, Box<ParserError>> {
        let mut statements = vec![];
        self.consume_newlines();

        while !self.is_at_end() {
            statements.push(self.parse_statement()?);
            self.expect(TokenTag::NewLine)?;
            self.consume_newlines();
        }
        Ok(statements)
    }

    fn parse_item(&mut self) -> Result<ast::Item, Box<ParserError>> {
        let public = self.check_and_advance(TokenTag::Pub);
        match self.current().kind {
            TokenKind::Type => self.parse_type_decl(public).map(ast::Item::Type),
            TokenKind::Const => self.parse_const_decl(public).map(ast::Item::Const),
            TokenKind::Fun => self.parse_fun_decl(public).map(ast::Item::Function),
            TokenKind::Import => {
                if public {
                    return Err(self.error(ParserErrorKind::InvalidImport {
                        found: "pub import".to_string(),
                    }));
                }
                self.parse_import_decl().map(ast::Item::Import)
            }
            _ => Err(self.error(ParserErrorKind::ExpectedTopLevelItem {
                found: if public {
                    "pub".to_string()
                } else {
                    self.current_tag().to_string()
                },
            })),
        }
    }

    fn parse_statement(&mut self) -> Result<ast::Stmt, Box<ParserError>> {
        match self.current().kind {
            TokenKind::Let => self.parse_binding(false),
            TokenKind::Var => self.parse_binding(true),
            TokenKind::Return => {
                self.advance();
                Ok(ast::Stmt::Return(self.parse_expr()?))
            }
            TokenKind::Yield => {
                self.advance();
                Ok(ast::Stmt::Yield(self.parse_expr()?))
            }
            TokenKind::Fun => self.parse_fun_decl(false).map(ast::Stmt::Function),
            TokenKind::Const | TokenKind::Import | TokenKind::Pub | TokenKind::Type => Err(self
                .error(ParserErrorKind::UnexpectedToken {
                    found: self.current_tag().to_string(),
                })),
            _ => {
                let starting_span = self.current().span;
                let expr = self.parse_expr()?;
                if self.check(TokenTag::Equals) {
                    if !matches!(
                        expr.kind,
                        ast::ExprKind::Ident(_)
                            | ast::ExprKind::Path(_)
                            | ast::ExprKind::Tuple(_)
                            | ast::ExprKind::Index { .. }
                            | ast::ExprKind::Field { .. }
                    ) {
                        return Err(Box::new(ParserError {
                            kind: ParserErrorKind::InvalidAssignmentTarget {
                                expr: expr.kind.to_string(),
                            },
                            file_path: self.file_path.clone(),
                            pos: starting_span,
                        }));
                    }
                    self.advance();
                    let value = self.parse_expr()?;
                    Ok(ast::Stmt::Assignment {
                        target: expr,
                        value,
                    })
                } else {
                    Ok(ast::Stmt::Expr(expr))
                }
            }
        }
    }

    fn parse_binding(&mut self, mutable: bool) -> Result<ast::Stmt, Box<ParserError>> {
        let start_span = self.current().span;
        if mutable {
            self.expect(TokenTag::Var)?;
        } else {
            self.expect(TokenTag::Let)?;
        }
        let name = self.expect_ident()?;
        self.expect(TokenTag::Colon)?;
        let type_annotation = self.parse_option_type()?;
        self.expect(TokenTag::Equals)?;
        let value = if self.check(TokenTag::NewLine) {
            let kind = ExprKind::Block(self.parse_block()?);
            self.new_expr(start_span, kind)
        } else {
            self.parse_expr()?
        };

        let value_span = value.span;
        Ok(ast::Stmt::Bind {
            mutable,
            name,
            type_annotation,
            value,
            span: Span {
                start: start_span.start,
                end: value_span.end,
            },
        })
    }

    fn parse_type_decl(&mut self, public: bool) -> Result<TypeDecl, Box<ParserError>> {
        let start = self.current().span;
        self.expect(TokenTag::Type)?;
        let name = self.expect_ident()?;
        self.expect(TokenTag::Colon)?;
        let kind = match self.current_tag() {
            TokenTag::Struct => TypeDeclKind::Struct(self.parse_struct_decl()?),
            TokenTag::Enum => TypeDeclKind::Enum(self.parse_enum_decl()?),
            _ => {
                return Err(self.error(ParserErrorKind::ExpectedTokens {
                    expected: vec!["struct".to_string(), "enum".to_string()],
                    found: self.current_tag().to_string(),
                }));
            }
        };
        let end = match &kind {
            TypeDeclKind::Struct(decl) => decl.span.end,
            TypeDeclKind::Enum(decl) => decl.span.end,
        };
        Ok(TypeDecl {
            public,
            name,
            kind,
            span: Span {
                start: start.start,
                end,
            },
        })
    }

    fn parse_struct_decl(&mut self) -> Result<StructDecl, Box<ParserError>> {
        let start = self.current().span;
        self.expect(TokenTag::Struct)?;
        self.expect(TokenTag::Equals)?;
        self.expect(TokenTag::NewLine)?;
        self.consume_newlines();

        let mut fields = Vec::new();
        while !self.check(TokenTag::End) {
            let field_start = self.current().span;
            let name = self.expect_ident()?;
            self.expect(TokenTag::Colon)?;
            let ty = self.parse_required_type()?;
            let end = self.current().span;
            self.check_and_advance(TokenTag::Comma);
            self.expect(TokenTag::NewLine)?;
            self.consume_newlines();
            fields.push(StructFieldDecl {
                name,
                ty,
                span: Span {
                    start: field_start.start,
                    end: end.end,
                },
            });
        }
        let end = self.current().span;
        self.expect(TokenTag::End)?;
        Ok(StructDecl {
            fields,
            span: Span {
                start: start.start,
                end: end.end,
            },
        })
    }

    fn parse_enum_decl(&mut self) -> Result<EnumDecl, Box<ParserError>> {
        let start = self.current().span;
        self.expect(TokenTag::Enum)?;
        self.expect(TokenTag::Equals)?;
        self.expect(TokenTag::NewLine)?;
        self.consume_newlines();

        let mut variants = Vec::new();
        while !self.check(TokenTag::End) {
            let variant_start = self.current().span;
            let name = self.expect_ident()?;
            let payload = if self.check_and_advance(TokenTag::Colon) {
                if self.check(TokenTag::Struct) {
                    Some(EnumPayloadDecl::InlineStruct(self.parse_struct_decl()?))
                } else {
                    Some(EnumPayloadDecl::Type(self.parse_required_type()?))
                }
            } else {
                None
            };

            let end = match &payload {
                Some(EnumPayloadDecl::InlineStruct(decl)) => decl.span.end,
                _ => self.current().span.end,
            };
            if !matches!(payload, Some(EnumPayloadDecl::InlineStruct(_))) {
                self.check_and_advance(TokenTag::Comma);
                self.expect(TokenTag::NewLine)?;
                self.consume_newlines();
            } else {
                self.check_and_advance(TokenTag::Comma);
                self.consume_newlines();
            }
            variants.push(EnumVariantDecl {
                name,
                payload,
                span: Span {
                    start: variant_start.start,
                    end,
                },
            });
        }
        let end = self.current().span;
        self.expect(TokenTag::End)?;
        Ok(EnumDecl {
            variants,
            span: Span {
                start: start.start,
                end: end.end,
            },
        })
    }

    fn parse_const_decl(&mut self, public: bool) -> Result<ast::ConstDecl, Box<ParserError>> {
        let start_span = self.current().span;
        self.expect(TokenTag::Const)?;
        let name = self.expect_ident()?;
        self.expect(TokenTag::Colon)?;
        let TypeAnnotation::Explicit(type_annotation) = self.parse_option_type()? else {
            return Err(self.error(ParserErrorKind::ConstRequiresType));
        };
        self.expect(TokenTag::Equals)?;
        let value = self.parse_expr()?;
        Ok(ast::ConstDecl {
            public,
            name,
            type_annotation,
            span: Span {
                start: start_span.start,
                end: value.span.end,
            },
            value,
        })
    }

    fn parse_import_decl(&mut self) -> Result<ast::ImportDecl, Box<ParserError>> {
        let start_span = self.current().span;
        self.expect(TokenTag::Import)?;
        match self.current_tag() {
            TokenTag::String => {
                let TokenKind::String(path) = self.current().kind else {
                    unreachable!("the current tag was checked above")
                };
                let path = path.to_string();
                self.expect(TokenTag::String)?;
                let alias = if self.check_and_advance(TokenTag::As) {
                    Some(self.expect_ident()?)
                } else {
                    None
                };
                let mode = match alias {
                    Some(alias) => RelativeImportMode::Namespace { alias },
                    None => RelativeImportMode::Glob,
                };
                Ok(ast::ImportDecl {
                    source: ImportSource::Relative { path, mode },
                    span: Span {
                        start: start_span.start,
                        end: self.current().span.end,
                    },
                })
            }
            TokenTag::Ident => {
                let mut module_path = vec![self.expect_ident()?];
                while self.check_and_advance(TokenTag::ColonColon) {
                    if self.current_tag() == TokenTag::Ident {
                        module_path.push(self.expect_ident()?);
                    } else {
                        break;
                    }
                }
                let tree = match self.current_tag() {
                    TokenTag::NewLine => ImportTree::Path {
                        segments: module_path,
                        alias: None,
                    },
                    TokenTag::Star => {
                        self.expect(TokenTag::Star)?;
                        ImportTree::Glob { module_path }
                    }
                    TokenTag::LCurly => {
                        self.expect(TokenTag::LCurly)?;
                        let mut items = vec![];
                        self.consume_newlines();
                        while !self.check_and_advance(TokenTag::RCurly) {
                            let item_span = self.current().span;
                            let name = self.expect_ident()?;
                            let alias = if self.check_and_advance(TokenTag::As) {
                                Some(self.expect_ident()?)
                            } else {
                                None
                            };
                            items.push(ImportItem {
                                name,
                                alias,
                                span: item_span,
                            });
                            if self.current_tag() != TokenTag::RCurly {
                                self.expect(TokenTag::Comma)?;
                                self.consume_newlines();
                            }
                        }
                        ImportTree::Group { module_path, items }
                    }
                    TokenTag::As => {
                        self.expect(TokenTag::As)?;
                        ImportTree::Path {
                            segments: module_path,
                            alias: Some(self.expect_ident()?),
                        }
                    }
                    other => {
                        return Err(self.error(ParserErrorKind::InvalidModulePath {
                            found: other.to_string(),
                        }));
                    }
                };
                Ok(ast::ImportDecl {
                    source: ImportSource::Module { tree },
                    span: Span {
                        start: start_span.start,
                        end: self.current().span.end,
                    },
                })
            }
            _ => Err(self.error(ParserErrorKind::InvalidImport {
                found: self.current_tag().to_string(),
            })),
        }
    }

    fn parse_while(&mut self) -> Result<Expr, Box<ParserError>> {
        self.expect(TokenTag::While)?;
        let condition = Box::new(self.parse_expr()?);
        self.expect(TokenTag::Do)?;
        let block = self.parse_block()?;
        Ok(self.new_expr(self.current().span, ExprKind::While { condition, block }))
    }

    fn parse_for(&mut self) -> Result<Expr, Box<ParserError>> {
        self.expect(TokenTag::For)?;
        let binding = Box::new(self.parse_expr()?);
        self.expect(TokenTag::In)?;
        let iterable = Box::new(self.parse_expr()?);
        self.expect(TokenTag::Do)?;
        let block = self.parse_block()?;
        Ok(self.new_expr(
            self.current().span,
            ExprKind::For {
                binding,
                iterable,
                block,
            },
        ))
    }

    fn parse_fun_decl(&mut self, public: bool) -> Result<ast::FunctionDecl, Box<ParserError>> {
        let start_span = self.current().span;
        self.expect(TokenTag::Fun)?;
        let mut generics = vec![];
        if let TokenTag::LBrace = self.current_tag() {
            self.advance();
            while !self.check(TokenTag::RBrace) {
                let name = self.expect_ident()?;
                generics.push(name);
                if let TokenTag::Comma = self.current_tag() {
                    self.expect(TokenTag::Comma)?;
                }
            }
            self.expect(TokenTag::RBrace)?;
        }
        let name = self.expect_ident()?;
        self.expect(TokenTag::LParen)?;
        let mut parameters = vec![];
        self.check_and_advance(TokenTag::NewLine);
        while !self.check(TokenTag::NewLine) {
            if let TokenTag::RParen = self.current_tag() {
                self.advance();
                break;
            }
            let name = self.expect_ident()?;
            self.expect(TokenTag::Colon)?;
            let TypeAnnotation::Explicit(t) = self.parse_option_type()? else {
                return Err(self.error(ParserErrorKind::ExpectedTypeAnnotation));
            };
            parameters.push(ast::Parameter {
                name,
                t: TypeAnnotation::Explicit(t),
            });
            match self.current_tag() {
                TokenTag::Comma => {
                    self.expect(TokenTag::Comma)?;
                    self.check_and_advance(TokenTag::NewLine);
                }
                TokenTag::RParen => {
                    self.expect(RParen)?;
                    break;
                }
                other => {
                    return Err(self.error(ParserErrorKind::ExpectedTokens {
                        expected: vec![")".to_string(), ",".to_string()],
                        found: other.to_string(),
                    }));
                }
            }
        }
        self.expect(TokenTag::Colon)?;
        let return_type = self.parse_option_type()?;
        let end_span = self.current().span;
        self.expect(TokenTag::Equals)?;
        let body = self.parse_block()?;
        Ok(ast::FunctionDecl {
            public,
            name,
            generics,
            parameters,
            body,
            return_type,
            span: Span {
                start: start_span.start,
                end: end_span.end,
            },
        })
    }

    fn parse_expr(&mut self) -> Result<ast::Expr, Box<ParserError>> {
        self.parse_lambda()
    }

    fn parse_lambda(&mut self) -> Result<ast::Expr, Box<ParserError>> {
        let start_span = self.current().span;
        let expr = self.parse_binary_expr(0)?;
        if let TokenTag::RArrow = self.current_tag() {
            let mut parameters = vec![];
            match expr.kind {
                ExprKind::Ident(name) => parameters.push(Parameter {
                    name,
                    t: TypeAnnotation::Inferred,
                }),
                ExprKind::Tuple(t) => {
                    for e in t {
                        match e.kind {
                            ExprKind::Ident(name) => parameters.push(Parameter {
                                name,
                                t: TypeAnnotation::Inferred,
                            }),
                            other => {
                                return Err(self.error(ParserErrorKind::InvalidParameter {
                                    expr: other.to_string(),
                                }));
                            }
                        }
                    }
                }
                other => {
                    return Err(self.error(ParserErrorKind::InvalidParameter {
                        expr: other.to_string(),
                    }));
                }
            }
            self.expect(TokenTag::RArrow)?;
            let span = self.current().span;
            let body = if self.check(TokenTag::NewLine) {
                let block = self.parse_block()?;
                Box::new(self.new_expr(span, ExprKind::Block(block)))
            } else {
                Box::new(self.parse_expr()?)
            };
            Ok(self.new_expr(start_span, ExprKind::Lambda { parameters, body }))
        } else {
            Ok(expr)
        }
    }

    fn parse_binary_expr(&mut self, min_level: u8) -> Result<ast::Expr, Box<ParserError>> {
        let start_span = self.current().span;
        let mut lhs = self.parse_unary_expr()?;

        while let Some((op, level)) = self.current_tag().precedence_level()
            && level >= min_level
        {
            self.advance();
            let rhs = self.parse_binary_expr(level + 1)?;
            if matches!(op, BinaryOp::InclusiveRange | BinaryOp::ExclusiveRange)
                && matches!(
                    rhs.kind,
                    ExprKind::Binary {
                        op: BinaryOp::InclusiveRange | BinaryOp::ExclusiveRange,
                        ..
                    }
                )
            {
                return Err(self.error(ParserErrorKind::ChainingRanges));
            }
            lhs = self.new_expr(
                start_span,
                ExprKind::Binary {
                    lhs: Box::new(lhs),
                    op,
                    rhs: Box::new(rhs),
                },
            );
        }

        Ok(lhs)
    }

    fn parse_block(&mut self) -> Result<ast::Block, Box<ParserError>> {
        let mut statements = vec![];

        if !self.check(TokenTag::NewLine) {
            let stmt = self.parse_statement()?;
            statements.push(stmt);
            return Ok(ast::Block { statements });
        }

        self.consume_newlines();
        while !self.check(TokenTag::End) {
            let stmt = self.parse_statement()?;
            statements.push(stmt);
            self.expect_many(vec![TokenTag::NewLine])?;
            self.consume_newlines();
        }

        self.advance();

        Ok(ast::Block { statements })
    }

    fn parse_unary_expr(&mut self) -> Result<ast::Expr, Box<ParserError>> {
        let start_span = self.current().span;
        if self.check_and_advance(TokenTag::Bang) {
            let expr = self.parse_unary_expr()?;
            return Ok(self.new_expr(
                start_span,
                ExprKind::Unary {
                    op: ast::UnaryOp::Not,
                    operand: Box::new(expr),
                },
            ));
        }

        if self.check_and_advance(TokenTag::Minus) {
            let expr = self.parse_unary_expr()?;
            return Ok(self.new_expr(
                start_span,
                ExprKind::Unary {
                    op: ast::UnaryOp::Negate,
                    operand: Box::new(expr),
                },
            ));
        }

        self.parse_postfix_expr()
    }

    fn parse_postfix_expr(&mut self) -> Result<ast::Expr, Box<ParserError>> {
        let start_span = self.current().span;
        let mut expr = self.parse_primary_expr()?;

        loop {
            match self.current_tag() {
                TokenTag::LBrace => {
                    self.expect(TokenTag::LBrace)?;
                    let index = Box::new(self.parse_expr()?);
                    self.expect(TokenTag::RBrace)?;

                    expr = self.new_expr(
                        start_span,
                        ExprKind::Index {
                            target: Box::new(expr),
                            index,
                        },
                    );

                    continue;
                }
                TokenTag::LParen => {
                    let args = self.parse_call_args()?;

                    expr = self.new_expr(
                        start_span,
                        ExprKind::Call {
                            callee: Box::new(expr),
                            args,
                        },
                    );

                    continue;
                }
                TokenTag::Dot => {
                    self.advance();

                    let method_name = self.expect_ident()?;
                    let end_span = self.current().span;

                    if self.check(TokenTag::LParen) {
                        let mut args = self.parse_call_args()?;

                        if let ExprKind::Tuple(mut t) = expr.kind {
                            while let Some(e) = t.pop() {
                                args.insert(
                                    0,
                                    ExprArgument {
                                        name: None,
                                        span: e.span,
                                        value: e,
                                    },
                                );
                            }
                        } else {
                            args.insert(
                                0,
                                ExprArgument {
                                    name: None,
                                    span: expr.span,
                                    value: expr,
                                },
                            );
                        }

                        expr = self.new_expr(
                            start_span,
                            ExprKind::Call {
                                callee: Box::new(Expr {
                                    kind: ExprKind::Ident(method_name),
                                    span: Span {
                                        start: start_span.start,
                                        end: end_span.end,
                                    },
                                }),
                                args,
                            },
                        );
                    } else {
                        expr = self.new_expr(
                            start_span,
                            ExprKind::Field {
                                target: Box::new(expr),
                                name: method_name,
                            },
                        );
                    }

                    continue;
                }
                _ => break,
            }
        }

        Ok(expr)
    }

    fn parse_call_args(&mut self) -> Result<Vec<ExprArgument>, Box<ParserError>> {
        self.expect(TokenTag::LParen)?;
        let mut args = vec![];

        if self.check_and_advance(TokenTag::RParen) {
            return Ok(args);
        }

        self.check_and_advance(TokenTag::NewLine);

        loop {
            let start = self.current().span;

            let name = if self
                .tokens
                .get(self.index + 1)
                .is_some_and(|token| token.kind.tag() == TokenTag::Equals)
            {
                let name_span = self.current().span;
                let ident = self.current().origin.to_string();
                self.advance();
                self.expect(TokenTag::Equals)?;
                Some(ast::ArgumentName {
                    value: ident,
                    span: name_span,
                })
            } else {
                None
            };

            let value = self.parse_expr()?;
            let span = Span {
                start: start.start,
                end: self.current().span.end,
            };

            let arg = ExprArgument { name, value, span };
            args.push(arg);

            if self.check_and_advance(TokenTag::Comma) {
                // here it cannot be collapsed because its not lazy i guess (it had some problems
                // once i did it)
                #[allow(clippy::collapsible_if)]
                if self.check_and_advance(TokenTag::NewLine) {
                    if self.check(TokenTag::RParen) {
                        break;
                    }
                }
            } else {
                break;
            }
        }
        self.expect(TokenTag::RParen)?;
        Ok(args)
    }

    fn parse_primary_expr(&mut self) -> Result<ast::Expr, Box<ParserError>> {
        let start_span = self.current().span;
        match self.current().kind.clone() {
            TokenKind::Int(n) => {
                self.advance();
                Ok(self.new_expr(start_span, ExprKind::Int(n)))
            }
            TokenKind::Float(n) => {
                self.advance();
                Ok(self.new_expr(start_span, ExprKind::Float(n)))
            }
            TokenKind::Bool(b) => {
                self.advance();
                Ok(self.new_expr(start_span, ExprKind::Bool(b)))
            }
            TokenKind::String(s) => {
                self.advance();
                Ok(self.new_expr(start_span, ExprKind::String(s.to_string())))
            }
            TokenKind::Ident(i) => {
                self.advance();
                let mut segments = vec![i.to_string()];
                while self.check_and_advance(TokenTag::ColonColon) {
                    segments.push(self.expect_ident()?);
                }
                let kind = if segments.len() == 1 {
                    ExprKind::Ident(segments.pop().expect("identifier path has one segment"))
                } else {
                    ExprKind::Path(segments)
                };
                Ok(self.new_expr(start_span, kind))
            }
            TokenKind::If => self.parse_if_expr(),
            TokenKind::Match => self.parse_match_expr(),
            TokenKind::While => self.parse_while(),
            TokenKind::For => self.parse_for(),
            TokenKind::Do => {
                self.expect(TokenTag::Do)?;
                let block = self.parse_block()?;
                Ok(self.new_expr(start_span, ExprKind::Block(block)))
            }
            TokenKind::LParen => {
                self.advance();
                let mut exprs = vec![];
                if self.check_and_advance(TokenTag::RParen) {
                    return Ok(self.new_expr(start_span, ExprKind::Unit));
                }
                let mut is_tuple = self.check(TokenTag::LParen);
                exprs.push(self.parse_expr()?);
                while self.check_and_advance(TokenTag::Comma) {
                    is_tuple = true;
                    exprs.push(self.parse_expr()?);
                }
                self.expect(TokenTag::RParen)?;
                if is_tuple {
                    Ok(self.new_expr(start_span, ExprKind::Tuple(exprs)))
                } else {
                    Ok(exprs[0].clone())
                }
            }
            TokenKind::LBrace => {
                self.expect(TokenTag::LBrace)?;
                let mut exprs = vec![];
                if self.check_and_advance(TokenTag::RBrace) {
                    return Ok(self.new_expr(start_span, ExprKind::Array(exprs)));
                }
                exprs.push(self.parse_expr()?);
                while self.check_and_advance(TokenTag::Comma) {
                    exprs.push(self.parse_expr()?);
                }
                self.expect(TokenTag::RBrace)?;
                Ok(self.new_expr(start_span, ExprKind::Array(exprs)))
            }
            other => Err(self.error(ParserErrorKind::ExpectedExpression {
                found: other.tag().to_string(),
            })),
        }
    }

    fn parse_option_type(&mut self) -> Result<ast::TypeAnnotation, Box<ParserError>> {
        if self.check(TokenTag::Equals) {
            Ok(ast::TypeAnnotation::Inferred)
        } else {
            self.parse_required_type()
                .map(ast::TypeAnnotation::Explicit)
        }
    }

    fn parse_required_type(&mut self) -> Result<TypeExpr, Box<ParserError>> {
        match self.current_tag() {
            TokenTag::Ident => {
                let mut path = vec![self.expect_ident()?];
                while self.check_and_advance(TokenTag::ColonColon) {
                    path.push(self.expect_ident()?);
                }
                if self.check_and_advance(TokenTag::LAngle) {
                    let mut arguments = vec![self.parse_required_type()?];
                    while self.check_and_advance(TokenTag::Comma) {
                        arguments.push(self.parse_required_type()?);
                    }
                    self.expect(TokenTag::RAngle)?;
                    Ok(TypeExpr::Apply {
                        constructor: path,
                        arguments,
                    })
                } else {
                    Ok(TypeExpr::Path(path))
                }
            }
            TokenTag::LParen => {
                self.expect(TokenTag::LParen)?;
                if self.check_and_advance(TokenTag::RParen) {
                    return Ok(TypeExpr::Unit);
                }
                let mut items = vec![self.parse_required_type()?];
                while self.check_and_advance(TokenTag::Comma) {
                    items.push(self.parse_required_type()?);
                }
                self.expect(TokenTag::RParen)?;
                if self.check_and_advance(TokenTag::RArrow) {
                    Ok(TypeExpr::Function {
                        parameters: items,
                        return_type: Box::new(self.parse_required_type()?),
                    })
                } else if items.len() == 1 {
                    Ok(items.remove(0))
                } else {
                    Ok(TypeExpr::Tuple(items))
                }
            }
            other => Err(self.error(ParserErrorKind::ExpectedTokens {
                expected: vec!["type".to_string()],
                found: other.to_string(),
            })),
        }
    }

    fn parse_match_expr(&mut self) -> Result<ast::Expr, Box<ParserError>> {
        let start = self.current().span;
        self.expect(TokenTag::Match)?;
        let value = Box::new(self.parse_expr()?);
        self.expect(TokenTag::In)?;
        self.expect(TokenTag::NewLine)?;
        self.consume_newlines();

        let mut arms = Vec::new();
        while !self.check(TokenTag::End) {
            let arm_start = self.current().span;
            let pattern = self.parse_pattern()?;
            self.expect(TokenTag::FatArrow)?;
            let body = self.parse_block()?;
            let arm_end = body.span().end;
            arms.push(ast::MatchArm {
                pattern,
                body,
                span: Span {
                    start: arm_start.start,
                    end: arm_end,
                },
            });
            self.expect(TokenTag::NewLine)?;
            self.consume_newlines();
        }
        self.expect(TokenTag::End)?;
        Ok(self.new_expr(start, ExprKind::Match { value, arms }))
    }

    fn parse_pattern(&mut self) -> Result<ast::Pattern, Box<ParserError>> {
        let span = self.current().span;
        if self.check(TokenTag::Ident) && self.current().origin == "_" {
            self.advance();
            return Ok(ast::Pattern::Wildcard { span });
        }

        let (qualifier, variant) = if self.check_and_advance(TokenTag::Dot) {
            (None, self.expect_ident()?)
        } else {
            let mut path = vec![self.expect_ident()?];
            while self.check_and_advance(TokenTag::ColonColon) {
                path.push(self.expect_ident()?);
            }
            let variant = path.pop().expect("a pattern path has a final segment");
            (Some(path), variant)
        };

        let binding = if self.check_and_advance(TokenTag::LParen) {
            let binding = self.expect_ident()?;
            self.expect(TokenTag::RParen)?;
            Some(binding)
        } else {
            None
        };
        Ok(ast::Pattern::EnumVariant {
            qualifier,
            variant,
            binding,
            span,
        })
    }

    fn parse_if_expr(&mut self) -> Result<ast::Expr, Box<ParserError>> {
        let start_span = self.current().span;
        self.expect(TokenTag::If)?;
        let condition = Box::new(self.parse_expr()?);
        let then_branch = Box::new(self.parse_expr()?);
        let else_branch = if self.check_and_advance(TokenTag::Else) {
            Some(Box::new(self.parse_expr()?))
        } else {
            None
        };
        Ok(self.new_expr(
            start_span,
            ExprKind::If {
                condition,
                then_branch,
                else_branch,
            },
        ))
    }

    fn new_expr(&self, start_span: Span, kind: ExprKind) -> Expr {
        Expr {
            kind,
            span: Span {
                start: start_span.start,
                end: self.current().span.end,
            },
        }
    }

    fn current(&self) -> &Token<'a> {
        &self.tokens[self.index]
    }

    fn current_tag(&self) -> TokenTag {
        self.current().kind.tag()
    }

    fn check(&self, tag: TokenTag) -> bool {
        self.current_tag() == tag
    }

    fn check_and_advance(&mut self, tag: TokenTag) -> bool {
        if self.check(tag) {
            self.advance();
            true
        } else {
            false
        }
    }

    fn is_at_end(&self) -> bool {
        self.check(TokenTag::Eof)
    }

    fn advance(&mut self) {
        if !self.is_at_end() {
            self.index += 1
        }
    }

    fn _previous(&self) -> Option<&Token<'a>> {
        if self.index > 0 {
            Some(&self.tokens[self.index - 1])
        } else {
            None
        }
    }

    fn consume_newlines(&mut self) {
        while self.check(TokenTag::NewLine) {
            self.advance();
        }
    }

    fn error(&self, kind: ParserErrorKind) -> Box<ParserError> {
        Box::new(ParserError {
            kind,
            file_path: self.file_path.clone(),
            pos: self.current().span,
        })
    }

    fn expect(&mut self, tag: TokenTag) -> Result<(), Box<ParserError>> {
        if self.current_tag() == tag {
            self.advance();
            Ok(())
        } else {
            Err(self.error(ParserErrorKind::ExpectedToken {
                expected: tag.to_string(),
                found: self.current().origin.to_string(),
            }))
        }
    }
    fn expect_many(&mut self, tags: Vec<TokenTag>) -> Result<(), Box<ParserError>> {
        let mut any = false;
        for tag in &tags {
            if *tag == self.current_tag() {
                any = true
            }
        }
        if any {
            self.advance();
            Ok(())
        } else {
            Err(self.error(ParserErrorKind::ExpectedTokens {
                expected: tags.iter().map(|t| t.to_string()).collect(),
                found: self.current().origin.to_string(),
            }))
        }
    }
    fn expect_ident(&mut self) -> Result<String, Box<ParserError>> {
        match self.current().kind.clone() {
            TokenKind::Ident(s) => {
                self.advance();
                Ok(s.to_string())
            }
            other => Err(self.error(ParserErrorKind::ExpectedIdentifier {
                found: format!("{other:?}"),
            })),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::lexer::Lexer;
    use std::{
        ffi::OsStr,
        fs::{read_dir, read_to_string},
    };

    use crate::{dump::create_expected_by_ext, parser};

    #[test]
    fn validate_expected_ast() {
        let cargo_dir = env!("CARGO_MANIFEST_DIR");
        let entries = read_dir(format!("{cargo_dir}/fixtures")).unwrap();

        for entry in entries {
            let current_file_path = entry.unwrap().path();

            if current_file_path.is_file()
                && current_file_path.extension() == Some(OsStr::new("fob"))
            {
                let content = read_to_string(&current_file_path).unwrap();
                let tokens = Lexer::new(&current_file_path, &content).tokenize();

                let ast_str = match tokens {
                    Ok(tokens) => {
                        let ast = parser::Parser::new(tokens, &current_file_path).parse_program();

                        match ast {
                            Ok(program) => {
                                crate::dump::normalize_snapshot_paths(&format!("{program:#?}"))
                            }
                            Err(e) => crate::dump::normalize_snapshot_paths(&format!("{e:#?}")),
                        }
                    }

                    Err(e) => crate::dump::normalize_snapshot_paths(&format!("{e:#?}")),
                };

                let ast_expected_path = create_expected_by_ext(&current_file_path, ".ast").unwrap();

                let expected_ast = match read_to_string(ast_expected_path.clone()) {
                    Ok(s) => crate::dump::normalize_snapshot_paths(&s),
                    Err(_) => {
                        eprintln!("Expected ast file {ast_expected_path:?} not found. Skipping it");
                        continue;
                    }
                };

                for (a, e) in ast_str
                    .trim_end()
                    .lines()
                    .zip(expected_ast.trim_end().lines())
                {
                    assert_eq!(
                        snapshot_line(a),
                        snapshot_line(e),
                        "failed to match ast output in file {ast_expected_path:?}"
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
