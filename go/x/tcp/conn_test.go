package wrpctcp

import (
	"bytes"
	"context"
	"errors"
	"io"
	"net"
	"syscall"
	"testing"
	"time"

	wrpc "wrpc.io/go"
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
		"stalled":  nil,
		"version":  {0xff},
		"instance": {0x01, 1, 'a', 1, 'b'},
	} {
		t.Run(name, func(t *testing.T) {
			l := listen(t)
			srv := NewServerWithContext(context.Background(), l)
			accepted := make(chan error, 1)
			go func() {
				accept, err := srv.Accept()
				if err != nil {
					accepted <- err
					return
				}
				ctx, cancel := context.WithTimeout(context.Background(), 100*time.Millisecond)
				defer cancel()
				accepted <- accept(ctx)
			}()
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
			if _, err := c.Read(make([]byte, 1)); !errors.Is(err, syscall.ECONNRESET) {
				t.Fatalf("expected connection to be reset, got `%v`", err)
			}
		})
	}
}

func TestServerInvalidFrame(t *testing.T) {
	l := listen(t)
	srv := NewServerWithContext(context.Background(), l)
	written := make(chan error, 1)
	stop, err := srv.Serve("foo", "bar", func(ctx context.Context, w wrpc.IndexWriteCloser, r wrpc.IndexReadCloser) {
		defer r.Close()
		if _, err := io.ReadAll(r); err == nil {
			t.Error("expected reading parameters to fail")
		}
		if _, err := w.Write([]byte("x")); err != nil {
			written <- err
			return
		}
		written <- w.Close()
	})
	if err != nil {
		t.Fatalf("failed to serve: %s", err)
	}
	t.Cleanup(func() { stop() })
	accepted := make(chan error, 1)
	go func() {
		accept, err := srv.Accept()
		if err != nil {
			accepted <- err
			return
		}
		accepted <- accept(context.Background())
	}()
	buf := bytes.NewBuffer([]byte{0x01, 3, 'f', 'o', 'o', 3, 'b', 'a', 'r'})
	if _, err := wrpc.WriteFrame(wrpc.Frame{Path: []uint32{1}, Data: []byte("x")}, buf); err != nil {
		t.Fatalf("failed to encode frame: %s", err)
	}
	c := dial(t, l)
	if _, err := c.Write(buf.Bytes()); err != nil {
		t.Fatalf("failed to write: %s", err)
	}
	if err := c.SetReadDeadline(time.Now().Add(5 * time.Second)); err != nil {
		t.Fatalf("failed to set deadline: %s", err)
	}
	if _, err := io.ReadAll(c); !errors.Is(err, syscall.ECONNRESET) {
		t.Fatalf("expected connection to be reset, got `%v`", err)
	}
	if err := <-accepted; err != nil {
		t.Fatalf("failed to accept: %s", err)
	}
	if err := <-written; err == nil {
		t.Fatal("expected writing results to fail")
	}
}
