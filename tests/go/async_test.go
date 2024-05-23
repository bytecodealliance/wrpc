//go:generate $WIT_BINDGEN_WRPC go --world async-client --out-dir bindings/async_client --package github.com/wrpc/wrpc/tests/go/bindings/async_client ../wit

package integration_test

import (
	"context"
	"io"
	"log/slog"
	"reflect"
	"testing"
	"time"

	"github.com/nats-io/nats.go"
	wrpcnats "github.com/wrpc/wrpc/go/nats"
	integration "github.com/wrpc/wrpc/tests/go"
	"github.com/wrpc/wrpc/tests/go/bindings/async_client/wrpc_test/integration/async"
	"github.com/wrpc/wrpc/tests/go/bindings/async_server"
	"github.com/wrpc/wrpc/tests/go/internal"
)

func TestAsync(t *testing.T) {
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

	stop, err := async_server.Serve(client, integration.AsyncHandler{})
	if err != nil {
		t.Errorf("failed to serve `async-server` world: %s", err)
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

	{
		slog.DebugContext(ctx, "calling `wrpc-test:integration/async.with-streams`")
		byteRx, stringListRx, shutdown, err := async.WithStreams(ctx, client, true)
		if err != nil {
			t.Errorf("failed to call `wrpc-test:integration/async.with-streams`: %s", err)
			return
		}
		b, err := io.ReadAll(byteRx)
		if err != nil {
			t.Errorf("failed to read from stream: %s", err)
			return
		}
		if string(b) != "test" {
			t.Errorf("expected: `test`, got: %s", string(b))
			return
		}
		ss, err := stringListRx.Receive()
		if err != nil {
			t.Errorf("failed to receive ready list<string> stream: %s", err)
			return
		}
		expected := [][]string{{"foo", "bar"}, {"baz"}}
		if !reflect.DeepEqual(ss, expected) {
			t.Errorf("expected: `%#v`, got: %#v", expected, ss)
			return
		}
		ss, err = stringListRx.Receive()
		if ss != nil || err != io.EOF {
			t.Errorf("ready list<string> should have returned (nil, io.EOF), got: (%#v, %v)", ss, err)
			return
		}
		if err := shutdown(); err != nil {
			t.Errorf("failed to shutdown: %s", err)
			return
		}
	}

	{
		slog.DebugContext(ctx, "calling `wrpc-test:integration/async.with-streams`")
		byteRx, stringListRx, shutdown, err := async.WithStreams(ctx, client, false)
		if err != nil {
			t.Errorf("failed to call `wrpc-test:integration/async.with-streams`: %s", err)
			return
		}
		b, err := io.ReadAll(byteRx)
		if err != nil {
			t.Errorf("failed to read from stream: %s", err)
			return
		}
		if string(b) != "test" {
			t.Errorf("expected: `test`, got: %s", string(b))
			return
		}
		ss, err := stringListRx.Receive()
		if err != nil {
			t.Errorf("failed to receive ready list<string> stream: %s", err)
			return
		}
		expected := [][]string{{"foo", "bar"}, {"baz"}}
		if !reflect.DeepEqual(ss, expected) {
			t.Errorf("expected: `%#v`, got: %#v", expected, ss)
			return
		}
		ss, err = stringListRx.Receive()
		if ss != nil || err != io.EOF {
			t.Errorf("ready list<string> should have returned (nil, io.EOF), got: (%#v, %v)", ss, err)
			return
		}
		if err := shutdown(); err != nil {
			t.Errorf("failed to shutdown: %s", err)
			return
		}
	}

	if err = stop(); err != nil {
		t.Errorf("failed to stop serving `async-server` world: %s", err)
		return
	}
}
