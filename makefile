build/dictator: $(shell find . -name '*.go')
	go build -o build/dictator .

build: build/dictator

install:
	go install

deps:
	go mod tidy

clean:
	rm -f build/dictator

run: build
	./build/dictator
