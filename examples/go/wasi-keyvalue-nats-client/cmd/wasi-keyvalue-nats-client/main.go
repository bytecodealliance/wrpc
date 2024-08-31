package main

import (
	"context"
	"fmt"
	"log"
	"log/slog"
	"os"

	"github.com/nats-io/nats.go"
	"wrpc.io/examples/go/wasi-keyvalue-nats-client/bindings/wasi/keyvalue/store"
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

	for _, prefix := range os.Args[1:] {
		client := wrpcnats.NewClient(nc, wrpcnats.WithPrefix(prefix))
		result, err := store.Open(context.Background(), client, "example")
		if err != nil || result.Err != nil {
			return fmt.Errorf("failed to call `wrpc-examples:keyvalue/store.open`: %w", err)
		}
		bucket := result.Ok

		_, err = store.Bucket_Set(context.Background(), client, bucket.Borrow(), "foo", []byte("bar"))
		if err != nil {
			return fmt.Errorf("failed to call `wrpc-examples:keyvalue/store.bucket.set`: %w", err)
		}
		exist, err := store.Bucket_Exists(context.Background(), client, bucket.Borrow(), "foo")
		if err != nil {
			return fmt.Errorf("failed to call `wrpc-examples:keyvalue/store.bucket.exists`: %w", err)
		} else {
			fmt.Printf("%s exists: %t\n", prefix, *exist.Ok)
		}
		value, err := store.Bucket_Get(context.Background(), client, bucket.Borrow(), "foo")
		if err != nil {
			return fmt.Errorf("failed to call `wrpc-examples:keyvalue/store.bucket.get`: %w", err)
		} else {
			fmt.Printf("%s get: %s\n", prefix, *value.Ok)
		}
		keys, err := store.Bucket_ListKeys(context.Background(), client, bucket.Borrow(), nil)
		if err != nil {
			return fmt.Errorf("failed to call `wrpc-examples:keyvalue/store.bucket.list-keys`: %w", err)
		} else {
			fmt.Printf("%s keys: %v\n", prefix, (*keys.Ok).Keys)
		}
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
