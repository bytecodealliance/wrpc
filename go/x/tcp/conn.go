// Package wrpctcp implements the wRPC TCP transport for Go.
//
// This package is not ready for production use.
package wrpctcp

import (
	"errors"
	"net"
	"sync"
	"sync/atomic"
)

type conn struct {
	*net.TCPConn
	open atomic.Int32
}

func newConn(c *net.TCPConn) (*writeConn, *readConn) {
	conn := &conn{TCPConn: c}
	conn.open.Store(2)
	return &writeConn{conn: conn}, &readConn{conn: conn}
}

func (c *conn) closeHalf(err error) error {
	if c.open.Add(-1) == 0 {
		err = errors.Join(err, c.Close())
	}
	if err == nil || errors.Is(err, net.ErrClosed) {
		return nil
	}
	return err
}

func (c *conn) reset() error {
	err := c.SetLinger(0)
	cErr := c.TCPConn.Close()
	if errors.Is(cErr, net.ErrClosed) {
		return nil
	}
	return errors.Join(err, cErr)
}

type writeConn struct {
	*conn
	once sync.Once
}

func (c *writeConn) Close() (err error) {
	c.once.Do(func() {
		err = c.closeHalf(c.CloseWrite())
	})
	return err
}

func (c *writeConn) CloseWithError(err error) error {
	if err == nil {
		return c.Close()
	}
	return c.reset()
}

type readConn struct {
	*conn
	once sync.Once
}

func (c *readConn) Close() (err error) {
	c.once.Do(func() {
		err = c.closeHalf(c.CloseRead())
	})
	return err
}

func (c *readConn) CloseWithError(err error) error {
	if err == nil {
		return c.Close()
	}
	return c.reset()
}
