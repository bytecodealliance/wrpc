package wrpcwebsockets

import (
	"context"
	"log/slog"
	"net/http"

	"github.com/coder/websocket"
	wrpc "wrpc.io/go"
)

// Server is an `http.Handler`, which serves the invocations on a WebSocket connection one after another
type Server struct {
	// AcceptOptions are passed to `websocket.Accept`, e.g. to allow cross-origin requests
	AcceptOptions *websocket.AcceptOptions

	ctx context.Context
	srv *wrpc.FramedServer
}

func NewServerWithContext(ctx context.Context, opts ...wrpc.FramedServerOption) *Server {
	acceptCtx, cancel := context.WithCancelCause(context.WithoutCancel(ctx))
	context.AfterFunc(ctx, func() { cancel(errShutdown) })
	return &Server{
		ctx: acceptCtx,
		srv: wrpc.NewFramedServer(ctx, opts...),
	}
}

func NewServer(opts ...wrpc.FramedServerOption) *Server {
	return NewServerWithContext(context.Background(), opts...)
}

func (s *Server) Serve(instance string, name string, f wrpc.HandleFunc, paths ...wrpc.SubscribePath) (func() error, error) {
	return s.srv.Serve(instance, name, f, paths...)
}

// ServeHTTP accepts a WebSocket connection and serves the invocations on it one after another until
// the client ends transmit (see `wrpc.FramedServer.Accept`). Once the server context is done,
// reading an invocation header is aborted and the connection is closed with `StatusNormalClosure`.
func (s *Server) ServeHTTP(w http.ResponseWriter, r *http.Request) {
	ws, err := websocket.Accept(w, r, s.AcceptOptions)
	if err != nil {
		slog.DebugContext(r.Context(), "failed to accept WebSocket connection", "err", err)
		return
	}
	wc, rc := newConn(ws)
	if err := s.srv.Accept(s.ctx, wc, rc); err != nil {
		slog.DebugContext(s.ctx, "failed to serve connection", "err", err)
	}
}
