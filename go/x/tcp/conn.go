package wrpctcp

import (
	"errors"
	"net"
)

type writeConn struct {
	*net.TCPConn
}

func (c writeConn) Close() error {
	err := c.CloseWrite()
	if err == nil || errors.Is(err, net.ErrClosed) {
		return nil
	}
	return err
}

type readConn struct {
	*net.TCPConn
}

func (c readConn) Close() error {
	err := c.CloseRead()
	if err == nil || errors.Is(err, net.ErrClosed) {
		return nil
	}
	return err
}
