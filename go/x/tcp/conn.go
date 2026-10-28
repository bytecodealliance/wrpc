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
