//go:generate cargo run --bin wit-bindgen-wrpc go --out-dir bindings --package wrpc.io/examples/go/wasi-keyvalue-client/bindings wit

package app

import (
	"context"
	"errors"
	"fmt"
	"log/slog"
	"os"

	"wrpc.io/examples/go/wasi-keyvalue-client/bindings/wasi/keyvalue/store"
	wrpc "wrpc.io/go"
)

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

func Run(prefix string, client wrpc.Invoker) error {
	open, err := store.Open(context.Background(), client, "example")
	if err != nil {
		return fmt.Errorf("failed to invoke `wrpc-examples:keyvalue/store.open`: %w", err)
	}
	if open.Err != nil {
		return fmt.Errorf("failed to open `example` bucket: %w", open.Err)
	}
	bucket := open.Ok

	set, err := store.Bucket_Set(context.Background(), client, bucket.Borrow(), "foo", []byte("bar"))
	if err != nil {
		return fmt.Errorf("failed to invoke `wrpc-examples:keyvalue/store.bucket.set`: %w", err)
	}
	if set.Err != nil {
		return fmt.Errorf("failed to set `foo`: %w", set.Err)
	}

	exists, err := store.Bucket_Exists(context.Background(), client, bucket.Borrow(), "foo")
	if err != nil {
		return fmt.Errorf("failed to invoke `wrpc-examples:keyvalue/store.bucket.exists`: %w", err)
	}
	if exists.Err != nil {
		return fmt.Errorf("failed to check if `foo` exists: %w", exists.Err)
	}
	if !*exists.Ok {
		return errors.New("key `foo` does not exist in bucket")
	}

	get, err := store.Bucket_Get(context.Background(), client, bucket.Borrow(), "foo")
	if err != nil {
		return fmt.Errorf("failed to invoke `wrpc-examples:keyvalue/store.bucket.get`: %w", err)
	}
	if get.Err != nil {
		return fmt.Errorf("failed to get `foo`: %w", get.Err)
	}
	if string(*get.Ok) != "bar" {
		return errors.New("key `foo` value is not `bar`")
	}

	listKeys, err := store.Bucket_ListKeys(context.Background(), client, bucket.Borrow(), nil)
	if err != nil {
		return fmt.Errorf("failed to invoke `wrpc-examples:keyvalue/store.bucket.list-keys`: %w", err)
	}
	if listKeys.Err != nil {
		return fmt.Errorf("failed to list keys: %w", listKeys.Err)
	}
	for _, key := range listKeys.Ok.Keys {
		fmt.Printf("%s key: %s\n", prefix, key)
	}
	return nil
}
