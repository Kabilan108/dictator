.PHONY: build install deps clean test fmt check run release FORCE

VERSION ?= $(shell python3 scripts/version.py)
CARGO ?= cargo

build/dictator: check-version FORCE $(shell find src tests -name '*.rs') Cargo.toml Cargo.lock build.rs
	DICTATOR_VERSION=$(VERSION) $(CARGO) build --release
	mkdir -p build
	cp target/release/dictator build/dictator

build: build/dictator

install: check-version
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

FORCE:

.PHONY: gui check-gui test-gui

gui:
	$(CARGO) build --features gui --bin dictator-gui

check-gui:
	$(CARGO) fmt --check
	$(CARGO) clippy --features gui --all-targets -- -D warnings

test-gui:
	$(CARGO) test --features gui

.PHONY: check-version bump-version

check-version:
	python3 scripts/version.py

bump-version:
	python3 scripts/version.py --bump "$(NEW_VERSION)"
