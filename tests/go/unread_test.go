package integration_test

import (
	"bytes"
	"context"
	"log/slog"
	"os"
	"testing"
	"time"

	"github.com/stretchr/testify/require"
	integration "wrpc.io/tests/go"
	"wrpc.io/tests/go/bindings/async_client/wrpc_test/integration/async"
	"wrpc.io/tests/go/bindings/async_server"
	server "wrpc.io/tests/go/bindings/async_server/exports/wrpc_test/integration/async"
	"wrpc.io/tests/go/internal"
)

type unreadHandler struct {
	integration.AsyncHandler
}

func (unreadHandler) WithFuture(context.Context, *server.Something, <-chan []byte) (<-chan <-chan []byte, error) {
	return internal.Ready(internal.Ready([]byte("test"))), nil
}

func TestUnreadStream(t *testing.T) {
	logger := slog.Default()
	slog.SetDefault(slog.New(slog.NewTextHandler(os.Stderr, &slog.HandlerOptions{Level: slog.LevelInfo})))
	t.Cleanup(func() { slog.SetDefault(logger) })

	ctx, cancel := context.WithTimeout(context.Background(), time.Minute)
	defer cancel()

	srv, client := internal.RunTCP(t, ctx)
	stop, err := async_server.Serve(srv, unreadHandler{})
	require.NoError(t, err, "failed to serve `async-server` world")
	defer stop()
	internal.Accept(ctx, srv)

	s := make(chan []byte)
	go func() {
		defer close(s)
		chunk := bytes.Repeat([]byte{0x2a}, 1<<20)
		for range 256 {
			select {
			case s <- chunk:
			case <-ctx.Done():
				return
			}
		}
	}()
	fut, errCh, err := async.WithFuture(ctx, client, &async.Something{Foo: "bar"}, s)
	require.NoError(t, err, "failed to call `wrpc-test:integration/async.with-future`")
	var b []byte
	for chunk := range <-fut {
		b = append(b, chunk...)
	}
	require.Equal(t, "test", string(b))

	timeout := time.After(30 * time.Second)
	var errs []error
	for {
		select {
		case err, ok := <-errCh:
			if !ok {
				require.NotEmpty(t, errs, "expected the unread stream to fail the invocation")
				return
			}
			errs = append(errs, err)
		case <-timeout:
			t.Fatal("invocation did not complete")
		}
	}
}
