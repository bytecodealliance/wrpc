//go:generate $WIT_BINDGEN_WRPC go --world async-client --out-dir bindings/async_client --package wrpc.io/tests/go/bindings/async_client ../wit

package integration_test

import (
	"context"
	"errors"
	"io"
	"log/slog"
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

	t.Run("identity-future", func(t *testing.T) {
		slog.DebugContext(ctx, "calling `wrpc-test:integration/async.identity-future`")
		ch := make(chan struct{}, 1)
		fut, errCh, err := async.IdentityFuture(ctx, client, ch)
		require.NoError(t, err, "failed to call `wrpc-test:integration/async.identity-future`")
		select {
		case <-fut:
			t.Fatal("future resolved before its parameter")
		case <-time.After(100 * time.Millisecond):
		}
		ch <- struct{}{}
		select {
		case _, ok := <-fut:
			require.True(t, ok, "future closed without resolving")
		case <-ctx.Done():
			t.Fatal("future did not resolve")
		}
		for err := range errCh {
			require.NoError(t, err, "failed to exchange async values with peer")
		}

		fut, errCh, err = async.IdentityFuture(ctx, client, internal.Ready(struct{}{}))
		require.NoError(t, err, "failed to call `wrpc-test:integration/async.identity-future`")
		select {
		case _, ok := <-fut:
			require.True(t, ok, "future closed without resolving")
		default:
			t.Fatal("expected a ready future")
		}
		for err := range errCh {
			require.NoError(t, err, "failed to exchange async values with peer")
		}
	})

	if err = stop(); err != nil {
		t.Errorf("failed to stop serving `async-server` world: %s", err)
		return
	}
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

func TestAsyncTruncatedStream(t *testing.T) {
	ctx, cancel := context.WithTimeout(context.Background(), time.Minute)
	defer cancel()

	srv, client := internal.RunTCP(t, ctx)
	received := make(chan []byte, 1)
	cause := make(chan error, 1)
	stop, err := async_server.Serve(srv, truncatedStreamHandler{
		received: received,
		cause:    cause,
	})
	require.NoError(t, err, "failed to serve `async-server` world")
	defer stop()
	internal.Accept(ctx, srv)

	w, r, err := client.Invoke(ctx, "wrpc-test:integration/async", "with-future", []byte{0x03, 'b', 'a', 'r', 0x00})
	require.NoError(t, err, "failed to invoke `wrpc-test:integration/async.with-future`")
	defer r.Close()
	sw, err := w.Index(1)
	require.NoError(t, err, "failed to index stream writer")
	_, err = sw.Write([]byte{0x02, 'a', 'b', 0x04, 'c'})
	require.NoError(t, err, "failed to write truncated stream")
	require.NoError(t, sw.Close(), "failed to close stream writer")
	require.NoError(t, w.Close(), "failed to close writer")

	select {
	case b := <-received:
		assert.Equal(t, "ab", string(b))
	case <-ctx.Done():
		t.Fatal("handler did not observe the truncated stream")
	}
	err = <-cause
	assert.ErrorIs(t, err, io.ErrUnexpectedEOF)
}

func TestAsyncEchoTruncatedStream(t *testing.T) {
	ctx, cancel := context.WithTimeout(context.Background(), time.Minute)
	defer cancel()

	srv, client := internal.RunTCP(t, ctx)
	stop, err := async_server.Serve(srv, integration.AsyncHandler{})
	require.NoError(t, err, "failed to serve `async-server` world")
	defer stop()
	internal.Accept(ctx, srv)

	w, r, err := client.Invoke(ctx, "wrpc-test:integration/async", "with-future", []byte{0x03, 'b', 'a', 'r', 0x00}, wrpc.NewSubscribePath().Index(0))
	require.NoError(t, err, "failed to invoke `wrpc-test:integration/async.with-future`")
	defer r.Close()
	sw, err := w.Index(1)
	require.NoError(t, err, "failed to index stream writer")
	_, err = sw.Write([]byte{0x02, 'a', 'b', 0x04, 'c'})
	require.NoError(t, err, "failed to write truncated stream")
	require.NoError(t, sw.Close(), "failed to close stream writer")
	require.NoError(t, w.Close(), "failed to close writer")

	fut, deferred, err := wrpc.ReadFuture(r, func(r wrpc.IndexReadCloser, path ...uint32) (<-chan []byte, func(context.Context) error, error) {
		return wrpc.ReadByteStream(r, path...)
	}, 0)
	require.NoError(t, err, "failed to read result")
	require.NotNil(t, deferred, "expected a pending result")
	done := make(chan error, 1)
	go func() {
		done <- deferred(ctx)
	}()
	var s <-chan []byte
	select {
	case s = <-fut:
	case err := <-done:
		require.Error(t, err, "result future resolved without a value")
		return
	case <-ctx.Done():
		t.Fatal("result future did not resolve")
	}
	var b []byte
	for {
		select {
		case chunk, ok := <-s:
			if !ok {
				s = nil
			}
			b = append(b, chunk...)
			continue
		case err := <-done:
			require.Error(t, err, "expected the echoed stream to fail")
		case <-ctx.Done():
			t.Fatal("echoed stream did not terminate")
		}
		break
	}
	assert.True(t, strings.HasPrefix("ab", string(b)), "unexpected echoed data %q", b)
}
