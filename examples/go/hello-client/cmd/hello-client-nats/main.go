package main

import (
	"context"
	"fmt"
	"log"
	"log/slog"
	"os"

	"github.com/nats-io/nats.go"
	"github.com/wrpc/wrpc/examples/go/hello-server/bindings/wrpc_examples/hello/handler"
	wrpcnats "github.com/wrpc/wrpc/go/nats"
)

func run() (err error) {
	nc, err := nats.Connect(nats.DefaultURL)
	if err != nil {
		return fmt.Errorf("failed to connect to NATS.io: %w", err)
	}
	defer nc.Close()
	defer func() {
		if dErr := nc.Drain(); dErr != nil {
			if err == nil {
				err = fmt.Errorf("failed to drain NATS.io connection: %w", dErr)
			} else {
				slog.Error("failed to drain NATS.io connection", "err", dErr)
			}
		}
	}()

	for _, prefix := range os.Args[1:] {
		wrpc := wrpcnats.NewClient(nc, prefix)
		greeting, err := handler.Hello(context.Background(), wrpc)
		if err != nil {
			return fmt.Errorf("failed to call `wrpc-examples:hello/handler.hello`: %w", err)
		}
		fmt.Printf("%s: %s\n", prefix, greeting)
	}
	return nil
}

func init() {
	slog.SetDefault(slog.New(slog.NewTextHandler(os.Stderr, &slog.HandlerOptions{
		Level: slog.LevelInfo, ReplaceAttr: func(groups []string, a slog.Attr) slog.Attr {
			if a.Key == slog.TimeKey {
				return slog.Attr{}
			}
			return a
		},
	})))
}

func main() {
	if err := run(); err != nil {
		log.Fatal(err)
	}
}
