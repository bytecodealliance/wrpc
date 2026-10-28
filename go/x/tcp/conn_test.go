package wrpctcp

import (
	"context"
	"errors"
	"io"
	"net"
	"testing"
	"time"
)

func listen(t *testing.T) *net.TCPListener {
	t.Helper()
	l, err := net.ListenTCP("tcp", &net.TCPAddr{IP: net.IPv4(127, 0, 0, 1)})
	if err != nil {
		t.Fatalf("failed to listen: %s", err)
	}
	t.Cleanup(func() { l.Close() })
	return l
}

func dial(t *testing.T, l *net.TCPListener) *net.TCPConn {
	t.Helper()
	c, err := net.DialTCP("tcp", nil, l.Addr().(*net.TCPAddr))
	if err != nil {
		t.Fatalf("failed to dial: %s", err)
	}
	t.Cleanup(func() { c.Close() })
	return c
}

func TestConnClose(t *testing.T) {
	l := listen(t)
	c := dial(t, l)
	w, r := newConn(c)
	for range 2 {
		if err := w.Close(); err != nil {
			t.Fatalf("failed to close write half: %s", err)
		}
	}
	if err := c.SetDeadline(time.Time{}); err != nil {
		t.Fatalf("expected connection to stay open, got `%s`", err)
	}
	if err := r.Close(); err != nil {
		t.Fatalf("failed to close read half: %s", err)
	}
	if err := c.SetDeadline(time.Time{}); !errors.Is(err, net.ErrClosed) {
		t.Fatalf("expected connection to be closed, got `%v`", err)
	}
}

func TestServerAcceptError(t *testing.T) {
	for name, payload := range map[string][]byte{
		"version":  {0xff},
		"instance": {0x00, 1, 'a', 1, 'b'},
	} {
		t.Run(name, func(t *testing.T) {
			l := listen(t)
			srv := NewServerWithContext(context.Background(), l)
			accepted := make(chan error, 1)
			go func() { accepted <- srv.Accept() }()
			c := dial(t, l)
			if _, err := c.Write(payload); err != nil {
				t.Fatalf("failed to write: %s", err)
			}
			if err := <-accepted; err == nil {
				t.Fatal("expected accept to fail")
			}
			if err := c.SetReadDeadline(time.Now().Add(5 * time.Second)); err != nil {
				t.Fatalf("failed to set deadline: %s", err)
			}
			if _, err := c.Read(make([]byte, 1)); !errors.Is(err, io.EOF) {
				t.Fatalf("expected connection to be closed, got `%v`", err)
			}
		})
	}
}
