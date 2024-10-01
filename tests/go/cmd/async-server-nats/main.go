package main

import (
	"fmt"
	"log"
	"log/slog"
	"os"
	"os/signal"
	"syscall"

	"github.com/lmittmann/tint"
	"github.com/nats-io/nats.go"
	wrpcnats "wrpc.io/go/nats"
	integration "wrpc.io/tests/go"
	"wrpc.io/tests/go/bindings/async_server"
)

func run(url string) error {
	nc, err := nats.Connect(url)
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

	client := wrpcnats.NewClient(nc, wrpcnats.WithPrefix("go"))
	var h integration.AsyncHandler
	stop, err := async_server.Serve(client, h)
	if err != nil {
		return fmt.Errorf("failed to serve world: %w", err)
	}

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
