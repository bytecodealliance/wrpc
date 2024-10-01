//go:generate $WIT_BINDGEN_WRPC go --world async-client --out-dir bindings/async_client --package wrpc.io/tests/go/bindings/async_client ../wit

package integration_test

import (
	"context"
	"io"
	"log/slog"
	"reflect"
	"testing"
	"time"

	"github.com/nats-io/nats.go"
	"github.com/stretchr/testify/assert"
	"github.com/stretchr/testify/require"
	wrpc "wrpc.io/go"
	wrpcnats "wrpc.io/go/nats"
	integration "wrpc.io/tests/go"
	"wrpc.io/tests/go/bindings/async_client/wrpc_test/integration/async"
	"wrpc.io/tests/go/bindings/async_server"
	"wrpc.io/tests/go/internal"
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
	client := wrpcnats.NewClient(nc, wrpcnats.WithPrefix("go"))

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

	t.Run("with-streams", func(t *testing.T) {
		slog.DebugContext(ctx, "calling `wrpc-test:integration/async.with-streams`")
		byteRx, stringListRx, err := async.WithStreams(ctx, client)
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
		if err := byteRx.Close(); err != nil {
			t.Errorf("failed to close byte reader: %s", err)
			return
		}

		ss, err := stringListRx.Receive()
		if err != nil {
			t.Errorf("failed to receive ready list<string> stream: %s", err)
			return
		}
		expected := [][]string{{"foo"}, {"bar", "baz"}}
		if !reflect.DeepEqual(ss, expected) {
			t.Errorf("expected: `%#v`, got: %#v", expected, ss)
			return
		}
		ss, err = stringListRx.Receive()
		if ss != nil || err != io.EOF {
			t.Errorf("ready list<string> should have returned (nil, io.EOF), got: (%#v, %v)", ss, err)
			return
		}
		if err := stringListRx.Close(); err != nil {
			t.Errorf("failed to close string list receiver: %s", err)
			return
		}
	})

	t.Run("identity-nested-async", func(t *testing.T) {
		slog.DebugContext(ctx, "calling `wrpc-test:integration/async.identity-nested-async`")
		fut0, errCh, err := async.IdentityNestedAsync(ctx, client,
			wrpc.NewNestedReceiver(wrpc.NewCompleteReceiver(
				wrpc.NewCompleteReceiver(
					wrpc.NewNestedReceiver(wrpc.NewCompleteReceiver(
						wrpc.NewCompleteReceiver(
							[]string{"foo", "bar", "baz"},
						),
					)),
				),
			)),
		)
		if err != nil {
			t.Errorf("failed to call `wrpc-test:integration/async.identity-nested-async`: %s", err)
			return
		}
		for err := range errCh {
			t.Errorf("failed to send async value to peer: %s", err)
			return
		}
		fut1, err := fut0.Receive()
		require.NoError(t, err, "failed to receive future 0")
		fut2, err := fut1.Receive()
		require.NoError(t, err, "failed to receive future 1")

		stream, err := fut2.Receive()
		require.NoError(t, err, "failed to receive future 2")

		var ss []string
		for err != io.EOF {
			var v []string
			v, err = stream.Receive()
			if err != io.EOF {
				require.NoError(t, err, "failed to receive stream element")
			}
			ss = append(ss, v...)
		}
		assert.Equal(t, ss, []string{"foo", "bar", "baz"})

		if err := fut0.Close(); err != nil {
			t.Errorf("failed to close async value receiver: %s", err)
			return
		}
	})

	if err = stop(); err != nil {
		t.Errorf("failed to stop serving `async-server` world: %s", err)
		return
	}

	time.Sleep(time.Second)
	if nc.NumSubscriptions() != 0 {
		t.Errorf("NATS subscriptions leaked: %d active after client stop", nc.NumSubscriptions())
	}
}
