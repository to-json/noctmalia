use std::env;
use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=src/shim.cpp");
    println!("cargo:rerun-if-env-changed=LITEHTML_ROOT");

    // litehtml ships no pkg-config file (only cmake config files) — see docs/html-mail-plan.md
    // Stream 0.1 and flake.nix's devShell, which sets this to the package's own store path.
    let root: PathBuf = env::var("LITEHTML_ROOT")
        .expect("LITEHTML_ROOT must point at a litehtml install (include/ + lib/) — set by flake.nix's devShell")
        .into();

    cc::Build::new()
        .cpp(true)
        .std("c++17")
        .file("src/shim.cpp")
        .include(root.join("include"))
        .warnings(false) // litehtml's own headers, not our code, trip -Wunused-parameter etc.
        .compile("litehtml_shim");

    println!("cargo:rustc-link-search=native={}", root.join("lib").display());
    println!("cargo:rustc-link-lib=static=litehtml");

    // litehtml parses HTML5 through gumbo rather than bundling it — a separate static lib, not
    // pulled in transitively by liblitehtml.a's own link metadata (it has none: a plain .a, no
    // .pc/.cmake dependency graph to follow), so link it explicitly.
    let gumbo_root: PathBuf = env::var("GUMBO_ROOT")
        .expect("GUMBO_ROOT must point at a gumbo install (lib/) — set by flake.nix's devShell")
        .into();
    println!("cargo:rustc-link-search=native={}", gumbo_root.join("lib").display());
    println!("cargo:rustc-link-lib=dylib=gumbo");

    println!("cargo:rustc-link-lib=dylib=stdc++");
}
