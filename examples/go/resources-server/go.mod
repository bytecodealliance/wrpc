module wrpc.io/examples/go/resources-server

go 1.22.2

require (
	github.com/google/uuid v1.6.0
	github.com/nats-io/nats.go v1.37.0
	wrpc.io/go v0.1.0
)

require (
	github.com/klauspost/compress v1.17.8 // indirect
	github.com/nats-io/nkeys v0.4.7 // indirect
	github.com/nats-io/nuid v1.0.1 // indirect
	golang.org/x/crypto v0.31.0 // indirect
	golang.org/x/sys v0.28.0 // indirect
)

replace wrpc.io/go v0.1.0 => ../../../go
