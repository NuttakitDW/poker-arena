//! Optional blueprint embedding.
//!
//! When `POKER_BOT_EMBED_BLUEPRINT` names a blueprint JSON file at compile
//! time, its contents are baked into the binary and the loader plays it
//! for its game (embedding is an explicit operator action, so an embedded
//! blueprint bypasses the validated-edge gate that governs directory
//! loading). Built without the variable, the binary carries nothing and
//! behaves exactly as before.

use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    println!("cargo::rustc-check-cfg=cfg(embedded_blueprint)");
    println!("cargo:rerun-if-env-changed=POKER_BOT_EMBED_BLUEPRINT");
    if let Ok(source) = env::var("POKER_BOT_EMBED_BLUEPRINT") {
        println!("cargo:rerun-if-changed={source}");
        let out = PathBuf::from(env::var("OUT_DIR").expect("cargo sets OUT_DIR"))
            .join("embedded-blueprint.json");
        fs::copy(&source, &out)
            .unwrap_or_else(|e| panic!("copying blueprint {source} for embedding: {e}"));
        println!("cargo:rustc-cfg=embedded_blueprint");
    }
}
