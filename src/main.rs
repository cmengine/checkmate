// This binary is only compiled if the user installs the CLI toolchain.
#[cfg(feature = "cli")]
fn main() {
    println!("Checkmate Engine Toolchain v{}", env!("CARGO_PKG_VERSION"));
    println!("The Checkmate compiler and language server will live here.");
}

#[cfg(not(feature = "cli"))]
fn main() {
    eprintln!("Error: The 'cme' CLI was built without the 'cli' feature.");
    eprintln!("Try compiling with: cargo build --features cli");
    std::process::exit(1);
}
