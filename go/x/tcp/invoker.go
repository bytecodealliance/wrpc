package wrpctcp

import (
	"context"
	"fmt"
	"net"

	wrpc "wrpc.io/go"
)

// Invoker is a thin wrapper around *net.TCPAddr, which is able to invoke wRPC functions
type Invoker struct {
	addr string
}

func NewInvoker(addr string) *Invoker {
	return &Invoker{addr: addr}
}

func (c *Invoker) Invoke(ctx context.Context, instance string, name string, buf []byte, paths ...wrpc.SubscribePath) (wrpc.IndexWriteCloser, wrpc.IndexReadCloser, error) {
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
