# Makefile for the C host application (WHITEPAPER §13.2 consumer).
#
# Builds apps/c_host/main.c against the cme-ffi static library and runs it.
# The Rust test suite compiles this exact source file too (see
# crates/cme-ffi/build.rs), so `cargo test --workspace` exercises the same
# consumer end to end.

CME_ROOT ?= $(abspath $(dir $(lastword $(MAKEFILE_LIST)))/../..)
CC ?= cc
CFLAGS ?= -std=c11 -Wall -Wextra
INCLUDES = -I$(CME_ROOT)/crates/cme-ffi/include
LIBDIR = $(CME_ROOT)/target/release
LIBS = $(LIBDIR)/libcme_ffi.a -lm -lpthread -ldl
TARGET = c_host_app

.PHONY: all clean test

all: $(TARGET)

$(TARGET): main.c $(LIBS)
	$(CC) $(CFLAGS) $(INCLUDES) -o $@ main.c $(LIBS)

test: $(TARGET)
	./$(TARGET)

clean:
	rm -f $(TARGET)

# The §9.6 C bindings demo: generate the schema header with the CLI and
# syntax-check it against the ABI header (requires `cargo build --features cli`).
CME_CLI ?= $(CME_ROOT)/target/debug/cme

schema-gen: engine_schema_gen.h

engine_schema_gen.h: engine.cm $(CME_CLI)
	$(CME_CLI) codegen-c engine.cm > $@
	@printf '#include "engine_schema_gen.h"\nint main(void) { return 0; }\n' | \
		$(CC) $(CFLAGS) $(INCLUDES) -I. -x c - -fsyntax-only
	@echo "schema header generated and verified: $@"

clean:
	rm -f engine_schema_gen.h
