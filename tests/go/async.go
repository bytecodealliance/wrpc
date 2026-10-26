//go:generate $WIT_BINDGEN_WRPC go --world async-server --out-dir bindings/async_server --package wrpc.io/tests/go/bindings/async_server ../wit

package integration

import (
	"context"
	"errors"
	"log/slog"

	"wrpc.io/tests/go/bindings/async_server/exports/wrpc_test/integration/async"
	"wrpc.io/tests/go/internal"
)

type AsyncHandler struct{}

func (AsyncHandler) WithStreams(ctx context.Context) (<-chan []byte, <-chan [][]string, error) {
	slog.DebugContext(ctx, "handling `with-streams`")
	return internal.Ready([]byte("test")), internal.Ready([][]string{{"foo"}, {"bar", "baz"}}), nil
}

func (AsyncHandler) WithFuture(ctx context.Context, x *async.Something, s <-chan []byte) (<-chan <-chan []byte, error) {
	slog.DebugContext(ctx, "handling `with-future`", "x", x)
	if x.Foo != "bar" {
		return nil, errors.New("`foo` is not `bar`")
	}
	return internal.Ready(s), nil
}

func (AsyncHandler) IdentityNestedAsync(ctx context.Context, v <-chan <-chan <-chan <-chan []string) (<-chan <-chan <-chan <-chan []string, error) {
	slog.DebugContext(ctx, "handling `identity-nested-async`")
	return v, nil
}
