package main

import (
	"fmt"
	"log"
	"log/slog"
	"os"
	"os/signal"
	"syscall"

	"github.com/nats-io/nats.go"
	app "wrpc.io/examples/go/wasi-keyvalue-server"
	server "wrpc.io/examples/go/wasi-keyvalue-server/bindings"
	wrpcnats "wrpc.io/go/nats"
)

func run() error {
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

	client := wrpcnats.NewClient(nc, wrpcnats.WithPrefix("go"))
	stop, err := server.Serve(client, &app.Handler{})
	if err != nil {
		return fmt.Errorf("failed to serve `server` world: %w", err)
	}

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
