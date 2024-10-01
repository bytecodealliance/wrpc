//go:generate $WIT_BINDGEN_WRPC go --world async-server --out-dir bindings/async_server --package wrpc.io/tests/go/bindings/async_server ../wit

package integration

import (
	"bytes"
	"context"
	"errors"
	"io"
	"log/slog"

	wrpc "wrpc.io/go"
	"wrpc.io/tests/go/bindings/async_server/exports/wrpc_test/integration/async"
)

type AsyncHandler struct{}

func (AsyncHandler) WithStreams(ctx context.Context) (io.ReadCloser, wrpc.Receiver[[][]string], error) {
	slog.DebugContext(ctx, "handling `with-streams`")
	buf := io.NopCloser(bytes.NewBuffer([]byte("test")))
	str := wrpc.NewCompleteReceiver([][]string{{"foo"}, {"bar", "baz"}})
	return buf, str, nil
}

func (AsyncHandler) WithFuture(ctx context.Context, x *async.Something, s io.ReadCloser) (wrpc.Receiver[io.ReadCloser], error) {
	slog.DebugContext(ctx, "handling `with-future`", "x", x)
	if x.Foo != "bar" {
		return nil, errors.New("`foo` is not `bar`")
	}
	return wrpc.NewCompleteReceiver(s), nil
}

func (AsyncHandler) IdentityNestedAsync(ctx context.Context, v wrpc.Receiver[wrpc.Receiver[wrpc.Receiver[wrpc.Receiver[[]string]]]]) (wrpc.Receiver[wrpc.Receiver[wrpc.Receiver[wrpc.Receiver[[]string]]]], error) {
	slog.DebugContext(ctx, "handling `identity-nested-async`")
	return v, nil
}
