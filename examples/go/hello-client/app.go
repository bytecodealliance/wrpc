//go:generate cargo run --bin wit-bindgen-wrpc go --out-dir bindings --package wrpc.io/examples/go/hello-client/bindings wit

package app

import (
	"context"
	"fmt"
	"log/slog"
	"os"

	"wrpc.io/examples/go/hello-client/bindings/wrpc_examples/hello/handler"
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
	greeting, err := handler.Hello(context.Background(), client)
	if err != nil {
		return fmt.Errorf("failed to call `wrpc-examples:hello/handler.hello`: %w", err)
	}
	fmt.Printf("%s: %s\n", prefix, greeting)
	return nil
}
