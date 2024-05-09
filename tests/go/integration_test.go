package integration_test

import (
	"context"
	"testing"
	"time"

	"github.com/nats-io/nats-server/v2/test"
	"github.com/nats-io/nats.go"
	wrpcnats "github.com/wrpc/wrpc/go/nats"
	integration "github.com/wrpc/wrpc/tests/go"
	"github.com/wrpc/wrpc/tests/go/bindings/sync_client/foo"
	"github.com/wrpc/wrpc/tests/go/bindings/sync_server"
)

func TestSync(t *testing.T) {
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
	client := wrpcnats.NewClient(nc, "go")

	stop, err := sync_server.Serve(client, integration.SyncHandler{})
	if err != nil {
		t.Errorf("failed to serve `sync-server` world: %s", err)
		return
	}

	var cancel func()
	ctx := context.Background()
	dl, ok := t.Deadline()
	if ok {
		ctx, cancel = context.WithDeadline(ctx, dl)
	} else {
		ctx, cancel = context.WithTimeout(ctx, time.Minute)
	}
	defer cancel()
	v, err := foo.F(ctx, client, "f")
	if err != nil {
		t.Errorf("failed to call `wrpc-test:integration/sync-client.foo.f`: %s", err)
		return
	}
	if v != 42 {
		t.Errorf("expected: 42, got: %d", v)
		return
	}

	if err = stop(); err != nil {
		t.Errorf("failed to stop serving `sync-server` world: %s", err)
		return
	}
}
