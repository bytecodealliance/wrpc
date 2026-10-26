//go:generate $WIT_BINDGEN_WRPC go --world async-client --out-dir bindings/async_client --package wrpc.io/tests/go/bindings/async_client ../wit

package integration_test

import (
	"context"
	"log/slog"
	"testing"
	"time"

	"github.com/stretchr/testify/assert"
	"github.com/stretchr/testify/require"
	integration "wrpc.io/tests/go"
	"wrpc.io/tests/go/bindings/async_client/wrpc_test/integration/async"
	"wrpc.io/tests/go/bindings/async_server"
	"wrpc.io/tests/go/internal"
)

func TestAsync(t *testing.T) {
	var cancel func()
	ctx := context.Background()
	dl, ok := t.Deadline()
	if ok {
		ctx, cancel = context.WithDeadline(ctx, dl)
	} else {
		ctx, cancel = context.WithTimeout(ctx, time.Minute)
	}
	defer cancel()

	srv, client := internal.RunTCP(t, ctx)

	stop, err := async_server.Serve(srv, integration.AsyncHandler{})
	if err != nil {
		t.Errorf("failed to serve `async-server` world: %s", err)
		return
	}
	internal.Accept(ctx, srv)

	t.Run("with-streams", func(t *testing.T) {
		slog.DebugContext(ctx, "calling `wrpc-test:integration/async.with-streams`")
		byteRx, stringListRx, errCh, err := async.WithStreams(ctx, client)
		require.NoError(t, err, "failed to call `wrpc-test:integration/async.with-streams`")
		var b []byte
		for chunk := range byteRx {
			b = append(b, chunk...)
		}
		assert.Equal(t, "test", string(b))
		var ss [][]string
		for chunk := range stringListRx {
			ss = append(ss, chunk...)
		}
		assert.Equal(t, [][]string{{"foo"}, {"bar", "baz"}}, ss)
		for err := range errCh {
			require.NoError(t, err, "failed to receive async value from peer")
		}
	})

	t.Run("with-streams-cancel", func(t *testing.T) {
		callCtx, callCancel := context.WithCancel(ctx)
		_, stringListRx, errCh, err := async.WithStreams(callCtx, client)
		require.NoError(t, err, "failed to call `wrpc-test:integration/async.with-streams`")
		for range stringListRx {
		}
		callCancel()
		timeout := time.After(5 * time.Second)
		for {
			select {
			case _, ok := <-errCh:
				if !ok {
					return
				}
			case <-timeout:
				t.Fatal("error channel not closed after cancellation")
			}
		}
	})

	t.Run("identity-nested-async", func(t *testing.T) {
		slog.DebugContext(ctx, "calling `wrpc-test:integration/async.identity-nested-async`")
		stream := make(chan []string, 3)
		stream <- []string{"foo"}
		stream <- []string{}
		stream <- []string{"bar", "baz"}
		close(stream)
		fut0, errCh, err := async.IdentityNestedAsync(ctx, client,
			internal.Ready(internal.Ready(internal.Ready((<-chan []string)(stream)))),
		)
		require.NoError(t, err, "failed to call `wrpc-test:integration/async.identity-nested-async`")
		fut1, ok := <-fut0
		require.True(t, ok, "failed to receive future 0")
		fut2, ok := <-fut1
		require.True(t, ok, "failed to receive future 1")
		rx, ok := <-fut2
		require.True(t, ok, "failed to receive future 2")
		var ss []string
		for chunk := range rx {
			ss = append(ss, chunk...)
		}
		assert.Equal(t, []string{"foo", "bar", "baz"}, ss)
		for err := range errCh {
			require.NoError(t, err, "failed to exchange async values with peer")
		}
	})

	if err = stop(); err != nil {
		t.Errorf("failed to stop serving `async-server` world: %s", err)
		return
	}
}
