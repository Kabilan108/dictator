.PHONY: build install deps clean test fmt check run

VERSION ?= $(shell git describe --tags --always --dirty)
LDFLAGS := -ldflags "-s -w -X main.version=$(VERSION)"

build/dictator: $(shell find . -name '*.go')
	go build $(LDFLAGS) -o build/dictator .

build: build/dictator

install:
	go install

deps:
	go mod tidy

clean:
	rm -f build/dictator

test:
	go test -v ./...

fmt:
	go fmt ./...

check:
	go vet ./...

run: build
