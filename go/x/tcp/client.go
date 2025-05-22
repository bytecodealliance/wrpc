package wrpctcp

import (
	"context"
	"fmt"
	"net"

	wrpc "wrpc.io/go"
)

// Client is a thin wrapper around *net.TCPAddr, which is able to invoke wRPC functions
type Client struct {
	addr string
}

func NewClient(addr string) *Client {
	return &Client{addr: addr}
}

type writeConn struct {
	*net.TCPConn
}

func (c writeConn) Close() error {
	return c.CloseWrite()
}

type readConn struct {
	*net.TCPConn
}

func (c readConn) Close() error {
	return c.CloseRead()
}

func (c *Client) Invoke(ctx context.Context, instance string, name string, buf []byte, paths ...wrpc.SubscribePath) (wrpc.IndexWriteCloser, wrpc.IndexReadCloser, error) {
	addr, err := net.ResolveTCPAddr("tcp", c.addr)
	if err != nil {
		return nil, nil, fmt.Errorf("failed to resolve `%s` as TCP address: %w", c.addr, err)
	}
	conn, err := net.DialTCP("tcp", nil, addr)
	if err != nil {
		return nil, nil, fmt.Errorf("failed to establish TCP connection: %w", err)
	}
	return wrpc.InvokeFramed(ctx, writeConn{conn}, readConn{conn}, instance, name, buf, paths...)
}
