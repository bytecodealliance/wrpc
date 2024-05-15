//go:generate $WIT_BINDGEN_WRPC go --world resources-client --out-dir bindings/resources_client --package github.com/wrpc/wrpc/tests/go/bindings/resources_client ../wit

package integration_test

import (
	"context"
	"testing"
	"time"

	"github.com/nats-io/nats.go"
	wrpcnats "github.com/wrpc/wrpc/go/nats"
	"github.com/wrpc/wrpc/tests/go/internal"
	// TODO: Implement resource support
	// integration "github.com/wrpc/wrpc/tests/go"
)

func TestResources(t *testing.T) {
	natsSrv := internal.RunNats(t)
	nc, err := nats.Connect(natsSrv.ClientURL())
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
	_ = wrpcnats.NewClient(nc, "go")

	// TODO: Implement resource support
	//stop, err := resources_server.Serve(client, integration.ResourcesHandler{})
	//if err != nil {
	//	t.Errorf("failed to serve `async-server` world: %s", err)
	//	return
	//}

	var cancel func()
	ctx := context.Background()
	dl, ok := t.Deadline()
	if ok {
		ctx, cancel = context.WithDeadline(ctx, dl)
	} else {
		ctx, cancel = context.WithTimeout(ctx, time.Minute)
	}
	defer cancel()

	//if err = stop(); err != nil {
	//	t.Errorf("failed to stop serving `async-server` world: %s", err)
	//	return
	//}
}
