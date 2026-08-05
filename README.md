CME

Checkmate Embedding is the Rust-facing package for embedding and integrating the Checkmate scripting language.

Version "0.1.0" provides the Checkmate language whitepaper as a compile-time embedded resource. Compiler, runtime, and host-integration functionality will be introduced in later releases.

let specification = cme::whitepaper();
println!("{specification}");

The embedded specification is included at compile time, so it is available without reading external files at runtime.
