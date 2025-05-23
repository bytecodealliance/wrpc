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

func NewServerWithContext(ctx context.Context, l *net.TCPListener) *Server {
	return &Server{
		l:   l,
		srv: wrpc.NewFramedServer(ctx),
	}
}

func NewServer(l *net.TCPListener) *Server {
	return NewServerWithContext(context.Background(), l)
}

func (s *Server) Serve(instance string, name string, f wrpc.HandleFunc, paths ...wrpc.SubscribePath) (func() error, error) {
	return s.srv.Serve(instance, name, f, paths...)
}

func (s *Server) Accept() error {
	conn, err := s.l.AcceptTCP()
	if err != nil {
		return fmt.Errorf("failed to accept TCP connection: %w", err)
	}
	if err := s.srv.Accept(writeConn{conn}, readConn{conn}); err != nil {
		return fmt.Errorf("failed to accept invocation: %w", err)
	}
	return nil
}
