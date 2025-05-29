build/dictator: $(shell find . -name '*.go')
	CGO_ENABLED=0 go build -ldflags="-s -w" -o build/dictator .

build/dictator-linux-amd64: $(shell find . -name '*.go')
	CGO_ENABLED=0 GOOS=linux GOARCH=amd64 go build -ldflags="-s -w" -o build/dictator-linux-amd64 .

build: build/dictator

install:
	go install

deps:
	go mod tidy

clean:
	rm -f build/dictator
	rm -rf dictator-linux-amd64
	rm -f dictator-linux-amd64.tar.gz

run: build
	./build/dictator

release: build/dictator-linux-amd64
	cp build/dictator-linux-amd64 dictator
	tar czf dictator-linux-amd64.tar.gz -C build dictator
	rm -rf dictator
