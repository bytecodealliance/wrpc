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

	app "wrpc.io/examples/go/wasi-keyvalue-server"
	server "wrpc.io/examples/go/wasi-keyvalue-server/bindings"
	wrpctcp "wrpc.io/go/x/tcp"
)

func run() error {
	l, err := net.ListenTCP("tcp", &net.TCPAddr{
		IP:   net.IPv6loopback,
		Port: 7761,
	})
	if err != nil {
		return fmt.Errorf("failed to listen on TCP socket: %w", err)
	}

	ctx := context.Background()
	ctx, cancel := context.WithCancel(ctx)
	defer cancel()

	srv := wrpctcp.NewServerWithContext(ctx, l)
	stop, err := server.Serve(srv, &app.Handler{})
	if err != nil {
		return fmt.Errorf("failed to serve `server` world: %w", err)
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
		return fmt.Errorf("failed to stop `server` world: %w", err)
	}
	return nil
}

func main() {
	if err := run(); err != nil {
		log.Fatal(err)
	}
}
