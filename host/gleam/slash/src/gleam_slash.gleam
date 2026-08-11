//// Slash-command parser owned by Gleam; string primitives via `@external(wasm)`.
////
//// Export: `gleam_slash__parse(String) -> Slash` (i32 struct ptr).
//// Host decodes the custom type from linear memory (tag @0, string fields @4…).
////
//// Wasm MIR limits we work around:
//// - no `case` on tuples (`RuntimeCheck::Tuple` todo)
//// - no `case` on string values (lowers to indirect call)
//// - no first-class / indirect function calls
//// - nested custom-type patterns can crash wasm codegen (avoid Arg wrappers)

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
  let cmd_raw = split_cmd(stripped)
  let arg_text = split_arg(stripped)
  let cmd = lowercase(cmd_raw)
  case length(cmd) {
    0 -> Empty
    _ -> dispatch(cmd, arg_text, stripped)
  }
}

fn dispatch(cmd: String, arg_text: String, stripped: String) -> Slash {
  case cmd == "join" {
    True -> join_text(arg_text)
    False ->
      case cmd == "part" {
        True -> PartActive
        False ->
          case cmd == "leave" {
            True -> PartActive
            False ->
              case cmd == "nick" {
                True -> nick_text(arg_text)
                False ->
                  case cmd == "me" {
                    True -> me_text(arg_text)
                    False ->
                      case cmd == "topic" {
                        True -> topic_text(arg_text)
                        False ->
                          case cmd == "msg" {
                            True -> parse_msg(arg_text)
                            False -> Raw(stripped)
                          }
                      }
                  }
              }
          }
      }
  }
}

fn join_text(text: String) -> Slash {
  case length(text) {
    0 -> Empty
    _ -> Join(text)
  }
}

fn nick_text(text: String) -> Slash {
  case length(text) {
    0 -> Empty
    _ -> Nick(text)
  }
}

fn me_text(text: String) -> Slash {
  case length(text) {
    0 -> Empty
    _ -> Me(text)
  }
}

fn topic_text(text: String) -> Slash {
  case length(text) {
    0 -> Empty
    _ -> Topic(text)
  }
}

fn parse_msg(text: String) -> Slash {
  case length(text) {
    0 -> Empty
    _ -> {
      let target = split_cmd(text)
      let body = split_arg(text)
      case length(target) > 0 {
        False -> Empty
        True ->
          case length(body) > 0 {
            True -> Msg(target, body)
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

fn split_cmd(s: String) -> String {
  let n = length(s)
  case find_space(s, 0, n) {
    -1 -> s
    i -> slice(s, 0, i)
  }
}

fn split_arg(s: String) -> String {
  let n = length(s)
  case find_space(s, 0, n) {
    -1 -> ""
    i -> slice(s, i + 1, n)
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
