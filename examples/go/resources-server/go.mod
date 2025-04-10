module wrpc.io/examples/go/resources-server

go 1.23.0

toolchain go1.24.1

require (
	github.com/google/uuid v1.6.0
	github.com/nats-io/nats.go v1.41.1
	wrpc.io/go v0.1.0
)

require (
	github.com/klauspost/compress v1.18.0 // indirect
	github.com/nats-io/nkeys v0.4.10 // indirect
	github.com/nats-io/nuid v1.0.1 // indirect
	golang.org/x/crypto v0.36.0 // indirect
	golang.org/x/sys v0.31.0 // indirect
)

replace wrpc.io/go v0.1.0 => ../../../go
