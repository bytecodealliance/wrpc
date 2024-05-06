package wrpc_test

import (
	"context"
	"testing"

	"github.com/nats-io/nats-server/v2/test"
	"github.com/nats-io/nats.go"
	wrpc "github.com/wrpc/wrpc/go"
	wrpcnats "github.com/wrpc/wrpc/go/nats"
	"github.com/wrpc/wrpc/tests/go/bindings/keyvalue_server"
	"github.com/wrpc/wrpc/tests/go/bindings/keyvalue_server/exports/wrpc/keyvalue/store"
)

type Handler struct{}

func (Handler) Delete(ctx context.Context, bucket string, key string) (*wrpc.Result[struct{}, store.Error], error) {
	panic("todo")
}

func (Handler) Exists(ctx context.Context, bucket string, key string) (*wrpc.Result[bool, store.Error], error) {
	panic("todo")
}

func (Handler) Get(ctx context.Context, bucket string, key string) (*wrpc.Result[[]uint8, store.Error], error) {
	panic("todo")
}

func (Handler) Set(ctx context.Context, bucket string, key string, value []byte) (*wrpc.Result[struct{}, store.Error], error) {
	panic("todo")
}

func (Handler) ListKeys(ctx context.Context, bucket string, cursor *uint64) (*wrpc.Result[store.KeyResponse, store.Error], error) {
	panic("todo")
}

func (Handler) Increment(ctx context.Context, bucket string, key string, delta uint64) (*wrpc.Result[uint64, store.Error], error) {
	panic("todo")
}

func TestKeyvalue(t *testing.T) {
	srv := test.RunRandClientPortServer()
	nc, err := nats.Connect(srv.ClientURL())
	if err != nil {
		t.Errorf("failed to connect to NATS.io: %s", err)
		return
	}
	defer nc.Close()
	defer func() {
		if err := nc.Drain(); err != nil {
			t.Errorf("failed to drain NATS.io connection: %s", err)
			return
		}
	}()
	wrpc := wrpcnats.NewClient(nc, "go")
	stop, err := keyvalue_server.Serve(wrpc, Handler{})
	if err != nil {
		t.Errorf("failed to serve world: %s", err)
		return
	}
	if err = stop(); err != nil {
		t.Errorf("failed to stop serving world: %s", err)
		return
	}
}
