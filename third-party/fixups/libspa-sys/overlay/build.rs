use std::env;
use std::path::PathBuf;

fn main() {
    if let Ok(python_path) = env::var("PYTHONPATH") {
        let absolute = std::fs::canonicalize(python_path).expect("resolve PYTHONPATH");
        env::set_var("PYTHONPATH", absolute);
    }
    let libs = system_deps::Config::new()
        .probe()
        .expect("Cannot find libraries");
    println!("cargo:rerun-if-changed=wrapper.h");
    let out_path = PathBuf::from(env::var("OUT_DIR").unwrap());
    let stdint_compat = out_path.join("stdint-compat.h");
    std::fs::write(
        &stdint_compat,
        r#"
typedef __UINT8_TYPE__ uint8_t;
typedef __UINT32_TYPE__ uint32_t;
typedef __UINT64_TYPE__ uint64_t;
typedef __INTPTR_TYPE__ intptr_t;
typedef __UINTPTR_TYPE__ uintptr_t;
#define UINT32_MAX __UINT32_MAX__
#define INT32_MAX __INT32_MAX__
#define INT32_MIN (-__INT32_MAX__ - 1)
#define INT64_MAX __INT64_MAX__
#define INT64_MIN (-__INT64_MAX__ - 1)
#define UINT64_C(value) value##UL
#define PRIu32 "u"
#define PRId32 "d"
#define PRIx32 "x"
#define PRIu64 "lu"
#define PRIi64 "li"
#define PRIx64 "lx"
"#,
    )
    .expect("write stdint compatibility header");

    let builder = bindgen::builder()
        .header("wrapper.h")
        .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()))
        .size_t_is_usize(true)
        .allowlist_function("spa_.*")
        .allowlist_type("spa_.*")
        .allowlist_var("SPA_.*")
        .prepend_enum_name(false)
        .derive_eq(true)
        .wrap_static_fns(true)
        .wrap_static_fns_suffix("_libspa_rs")
        .wrap_static_fns_path(out_path.join("static_fns"))
        .clang_args(["-include", stdint_compat.to_str().unwrap()]);

    let builder = libs
        .iter()
        .iter()
        .flat_map(|(_, lib)| lib.include_paths.iter())
        .fold(builder, |builder, path| {
            builder.clang_arg(format!("-I{}", path.to_string_lossy()))
        });

    let bindings = builder.generate().expect("Unable to generate bindings");
    bindings
        .write_to_file(out_path.join("bindings.rs"))
        .expect("Couldn't write bindings!");
    let static_fns = out_path.join("static_fns.c");
    let generated = std::fs::read_to_string(&static_fns).expect("read generated wrappers");
    std::fs::write(
        &static_fns,
        format!("#include <stdint.h>\n#include <inttypes.h>\n{generated}"),
    )
    .expect("add standard integer types to generated wrappers");

    const FILES: &[&str] = &["src/type-info.c"];
    let cc_files = &[PathBuf::from(FILES[0]), static_fns.clone()];
    let mut cc = cc::Build::new();
    cc.files(cc_files);
    cc.include(".");
    cc.includes(libs.all_include_paths());
    cc.flag("-target");
    cc.flag("x86_64-linux-gnu");
    #[cfg(feature = "v0_3_65")]
    cc.define("FEATURE_0_3_65", "1");
    cc.compile("libspa-rs-reexports");
}
