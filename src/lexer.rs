use crate::{
    ast::BinaryOp,
    errors::{LexerError, LexerErrorKind},
    source::{Span, SrcPos},
};
use std::{iter::Peekable, path::PathBuf, str::Chars};

#[derive(Debug, Clone)]
pub struct Lexer<'a> {
    file_path: &'a PathBuf,
    raw_src: &'a str,
    source: Peekable<Chars<'a>>,
    pos: SrcPos,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Token<'a> {
    pub kind: TokenKind<'a>,
    pub origin: &'a str,
    pub span: Span,
}

impl<'a> Token<'a> {
    pub fn new(kind: TokenKind<'a>) -> Self {
        Self {
            kind,
            origin: "",
            span: Span::dummy(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum TokenKind<'a> {
    LParen, // (
    RParen, // )
    LBrace, // [
    RBrace, // ]
    LAngle, // <
    RAngle, // >
    LCurly, // {
    RCurly, // }

    Colon,      // :
    Comma,      // ,
    Dot,        // .
    RArrow,     // ->
    FatArrow,   // =>
    ColonColon, // ::

    Equals,  // =
    Plus,    // +
    Minus,   // -
    Slash,   // /
    Star,    // *
    Bang,    // !
    Combine, // <>

    BangEquals,    // !=
    EqualsEquals,  // ==
    GreaterEquals, // <=
    LessEquals,    // >=

    DotDotEqual, // ..=
    DotDotLess,  // ..<

    Ident(&'a str),
    String(&'a str),
    Int(i64),
    Float(f64),
    Bool(bool),

    Fun,
    Return,
    Yield,
    End,
    Var,
    Let,
    Const,
    For,
    While,
    In,
    Do,
    Match,
    If,
    Else,
    Import,
    As,
    Pub,
    Type,
    Struct,
    Enum,

    NewLine,
    Eof,
}

impl<'a> TokenKind<'a> {
    pub fn tag(&self) -> TokenTag {
        match self {
            TokenKind::LParen => TokenTag::LParen,
            TokenKind::RParen => TokenTag::RParen,
            TokenKind::LBrace => TokenTag::LBrace,
            TokenKind::RBrace => TokenTag::RBrace,
            TokenKind::LAngle => TokenTag::LAngle,
            TokenKind::RAngle => TokenTag::RAngle,
            TokenKind::LCurly => TokenTag::LCurly,
            TokenKind::RCurly => TokenTag::RCurly,

            TokenKind::Colon => TokenTag::Colon,
            TokenKind::Comma => TokenTag::Comma,
            TokenKind::Dot => TokenTag::Dot,
            TokenKind::RArrow => TokenTag::RArrow,
            TokenKind::FatArrow => TokenTag::FatArrow,
            TokenKind::ColonColon => TokenTag::ColonColon,

            TokenKind::Equals => TokenTag::Equals,
            TokenKind::Plus => TokenTag::Plus,
            TokenKind::Minus => TokenTag::Minus,
            TokenKind::Slash => TokenTag::Slash,
            TokenKind::Star => TokenTag::Star,
            TokenKind::Bang => TokenTag::Bang,
            TokenKind::Combine => TokenTag::Combine,

            TokenKind::BangEquals => TokenTag::BangEquals,
            TokenKind::EqualsEquals => TokenTag::EqualsEquals,
            TokenKind::GreaterEquals => TokenTag::GreaterEquals,
            TokenKind::LessEquals => TokenTag::LessEquals,

            TokenKind::DotDotEqual => TokenTag::DotDotEqual,
            TokenKind::DotDotLess => TokenTag::DotDotLess,

            TokenKind::Ident(_) => TokenTag::Ident,
            TokenKind::String(_) => TokenTag::String,
            TokenKind::Int(_) => TokenTag::Int,
            TokenKind::Float(_) => TokenTag::Float,
            TokenKind::Bool(_) => TokenTag::Bool,

            TokenKind::Fun => TokenTag::Fun,
            TokenKind::Return => TokenTag::Return,
            TokenKind::Yield => TokenTag::Yield,
            TokenKind::End => TokenTag::End,
            TokenKind::Var => TokenTag::Var,
            TokenKind::Let => TokenTag::Let,
            TokenKind::Const => TokenTag::Const,
            TokenKind::For => TokenTag::For,
            TokenKind::While => TokenTag::While,
            TokenKind::In => TokenTag::In,
            TokenKind::Do => TokenTag::Do,
            TokenKind::Match => TokenTag::Match,
            TokenKind::If => TokenTag::If,
            TokenKind::Else => TokenTag::Else,
            TokenKind::Import => TokenTag::Import,
            TokenKind::As => TokenTag::As,
            TokenKind::Pub => TokenTag::Pub,
            TokenKind::Type => TokenTag::Type,
            TokenKind::Struct => TokenTag::Struct,
            TokenKind::Enum => TokenTag::Enum,

            TokenKind::NewLine => TokenTag::NewLine,
            TokenKind::Eof => TokenTag::Eof,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenTag {
    LParen,
    RParen,
    LBrace,
    RBrace,
    LAngle,
    RAngle,
    LCurly,
    RCurly,

    Colon,
    Comma,
    Dot,
    RArrow,
    FatArrow,
    ColonColon,

    Equals,
    Plus,
    Minus,
    Slash,
    Star,
    Bang,
    Combine,

    BangEquals,
    EqualsEquals,
    GreaterEquals,
    LessEquals,

    DotDotEqual,
    DotDotLess,

    Ident,
    String,
    Int,
    Float,
    Bool,

    Fun,
    Return,
    Yield,
    End,
    Var,
    Let,
    Const,
    For,
    While,
    In,
    Do,
    Match,
    If,
    Else,
    Import,
    As,
    Pub,
    Type,
    Struct,
    Enum,

    NewLine,
    Eof,
}

impl TokenTag {
    pub fn precedence_level(&self) -> Option<(BinaryOp, u8)> {
        match self {
            TokenTag::DotDotEqual => Some((BinaryOp::InclusiveRange, 1)),
            TokenTag::DotDotLess => Some((BinaryOp::ExclusiveRange, 1)),
            TokenTag::EqualsEquals => Some((BinaryOp::Eq, 2)),
            TokenTag::BangEquals => Some((BinaryOp::NotEq, 2)),
            TokenTag::RAngle => Some((BinaryOp::Greater, 3)),
            TokenTag::GreaterEquals => Some((BinaryOp::GreaterEq, 3)),
            TokenTag::LAngle => Some((BinaryOp::Less, 3)),
            TokenTag::LessEquals => Some((BinaryOp::LessEq, 3)),
            TokenTag::Combine => Some((BinaryOp::Combine, 4)),
            TokenTag::Plus => Some((BinaryOp::Add, 4)),
            TokenTag::Minus => Some((BinaryOp::Sub, 4)),
            TokenTag::Star => Some((BinaryOp::Mul, 5)),
            TokenTag::Slash => Some((BinaryOp::Div, 5)),
            _ => None,
        }
    }
}

impl std::fmt::Display for TokenTag {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let str = match self {
            TokenTag::LParen => "(",
            TokenTag::RParen => ")",
            TokenTag::LBrace => "[",
            TokenTag::RBrace => "]",
            TokenTag::LAngle => "<",
            TokenTag::RAngle => ">",
            TokenTag::LCurly => "{",
            TokenTag::RCurly => "}",

            TokenTag::Colon => ":",
            TokenTag::Comma => ",",
            TokenTag::Dot => ".",
            TokenTag::RArrow => "->",
            TokenTag::FatArrow => "=>",
            TokenTag::ColonColon => "::",

            TokenTag::Equals => "=",
            TokenTag::Plus => "+",
            TokenTag::Minus => "-",
            TokenTag::Slash => "/",
            TokenTag::Star => "*",
            TokenTag::Bang => "!",
            TokenTag::Combine => "<>",

            TokenTag::BangEquals => "!=",
            TokenTag::EqualsEquals => "==",
            TokenTag::GreaterEquals => ">=",
            TokenTag::LessEquals => "<=",

            TokenTag::DotDotEqual => "..=",
            TokenTag::DotDotLess => "..<",

            TokenTag::Ident => "identifier",
            TokenTag::String => "string",
            TokenTag::Int => "integer",
            TokenTag::Float => "float",
            TokenTag::Bool => "boolean",

            TokenTag::Fun => "fun",
            TokenTag::Return => "return",
            TokenTag::Yield => "yield",
            TokenTag::End => "end",
            TokenTag::Var => "var",
            TokenTag::Let => "let",
            TokenTag::Const => "const",
            TokenTag::For => "for",
            TokenTag::While => "while",
            TokenTag::In => "in",
            TokenTag::Do => "do",
            TokenTag::Match => "match",
            TokenTag::If => "if",
            TokenTag::Else => "else",
            TokenTag::Import => "import",
            TokenTag::As => "as",
            TokenTag::Pub => "pub",
            TokenTag::Type => "type",
            TokenTag::Struct => "struct",
            TokenTag::Enum => "enum",

            TokenTag::NewLine => "newline",
            TokenTag::Eof => "EOF",
        };
        write!(f, "{str}")
    }
}

impl<'a> Lexer<'a> {
    pub fn new(file_path: &'a PathBuf, source: &'a str) -> Self {
        Self {
            file_path,
            raw_src: source,
            source: source.chars().peekable(),
            pos: SrcPos {
                line: 1,
                col: 1,
                idx: 0,
            },
        }
    }

    fn advance(&mut self) -> Option<char> {
        let result = self.source.next();
        match result {
            Some('\n') => {
                self.pos.line += 1;
                self.pos.col = 1;
                self.pos.idx += '\n'.len_utf8();
                result
            }
            Some(ch) => {
                self.pos.col += 1;
                self.pos.idx += ch.len_utf8();
                result
            }
            None => None,
        }
    }

    fn skip_whitespace(&mut self) {
        while let Some(c) = self.source.peek() {
            if c.is_whitespace() && *c != '\n' {
                self.pos.col += c.len_utf8();
                self.pos.idx += c.len_utf8();
                self.source.next();
            } else {
                break;
            }
        }
    }

    fn next_token(&mut self) -> Result<Token<'a>, LexerError> {
        self.skip_whitespace();
        let start_idx = self.pos.idx;
        // println!("current = {:?}", self.source.peek());
        match self.advance() {
            Some('(') => Ok(self.new_token(TokenKind::LParen, "(")),
            Some(')') => Ok(self.new_token(TokenKind::RParen, ")")),
            Some('{') => Ok(self.new_token(TokenKind::LCurly, "{")),
            Some('}') => Ok(self.new_token(TokenKind::RCurly, "}")),
            Some('[') => Ok(self.new_token(TokenKind::LBrace, "[")),
            Some(']') => Ok(self.new_token(TokenKind::RBrace, "]")),
            Some(',') => Ok(self.new_token(TokenKind::Comma, ",")),
            Some(':') => match self.source.peek().copied() {
                Some(':') => {
                    self.advance();
                    Ok(self.new_token(TokenKind::ColonColon, "::"))
                }
                _ => Ok(self.new_token(TokenKind::Colon, ":")),
            },
            Some('.') => match self.source.peek() {
                Some('.') => {
                    self.advance();
                    match self.source.peek().copied() {
                        Some('<') => {
                            self.advance();
                            Ok(self.new_token(TokenKind::DotDotLess, "..<"))
                        }
                        Some('=') => {
                            self.advance();
                            Ok(self.new_token(TokenKind::DotDotEqual, "..="))
                        }
                        Some(other) => Err(self.error(LexerErrorKind::ExpectedChars {
                            expected: vec!["=".to_string(), "<".to_string()],
                            found: other.to_string(),
                        })),
                        None => Err(self.error(LexerErrorKind::ExpectedChars {
                            expected: vec!["=".to_string(), "<".to_string()],
                            found: "EOF".to_string(),
                        })),
                    }
                }
                _ => Ok(self.new_token(TokenKind::Dot, ".")),
            },
            Some('+') => Ok(self.new_token(TokenKind::Plus, "+")),
            Some('*') => Ok(self.new_token(TokenKind::Star, "*")),
            Some('\n') => Ok(self.new_token(TokenKind::NewLine, "\n")),
            Some('>') => match self.source.peek() {
                Some('=') => {
                    self.advance();
                    Ok(self.new_token(TokenKind::GreaterEquals, ">="))
                }
                _ => Ok(self.new_token(TokenKind::RAngle, ">")),
            },
            Some('<') => match self.source.peek() {
                Some('=') => {
                    self.advance();
                    Ok(self.new_token(TokenKind::LessEquals, "<="))
                }
                Some('>') => {
                    self.advance();
                    Ok(self.new_token(TokenKind::Combine, "<>"))
                }
                _ => Ok(self.new_token(TokenKind::LAngle, "<")),
            },
            Some('-') => match self.source.peek() {
                Some('>') => {
                    self.advance();
                    Ok(self.new_token(TokenKind::RArrow, "->"))
                }
                _ => Ok(self.new_token(TokenKind::Minus, "-")),
            },
            Some('/') => match self.source.peek() {
                Some('/') => {
                    while let Some(c) = self.advance()
                        && c != '\n'
                    {}
                    Ok(self.new_token(TokenKind::NewLine, "\\n"))
                }
                _ => Ok(self.new_token(TokenKind::Slash, "/")),
            },
            Some('=') => match self.source.peek() {
                Some('=') => {
                    self.advance();
                    Ok(self.new_token(TokenKind::EqualsEquals, "=="))
                }
                Some('>') => {
                    self.advance();
                    Ok(self.new_token(TokenKind::FatArrow, "=>"))
                }
                _ => Ok(self.new_token(TokenKind::Equals, "=")),
            },
            Some('!') => match self.source.peek() {
                Some('=') => {
                    self.advance();
                    Ok(self.new_token(TokenKind::BangEquals, "!="))
                }
                _ => Ok(self.new_token(TokenKind::Bang, "!")),
            },
            Some('a'..='z') | Some('A'..='Z') | Some('_') => {
                while let Some('a'..='z') | Some('A'..='Z') | Some('_') | Some('0'..='9') =
                    self.source.peek()
                {
                    self.advance();
                }

                let end_idx = self.pos.idx;

                let s = &self.raw_src[start_idx..end_idx];
                match s {
                    "true" => Ok(self.new_token(TokenKind::Bool(true), "true")),
                    "false" => Ok(self.new_token(TokenKind::Bool(false), "false")),
                    "fun" => Ok(self.new_token(TokenKind::Fun, "fun")),
                    "return" => Ok(self.new_token(TokenKind::Return, "return")),
                    "yield" => Ok(self.new_token(TokenKind::Yield, "yield")),
                    "end" => Ok(self.new_token(TokenKind::End, "end")),
                    "var" => Ok(self.new_token(TokenKind::Var, "var")),
                    "let" => Ok(self.new_token(TokenKind::Let, "let")),
                    "const" => Ok(self.new_token(TokenKind::Const, "const")),
                    "for" => Ok(self.new_token(TokenKind::For, "for")),
                    "while" => Ok(self.new_token(TokenKind::While, "while")),
                    "in" => Ok(self.new_token(TokenKind::In, "in")),
                    "do" => Ok(self.new_token(TokenKind::Do, "do")),
                    "match" => Ok(self.new_token(TokenKind::Match, "match")),
                    "if" => Ok(self.new_token(TokenKind::If, "if")),
                    "else" => Ok(self.new_token(TokenKind::Else, "else")),
                    "import" => Ok(self.new_token(TokenKind::Import, "import")),
                    "as" => Ok(self.new_token(TokenKind::As, "as")),
                    "pub" => Ok(self.new_token(TokenKind::Pub, "pub")),
                    "type" => Ok(self.new_token(TokenKind::Type, "type")),
                    "struct" => Ok(self.new_token(TokenKind::Struct, "struct")),
                    "enum" => Ok(self.new_token(TokenKind::Enum, "enum")),
                    _ => Ok(self.new_token(TokenKind::Ident(s), s)),
                }
            }
            Some('0'..='9') => {
                // Consume remaining integer digits.
                while matches!(self.source.peek().copied(), Some('0'..='9')) {
                    self.advance();
                }

                if matches!(self.source.peek().copied(), Some('.'))
                    && matches!(self.peek_next(), Some('0'..='9'))
                {
                    self.advance();

                    while matches!(self.source.peek().copied(), Some('0'..='9')) {
                        self.advance();
                    }
                }

                if let Some(c) = self.source.peek().copied()
                    && matches!(c, 'a'..='z' | 'A'..='Z' | '_')
                {
                    let end_idx = self.pos.idx + c.len_utf8();
                    let num_str = &self.raw_src[start_idx..end_idx];

                    return Err(self.error(LexerErrorKind::InvalidNumber(num_str.to_string())));
                }

                let end_idx = self.pos.idx;
                let num_str = &self.raw_src[start_idx..end_idx];

                self.parse_num(num_str)
            }
            Some('"') => {
                let string_start_pos = self.pos;
                while let Some(c) = self.source.peek() {
                    if *c == '"' {
                        break;
                    }

                    self.advance();
                }

                match self.source.peek() {
                    Some('"') => {
                        let end_idx = self.pos.idx;

                        self.advance(); // consume closing quote

                        let s = &self.raw_src[start_idx + 1..end_idx];
                        let origin = &self.raw_src[start_idx..end_idx + 1];
                        Ok(self.new_token(TokenKind::String(s), origin))
                    }
                    _ => Err(LexerError {
                        kind: LexerErrorKind::UnterminatedString,
                        file_path: self.file_path.clone(),
                        pos: Span {
                            start: string_start_pos,
                            end: self.pos,
                        },
                    }),
                }
            }
            Some(other) => Err(self.error(LexerErrorKind::UnknownChar(other))),
            None => Ok(self.new_token(TokenKind::Eof, "EOF")),
        }
    }

    fn parse_num(&mut self, num_str: &'a str) -> Result<Token<'a>, LexerError> {
        if num_str.contains(".") {
            match num_str.parse::<f64>() {
                Ok(num) => Ok(self.new_token(TokenKind::Float(num), num_str)),
                Err(_) => Err(self.error(LexerErrorKind::InvalidNumber(num_str.to_string()))),
            }
        } else {
            match num_str.parse::<i64>() {
                Ok(num) => Ok(self.new_token(TokenKind::Int(num), num_str)),
                Err(_) => Err(self.error(LexerErrorKind::InvalidNumber(num_str.to_string()))),
            }
        }
    }

    fn peek_next(&self) -> Option<char> {
        let mut source = self.source.clone();

        source.next();

        source.peek().copied()
    }

    fn error(&self, kind: LexerErrorKind) -> LexerError {
        LexerError {
            kind,
            file_path: self.file_path.clone(),
            pos: Span {
                start: self.pos,
                end: SrcPos {
                    line: self.pos.line,
                    col: self.pos.col + 1,
                    idx: self.pos.idx + 1,
                },
            },
        }
    }

    fn new_token(&self, kind: TokenKind<'a>, origin: &'a str) -> Token<'a> {
        let offset = match origin {
            "EOF" => 0,
            "\\n" => 0,
            _ if self.pos.col < 1 => 0,
            _ => origin.len(),
        };
        // eprintln!("offset = {offset}\nkind = {kind:?}");
        Token {
            origin,
            kind,
            span: Span::new(
                SrcPos {
                    line: self.pos.line,
                    col: self.pos.col - offset,
                    idx: self.pos.idx - offset,
                },
                SrcPos {
                    line: self.pos.line,
                    col: self.pos.col,
                    idx: self.pos.idx,
                },
            ),
        }
    }

    pub fn tokenize(&mut self) -> Result<Vec<Token<'a>>, LexerError> {
        let mut tokens = vec![];
        loop {
            let tk = self.next_token()?;
            if tk.kind == TokenKind::Eof {
                tokens.push(tk);
                break;
            }
            tokens.push(tk);
        }
        Ok(tokens)
    }
}

#[cfg(test)]
mod tests {
    use std::{
        ffi::OsStr,
        fs::{read_dir, read_to_string},
    };

    use crate::dump::create_expected_by_ext;

    use super::*;

    #[test]
    fn validate_expected_tokens() {
        let cargo_dir = env!("CARGO_MANIFEST_DIR");
        let entries = read_dir(format!("{cargo_dir}/fixtures")).unwrap();

        for entry in entries {
            let current_file_path = entry.unwrap().path();

            if current_file_path.is_file()
                && current_file_path.extension() == Some(OsStr::new("fob"))
            {
                let content = read_to_string(&current_file_path).unwrap();
                let tokens = Lexer::new(&current_file_path, &content).tokenize();
                let tokens_str = crate::dump::normalize_snapshot_paths(&format!("{tokens:#?}"));

                let token_expected_path =
                    create_expected_by_ext(&current_file_path, ".tokens").unwrap();
                let expected_tokens = match read_to_string(token_expected_path.clone()) {
                    Ok(s) => crate::dump::normalize_snapshot_paths(&s),
                    Err(_) => {
                        println!(
                            "Expected tokens file {token_expected_path:?} not found. Skipping it"
                        );
                        continue;
                    }
                };

                for (i, (my_line, expected_line)) in
                    tokens_str.lines().zip(expected_tokens.lines()).enumerate()
                {
                    assert_eq!(
                        snapshot_line(my_line),
                        snapshot_line(expected_line),
                        "failed to match at line '{i}' in file {token_expected_path:?}: "
                    )
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
