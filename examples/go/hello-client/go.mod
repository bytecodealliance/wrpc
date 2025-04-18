module wrpc.io/examples/go/hello-client

go 1.23.0

toolchain go1.24.1

require (
	github.com/nats-io/nats.go v1.41.2
	wrpc.io/go v0.1.0
)

require (
	github.com/klauspost/compress v1.18.0 // indirect
	github.com/nats-io/nkeys v0.4.11 // indirect
	github.com/nats-io/nuid v1.0.1 // indirect
	golang.org/x/crypto v0.37.0 // indirect
	golang.org/x/sys v0.32.0 // indirect
)

replace wrpc.io/go v0.1.0 => ../../../go
