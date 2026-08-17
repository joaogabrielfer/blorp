use std::{fs::read_to_string, path::PathBuf, process::exit};

use anyhow::Result;
use clap::{Parser, Subcommand};

use fobos::{
    ast::Item,
    dump::dump_expected,
    interpreter::Interpreter,
    lexer::Lexer,
    module::{CompilerSession, RuntimeModules},
    parser::Parser as FobosParser,
    repl,
};

#[derive(Parser)]
#[command(
    name = "fobos",
    version,
    about = "The Fobos language interpreter",
    long_about = "Run Fobos source files or start an interactive Fobos session."
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Start an interactive Fobos session.
    Repl {
        /// Evaluate without static type checking.
        #[arg(long = "no-check", visible_alias = "disable-checker")]
        no_check: bool,
    },
    /// Run a Fobos source file.
    Run {
        /// The .fob source file to execute.
        #[arg(value_name = "FILE")]
        path: PathBuf,
        /// Evaluate one parsed file directly, without the type or module pipeline.
        #[arg(long = "no-check", visible_alias = "disable-checker")]
        no_check: bool,
    },
    /// Development-only inspection and fixture commands.
    Debug {
        #[command(subcommand)]
        command: DebugCommands,
    },
}

#[derive(Subcommand)]
enum DebugCommands {
    /// Print the tokens produced for a source file.
    Tokens {
        /// The .fob source file to inspect.
        #[arg(value_name = "FILE")]
        path: PathBuf,
        /// Print only token kinds.
        #[arg(short = 'k', long = "kinds")]
        only_kinds: bool,
    },
    /// Print the parsed abstract syntax tree for a source file.
    Ast {
        /// The .fob source file to inspect.
        #[arg(value_name = "FILE")]
        path: PathBuf,
    },
    /// Regenerate expected fixture output. Intended for maintainers only.
    GenerateExpected,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("{error:#}");
        exit(1);
    }
}

fn run() -> Result<()> {
    match Cli::parse().command {
        None => repl::run(true),
        Some(Commands::Repl { no_check }) => repl::run(!no_check),
        Some(Commands::Run { path, no_check }) => run_file(&path, no_check),
        Some(Commands::Debug {
            command: DebugCommands::Tokens { path, only_kinds },
        }) => print_tokens(&path, only_kinds),
        Some(Commands::Debug {
            command: DebugCommands::Ast { path },
        }) => print_ast(&path),
        Some(Commands::Debug {
            command: DebugCommands::GenerateExpected,
        }) => dump_expected(),
    }
}

fn run_file(path: &PathBuf, no_check: bool) -> Result<()> {
    let stdout = std::io::stdout();
    if no_check {
        let content = read_to_string(path)?;
        let tokens = Lexer::new(path, &content).tokenize()?;
        let program = FobosParser::new(tokens, path).parse_program()?;
        if program
            .items
            .iter()
            .any(|item| matches!(item, Item::Import(_) | Item::Const(_) | Item::Type(_)))
        {
            anyhow::bail!(
                "--no-check only supports a function-only entry file; imports, types, and compile-time constants require the module compiler"
            );
        }
        Interpreter::new(path, stdout.lock()).eval_program(program)?;
    } else {
        let compilation = CompilerSession::default().compile_file(path)?;
        RuntimeModules::new(compilation)
            .execute_root(&mut Interpreter::new(path, stdout.lock()))?;
    }
    Ok(())
}

fn print_tokens(path: &PathBuf, only_kinds: bool) -> Result<()> {
    let content = read_to_string(path)?;
    let tokens = Lexer::new(path, &content).tokenize()?;
    if only_kinds {
        for token in tokens {
            println!("{:?}", token.kind);
        }
    } else {
        println!("{tokens:#?}");
    }
    Ok(())
}

fn print_ast(path: &PathBuf) -> Result<()> {
    let content = read_to_string(path)?;
    let tokens = Lexer::new(path, &content).tokenize()?;
    let ast = FobosParser::new(tokens, path).parse_program()?;
    println!("{ast:#?}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::{Cli, Commands, DebugCommands};

    #[test]
    fn cli_accepts_the_new_public_contract_and_legacy_checker_flag() {
        assert!(matches!(
            Cli::try_parse_from(["fobos", "repl", "--no-check"])
                .unwrap()
                .command,
            Some(Commands::Repl { no_check: true })
        ));
        assert!(matches!(
            Cli::try_parse_from(["fobos", "run", "--disable-checker", "example.fob"])
                .unwrap()
                .command,
            Some(Commands::Run { no_check: true, .. })
        ));
        assert!(matches!(
            Cli::try_parse_from(["fobos", "debug", "tokens", "example.fob"])
                .unwrap()
                .command,
            Some(Commands::Debug {
                command: DebugCommands::Tokens { .. }
            })
        ));
    }
}
