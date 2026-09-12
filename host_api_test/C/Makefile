# Makefile for the C host example (WHITEPAPER §13.2 consumer).
#
# Mirrors apps/c_host/Makefile for the shop contract: the header is
# GENERATED from schemas/shop.cm, then main.c compiles against it and
# links the cme-ffi static library.
#
# Prerequisites (built once from the repo root):
#   cargo build --features cli          # provides $(CME_CLI) for codegen-c
#   cargo build --release -p cme-ffi    # provides $(LIBS)
#
# Then, from this directory:
#   make            # generates shop_schema_gen.h, builds ./shop_host
#   ./shop_host shop_mod
# Expected output: main -> 72, on_order(Checkout(21)) -> 41, total_for(4) -> 40.

CME_ROOT ?= $(abspath $(dir $(lastword $(MAKEFILE_LIST)))/../..)
CC ?= cc
CFLAGS ?= -std=c11 -Wall -Wextra
INCLUDES = -I$(CME_ROOT)/crates/cme-ffi/include
LIBDIR = $(CME_ROOT)/target/release
LIBS = $(LIBDIR)/libcme_ffi.a -lm -lpthread -ldl
TARGET = shop_host
CME_CLI ?= $(CME_ROOT)/target/debug/cme

.PHONY: all clean run lib cli

all: $(TARGET)

# The §9.6 C header, generated from THIS directory's schemas/shop.cm — the
# same file main.c registers at runtime via cm_schema_parse_file. One
# source of truth: the REGISTER macro's _Static_asserts, the pack/unpack
# helpers, and the exact-arity interface helpers all come from this file.
shop_schema_gen.h: schemas/shop.cm $(CME_CLI)
	$(CME_CLI) codegen-c schemas/shop.cm > $@
	@echo "schema header generated: $@"

$(TARGET): main.c shop_schema_gen.h $(LIBDIR)/libcme_ffi.a
	$(CC) $(CFLAGS) $(INCLUDES) -I. -o $@ main.c $(LIBS)

# Convenience: build the two prerequisites from the repo root.
lib:
	cargo build --release -p cme-ffi --manifest-path $(CME_ROOT)/Cargo.toml

cli:
	cargo build --features cli --manifest-path $(CME_ROOT)/Cargo.toml

run: $(TARGET)
	./$(TARGET) shop_mod

clean:
	rm -f $(TARGET) shop_schema_gen.h
