//! Schema-driven Checkmate embedding over a real multi-file MODULE.
//!
//! Run from this directory with:
//! ```sh
//! cargo run --manifest-path Cargo.toml -- shop_mod
//! ```
//! Exit codes: 0 on success, 1 on runtime failure, 2 on compile failure.
//!
//! What this program proves, in order:
//!   1. The `cme_schema_bindings!` proc macro turns `schemas/shop.cm` into
//!      Rust scaffolding at HOST COMPILE TIME (no Checkmate magic macros
//!      involved; this is 100% a Rust proc macro reading the schema file).
//!   2. The host implements the GENERATED capability trait, registers the
//!      GENERATED schema descriptor, and loads a whole `shop_mod/` MODULE
//!      (two `.cm` files linked as one program) through the schema gate.
//!   3. The host calls the script twice: once via plain `invoke("main")`
//!      (script calls back into the host capability), and once via the
//!      GENERATED interface proxy (`get_interface`, typed host->script).

use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

use cme::{Engine, ExecutionLimits};

// ---------------------------------------------------------------------------
// STEP 1: the Rust macro. This ONE line is the whole code-generation step.
// ---------------------------------------------------------------------------
//
// `cme_schema_bindings!` runs while THIS crate compiles:
//   a. It resolves `path` relative to THIS crate's manifest dir (the same
//      rule `include_str!` uses), reads `schemas/shop.cm` to a string.
//   b. It runs the REAL schema front end (`cme_compiler::schema::parse_
//      schema_file`) on that text. A schema defect becomes a `compile_error!`
//      here: a bad contract can never silently generate bindings.
//   c. On success it emits `pub mod shop { ... }` into this file, shaped by
//      `crates/cme-schema-macro/src/codegen.rs`.
//
// Concretely, for our schema the macro generates (see `codegen.rs`):
//   - Boundary types: `shop::Item { id: i64, sku: String, price: i64 }` and
//     `shop::OrderEvent::{Started | Checkout { total: i64 }}` — plain Rust
//     types with `to_value()` / `from_value()` pack/unpack over `cme::Value`.
//     Schema `int/str` map to `i64/String`; names keep schema casing for
//     types, members become snake_case methods.
//   - Capability trait `shop::ShopStoreCapability` (one trait per
//     `capability` block, named `Pascal(namespace) + Pascal(name) +
//     "Capability"`): `fn fetch_item(&self, sku: String) -> Item` and
//     `fn record_sale(&self, item: Item, qty: i64)`. Implementing THIS trait
//     is the verification: a missing method or wrong signature fails the
//     HOST build, never a script call at runtime.
//   - Bridge + registration: `ShopStoreCapabilityBridge` (the internal
//     `CapabilityProvider` impl that unpacks script `Value`s in declaration
//     order and packs the return) plus `shop::register_shop_store(engine,
//     provider)`, named `register_<ns>_<capability>`.
//   - Interface proxy `shop::ShopCheckoutProxy` (one struct per `interface`
//     block): `TARGET = "shop.checkout"`, typed methods like
//     `on_order(&self, event: OrderEvent) -> Result<i64, ExecutionError>`
//     that wrap `context.invoke_member` with exact arity and typed convert.
//   - Descriptor: `shop::schema()` (the contract rebuilt as data, no
//     re-parse), `shop::register_schema(engine)`, `shop::NAMESPACE`, and
//     `shop::SCHEMA_VERSION` — so script-side checking and these bindings
//     provably come from the same file.
pub mod bindings {
    cme::cme_schema_bindings!(path = "schemas/shop.cm", crate = ::cme::api);
}

// ---------------------------------------------------------------------------
// STEP 2: implement the GENERATED capability trait (host provides `store`).
// ---------------------------------------------------------------------------
//
// The method names/signatures below are dictated by the macro output. Try
// renaming `fetch_item`, dropping `record_sale`, or changing a type: this
// file stops compiling. That compile break IS the §9.6 guarantee — the host
// can never be out of sync with the schema it compiled against.
pub struct StoreService {
    next_id: AtomicI64,
    recorded_qty: AtomicI64,
}

impl StoreService {
    fn new() -> Self {
        StoreService {
            next_id: AtomicI64::new(1000),
            recorded_qty: AtomicI64::new(0),
        }
    }
}

impl bindings::shop::ShopStoreCapability for StoreService {
    fn fetch_item(&self, sku: String) -> bindings::shop::Item {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        bindings::shop::Item {
            id,
            sku,
            price: 42,
        }
    }

