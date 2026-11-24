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

func serveAsync(t *testing.T, h server.Handler) (context.Context, func() *wrpc.Framed) {
	t.Helper()
	ctx, cancel := context.WithTimeout(context.Background(), time.Minute)
	t.Cleanup(cancel)
	return ctx, internal.Serve(t, ctx, func(ctx context.Context, hdr wrpc.Header, c *wrpc.Framed) (bool, error) {
		return async_server.Handle(ctx, hdr, c, h)
	})
}

func wait(inv *wrpc.Invocation) <-chan error {
	done := make(chan error, 1)
	go func() { done <- inv.Wait() }()
	return done
}

func TestAsync(t *testing.T) {
	ctx, connect := serveAsync(t, integration.AsyncHandler{})
	client := connect()

	t.Run("with-streams", func(t *testing.T) {
		byteRx, stringListRx, inv, err := async.WithStreams(ctx, client)
		require.NoError(t, err)
		done := wait(inv)
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
		require.NoError(t, <-done)
	})

	t.Run("with-streams-cancel", func(t *testing.T) {
		callCtx, callCancel := context.WithCancel(ctx)
		_, stringListRx, inv, err := async.WithStreams(callCtx, connect())
		require.NoError(t, err)
		done := wait(inv)
		for range stringListRx {
		}
		callCancel()
		select {
		case <-done:
		case <-time.After(5 * time.Second):
			t.Fatal("invocation did not complete after cancellation")
		}
	})

	t.Run("identity-nested-async", func(t *testing.T) {
		stream := make(chan []string, 3)
		stream <- []string{"foo"}
		stream <- []string{}
		stream <- []string{"bar", "baz"}
		close(stream)
		fut0, inv, err := async.IdentityNestedAsync(ctx, client,
			internal.Ready(internal.Ready(internal.Ready((<-chan []string)(stream)))),
		)
		require.NoError(t, err)
		done := wait(inv)
		rx := <-<-<-fut0
		require.NotNil(t, rx)
		var ss []string
		for chunk := range rx {
			ss = append(ss, chunk...)
		}
		assert.Equal(t, []string{"foo", "bar", "baz"}, ss)
		require.NoError(t, <-done)
	})

	t.Run("identity-future", func(t *testing.T) {
		ch := make(chan struct{}, 1)
		fut, inv, err := async.IdentityFuture(ctx, client, ch)
		require.NoError(t, err)
		done := wait(inv)
		select {
		case <-fut:
			t.Fatal("future resolved before its parameter")
		case <-time.After(100 * time.Millisecond):
		}
		ch <- struct{}{}
		close(ch)
		_, ok := <-fut
		require.True(t, ok, "future closed without resolving")
		require.NoError(t, <-done)

		fut, inv, err = async.IdentityFuture(ctx, client, internal.Ready(struct{}{}))
		require.NoError(t, err)
		select {
		case _, ok := <-fut:
			require.True(t, ok, "future closed without resolving")
		default:
			t.Fatal("expected a ready future")
		}
		require.NoError(t, inv.Wait())
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

func invokeTruncatedStream(t *testing.T, c *wrpc.Framed) {
	t.Helper()
	require.NoError(t, c.W.WriteHeader(wrpc.Header{Instance: "wrpc-test:integration/async", Name: "with-future"}))
	require.NoError(t, c.W.WriteFrame(nil, []byte{0x03, 'b', 'a', 'r', 0x00}))
	require.NoError(t, c.W.WriteFrame(nil, nil))
	require.NoError(t, c.W.WriteFrame([]uint32{1}, []byte{0x02, 'a', 'b', 0x04, 'c'}))
	require.NoError(t, c.W.WriteFrame([]uint32{1}, nil))
}

func TestAsyncTruncatedStream(t *testing.T) {
	received := make(chan []byte, 1)
	cause := make(chan error, 1)
	ctx, connect := serveAsync(t, truncatedStreamHandler{received: received, cause: cause})
	invokeTruncatedStream(t, connect())
	select {
	case b := <-received:
		assert.True(t, strings.HasPrefix("abc", string(b)), "unexpected received data %q", b)
	case <-ctx.Done():
		t.Fatal("handler did not observe the truncated stream")
	}
	assert.ErrorIs(t, <-cause, io.ErrUnexpectedEOF)
}

func TestAsyncEchoTruncatedStream(t *testing.T) {
	ctx, connect := serveAsync(t, integration.AsyncHandler{})
	c := connect()
	invokeTruncatedStream(t, c)
	res, rx, err := wrpc.Receive(c.R, wrpc.Tuple1Codec[<-chan <-chan []byte]{C0: wrpc.FutureCodec[<-chan []byte]{Elem: wrpc.ByteStreamCodec{}}})
	if err != nil {
		var rerr *wrpc.ReceiveError
		require.ErrorAs(t, err, &rerr)
		return
	}
	done := make(chan error, 1)
	go func() { done <- rx.Wait(ctx) }()
	fut := res.V0
	var s <-chan []byte
	var b []byte
	for {
		select {
		case s = <-fut:
			fut = nil
		case chunk, ok := <-s:
			if !ok {
				s = nil
				continue
			}
			b = append(b, chunk...)
		case err := <-done:
			require.Error(t, err, "expected the echoed stream to fail")
			require.NoError(t, ctx.Err(), "echoed stream did not terminate")
			assert.True(t, strings.HasPrefix("abc", string(b)), "unexpected echoed data %q", b)
			return
		}
	}
}
