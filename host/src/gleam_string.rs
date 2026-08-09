//! Gleam Wasm string ABI helpers for wasmtime hosts.
//!
//! Layout (see gleam `compiler-core/src/wasm.rs`):
//!
//! ```text
//! String = i32 pointer → { len: u32 LE, data: [u8; len] } in linear `memory`
//! ```
//!
//! Host→guest writes use a **host arena** in freshly grown high pages so they
//! do not collide with the guest bump allocator (`$gleam_heap_ptr`).

use wasmtime::{AsContext, AsContextMut, Memory, StoreContext, StoreContextMut};

const PAGE: u64 = 65_536;

/// Next free offset in the host-owned high arena (`0` = not yet placed).
#[derive(Debug, Default, Clone, Copy)]
pub struct HostStringArena {
    next: u32,
}

impl HostStringArena {
    pub fn new() -> Self {
        Self { next: 0 }
    }
}

/// Read a Gleam `String` from guest linear memory.
pub fn read(memory: &Memory, store: impl AsContext, ptr: i32) -> Result<String, String> {
    read_ctx(memory, store.as_context(), ptr)
}

pub fn read_ctx<T>(
    memory: &Memory,
    store: StoreContext<'_, T>,
    ptr: i32,
) -> Result<String, String> {
    if ptr < 0 {
        return Err(format!("gleam string ptr {ptr} is negative"));
    }
    let ptr = ptr as usize;
    let data = memory.data(store);
    if ptr.checked_add(4).is_none_or(|end| end > data.len()) {
        return Err(format!(
            "gleam string ptr {ptr} out of bounds for len header (mem {})",
            data.len()
        ));
    }
    let len = u32::from_le_bytes(data[ptr..ptr + 4].try_into().unwrap()) as usize;
    let data_start = ptr + 4;
    let data_end = data_start
        .checked_add(len)
        .ok_or_else(|| format!("gleam string len {len} overflows"))?;
    if data_end > data.len() {
        return Err(format!(
            "gleam string ptr {ptr} len {len} out of bounds (mem {})",
            data.len()
        ));
    }
    let bytes = &data[data_start..data_end];
    String::from_utf8(bytes.to_vec()).map_err(|e| format!("gleam string utf-8: {e}"))
}

/// Write `value` into a host-owned high arena and return the Gleam string pointer.
pub fn write(
    arena: &mut HostStringArena,
    memory: &Memory,
    mut store: impl AsContextMut,
    value: &str,
) -> Result<i32, String> {
    write_ctx(arena, memory, store.as_context_mut(), value)
}

pub fn write_ctx<T>(
    arena: &mut HostStringArena,
    memory: &Memory,
    mut store: StoreContextMut<'_, T>,
    value: &str,
) -> Result<i32, String> {
    let bytes = value.as_bytes();
    let need = 4u64
        .checked_add(bytes.len() as u64)
        .ok_or_else(|| "gleam string too large".to_string())?;
    let need_aligned = (need + 3) & !3;

    if arena.next == 0 {
        let old_size = memory.data_size(&mut store) as u64;
        let start = (old_size + 3) & !3;
        let end = start
            .checked_add(need_aligned)
            .ok_or_else(|| "gleam string arena overflow".to_string())?;
        ensure_pages(memory, &mut store, end)?;
        arena.next = start as u32;
    }

    let start = arena.next as u64;
    let end = start
        .checked_add(need_aligned)
        .ok_or_else(|| "gleam string arena overflow".to_string())?;
    ensure_pages(memory, &mut store, end)?;

    let start_usize = start as usize;
    let data = memory.data_mut(&mut store);
    data[start_usize..start_usize + 4].copy_from_slice(&(bytes.len() as u32).to_le_bytes());
    data[start_usize + 4..start_usize + 4 + bytes.len()].copy_from_slice(bytes);

    arena.next = end as u32;
    Ok(start as i32)
}

fn ensure_pages<T>(
    memory: &Memory,
    store: &mut StoreContextMut<'_, T>,
    end_byte: u64,
) -> Result<(), String> {
    let need_pages = end_byte.div_ceil(PAGE);
    let cur_pages = memory.size(&mut *store);
    if need_pages > cur_pages {
        memory
            .grow(&mut *store, need_pages - cur_pages)
            .map_err(|e| format!("memory.grow({}): {e}", need_pages - cur_pages))?;
    }
    Ok(())
}
