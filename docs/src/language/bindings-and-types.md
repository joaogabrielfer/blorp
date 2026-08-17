# Bindings and types

Inside functions, use `let` for an immutable binding and `var` for a mutable
binding. `:=` allows inference; an explicit annotation follows the name:

```fob
fun main(): () =
    let answer := 42
    var count: Int = 0
    let label: String = "items"
end
```

Bindings can be assigned only when mutable:

```fob
fun main(): () =
    var count: Int = 0
    count = count + 1
end
```

The implemented type vocabulary is `Int`, `Float`, `Bool`, `String`, `Unit`,
`Any`, `Range`, tuples such as `(Int, String)`, arrays such as `Arr<Int>`, and
function types such as `(Int, Int) -> Int`. Inferred annotations are represented
as `Any` while the type system is still evolving.

Tuple and array literals are available:

```fob
let pair := (10, "ten")
var values: Arr<Int> = [1, 2, 3]
echo(values[0])
```

## Top-level constants

Use `const` for a top-level, immutable module value. A constant always has an
explicit type and can be public:

```fob
pub const MAX_RETRIES: Int = 3
const LABEL: String = "retry"
const RETRIES: Arr<Int> = [1, MAX_RETRIES]
```

Constants are checked and evaluated while compiling the module. Their values
may use literals, other constants (including imported public constants),
tuples, arrays, indexing, unary and binary operations, and `if` expressions.
Calls, blocks, loops, lambdas, and mutable bindings are not constant
expressions, so `const LIMIT: Int = size()` is rejected.

Fobos does not yet have type-level values or fixed-size array types. Constants
already provide the compile-time value layer those features will consume, but
they cannot yet declare an array length in a type.

The type checker reports mismatched values, invalid calls, invalid assignments,
and invalid indexing with source locations. See [user-defined
types](user-defined-types.md) for structs, enums, and enum matching. Generic
types, interfaces, aliases, tags, and general pattern types remain design work;
see [types and compile-time features](../design/types-and-compile-time.md).
