// Package wrpcwebsockets implements the wRPC WebSocket transport for Go.
//
// This package is not ready for production use.
package wrpcwebsockets

import (
	"context"
	"errors"
	"io"
	"net"
	"sync"
	"sync/atomic"

	"github.com/coder/websocket"
)

// StatusFailed is the status code a connection is closed with once its invocation failed
const StatusFailed websocket.StatusCode = 4000

// MaxMessageLen is the maximum length of a message sent or received
const MaxMessageLen = 1 << 20

var errShutdown = errors.New("server is shutting down")

type conn struct {
	ws     *websocket.Conn
	r      io.Reader
	open   atomic.Int32
	closed sync.Once
}

func newConn(ws *websocket.Conn) (*half, *half) {
	ws.SetReadLimit(MaxMessageLen)
	conn := &conn{ws: ws}
	conn.open.Store(2)
	return &half{conn: conn}, &half{conn: conn}
}

// Read reads the binary messages of the connection, ending once the connection is closed
// with `StatusNormalClosure` or ends without the closing handshake
func (c *conn) Read(p []byte) (int, error) {
	for {
		if c.r == nil {
			typ, r, err := c.ws.Reader(context.Background())
			if websocket.CloseStatus(err) == websocket.StatusNormalClosure || errors.Is(err, io.EOF) {
				return 0, io.EOF
			}
			if err != nil {
				return 0, err
			}
			if typ != websocket.MessageBinary {
				return 0, errors.New("unexpected WebSocket text message")
			}
			c.r = r
		}
		n, err := c.r.Read(p)
		if err != io.EOF {
			return n, err
		}
		c.r = nil
		if n > 0 {
			return n, nil
		}
	}
}

// Write writes `p` as binary messages of at most `MaxMessageLen` bytes
func (c *conn) Write(p []byte) (int, error) {
	var n int
	for n < len(p) {
		k := min(len(p)-n, MaxMessageLen)
		if err := c.ws.Write(context.Background(), websocket.MessageBinary, p[n:n+k]); err != nil {
			return n, err
		}
		n += k
	}
	return n, nil
}

type half struct {
	*conn
	once sync.Once
}

// Close closes the connection without the closing handshake once both halves are closed
func (c *half) Close() (err error) {
	c.once.Do(func() {
		if c.open.Add(-1) == 0 {
			c.closed.Do(func() { err = c.ws.CloseNow() })
		}
	})
	if errors.Is(err, net.ErrClosed) {
		return nil
	}
	return err
}

// CloseWithError closes the half like `Close` if `err` is nil and otherwise closes the
// connection with `StatusFailed` without waiting for the peer to acknowledge it
func (c *half) CloseWithError(err error) error {
	switch {
	case err == nil:
		return c.Close()
	case errors.Is(err, errShutdown):
		c.closeHandshake(websocket.StatusNormalClosure)
	default:
		c.closeHandshake(StatusFailed)
	}
	return nil
}

func (c *conn) closeHandshake(code websocket.StatusCode) {
	c.closed.Do(func() { go c.ws.Close(code, "") })
}