    fn record_sale(&self, item: bindings::shop::Item, qty: i64) {
        let _ = item;
        self.recorded_qty.store(qty, Ordering::SeqCst);
    }
}

fn main() {
    // The mod directory comes from argv[1] so the binary stays reusable;
    // it defaults to the sibling `shop_mod/` shipped with this example.
    let root = std::env::args().nth(1).unwrap_or_else(|| "shop_mod".to_string());

    // STEP 3: engine + schema registration. `register_schema` feeds the
    // GENERATED descriptor (`shop::schema()`) into the engine, so the exact
    // contract the trait above was checked against becomes the contract the
    // SCRIPT is checked against. No schema => pre-schema acceptance; with a
    // schema => imports, capability calls, versions, and `impl` completeness
    // are all gated at load time.
    let mut engine = Engine::new();
    if let Err(error) = bindings::shop::register_schema(&mut engine) {
        eprintln!("schema registration failed: {}", error.message());
        std::process::exit(2);
    }

    // STEP 4: wire the provider through the GENERATED register fn. This
    // wraps `StoreService` in the generated `ShopStoreCapabilityBridge`
    // (which speaks the untyped `CapabilityProvider::call(member, args)`
    // seam the interpreter dispatches through) and stores it under the
    // `"shop.store"` path. Registration order matters: the load-time
    // provider-presence check runs at LOAD, so providers must be registered
    // BEFORE `load_mod` below.
    if let Err(error) =
        bindings::shop::register_shop_store(&mut engine, Arc::new(StoreService::new()))
    {
        eprintln!("capability registration failed: {error}");
        std::process::exit(2);
    }

    // STEP 5: compile the MODULE, not a loose file. `load_mod` reads
    // `mod.toml` (whose `[schemas] shop = "1.0.0"` narrows the grant to the
    // version this mod targets), discovers `src/main.cm` +
    // `src/pricing.cm`, links them into ONE virtual program (which is why
    // `main.cm` can call `priceFor` from `pricing.cm` with no import), and
    // runs parse + schema-aware check once. ANY diagnostic — a type error,
    // a missing interface member, a call to a capability with no provider —
    // fails the load, and nothing un-checked ever reaches a `Context`.
    let program = match engine.load_mod(root.as_str()) {
        Ok(program) => program,
        Err(error) => {
            eprintln!("compile failed: {}", error.message());
            std::process::exit(2);
        }
    };

    // STEP 6: execution context + limits. The context borrows the program
    // and snapshots the providers registered above (later registrations
    // affect only future contexts). Every invocation gets a fresh fuel /
    // deadline cell under these §5.5 limits; contexts are Send+Sync so
    // concurrent invocations stay race-free.
    let limits = ExecutionLimits {
        fuel: Some(1_000_000),
        deadline_ms: Some(5_000),
        max_call_depth: 64,
    };
    let context = engine.create_context(&program, limits);

    // STEP 7a: script-calls-host. `main` runs `FetchItem`/`RecordSale`
    // (dispatched to `StoreService`) plus the cross-file `priceFor` helper:
    // expected math is 42 (host price) + 3 * 10 (pricing.cm) = 72.
    match context.invoke("main", &[]) {
        Ok(value) => println!("main -> {value}"),
        Err(error) => {
            eprintln!("runtime failed: {}", error.render());
            std::process::exit(1);
        }
    }

    // STEP 7b: host-calls-script through the GENERATED proxy. `new` (also
    // reachable as `context.get_interface()`, the §13.1 shape) fails with
    // `UnknownEntry` when the program does not implement `shop.checkout`;
    // here the mod does, so each method is a typed `invoke_member` with the
    // enum payload packed/unpacked by the generated converters.
    let checkout = match bindings::shop::ShopCheckoutProxy::new(&context) {
        Ok(proxy) => proxy,
        Err(error) => {
            eprintln!("missing interface: {}", error.render());
            std::process::exit(1);
        }
    };
    match checkout.on_order(bindings::shop::OrderEvent::Checkout { total: 21 }) {
        Ok(score) => println!("on_order(Checkout(21)) -> {score}"),
        Err(error) => {
            eprintln!("proxy call failed: {}", error.render());
            std::process::exit(1);
        }
    }
    match checkout.total_for(4) {
        Ok(total) => println!("total_for(4) -> {total}"),
        Err(error) => {
            eprintln!("proxy call failed: {}", error.render());
            std::process::exit(1);
        }
    }
}
