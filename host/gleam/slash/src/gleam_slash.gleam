//// Slash-command parser owned by Gleam; string primitives via `@external(wasm)`.
////
//// Export: `gleam_slash__parse(String) -> Slash` (i32 struct ptr).
//// Host decodes the custom type from linear memory (tag @0, string fields @4…).
////
//// Wasm MIR limits we work around:
//// - no `case` on tuples (`RuntimeCheck::Tuple` todo)
//// - no `case` on string values (lowers to indirect call)
//// - no first-class / indirect function calls

pub type Slash {
  Empty
  PartActive
  Join(String)
  Nick(String)
  Me(String)
  Topic(String)
  Raw(String)
  Msg(String, String)
}

type Arg {
  NoArg
  Arg(String)
}

type Split {
  Split(String, Arg)
}

@external(wasm, "sleek", "length")
fn length(s: String) -> Int

@external(wasm, "sleek", "byte_at")
fn byte_at(s: String, i: Int) -> Int

@external(wasm, "sleek", "slice")
fn slice(s: String, start: Int, end: Int) -> String

@external(wasm, "sleek", "lowercase")
fn lowercase(s: String) -> String

/// Parse a compose-box string that starts with `/` (slash optional).
pub fn parse(input: String) -> Slash {
  let stripped = drop_slash(input)
  case split_once(stripped) {
    Split(cmd_raw, arg) -> {
      let cmd = lowercase(cmd_raw)
      case length(cmd) {
        0 -> Empty
        _ -> dispatch(cmd, arg, stripped)
      }
    }
  }
}

fn dispatch(cmd: String, arg: Arg, stripped: String) -> Slash {
  case cmd == "join" {
    True -> one_arg(arg, 0)
    False ->
      case cmd == "part" {
        True -> PartActive
        False ->
          case cmd == "leave" {
            True -> PartActive
            False ->
              case cmd == "nick" {
                True -> one_arg(arg, 1)
                False ->
                  case cmd == "me" {
                    True -> one_arg(arg, 2)
                    False ->
                      case cmd == "topic" {
                        True -> one_arg(arg, 3)
                        False ->
                          case cmd == "msg" {
                            True -> parse_msg(arg)
                            False -> Raw(stripped)
                          }
                      }
                  }
              }
          }
      }
  }
}

/// `which`: 0=Join, 1=Nick, 2=Me, 3=Topic — avoids passing constructors as fns.
fn one_arg(arg: Arg, which: Int) -> Slash {
  case arg {
    NoArg -> Empty
    Arg(a) ->
      case which {
        0 -> Join(a)
        1 -> Nick(a)
        2 -> Me(a)
        _ -> Topic(a)
      }
  }
}

fn parse_msg(arg: Arg) -> Slash {
  case arg {
    NoArg -> Empty
    Arg(a) ->
      case split_once(a) {
        Split(_, NoArg) -> Empty
        Split(target, Arg(text)) ->
          case length(target) > 0 {
            False -> Empty
            True ->
              case length(text) > 0 {
                True -> Msg(target, text)
                False -> Empty
              }
          }
      }
  }
}

fn drop_slash(s: String) -> String {
  case length(s) {
    0 -> s
    _ ->
      case byte_at(s, 0) {
        // '/'
        47 -> slice(s, 1, length(s))
        _ -> s
      }
  }
}

fn split_once(s: String) -> Split {
  let n = length(s)
  case find_space(s, 0, n) {
    -1 -> Split(s, NoArg)
    i -> {
      let rest = slice(s, i + 1, n)
      case length(rest) {
        0 -> Split(slice(s, 0, i), NoArg)
        _ -> Split(slice(s, 0, i), Arg(rest))
      }
    }
  }
}

fn find_space(s: String, i: Int, n: Int) -> Int {
  case i < n {
    False -> -1
    True ->
      case byte_at(s, i) {
        // ' '
        32 -> i
        _ -> find_space(s, i + 1, n)
      }
  }
}
