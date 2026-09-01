//// Module doc comment: four slashes, distinct from the doc comment below.

import gleam/int
import gleam/list
import gleam/string

/// Doc comment: three slashes.
/// Spans several lines.
// Plain line comment.

pub type Shape {
  Circle(radius: Float)
  Rectangle(width: Float, height: Float)
}

pub opaque type Counter {
  Counter(count: Int)
}

pub const greeting: String = "hello"

const limit = 100

@external(erlang, "lists", "reverse")
@deprecated("use list.reverse instead")
pub fn reverse(items: List(a)) -> List(a)

@target(javascript)
pub fn area(shape: Shape) -> Float {
  case shape {
    Circle(radius) -> 3.14159 *. radius *. radius
    Rectangle(width, height) -> width *. height
  }
}

pub fn classify(n: Int) -> Result(String, String) {
  case n {
    0 -> Ok("zero")
    _ if n < 0 -> Error("negative")
    _ -> Ok("positive")
  }
}

pub fn escapes() -> String {
  let quoted = "a \"quoted\" word"
  let escaped = "tab:\t newline:\n return:\r formfeed:\f backslash:\\"
  let codepoint = "snowman \u{2603} and \u{1F600}"
  let unknown = "not a real escape: \q"
  string.concat([quoted, escaped, codepoint, unknown])
}

pub fn numbers() -> List(Int) {
  let hex = 0xDEADbeef
  let octal = 0o755
  let binary = 0b1010_1010
  let decimal = 1_000_000
  let negative = -42
  let float = 3.14
  let exponent = 6.022e23
  let small = 1.6e-19
  // Digits immediately followed by word characters are not a valid
  // literal, so they must not highlight as numeric.
  let invalid = 123abc
  [hex, octal, binary, decimal, negative]
}

pub fn discards() -> Nil {
  let _ = "wholly discarded"
  let _unused = 1
  let assert Ok(value) = classify(1)
  echo value
  Nil
}

pub fn booleans() -> Bool {
  let yes = True
  let no = False
  yes && !no
}

pub fn types() -> Nil {
  let a: Int = 1
  let b: Float = 2.0
  let c: String = "three"
  let d: Bool = True
  let e: List(Int) = [1, 2, 3]
  let f: Result(Int, Nil) = Ok(1)
  let g: BitArray = <<1, 2, 3>>
  let h: Dynamic = dynamic.from(1)
  let i: UtfCodepoint = codepoint
  Nil
}

pub fn control(items: List(Int)) -> Int {
  use item <- list.map(items)
  case item {
    0 -> panic as "zero is not allowed"
    1 -> todo as "not implemented yet"
    _ -> item
  }
}

pub type Animal {
  Dog
  Cat
}

pub fn sound(animal: Animal) -> String {
  case animal {
    Dog -> "woof"
    Cat -> "meow"
  }
}

pub fn main() {
  let shapes = [Circle(1.0), Rectangle(2.0, 3.0)]
  list.map(shapes, area)
  |> list.each(io.debug)
}

// A string literal may carry a raw newline.
const span = "line one
line two"
