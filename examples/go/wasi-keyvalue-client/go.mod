module wrpc.io/examples/go/wasi-keyvalue-client

go 1.24

toolchain go1.24.2

replace wrpc.io/go v0.1.0 => ../../../go

require (
	github.com/nats-io/nats.go v1.42.0
	wrpc.io/go v0.1.0
)

require (
	github.com/klauspost/compress v1.18.0 // indirect
	github.com/nats-io/nkeys v0.4.11 // indirect
	github.com/nats-io/nuid v1.0.1 // indirect
	golang.org/x/crypto v0.37.0 // indirect
	golang.org/x/sys v0.32.0 // indirect
)
