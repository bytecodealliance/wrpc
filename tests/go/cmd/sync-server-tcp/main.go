package main

import (
	"context"
	"fmt"
	"log"
	"log/slog"
	"net"
	"os"

	wrpc "wrpc.io/go"
	integration "wrpc.io/tests/go"
	"wrpc.io/tests/go/bindings/sync_server"
	"wrpc.io/tests/go/internal"
)

func run(addr string) error {
	l, err := net.Listen("tcp", addr)
	if err != nil {
		return fmt.Errorf("failed to listen on TCP socket: %w", err)
	}
	defer l.Close()

	ctx := context.Background()
	var h integration.SyncHandler
	rh := &integration.ResourcesHandler{}
	handle := func(ctx context.Context, hdr wrpc.Header, c *wrpc.Framed) (bool, error) {
		return sync_server.Handle(ctx, hdr, c, h, h, h, rh, integration.StrangeHandler{Resources: rh})
	}
	for {
		conn, err := l.Accept()
		if err != nil {
			return fmt.Errorf("failed to accept connection: %w", err)
		}
		if err := conn.(*net.TCPConn).SetLinger(0); err != nil {
			return fmt.Errorf("failed to set linger: %w", err)
		}
		go func() {
			if err := internal.ServeConn(ctx, conn, handle); err != nil {
				slog.ErrorContext(ctx, "failed to serve connection", "err", err)
			}
		}()
	}
}

func main() {
	if err := run(os.Args[1]); err != nil {
		log.Fatal(err)
	}
}
