module github.com/wrpc/wrpc/examples/go/keyvalue-server

go 1.22.2

require (
	github.com/nats-io/nats.go v1.34.1
	github.com/wrpc/wrpc/go v0.0.0-unpublished
)

require (
	github.com/klauspost/compress v1.17.8 // indirect
	github.com/nats-io/nkeys v0.4.7 // indirect
	github.com/nats-io/nuid v1.0.1 // indirect
	golang.org/x/crypto v0.22.0 // indirect
	golang.org/x/sys v0.19.0 // indirect
)

replace github.com/wrpc/wrpc/go v0.0.0-unpublished => ../../../go
