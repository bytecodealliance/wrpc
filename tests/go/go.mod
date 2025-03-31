module wrpc.io/tests/go

go 1.23.0

toolchain go1.24.1

require (
	github.com/google/uuid v1.6.0
	github.com/nats-io/nats-server/v2 v2.10.14
	github.com/nats-io/nats.go v1.40.1
	wrpc.io/go v0.1.0
)

require github.com/lmittmann/tint v1.0.5

require (
	github.com/davecgh/go-spew v1.1.1 // indirect
	github.com/klauspost/compress v1.18.0 // indirect
	github.com/minio/highwayhash v1.0.2 // indirect
	github.com/nats-io/jwt/v2 v2.5.5 // indirect
	github.com/nats-io/nkeys v0.4.10 // indirect
	github.com/nats-io/nuid v1.0.1 // indirect
	github.com/pmezard/go-difflib v1.0.0 // indirect
	github.com/stretchr/testify v1.9.0
	golang.org/x/crypto v0.36.0 // indirect
	golang.org/x/sys v0.31.0 // indirect
	golang.org/x/time v0.5.0 // indirect
	gopkg.in/yaml.v3 v3.0.1 // indirect
)

replace wrpc.io/go v0.1.0 => ../../go
