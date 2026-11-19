package wrpcwebsockets

import (
	"bytes"
	"context"
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"

	"github.com/coder/websocket"
	wrpc "wrpc.io/go"
)

func serve(t *testing.T, srv *Server, f wrpc.HandleFunc) string {
	t.Helper()
	stop, err := srv.Serve("foo", "bar", f)
	if err != nil {
		t.Fatalf("failed to serve: %s", err)
	}
	t.Cleanup(func() { stop() })
	s := httptest.NewServer(srv)
	t.Cleanup(s.Close)
	return "ws" + strings.TrimPrefix(s.URL, "http")
}

func TestInvoke(t *testing.T) {
	url := serve(t, NewServer(), func(ctx context.Context, w wrpc.IndexWriteCloser, r wrpc.IndexReadCloser) {
		defer r.Close()
		params, err := io.ReadAll(r)
		if err != nil {
			t.Errorf("failed to read parameters: %s", err)
			return
		}
		if _, err := w.Write(params); err != nil {
			t.Errorf("failed to write results: %s", err)
			return
		}
		if err := w.Close(); err != nil {
			t.Errorf("failed to close results: %s", err)
		}
	})
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	w, r, err := NewInvoker(url).Invoke(ctx, "foo", "bar")
	if err != nil {
		t.Fatalf("failed to invoke: %s", err)
	}
	defer r.Close()
	if _, err := w.Write([]byte("hello")); err != nil {
		t.Fatalf("failed to write parameters: %s", err)
	}
	if err := w.Close(); err != nil {
		t.Fatalf("failed to close parameters: %s", err)
	}
	results, err := io.ReadAll(r)
	if err != nil {
		t.Fatalf("failed to read results: %s", err)
	}
	if string(results) != "hello" {
		t.Fatalf("unexpected results `%s`", results)
	}
}

func TestMaxMessageLen(t *testing.T) {
	data := bytes.Repeat([]byte{0x42}, MaxMessageLen+1)
	s := httptest.NewServer(http.HandlerFunc(func(rw http.ResponseWriter, req *http.Request) {
		ws, err := websocket.Accept(rw, req, nil)
		if err != nil {
			t.Errorf("failed to accept: %s", err)
			return
		}
		w, r := newConn(ws)
		defer r.Close()
		if _, err := w.Write(data); err != nil {
			t.Errorf("failed to write: %s", err)
		}
		if err := w.Close(); err != nil {
			t.Errorf("failed to close: %s", err)
		}
	}))
	t.Cleanup(s.Close)
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	ws, _, err := websocket.Dial(ctx, "ws"+strings.TrimPrefix(s.URL, "http"), nil)
	if err != nil {
		t.Fatalf("failed to dial: %s", err)
	}
	w, r := newConn(ws)
	defer w.Close()
	defer r.Close()
	buf, err := io.ReadAll(r)
	if err != nil {
		t.Fatalf("failed to read: %s", err)
	}
	if !bytes.Equal(buf, data) {
		t.Fatalf("expected %d bytes, got %d", len(data), len(buf))
	}
}

func TestServerReuse(t *testing.T) {
	url := serve(t, NewServer(), func(ctx context.Context, w wrpc.IndexWriteCloser, r wrpc.IndexReadCloser) {
		defer r.Close()
		params, err := io.ReadAll(r)
		if err != nil {
			t.Errorf("failed to read parameters: %s", err)
			return
		}
		if _, err := w.Write(params); err != nil {
			t.Errorf("failed to write results: %s", err)
			return
		}
		if err := w.Close(); err != nil {
			t.Errorf("failed to close results: %s", err)
		}
	})
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	c, _, err := websocket.Dial(ctx, url, nil)
	if err != nil {
		t.Fatalf("failed to dial: %s", err)
	}
	defer c.CloseNow()
	var buf bytes.Buffer
	for _, p := range []string{"a", "b"} {
		buf.Write([]byte{0x01, 3, 'f', 'o', 'o', 3, 'b', 'a', 'r'})
		for _, frame := range []wrpc.Frame{{Data: []byte(p)}, {}} {
			if _, err := wrpc.WriteFrame(frame, &buf); err != nil {
				t.Fatalf("failed to encode frame: %s", err)
			}
		}
	}
	if err := c.Write(ctx, websocket.MessageBinary, buf.Bytes()); err != nil {
		t.Fatalf("failed to write: %s", err)
	}
	want := []byte{0, 1, 'a', 0, 0, 0, 1, 'b', 0, 0}
	var res []byte
	for len(res) < len(want) {
		_, p, err := c.Read(ctx)
		if err != nil {
			t.Fatalf("failed to read: %s", err)
		}
		res = append(res, p...)
	}
	if !bytes.Equal(res, want) {
		t.Fatalf("expected results `%v`, got `%v`", want, res)
	}
	if err := c.Close(websocket.StatusNormalClosure, ""); err != nil {
		t.Fatalf("failed to close: %s", err)
	}
}

func TestServerInvalidFrame(t *testing.T) {
	url := serve(t, NewServer(), func(ctx context.Context, w wrpc.IndexWriteCloser, r wrpc.IndexReadCloser) {
		defer r.Close()
		if _, err := io.ReadAll(r); err == nil {
			t.Error("expected reading parameters to fail")
		}
	})
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	c, _, err := websocket.Dial(ctx, url, nil)
	if err != nil {
		t.Fatalf("failed to dial: %s", err)
	}
	defer c.CloseNow()
	buf := bytes.NewBuffer([]byte{0x01, 3, 'f', 'o', 'o', 3, 'b', 'a', 'r'})
	if _, err := wrpc.WriteFrame(wrpc.Frame{Path: []uint32{1}, Data: []byte("x")}, buf); err != nil {
		t.Fatalf("failed to encode frame: %s", err)
	}
	if err := c.Write(ctx, websocket.MessageBinary, buf.Bytes()); err != nil {
		t.Fatalf("failed to write: %s", err)
	}
	if _, _, err := c.Read(ctx); websocket.CloseStatus(err) != StatusFailed {
		t.Fatalf("expected connection to be closed with `%d`, got `%v`", StatusFailed, err)
	}
}

func TestServerShutdown(t *testing.T) {
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	srvCtx, stop := context.WithCancel(ctx)
	url := serve(t, NewServerWithContext(srvCtx), func(context.Context, wrpc.IndexWriteCloser, wrpc.IndexReadCloser) {})
	c, _, err := websocket.Dial(ctx, url, nil)
	if err != nil {
		t.Fatalf("failed to dial: %s", err)
	}
	defer c.CloseNow()
	stop()
	if _, _, err := c.Read(ctx); websocket.CloseStatus(err) != websocket.StatusNormalClosure {
		t.Fatalf("expected connection to be closed with `%d`, got `%v`", websocket.StatusNormalClosure, err)
	}
}
