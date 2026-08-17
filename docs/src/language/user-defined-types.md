# User-defined types

Fobos supports named nominal structs and enums in file modules. Type names are
declared at module scope and may be public with `pub type`.

## Structs

```fob
pub type Point: struct =
    x: Int
    y: Int
end
```

Struct constructors require named arguments. Their order does not matter:

```fob
let point := Point(y = 20, x = 10)
echo(point.x)
```

Fields are readable but cannot yet be assigned through a projection. Rebuild a
value instead of writing `point.x = 20`.

## Enums

An enum variant has no payload or exactly one type after `:`:

```fob
type Message: enum =
    Quit
    Text: String
    Position: Point
end

let quit := Message::Quit
let text := Message::Text("hello")
let position := Message::Position(Point(x = 10, y = 20))
```

An inline struct payload receives named constructor arguments. Its associated
type is available in type positions as `Message::Move`:

```fob
type Message: enum =
    Move: struct =
        point: Point
        relative: Bool
    end
end

let move := Message::Move(
    point = Point(x = 10, y = 20),
    relative = true,
)
```

## Matching enums

Enum matches are exhaustive unless a final `_` arm is present. A payload variant
binds its one payload value:

```fob
fun describe(message: Message): String =
    match message in
        .Quit =>
            return "quit"
        end

        .Text(text) =>
            return text
        end

        .Move(payload) =>
            if payload.relative do
                return "relative move"
            end

            return "absolute move"
        end
    end
end
```

General patterns, inline payload field destructuring, tags, custom
constructors, generics, and interfaces are not implemented yet.
