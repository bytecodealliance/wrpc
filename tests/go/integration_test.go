package integration_test

import (
	"testing"

	"github.com/nats-io/nats-server/v2/test"
	"github.com/nats-io/nats.go"
	wrpcnats "github.com/wrpc/wrpc/go/nats"
	integration "github.com/wrpc/wrpc/tests/go"
	"github.com/wrpc/wrpc/tests/go/bindings/sync_server"
)

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
	stop, err := sync_server.Serve(wrpc, integration.SyncHandler{})
	if err != nil {
		t.Errorf("failed to serve world: %s", err)
		return
	}
	if err = stop(); err != nil {
		t.Errorf("failed to stop serving world: %s", err)
		return
	}
}
