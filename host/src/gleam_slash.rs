//! Gleam Wasm slash parser: host string externals + custom-type decode.

use std::sync::{Mutex, OnceLock};

use wasmtime::{Caller, Engine, Linker, Memory, Module, Store, TypedFunc};

use crate::gleam_string::{self, HostStringArena};

const SLASH_WASM: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/gleam_slash.wasm"));

/// Mirrors `sleek::slash::SlashCommand` without depending on the android crate
/// layout from build scripts. Converted at the host boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlashCommand {
    Join(String),
    PartActive,
    Nick(String),
    Me(String),
    Msg { target: String, text: String },
    Topic(String),
    Raw(String),
    Empty,
}

struct HostState {
    arena: HostStringArena,
}

struct SlashSession {
    store: Store<HostState>,
    memory: Memory,
    parse: TypedFunc<i32, i32>,
}

static SESSION: OnceLock<Mutex<SlashSession>> = OnceLock::new();

fn session() -> Result<&'static Mutex<SlashSession>, String> {
    if let Some(s) = SESSION.get() {
        return Ok(s);
    }
    let loaded = SlashSession::load()?;
    let _ = SESSION.set(Mutex::new(loaded));
    SESSION
        .get()
        .ok_or_else(|| "gleam slash session missing after init".into())
}

impl SlashSession {
    fn load() -> Result<Self, String> {
        let engine = Engine::default();
        let module = Module::new(&engine, SLASH_WASM)
            .map_err(|e| format!("parse gleam_slash.wasm: {e}"))?;

        let mut linker = Linker::new(&engine);
        register_string_externals(&mut linker)?;

        let mut store = Store::new(
            &engine,
            HostState {
                arena: HostStringArena::new(),
            },
        );
        let instance = linker
            .instantiate(&mut store, &module)
            .map_err(|e| format!("instantiate gleam_slash.wasm: {e}"))?;

        let memory = instance
            .get_memory(&mut store, "memory")
            .ok_or_else(|| "gleam_slash.wasm missing exported memory".to_string())?;
        let parse = instance
            .get_typed_func(&mut store, "gleam_slash__parse")
            .map_err(|e| format!("export gleam_slash__parse: {e}"))?;

        Ok(Self {
            store,
            memory,
            parse,
        })
    }

    fn call_parse(&mut self, in_ptr: i32) -> Result<i32, String> {
        self.parse
            .call(&mut self.store, in_ptr)
            .map_err(|e| format!("call gleam_slash__parse: {e}"))
    }
}

fn register_string_externals(linker: &mut Linker<HostState>) -> Result<(), String> {
    linker
        .func_wrap(
            "sleek",
            "length",
            |mut caller: Caller<'_, HostState>, s: i32| -> i64 {
                match memory_of(&mut caller).and_then(|m| gleam_string::read(&m, &caller, s)) {
                    Ok(text) => text.len() as i64,
                    Err(_) => 0,
                }
            },
        )
        .map_err(|e| format!("link sleek.length: {e}"))?;

    linker
        .func_wrap(
            "sleek",
            "byte_at",
            |mut caller: Caller<'_, HostState>, s: i32, i: i64| -> i64 {
                match memory_of(&mut caller).and_then(|m| gleam_string::read(&m, &caller, s)) {
                    Ok(text) => {
                        let i = i as usize;
                        text.as_bytes().get(i).copied().unwrap_or(0) as i64
                    }
                    Err(_) => 0,
                }
            },
        )
        .map_err(|e| format!("link sleek.byte_at: {e}"))?;

    linker
        .func_wrap(
            "sleek",
            "slice",
            |mut caller: Caller<'_, HostState>, s: i32, start: i64, end: i64| -> i32 {
                let Ok(memory) = memory_of(&mut caller) else {
                    return 0;
                };
                let Ok(text) = gleam_string::read(&memory, &caller, s) else {
                    return 0;
                };
                let len = text.len() as i64;
                let start = start.clamp(0, len) as usize;
                let end = end.clamp(start as i64, len) as usize;
                let piece = text[start..end].to_string();
                write_host_string(&mut caller, memory, &piece)
            },
        )
        .map_err(|e| format!("link sleek.slice: {e}"))?;

    linker
        .func_wrap(
            "sleek",
            "lowercase",
            |mut caller: Caller<'_, HostState>, s: i32| -> i32 {
                let Ok(memory) = memory_of(&mut caller) else {
                    return 0;
                };
                let Ok(text) = gleam_string::read(&memory, &caller, s) else {
                    return 0;
                };
                let lower = text.to_ascii_lowercase();
                write_host_string(&mut caller, memory, &lower)
            },
        )
        .map_err(|e| format!("link sleek.lowercase: {e}"))?;

    Ok(())
}

