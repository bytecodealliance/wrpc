package wrpcwebsockets

import (
	"context"
	"fmt"
	"log/slog"

	"github.com/coder/websocket"
	wrpc "wrpc.io/go"
)

// Invoker invokes wRPC functions over a new WebSocket connection to a URL per invocation
type Invoker struct {
	url string
}

func NewInvoker(url string) *Invoker {
	return &Invoker{url: url}
}

func (c *Invoker) Invoke(ctx context.Context, instance string, name string, paths ...wrpc.SubscribePath) (wrpc.IndexWriteCloser, wrpc.IndexReadCloser, error) {
	ws, _, err := websocket.Dial(ctx, c.url, nil)
	if err != nil {
		return nil, nil, fmt.Errorf("failed to establish WebSocket connection: %w", err)
	}
	w, r := newConn(ws)
	wc, rc, err := wrpc.InvokeFramed(ctx, w, r, instance, name, paths...)
	if err != nil {
		if cErr := w.CloseWithError(err); cErr != nil {
			slog.DebugContext(ctx, "failed to close WebSocket connection", "err", cErr)
		}
		return nil, nil, err
	}
	return wc, rc, nil
}
