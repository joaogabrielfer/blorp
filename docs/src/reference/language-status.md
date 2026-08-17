# Language status

This page is the compact source of truth for what users can rely on. Update it
when a feature changes state, and link to the relevant implementation or
proposal.

| Area | Status | Notes |
| --- | --- | --- |
| Lexer and parser | Implemented | Modules contain imports, types, typed constants, and functions; expressions live in function bodies |
| Interpreter | Implemented for core | Declarative modules install constants and functions, then run root `main` |
| Type checker | Implemented for core | Imports and qualified paths are checked through module interfaces; constants are evaluated at compile time |
| REPL | Implemented for core | Persistent typed session with multiline entries; imports require file execution |
| Built-ins | Small core | `echo`, `range`, and `push` |
| Modules | Implemented initial version | Relative and `std::` imports, aliases, groups, globs, visibility, caching, and cycle rejection |
| Standard library | Minimal | File-backed `std::` resolution exists; the bundled module set is intentionally small |
| Named arguments | Implemented | Required named parameters remain planned |
| Overloads | Implemented | Resolution uses parameter names and compatible types |
| Structs and enums | Implemented initial version | Nominal module-aware structs, enums, constructors, inline enum payload structs, field reads, and exhaustive enum matching |
| Interfaces and generics | Proposal | No runtime representation yet |
| Compile-time constants | Implemented initial version | Explicitly typed `const` values support a restricted constant-expression subset; macros and type-level use remain planned |
| Macros | Proposal | Design notes only |
| Effects, streams, resource management | Proposal | See the design pages |

Code examples in the language and reference sections should be runnable unless
they are explicitly marked as a proposal.
