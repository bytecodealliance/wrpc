package internal

import (
	"context"
	"errors"
	"log/slog"
	"net"
	"os"
	"testing"

	"github.com/lmittmann/tint"
	wrpctcp "wrpc.io/go/x/tcp"
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

// RunTCP sets up an in-process wRPC TCP transport bound to a loopback address.
// It returns a server, on which worlds can be served, and an invoker connected
// to it. The listener is closed automatically when the test completes.
func RunTCP(t *testing.T, ctx context.Context) (*wrpctcp.Server, *wrpctcp.Invoker) {
	t.Helper()
	a, err := net.ResolveTCPAddr("tcp", "[::1]:0")
	if err != nil {
		t.Fatalf("failed to resolve TCP address: %s", err)
	}
	l, err := net.ListenTCP("tcp", a)
	if err != nil {
		t.Fatalf("failed to listen on TCP socket: %s", err)
	}
	t.Cleanup(func() {
		l.Close()
	})
	srv := wrpctcp.NewServerWithContext(ctx, l)
	return srv, wrpctcp.NewInvoker(l.Addr().String())
}

// Accept runs the server's accept loop in a goroutine until the listener is
// closed or the context is cancelled.
func Accept(ctx context.Context, srv *wrpctcp.Server) {
	go func() {
		for {
			select {
			case <-ctx.Done():
				return
			default:
				if err := srv.Accept(); err != nil {
					if !errors.Is(err, net.ErrClosed) {
						slog.Error("failed to accept invocation", "err", err)
					}
					return
				}
			}
		}
	}()
}
