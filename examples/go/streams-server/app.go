//go:generate cargo run --bin wit-bindgen-wrpc go --generate-all --out-dir bindings --package wrpc.io/examples/go/streams-server/bindings wit

package app

import (
	"context"
	"log/slog"
	"os"

	"wrpc.io/examples/go/streams-server/bindings/exports/wrpc_examples/streams/handler"
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

func (Handler) Echo(ctx context.Context, req *handler.Req) (<-chan []uint64, <-chan []byte, error) {
	slog.InfoContext(ctx, "handling `wrpc-examples:streams/handler.echo`")
	return req.Numbers, req.Bytes, nil
}
