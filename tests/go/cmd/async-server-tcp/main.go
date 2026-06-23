package main

import (
	"context"
	"errors"
	"fmt"
	"log"
	"log/slog"
	"net"
	"os"
	"os/signal"
	"syscall"

	"github.com/lmittmann/tint"
	wrpctcp "wrpc.io/go/x/tcp"
	integration "wrpc.io/tests/go"
	"wrpc.io/tests/go/bindings/async_server"
)

func run(addr string) (err error) {
	a, err := net.ResolveTCPAddr("tcp", addr)
	if err != nil {
		return fmt.Errorf("failed to resolve `%s` as TCP address: %w", addr, err)
	}
	l, err := net.ListenTCP("tcp", a)
	if err != nil {
		return fmt.Errorf("failed to listen on TCP socket: %w", err)
	}

	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	srv := wrpctcp.NewServerWithContext(ctx, l)
	var h integration.AsyncHandler
	stop, err := async_server.Serve(srv, h)
	if err != nil {
		return fmt.Errorf("failed to serve world: %w", err)
	}

	go func() {
		for {
			select {
			case <-ctx.Done():
				return
			default:
				slog.Debug("accepting invocation")
				if err := srv.Accept(); err != nil {
					if !errors.Is(err, net.ErrClosed) {
						slog.Error("failed to accept invocation", "err", err)
					}
				}
			}
		}
	}()

	signalCh := make(chan os.Signal, 1)
	signal.Notify(signalCh, syscall.SIGINT)
	<-signalCh

	if err = stop(); err != nil {
		return fmt.Errorf("failed to stop serving world: %w", err)
	}
	return nil
}

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

func main() {
	if err := run(os.Args[1]); err != nil {
		log.Fatal(err)
	}
}
