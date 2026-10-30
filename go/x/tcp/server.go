package wrpctcp

import (
	"context"
	"fmt"
	"net"

	wrpc "wrpc.io/go"
)

type Server struct {
	l   *net.TCPListener
	srv *wrpc.FramedServer
}

func NewServerWithContext(ctx context.Context, l *net.TCPListener, opts ...wrpc.FramedServerOption) *Server {
	return &Server{
		l:   l,
		srv: wrpc.NewFramedServer(ctx, opts...),
	}
}

func NewServer(l *net.TCPListener, opts ...wrpc.FramedServerOption) *Server {
	return NewServerWithContext(context.Background(), l, opts...)
}

func (s *Server) Serve(instance string, name string, f wrpc.HandleFunc, paths ...wrpc.SubscribePath) (func() error, error) {
	return s.srv.Serve(instance, name, f, paths...)
}

// Accept accepts a TCP connection, returning a function, which reads the invocation on it and
// calls its handler in a new goroutine. Reading the invocation is aborted once `ctx` is done.
func (s *Server) Accept() (func(ctx context.Context) error, error) {
	conn, err := s.l.AcceptTCP()
	if err != nil {
		return nil, fmt.Errorf("failed to accept TCP connection: %w", err)
	}
	w, r := newConn(conn)
	return func(ctx context.Context) error {
		if err := s.srv.Accept(ctx, w, r); err != nil {
			return fmt.Errorf("failed to accept invocation: %w", err)
		}
		return nil
	}, nil
}
