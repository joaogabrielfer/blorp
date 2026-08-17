# Types and compile-time features

The current checker supports primitive, tuple, array, range, function, `Any`,
and temporary type-variable representations. Typed top-level `const`
declarations are evaluated at compile time. They are intentionally a small,
value-only layer: calls and other runtime constructs are rejected, while
references to local or imported public constants are supported.

The following are implemented without generics or interfaces:

- named nominal structs with named-only constructors and read-only field projection;
- named nominal enums with unit variants, one colon-delimited payload type per
  variant, and generated constructors;
- inline anonymous struct payloads for enum variants, exposed as associated
  types such as `Message::Move` in type positions;
- exhaustive enum variant matching with payload binding and `_`.

The following remain design goals, not supported syntax:

- nominal wrapper types and transparent aliases;
- aliases, wrapper types, tags, and configurable enum representations;
- anonymous types outside enum payloads, custom constructors, field mutation,
  and general/custom patterns;
- generics and interfaces;
- type-level constant parameters, such as fixed-size arrays;
- macros, syntax types, templates, interpolation, and hygienic expansion.

When these are implemented, their documentation should be split into a user
reference and a contributor design page. In particular, generic syntax should
not be documented as working merely because an example demonstrates the
desired form.
