package internal

import (
	"context"
	"fmt"
	"io"
	"log/slog"
	"net"
	"os"
	"testing"

	"github.com/lmittmann/tint"
	wrpc "wrpc.io/go"
)

func init() {
	slog.SetDefault(slog.New(tint.NewHandler(os.Stderr, &tint.Options{
		AddSource: true,
		Level:     slog.LevelDebug,
		ReplaceAttr: func(groups []string, a slog.Attr) slog.Attr {
			if a.Key == slog.TimeKey {
				return slog.Attr{}
			}
			return a
		},
	})))
}

// Handle serves an invocation with header `hdr` over `c`, it returns false if the function is not handled
type Handle func(ctx context.Context, hdr wrpc.Header, c *wrpc.Framed) (bool, error)

// ServeConn serves the invocations sent over `conn` one after another using `handle` until `conn` ends or an invocation fails
func ServeConn(ctx context.Context, conn net.Conn, handle Handle) error {
	defer conn.Close()
	lis := wrpc.NewListener(conn, conn)
	for {
		hdr, c, err := lis.Accept(ctx)
		if err == io.EOF {
			return nil
		}
		if err != nil {
			return err
		}
		ok, err := handle(ctx, hdr, c)
		if !ok {
			return fmt.Errorf("function `%s#%s` is not handled", hdr.Instance, hdr.Name)
		}
		if err != nil {
			return err
		}
	}
}

// Serve serves the connections accepted on a loopback TCP listener using `handle` until the test completes.
// It returns a function connecting to the listener.
func Serve(t *testing.T, ctx context.Context, handle Handle) func() *wrpc.Framed {
	t.Helper()
	l, err := net.Listen("tcp", "[::1]:0")
	if err != nil {
		t.Fatalf("failed to listen on TCP socket: %s", err)
	}
	t.Cleanup(func() { l.Close() })
	go func() {
		for {
			conn, err := l.Accept()
			if err != nil {
				return
			}
			go func() {
				if err := ServeConn(ctx, conn, handle); err != nil {
					slog.ErrorContext(ctx, "failed to serve connection", "err", err)
				}
			}()
		}
	}()
	return func() *wrpc.Framed {
		t.Helper()
		conn, err := net.Dial("tcp", l.Addr().String())
		if err != nil {
			t.Fatalf("failed to connect: %s", err)
		}
		t.Cleanup(func() { conn.Close() })
		return wrpc.NewFramed(conn, conn)
	}
}

// Ready returns a closed channel, which contains `v`
func Ready[T any](v T) <-chan T {
	ch := make(chan T, 1)
	ch <- v
	close(ch)
	return ch
}
