package wrpc_test

import (
	"bytes"
	"context"
	"io"
	"runtime"
	"slices"
	"sync"
	"testing"
	"time"

	wrpc "wrpc.io/go"
)

type sink struct {
	bytes.Buffer
}

func (sink) Close() error { return nil }

func assertClose(t *testing.T, w io.Closer) {
	t.Helper()
	if err := w.Close(); err != nil {
		t.Fatalf("failed to close: %s", err)
	}
}

func assertWriteByte(t *testing.T, w io.ByteWriter, b byte) {
	t.Helper()
	if err := w.WriteByte(b); err != nil {
		t.Fatalf("failed to write byte `%v`: %s", b, err)
	}
}

func assertWriteString(t *testing.T, w io.Writer, s string) {
	t.Helper()
	n, err := io.WriteString(w, s)
	if err != nil {
		t.Fatalf("failed to write `%s`: %s", s, err)
	}
	if n != len(s) {
		t.Fatalf("expected %d, got %d", len(s), n)
	}
}

func assertReadFrame(t *testing.T, r wrpc.ByteReader, data []byte, path ...uint32) {
	t.Helper()
	frame, err := wrpc.ReadFrame(r)
	if err != nil {
		t.Fatalf("failed to read frame: %s", err)
	}
	if !slices.Equal(frame.Path, path) {
		t.Fatalf("unexpected frame path `%v`", frame.Path)
	}
	if !slices.Equal(frame.Data, data) {
		t.Fatalf("expected `%s`, got `%s`", data, frame.Data)
	}
}

func assertIndex(t *testing.T, w wrpc.IndexWriter, path ...uint32) wrpc.IndexWriteCloser {
	t.Helper()
	wi, err := w.Index(path...)
	if err != nil {
		t.Fatalf("failed to index by %v: %s", path, err)
	}
	return wi
}

func TestFrameStreamWriter(t *testing.T) {
	t.Run("nested", func(t *testing.T) {
		t.Parallel()
		var dst sink
		w := wrpc.NewFrameStreamWriter(context.Background(), &dst)
		if _, err := w.Index(); err == nil {
			t.Fatal("indexing an empty path should fail")
		}
		assertWriteString(t, w, "")
		if err := w.WriteBytes(nil); err != nil {
			t.Fatalf("failed to write empty bytes: %s", err)
		}
		assertWriteByte(t, w, 42)
		w0 := assertIndex(t, w, 0)
		w42_41 := assertIndex(t, w, 42, 41)
		assertClose(t, w)

		assertWriteString(t, w0, "hello")
		w0_1 := assertIndex(t, w0, 1)
		assertClose(t, w0)

		assertWriteString(t, w0_1, "test")
		assertClose(t, w0_1)

		assertWriteByte(t, w42_41, 0xfe)
		assertClose(t, w42_41)

		r := bytes.NewBuffer(dst.Bytes())
		assertReadFrame(t, r, []byte{42})
		assertReadFrame(t, r, []byte("hello"), 0)
		assertReadFrame(t, r, []byte("test"), 0, 1)
		assertReadFrame(t, r, []byte{0xfe}, 42, 41)
		if n := r.Len(); n > 0 {
			t.Fatalf("%v trailing bytes left in buffer: %s", n, r.String())
		}
	})

	t.Run("close path", func(t *testing.T) {
		t.Parallel()
		var dst sink
		w := wrpc.NewFrameStreamWriter(context.Background(), &dst)
		w0 := assertIndex(t, w, 0)
		assertWriteString(t, w0, "hello")
		assertWriteString(t, w0, "")
		if err := w0.ClosePath(); err != nil {
			t.Fatalf("failed to close path: %s", err)
		}
		assertClose(t, w0)
		assertClose(t, w0)
		if err := w.ClosePath(); err != nil {
			t.Fatalf("failed to close path: %s", err)
		}
		assertClose(t, w)

		r := bytes.NewBuffer(dst.Bytes())
		assertReadFrame(t, r, []byte("hello"), 0)
		assertReadFrame(t, r, nil, 0)
		assertReadFrame(t, r, nil)
		if n := r.Len(); n > 0 {
			t.Fatalf("%v trailing bytes left in buffer: %s", n, r.String())
		}
	})
}

type blockingWriter struct {
	once    sync.Once
	started chan struct{}
	release chan struct{}
}

func (w *blockingWriter) Write(p []byte) (int, error) {
	w.once.Do(func() { close(w.started) })
	<-w.release
	return len(p), nil
}

func (*blockingWriter) Close() error { return nil }

func TestFrameStreamWriterFinalizer(t *testing.T) {
	dst := &blockingWriter{
		started: make(chan struct{}),
		release: make(chan struct{}),
	}
	defer close(dst.release)
	func() {
		w := wrpc.NewFrameStreamWriter(context.Background(), dst)
		assertWriteByte(t, w, 42)
	}()
	select {
	case <-dst.started:
	case <-time.After(5 * time.Second):
		t.Fatal("frame was not written")
	}
	runtime.GC()
	time.Sleep(100 * time.Millisecond)

	finalized := make(chan struct{})
	func() {
		v := &struct{ p *int }{}
		runtime.SetFinalizer(v, func(*struct{ p *int }) { close(finalized) })
	}()
	for range 50 {
		runtime.GC()
		select {
		case <-finalized:
			return
		case <-time.After(100 * time.Millisecond):
		}
	}
	t.Fatal("finalizers are blocked")
}
