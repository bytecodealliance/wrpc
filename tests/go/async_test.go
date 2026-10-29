//go:generate $WIT_BINDGEN_WRPC go --world async-client --out-dir bindings/async_client --package wrpc.io/tests/go/bindings/async_client ../wit

package integration_test

import (
	"context"
	"errors"
	"io"
	"strings"
	"testing"
	"time"

	"github.com/stretchr/testify/assert"
	"github.com/stretchr/testify/require"
	wrpc "wrpc.io/go"
	integration "wrpc.io/tests/go"
	"wrpc.io/tests/go/bindings/async_client/wrpc_test/integration/async"
	"wrpc.io/tests/go/bindings/async_server"
	server "wrpc.io/tests/go/bindings/async_server/exports/wrpc_test/integration/async"
	"wrpc.io/tests/go/internal"
)

func serveAsync(t *testing.T, h server.Handler) (context.Context, wrpc.Invoker) {
	return serve(t, func(s wrpc.Server) (func() error, error) {
		return async_server.Serve(s, h)
	})
}

func TestAsync(t *testing.T) {
	ctx, client := serveAsync(t, integration.AsyncHandler{})

	t.Run("with-streams", func(t *testing.T) {
		byteRx, stringListRx, errCh, err := async.WithStreams(ctx, client)
		require.NoError(t, err)
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
			require.NoError(t, err)
		}
	})

	t.Run("with-streams-cancel", func(t *testing.T) {
		callCtx, callCancel := context.WithCancel(ctx)
		_, stringListRx, errCh, err := async.WithStreams(callCtx, client)
		require.NoError(t, err)
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
		stream := make(chan []string, 3)
		stream <- []string{"foo"}
		stream <- []string{}
		stream <- []string{"bar", "baz"}
		close(stream)
		fut0, errCh, err := async.IdentityNestedAsync(ctx, client,
			internal.Ready(internal.Ready(internal.Ready((<-chan []string)(stream)))),
		)
		require.NoError(t, err)
		rx := <-<-<-fut0
		require.NotNil(t, rx)
		var ss []string
		for chunk := range rx {
			ss = append(ss, chunk...)
		}
		assert.Equal(t, []string{"foo", "bar", "baz"}, ss)
		for err := range errCh {
			require.NoError(t, err)
		}
	})

	t.Run("identity-future", func(t *testing.T) {
		ch := make(chan struct{}, 1)
		fut, errCh, err := async.IdentityFuture(ctx, client, ch)
		require.NoError(t, err)
		select {
		case <-fut:
			t.Fatal("future resolved before its parameter")
		case <-time.After(100 * time.Millisecond):
		}
		ch <- struct{}{}
		close(ch)
		_, ok := <-fut
		require.True(t, ok, "future closed without resolving")
		for err := range errCh {
			require.NoError(t, err)
		}

		fut, errCh, err = async.IdentityFuture(ctx, client, internal.Ready(struct{}{}))
		require.NoError(t, err)
		select {
		case _, ok := <-fut:
			require.True(t, ok, "future closed without resolving")
		default:
			t.Fatal("expected a ready future")
		}
		for err := range errCh {
			require.NoError(t, err)
		}
	})
}

type truncatedStreamHandler struct {
	integration.AsyncHandler
	received chan<- []byte
	cause    chan<- error
}

func (h truncatedStreamHandler) WithFuture(ctx context.Context, x *server.Something, s <-chan []byte) (<-chan <-chan []byte, error) {
	var b []byte
	for {
		select {
		case chunk, ok := <-s:
			if !ok {
				h.received <- b
				h.cause <- nil
				return nil, errors.New("stream ended")
			}
			b = append(b, chunk...)
		case <-ctx.Done():
			h.received <- b
			h.cause <- context.Cause(ctx)
			return nil, context.Cause(ctx)
		}
	}
}

func invokeTruncatedStream(t *testing.T, ctx context.Context, client wrpc.Invoker, paths ...wrpc.SubscribePath) wrpc.IndexReadCloser {
	t.Helper()
	w, r, err := client.Invoke(ctx, "wrpc-test:integration/async", "with-future", []byte{0x03, 'b', 'a', 'r', 0x00}, paths...)
	require.NoError(t, err)
	t.Cleanup(func() { r.Close() })
	sw, err := w.Index(1)
	require.NoError(t, err)
	_, err = sw.Write([]byte{0x02, 'a', 'b', 0x04, 'c'})
	require.NoError(t, err)
	require.NoError(t, sw.Close())
	require.NoError(t, w.Close())
	return r
}

func TestAsyncTruncatedStream(t *testing.T) {
	received := make(chan []byte, 1)
	cause := make(chan error, 1)
	ctx, client := serveAsync(t, truncatedStreamHandler{received: received, cause: cause})
	invokeTruncatedStream(t, ctx, client)
	select {
	case b := <-received:
		assert.Equal(t, "ab", string(b))
	case <-ctx.Done():
		t.Fatal("handler did not observe the truncated stream")
	}
	assert.ErrorIs(t, <-cause, io.ErrUnexpectedEOF)
}

func TestAsyncEchoTruncatedStream(t *testing.T) {
	ctx, client := serveAsync(t, integration.AsyncHandler{})
	r := invokeTruncatedStream(t, ctx, client, wrpc.NewSubscribePath().Index(0))
	fut, deferred, err := wrpc.ReadFuture(r, wrpc.ReadByteStream, 0)
	require.NoError(t, err)
	require.NotNil(t, deferred, "expected a pending result")
	done := make(chan error, 1)
	go func() { done <- deferred(ctx) }()
	var s <-chan []byte
	var b []byte
	for {
		select {
		case s = <-fut:
			fut = nil
		case chunk := <-s:
			b = append(b, chunk...)
		case err := <-done:
			require.Error(t, err, "expected the echoed stream to fail")
			require.NoError(t, ctx.Err(), "echoed stream did not terminate")
			assert.True(t, strings.HasPrefix("ab", string(b)), "unexpected echoed data %q", b)
			return
		}
	}
}
