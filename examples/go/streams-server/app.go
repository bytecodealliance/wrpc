//go:generate cargo run --bin wit-bindgen-wrpc go --out-dir bindings --package wrpc.io/examples/go/streams-server/bindings wit

package app

import (
	"context"
	"io"
	"log/slog"
	"os"

	"wrpc.io/examples/go/streams-server/bindings/exports/wrpc_examples/streams/handler"
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

type Handler struct{}

func (Handler) Echo(ctx context.Context, req *handler.Req) (wrpc.Receiver[[]uint64], io.ReadCloser, error) {
	slog.InfoContext(ctx, "handling `wrpc-examples:streams/handler.echo`")
	return req.Numbers, req.Bytes, nil
}
