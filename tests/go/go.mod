module wrpc.io/tests/go

go 1.24.0

require (
	github.com/google/uuid v1.6.0
	github.com/lmittmann/tint v1.1.2
	github.com/nats-io/nats-server/v2 v2.11.7
	github.com/nats-io/nats.go v1.47.0
	wrpc.io/go v0.2.0
)

require github.com/google/go-tpm v0.9.5 // indirect

require (
	github.com/davecgh/go-spew v1.1.1 // indirect
	github.com/klauspost/compress v1.18.2 // indirect
	github.com/minio/highwayhash v1.0.3 // indirect
	github.com/nats-io/jwt/v2 v2.7.4 // indirect
	github.com/nats-io/nkeys v0.4.12 // indirect
	github.com/nats-io/nuid v1.0.1 // indirect
	github.com/pmezard/go-difflib v1.0.0 // indirect
	github.com/stretchr/testify v1.9.0
	golang.org/x/crypto v0.45.0 // indirect
	golang.org/x/sys v0.39.0 // indirect
	golang.org/x/time v0.12.0 // indirect
	gopkg.in/yaml.v3 v3.0.1 // indirect
)

replace wrpc.io/go v0.2.0 => ../../go
