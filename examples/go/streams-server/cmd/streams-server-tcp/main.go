package main

import (
	"context"
	"errors"
	"fmt"
	"io"
	"log"
	"log/slog"
	"net"
	"os"
	"os/signal"
	"syscall"

	app "wrpc.io/examples/go/streams-server"
	server "wrpc.io/examples/go/streams-server/bindings"
	wrpc "wrpc.io/go"
)

func serve(ctx context.Context, conn net.Conn, h app.Handler) {
	defer conn.Close()
	lis := wrpc.NewListener(conn, conn)
	for {
		hdr, c, err := lis.Accept(ctx)
		if err == io.EOF {
			return
		}
		if err != nil {
			slog.Error("failed to accept invocation", "err", err)
			return
		}
		ok, err := server.Handle(ctx, hdr, c, h)
		if !ok {
			slog.Error("received an invocation of an unknown function", "instance", hdr.Instance, "name", hdr.Name)
			return
		}
		if err != nil {
			slog.Error("failed to serve invocation", "err", err)
			return
		}
	}
}

func run() error {
	l, err := net.Listen("tcp", "[::1]:7761")
	if err != nil {
		return fmt.Errorf("failed to listen on TCP socket: %w", err)
	}

	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	h := app.Handler{}
	go func() {
		for {
			conn, err := l.Accept()
			if err != nil {
				if !errors.Is(err, net.ErrClosed) {
					slog.Error("failed to accept connection", "err", err)
				}
				return
			}
			go serve(ctx, conn, h)
		}
	}()

	signalCh := make(chan os.Signal, 1)
	signal.Notify(signalCh, syscall.SIGINT)
	<-signalCh

	return l.Close()
}

func main() {
	if err := run(); err != nil {
		log.Fatal(err)
	}
}
