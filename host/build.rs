//! Compile Gleam Wasm guests into OUT_DIR for `include_bytes!`.
//!
//! Prefers a live `gleam` binary (GLEAM env or well-known paths). When that is
//! unavailable — e.g. hermetic `nix build .#sleek` / Flatpak — falls back to
//! the checked-in `gleam/slash/prebuilt/gleam_slash.wasm` so offline CI still
//! links. Regenerate the prebuilt with a wasm-capable gleam:
//!   https://tangled.org/nandi.uk/gleam  (branch wasm)

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    println!("cargo:rerun-if-env-changed=GLEAM");

    build_guest(
        &manifest_dir,
        &out_dir,
        "gleam_slash",
        "gleam/slash",
        "src/gleam_slash.gleam",
        "gleam/slash/prebuilt/gleam_slash.wasm",
    );
}

fn build_guest(
    manifest_dir: &Path,
    out_dir: &Path,
    package: &str,
    guest_rel: &str,
    src_rel: &str,
    prebuilt_rel: &str,
) {
    let guest_dir = manifest_dir.join(guest_rel);
    let guest_src = guest_dir.join(src_rel);
    let prebuilt = manifest_dir.join(prebuilt_rel);

    println!("cargo:rerun-if-changed={}", guest_src.display());
    println!(
        "cargo:rerun-if-changed={}",
        guest_dir.join("gleam.toml").display()
    );
    println!("cargo:rerun-if-changed={}", prebuilt.display());

    let wasm_src = match find_gleam(manifest_dir) {
        Some(gleam) => {
            eprintln!("sleek-host build.rs: gleam → {}", gleam.display());
            run_gleam_build(&gleam, &guest_dir, package)
        }
        None => {
            if !prebuilt.is_file() {
                panic!(
                    "gleam not found and prebuilt wasm missing at {}.\n\
                     Set GLEAM to a wasm-capable gleam binary \
                     (https://tangled.org/nandi.uk/gleam , branch wasm),\n\
                     or commit {prebuilt_rel} under host/.",
                    prebuilt.display()
                );
            }
            eprintln!(
                "sleek-host build.rs: gleam unavailable; using prebuilt {}",
                prebuilt.display()
            );
            prebuilt
        }
    };

    let wasm_dst = out_dir.join(format!("{package}.wasm"));
    fs::copy(&wasm_src, &wasm_dst).unwrap_or_else(|err| {
        panic!(
            "copy {} → {}: {err}",
            wasm_src.display(),
            wasm_dst.display()
        );
    });

    let inspect = manifest_dir.join("target").join(format!("{package}.wasm"));
    if let Some(parent) = inspect.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = fs::copy(&wasm_src, &inspect);

    eprintln!(
        "sleek-host build.rs: {package} ready ({} bytes) → {}",
        fs::metadata(&wasm_dst).map(|m| m.len()).unwrap_or(0),
        wasm_dst.display()
    );
}

fn run_gleam_build(gleam: &Path, guest_dir: &Path, package: &str) -> PathBuf {
    let status = Command::new(gleam)
        .current_dir(guest_dir)
        .arg("build")
        .status()
        .unwrap_or_else(|err| {
            panic!(
                "failed to spawn gleam at {}: {err}\n\
                 Set GLEAM to a wasm-capable gleam binary \
                 (https://tangled.org/nandi.uk/gleam , branch wasm).",
                gleam.display()
            )
        });

    if !status.success() {
        panic!(
            "`gleam build` failed in {} (status {status}).\n\
             Need Gleam with target = \"wasm\" support:\n\
               https://tangled.org/nandi.uk/gleam  (branch wasm)\n\
               cargo build -p gleam && export GLEAM=$PWD/target/debug/gleam\n\
             Current candidate: {}",
            guest_dir.display(),
            gleam.display()
        );
    }

    let wasm_src = guest_dir
        .join("build/dev/wasm")
        .join(package)
        .join(format!("{package}.wasm"));
    if !wasm_src.is_file() {
        panic!(
            "gleam build succeeded but {} was not created",
            wasm_src.display()
        );
    }
    wasm_src
}

fn find_gleam(manifest_dir: &Path) -> Option<PathBuf> {
    if let Ok(path) = env::var("GLEAM") {
        let p = PathBuf::from(path);
        if p.is_file() {
            return Some(p);
        }
        panic!("GLEAM={p:?} is not a file");
    }

    let mut candidates = vec![
        manifest_dir.join("../../gleam/target/debug/gleam"),
        manifest_dir.join("../../gleam/target/release/gleam"),
    ];
    if let Some(home) = env::var_os("HOME") {
        let home = PathBuf::from(home);
        candidates.push(home.join("code/gleam/target/debug/gleam"));
        candidates.push(home.join("code/gleam/target/release/gleam"));
    }
    for candidate in candidates {
        if candidate.is_file() {
            return Some(candidate.canonicalize().unwrap_or(candidate));
        }
    }

    // PATH lookup — only accept if `gleam --version` works.
    let path_candidate = PathBuf::from("gleam");
    match Command::new(&path_candidate).arg("--version").status() {
        Ok(status) if status.success() => Some(path_candidate),
        _ => None,
    }
}
