package main

import (
	"fmt"
	"log"
	"log/slog"
	"os"

	"github.com/nats-io/nats.go"
	app "wrpc.io/examples/go/wasi-keyvalue-client"
	wrpcnats "wrpc.io/go/nats"
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
	prefixes := os.Args[1:]
	if len(prefixes) == 0 {
		prefixes = []string{"go"}
	}
	for _, prefix := range prefixes {
		client := wrpcnats.NewClient(nc, wrpcnats.WithPrefix(prefix))
		if err := app.Run(prefix, client); err != nil {
			return err
		}
	}
	return nil
}

func main() {
	if err := run(); err != nil {
		log.Fatal(err)
	}
}