fn write_host_string(caller: &mut Caller<'_, HostState>, memory: Memory, value: &str) -> i32 {
    let mut arena = std::mem::take(&mut caller.data_mut().arena);
    let ptr = gleam_string::write(&mut arena, &memory, &mut *caller, value).unwrap_or(0);
    caller.data_mut().arena = arena;
    ptr
}

fn memory_of(caller: &mut Caller<'_, HostState>) -> Result<Memory, String> {
    caller
        .get_export("memory")
        .and_then(|e| e.into_memory())
        .ok_or_else(|| "missing memory export during sleek string external".into())
}

/// Parse `input` via the Gleam Wasm guest.
pub fn parse(input: &str) -> Result<SlashCommand, String> {
    let mutex = session()?;
    let mut s = mutex
        .lock()
        .map_err(|_| "gleam slash session lock poisoned".to_string())?;

    let memory = s.memory;
    let mut arena = std::mem::take(&mut s.store.data_mut().arena);
    let in_ptr = gleam_string::write(&mut arena, &memory, &mut s.store, input);
    s.store.data_mut().arena = arena;
    let in_ptr = in_ptr?;

    let out_ptr = s.call_parse(in_ptr)?;
    decode_slash(&s.memory, &s.store, out_ptr)
}

/// Gleam custom type layout: `tag: u32 @0`, fields as `i32` slots at `4 + i*4`.
///
/// Declaration order in `gleam_slash.gleam`:
/// Empty, PartActive, Join, Nick, Me, Topic, Raw, Msg.
fn decode_slash(
    memory: &Memory,
    store: &Store<HostState>,
    ptr: i32,
) -> Result<SlashCommand, String> {
    if ptr <= 0 {
        return Err(format!("slash result ptr {ptr} invalid"));
    }
    let ptr = ptr as usize;
    let data = memory.data(store);
    if ptr + 4 > data.len() {
        return Err("slash result tag out of bounds".into());
    }
    let tag = u32::from_le_bytes(data[ptr..ptr + 4].try_into().unwrap());
    let field = |index: usize| -> Result<i32, String> {
        let off = ptr + 4 + index * 4;
        if off + 4 > data.len() {
            return Err(format!("slash field {index} out of bounds"));
        }
        Ok(i32::from_le_bytes(data[off..off + 4].try_into().unwrap()))
    };
    let read_str = |index: usize| -> Result<String, String> {
        let sp = field(index)?;
        gleam_string::read(memory, store, sp)
    };

    match tag {
        0 => Ok(SlashCommand::Empty),
        1 => Ok(SlashCommand::PartActive),
        2 => Ok(SlashCommand::Join(read_str(0)?)),
        3 => Ok(SlashCommand::Nick(read_str(0)?)),
        4 => Ok(SlashCommand::Me(read_str(0)?)),
        5 => Ok(SlashCommand::Topic(read_str(0)?)),
        6 => Ok(SlashCommand::Raw(read_str(0)?)),
        7 => Ok(SlashCommand::Msg {
            target: read_str(0)?,
            text: read_str(1)?,
        }),
        other => Err(format!("unknown slash tag {other}")),
    }
}
