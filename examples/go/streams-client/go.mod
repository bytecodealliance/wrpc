module wrpc.io/examples/go/streams-client

go 1.24.0

toolchain go1.24.2

require (
	github.com/nats-io/nats.go v1.47.0
	wrpc.io/go v0.2.0
)

require (
	github.com/klauspost/compress v1.18.0 // indirect
	github.com/nats-io/nkeys v0.4.11 // indirect
	github.com/nats-io/nuid v1.0.1 // indirect
	golang.org/x/crypto v0.45.0 // indirect
	golang.org/x/sys v0.38.0 // indirect
)

replace wrpc.io/go v0.2.0 => ../../../go
