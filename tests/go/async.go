//go:generate $WIT_BINDGEN_WRPC go --world async-server --out-dir bindings/async_server --package wrpc.io/tests/go/bindings/async_server ../wit

package integration

import (
	"bytes"
	"context"
	"io"
	"log/slog"

	wrpc "wrpc.io/go"
)

type AsyncHandler struct{}

func (AsyncHandler) WithStreams(ctx context.Context, complete bool) (io.ReadCloser, wrpc.Receiver[[][]string], error) {
	slog.DebugContext(ctx, "handling `with-streams`", "complete", complete)
	buf := io.NopCloser(bytes.NewBuffer([]byte("test")))
	str := wrpc.NewCompleteReceiver([][]string{{"foo", "bar"}, {"baz"}})
	if complete {
		return buf, str, nil
	} else {
		return buf, str, nil
	}
}

func (AsyncHandler) IdentityNestedAsync(ctx context.Context, v wrpc.Receiver[wrpc.Receiver[wrpc.Receiver[wrpc.Receiver[[]string]]]]) (wrpc.Receiver[wrpc.Receiver[wrpc.Receiver[wrpc.Receiver[[]string]]]], error) {
	slog.DebugContext(ctx, "handling `identity-nested-async`")
	return v, nil
}
