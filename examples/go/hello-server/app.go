//go:generate cargo run --bin wit-bindgen-wrpc go --out-dir bindings --package wrpc.io/examples/go/hello-server/bindings wit

package app

import (
	"context"
	"log/slog"
	"os"
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

type Handler struct{}

func (Handler) Hello(ctx context.Context) (string, error) {
	slog.InfoContext(ctx, "handling `wrpc-examples:hello/handler.hello`")
	return "hello from Go", nil
}
