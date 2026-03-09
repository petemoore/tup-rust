use std::env;
use std::path::PathBuf;

fn main() {
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();

    // Only build the LD_PRELOAD library on Linux
    // (macOS uses a different mechanism, Windows uses DLL injection)
    if target_os == "linux" {
        build_ldpreload();
    }
}

fn build_ldpreload() {
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());

    // Compile ldpreload.c as a shared library
    let status = std::process::Command::new("cc")
        .args([
            "-shared",
            "-fPIC",
            "-o",
            out_dir.join("libtup_ldpreload.so").to_str().unwrap(),
            "csrc/ldpreload.c",
            "-Icsrc",
            "-ldl",
            "-lpthread",
            "-Wall",
            "-Wextra",
            "-O2",
        ])
        .status();

    match status {
        Ok(s) if s.success() => {
            // Tell cargo where to find the library
            println!(
                "cargo:rustc-env=TUP_LDPRELOAD_PATH={}",
                out_dir.join("libtup_ldpreload.so").display()
            );
            println!("cargo:rerun-if-changed=csrc/ldpreload.c");
            println!("cargo:rerun-if-changed=csrc/tup_depfile.h");
        }
        Ok(s) => {
            eprintln!(
                "Warning: failed to compile ldpreload.c (exit code {:?})",
                s.code()
            );
            eprintln!("LD_PRELOAD dependency tracking will not be available.");
        }
        Err(e) => {
            eprintln!("Warning: cc not found ({e}), ldpreload.c not compiled.");
            eprintln!("LD_PRELOAD dependency tracking will not be available.");
        }
    }
}
