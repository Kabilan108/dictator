.PHONY: build install deps clean test fmt check run release

VERSION ?= $(shell git describe --tags --always --dirty)
CARGO ?= cargo

build/dictator: $(shell find src tests -name '*.rs') Cargo.toml Cargo.lock
	DICTATOR_VERSION=$(VERSION) $(CARGO) build --release
	mkdir -p build
	cp target/release/dictator build/dictator

build: build/dictator

install:
	DICTATOR_VERSION=$(VERSION) $(CARGO) install --path . --locked

deps:
	$(CARGO) update

clean:
	rm -rf build
	$(CARGO) clean

test:
	$(CARGO) test

fmt:
	$(CARGO) fmt

check:
	$(CARGO) fmt --check
	$(CARGO) clippy --all-targets -- -D warnings

run: build
	./build/dictator daemon
