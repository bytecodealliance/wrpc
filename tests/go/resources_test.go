//go:generate $WIT_BINDGEN_WRPC go --world resources-client --out-dir bindings/resources_client --package github.com/wrpc/wrpc/tests/go/bindings/resources_client ../wit

package integration_test

import (
	"context"
	"log/slog"
	"testing"
	"time"

	"github.com/nats-io/nats.go"
	wrpc "github.com/wrpc/wrpc/go"
	wrpcnats "github.com/wrpc/wrpc/go/nats"
	integration "github.com/wrpc/wrpc/tests/go"
	"github.com/wrpc/wrpc/tests/go/bindings/resources_client/strange"
	"github.com/wrpc/wrpc/tests/go/bindings/resources_client/wrpc_test/integration/resources"
	"github.com/wrpc/wrpc/tests/go/bindings/resources_server"
	"github.com/wrpc/wrpc/tests/go/internal"
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
	client := wrpcnats.NewClient(nc, "go")

	stop, err := resources_server.Serve(client, &integration.ResourcesHandler{}, integration.ResourcesStrangeHandler{})
	if err != nil {
		t.Errorf("failed to serve `resources-server` world: %s", err)
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

	var foo wrpc.Own[resources.Foo]
	{
		slog.DebugContext(ctx, "calling `wrpc-test:integration/resources.[constructor]foo`")
		v, shutdown, err := resources.NewFoo(context.Background(), client)
		if err != nil {
			t.Errorf("failed to call `wrpc-test:integration/resources.[constructor]foo`: %s", err)
			return
		}
		if err := shutdown(); err != nil {
			t.Errorf("failed to shutdown: %s", err)
			return
		}
		foo = v
	}
	if err := foo.Drop(ctx, client); err != nil {
		t.Errorf("failed to drop `foo` resource: %s", err)
		return
	}

	{
		slog.DebugContext(ctx, "calling `wrpc-test:integration/resources.[constructor]foo`")
		v, shutdown, err := resources.NewFoo(context.Background(), client)
		if err != nil {
			t.Errorf("failed to call `wrpc-test:integration/resources.[constructor]foo`: %s", err)
			return
		}
		if err := shutdown(); err != nil {
			t.Errorf("failed to shutdown: %s", err)
			return
		}
		foo = v
	}

	{
		slog.DebugContext(ctx, "calling `wrpc-test:integration/resources.[method]foo.bar`")
		v, shutdown, err := resources.FooBar(context.Background(), client, foo.Borrow())
		if err != nil {
			t.Errorf("failed to call `wrpc-test:integration/resources.[method]foo.bar`: %s", err)
			return
		}
		if v != "bar" {
			t.Errorf("expected: `bar`, got: %s", v)
			return
		}
		if err := shutdown(); err != nil {
			t.Errorf("failed to shutdown: %s", err)
			return
		}
	}

	{
		slog.DebugContext(ctx, "calling `wrpc-test:integration/resources.bar`")
		v, shutdown, err := resources.Bar(context.Background(), client, foo.Borrow())
		if err != nil {
			t.Errorf("failed to call `wrpc-test:integration/resources.bar`: %s", err)
			return
		}
		if v != "bar" {
			t.Errorf("expected: `bar`, got: %s", v)
			return
		}
		if err := shutdown(); err != nil {
			t.Errorf("failed to shutdown: %s", err)
			return
		}
	}

	{
		slog.DebugContext(ctx, "calling `wrpc-test:integration/strange.bar`")
		v, shutdown, err := strange.Bar(context.Background(), client, foo.Borrow())
		if err != nil {
			t.Errorf("failed to call `wrpc-test:integration/strange.bar`: %s", err)
			return
		}
		if v != 42 {
			t.Errorf("expected: `bar`, got: %v", v)
			return
		}
		if err := shutdown(); err != nil {
			t.Errorf("failed to shutdown: %s", err)
			return
		}
	}

	{
		slog.DebugContext(ctx, "calling `wrpc-test:integration/resources.[static]foo.foo`")
		v, shutdown, err := resources.FooFoo(context.Background(), client, foo)
		if err != nil {
			t.Errorf("failed to call `wrpc-test:integration/resources.bar`: %s", err)
			return
		}
		if v != "foo" {
			t.Errorf("expected: `foo`, got: %s", v)
			return
		}
		if err := shutdown(); err != nil {
			t.Errorf("failed to shutdown: %s", err)
			return
		}
	}

	if err := foo.Drop(ctx, client); err == nil {
		t.Errorf("`foo` resource did not get dropped by moving")
		return
	}

	if err = stop(); err != nil {
		t.Errorf("failed to stop serving `resources-server` world: %s", err)
		return
	}
}
