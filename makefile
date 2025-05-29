build/dictator: $(shell find . -name '*.go')
	go build -ldflags="-s -w" -o build/dictator .

build/dictator-linux-amd64: $(shell find . -name '*.go')
	GOOS=linux GOARCH=amd64 go build -ldflags="-s -w" -o build/dictator-linux-amd64 .

build: build/dictator

install:
	go install

deps:
	go mod tidy

clean:
	rm -f build/dictator dictator dictator-linux-amd64.tar.gz dictator-linux-amd64

run: build
	./build/dictator

release: build/dictator-linux-amd64
	mv build/dictator-linux-amd64 build/dictator
	tar czf dictator-linux-amd64.tar.gz -C build dictator
	rm -rf dictator
