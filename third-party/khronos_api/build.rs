// Copyright 2015 Brendan Zabarauskas and the gl-rs developers
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// This build script is used to discover all of the accepted extensions
// described in the Khronos WebGL repository, and construct a static slice
// containing the XML specifications for all of them.
use std::env;
use std::fs::File;
use std::io::Write;
use std::path::*;

fn main() {
    // Create and open a file in the output directory to contain our generated rust code
    let dest = env::var("OUT_DIR").unwrap();
    let mut file = File::create(&Path::new(&dest).join("webgl_exts.rs")).unwrap();

    // sleek patch: upstream resolved an *absolute* path here (via
    // `env::current_dir()`) and baked that string directly into the
    // generated `include_bytes!(..)` calls — correct under plain `cargo
    // build` (cwd is the crate root for the whole build), but wrong under
    // buck2: the build script and the crate's own compile action are
    // separate, independently-sandboxed actions, and under remote
    // execution the build script's cwd doesn't exist by the time the
    // compile action runs on a (possibly different) machine — "No such
    // file or directory" for every extension.xml.
    //
    // Fix: still list the directory here (this action does have the full
    // crate source as input, same as upstream assumed), but emit
    // `concat!(env!("CARGO_MANIFEST_DIR"), "/api_webgl/extensions/…")` as
    // *literal source text* into the generated file, not a pre-resolved
    // string. `env!(..)` is a compile-time macro — it gets evaluated fresh
    // by whichever action actually compiles this generated code, using
    // that action's own (correct, always-provided) CARGO_MANIFEST_DIR,
    // instead of the build script's private, possibly-gone-by-then cwd.
    let root = env::current_dir().unwrap().join("api_webgl/extensions");

    // Generate a slice literal, looking like this:
    // `&[&*include_bytes!(..), &*include_bytes!(..), ..]`
    writeln!(file, "&[").unwrap();

    // The slice will have one entry for each WebGL extension. To find the
    // extensions we mirror the behaviour of the `api_webgl/extensions/find-exts`
    // shell script.
    for entry in root.read_dir().unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        let ext_name = path.file_name().unwrap().to_str().unwrap();

        // Each item which is a directory, and is not named `template`, may be
        // an extension.
        if path.is_dir() && ext_name != "template" {
            // If the directory contains a file named `extension.xml`, then this
            // really is an extension.
            let ext_path = path.join("extension.xml");
            if ext_path.is_file() {
                // sleek patch: relative-to-manifest-dir literal, resolved at
                // the *compiling* action's own compile time — see comment above.
                writeln!(
                    file,
                    "&*include_bytes!(concat!(env!(\"CARGO_MANIFEST_DIR\"), \"/api_webgl/extensions/{}/extension.xml\")),",
                    ext_name,
                )
                .unwrap();
            }
        }
    }

    // Close the slice
    writeln!(file, "]").unwrap();
}
