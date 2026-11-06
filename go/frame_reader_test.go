package wrpc_test

import (
	"bufio"
	"bytes"
	"context"
	"fmt"
	"io"
	"log/slog"
	"os"
	"strings"
	"sync/atomic"
	"testing"
	"time"

	wrpc "wrpc.io/go"
)

type ingressKey struct{}

type ingressHandler struct {
	slog.Handler
}

func (h ingressHandler) Enabled(ctx context.Context, level slog.Level) bool {
	if _, ok := ctx.Value(ingressKey{}).(chan error); ok {
		return true
	}
	return h.Handler.Enabled(ctx, level)
}

func (h ingressHandler) Handle(ctx context.Context, r slog.Record) error {
	ch, ok := ctx.Value(ingressKey{}).(chan error)
	if !ok {
		return h.Handler.Handle(ctx, r)
	}
	switch r.Message {
	case "ingress failed":
		var err error
		r.Attrs(func(a slog.Attr) bool {
			if a.Key == "err" {
				err, _ = a.Value.Any().(error)
			}
			return true
		})
		ch <- err
	case "ingress successfully complete":
		ch <- nil
	}
	return nil
}

func (h ingressHandler) WithAttrs(attrs []slog.Attr) slog.Handler {
	return ingressHandler{h.Handler.WithAttrs(attrs)}
}

func (h ingressHandler) WithGroup(name string) slog.Handler {
	return ingressHandler{h.Handler.WithGroup(name)}
}

func TestMain(m *testing.M) {
	slog.SetDefault(slog.New(ingressHandler{slog.NewTextHandler(os.Stderr, nil)}))
	os.Exit(m.Run())
}

func withIngress() (context.Context, <-chan error) {
	ch := make(chan error, 1)
	return context.WithValue(context.Background(), ingressKey{}, ch), ch
}

func assertErr(t *testing.T, err error, msg string) {
	t.Helper()
	if msg == "" && err != nil || msg != "" && (err == nil || !strings.Contains(err.Error(), msg)) {
		t.Fatalf("expected error `%s`, got `%v`", msg, err)
	}
}

func assertIngress(t *testing.T, ch <-chan error, msg string) {
	t.Helper()
	select {
	case err := <-ch:
		assertErr(t, err, msg)
	case <-time.After(5 * time.Second):
		t.Fatal("ingress did not complete")
	}
}

func assertIndexReader(t *testing.T, r wrpc.IndexReader, path ...uint32) wrpc.IndexReadCloser {
	t.Helper()
	sub, err := r.Index(path...)
	if err != nil {
		t.Fatalf("failed to index by %v: %s", path, err)
	}
	return sub
}

func assertRead(t *testing.T, r io.Reader, want string) {
	t.Helper()
	buf := make([]byte, len(want))
	if _, err := io.ReadFull(r, buf); err != nil {
		t.Fatalf("failed to read: %s", err)
	}
	if string(buf) != want {
		t.Fatalf("expected `%s`, got `%s`", want, buf)
	}
}

func assertReadAll(t *testing.T, r io.Reader, want, msg string) {
	t.Helper()
	buf, err := io.ReadAll(r)
	assertErr(t, err, msg)
	if string(buf) != want {
		t.Fatalf("expected `%s`, got `%s`", want, buf)
	}
}

func encodeFrames(t *testing.T, frames ...wrpc.Frame) *sink {
	t.Helper()
	var src sink
	for _, frame := range frames {
		if _, err := wrpc.WriteFrame(frame, &src); err != nil {
			t.Fatalf("failed to write frame: %s", err)
		}
	}
	return &src
}

func pipeReader(pr *io.PipeReader) wrpc.ByteReadCloser {
	return struct {
		*bufio.Reader
		io.Closer
	}{bufio.NewReader(pr), pr}
}

type closerFunc func() error

func (f closerFunc) Close() error {
	return f()
}

func closedFrames(t *testing.T, frames ...wrpc.Frame) wrpc.ByteReadCloser {
	t.Helper()
	return encodeFrames(t, append(frames, wrpc.Frame{})...)
}

func endTransmit(t *testing.T, w *io.PipeWriter) {
	t.Helper()
	go func() {
		encodeFrames(t, wrpc.Frame{}).WriteTo(w)
		w.Close()
	}()
}

func writeFrames(t *testing.T, w io.Writer, frames ...wrpc.Frame) {
	t.Helper()
	done := make(chan error, 1)
	go func() {
		_, err := encodeFrames(t, frames...).WriteTo(w)
		done <- err
	}()
	select {
	case err := <-done:
		if err != nil {
			t.Fatalf("failed to write frames: %s", err)
		}
	case <-time.After(5 * time.Second):
		t.Fatal("frames were not read")
	}
}

func TestFrameStreamReader(t *testing.T) {
	for name, tc := range map[string]struct {
		path []uint32
		err  string
	}{
		"closed path":  {[]uint32{0}, "closed path"},
		"unknown path": {[]uint32{1}, "unknown path"},
		"deep path":    {[]uint32{0, 0, 0}, "exceeds maximum"},
	} {
		for _, data := range []bool{false, true} {
			t.Run(fmt.Sprintf("%s data=%v", name, data), func(t *testing.T) {
				t.Parallel()
				ctx, ingress := withIngress()
				frame := wrpc.Frame{Path: tc.path}
				if data {
					frame.Data = []byte("!")
				}
				r := wrpc.NewFrameStreamReader(ctx, closedFrames(t,
					wrpc.Frame{Path: []uint32{0}, Data: []byte("world")},
					wrpc.Frame{Path: []uint32{0}},
					wrpc.Frame{Data: []byte("hello")},
					frame,
				), wrpc.NewSubscribePath().Index(0))
				sub := assertIndexReader(t, r, 0)
				assertReadAll(t, sub, "world", "")
				assertReadAll(t, r, "hello", tc.err)
				assertClose(t, sub)
				assertClose(t, r)
				assertIngress(t, ingress, tc.err)
			})
		}
	}

	t.Run("end of transmit", func(t *testing.T) {
		t.Parallel()
		ctx, ingress := withIngress()
		frames := []wrpc.Frame{{Data: []byte{0}}}
		for range 100 {
			frames = append(frames, wrpc.Frame{Path: []uint32{1}, Data: []byte("x")})
		}
		frames = append(frames, wrpc.Frame{Path: []uint32{0}}, wrpc.Frame{Path: []uint32{1}})
		r := wrpc.NewFrameStreamReader(ctx, closedFrames(t, frames...),
			wrpc.NewSubscribePath().Index(0),
			wrpc.NewSubscribePath().Index(1),
		)
		fut, deferred, err := wrpc.ReadFutureUnit(r, 0)
		if err != nil {
			t.Fatalf("failed to read future: %s", err)
		}
		sub := assertIndexReader(t, r, 1)
		assertReadAll(t, sub, strings.Repeat("x", 100), "")
		if err := deferred(ctx); err != nil {
			t.Fatalf("failed to resolve future: %s", err)
		}
		if _, ok := <-fut; !ok {
			t.Fatal("future closed without resolving")
		}
		assertClose(t, sub)
		assertClose(t, r)
		assertIngress(t, ingress, "")
	})

	t.Run("drop once", func(t *testing.T) {
		t.Parallel()
		pr, pw := io.Pipe()
		r := wrpc.NewFrameStreamReader(context.Background(), pipeReader(pr),
			wrpc.NewSubscribePath().Index(0),
			wrpc.NewSubscribePath().Index(1),
		)
		sub0 := assertIndexReader(t, r, 0)
		sub1 := assertIndexReader(t, r, 1)
		assertClose(t, r)
		assertClose(t, sub0)
		assertClose(t, sub0)
		writeFrames(t, pw,
			wrpc.Frame{Path: []uint32{0}},
			wrpc.Frame{Path: []uint32{1}, Data: []byte("hello")},
			wrpc.Frame{Path: []uint32{1}, Data: []byte("world")},
			wrpc.Frame{Path: []uint32{1}},
		)
		endTransmit(t, pw)
		assertReadAll(t, sub1, "helloworld", "")
		assertClose(t, sub1)
	})

	t.Run("wildcard elements", func(t *testing.T) {
		t.Parallel()
		pr, pw := io.Pipe()
		r := wrpc.NewFrameStreamReader(context.Background(), pipeReader(pr), wrpc.NewSubscribePath().Index(0).Wildcard())
		writeFrames(t, pw, wrpc.Frame{Path: []uint32{0, 2}, Data: []byte("b")})
		sub := assertIndexReader(t, r, 0, 2)
		assertRead(t, sub, "b")
		for _, i := range []uint32{0, 1} {
			_, err := r.Index(0, i)
			assertErr(t, err, "released")
		}
		assertClose(t, sub)
		writeFrames(t, pw, wrpc.Frame{Path: []uint32{0, 0}, Data: []byte("y")})
		endTransmit(t, pw)
		assertReadAll(t, r, "", "released element")
		assertClose(t, r)
	})

	t.Run("unclaimed elements", func(t *testing.T) {
		t.Parallel()
		pr, pw := io.Pipe()
		r := wrpc.NewFrameStreamReader(context.Background(), pipeReader(pr), wrpc.NewSubscribePath().Wildcard())
		frames := make([]wrpc.Frame, 0, 66)
		for i := range uint32(64) {
			frames = append(frames, wrpc.Frame{Path: []uint32{i + 1}})
		}
		frames = append(frames,
			wrpc.Frame{Path: []uint32{0}, Data: []byte("a")},
			wrpc.Frame{Path: []uint32{65}, Data: []byte("b")},
		)
		writeFrames(t, pw, frames...)
		for _, el := range []struct {
			i    uint32
			data string
		}{{0, "a"}, {65, "b"}} {
			sub := assertIndexReader(t, r, el.i)
			assertRead(t, sub, el.data)
			assertClose(t, sub)
		}
		endTransmit(t, pw)
		assertReadAll(t, r, "", "")
		assertClose(t, r)
	})
}

type countingReader struct {
	*bytes.Reader
	n atomic.Int64
}

func (r *countingReader) Read(p []byte) (int, error) {
	n, err := r.Reader.Read(p)
	r.n.Add(int64(n))
	return n, err
}

func (r *countingReader) ReadByte() (byte, error) {
	b, err := r.Reader.ReadByte()
	if err == nil {
		r.n.Add(1)
	}
	return b, err
}

func (*countingReader) Close() error { return nil }

func TestFrameStreamReaderBoundsBuffering(t *testing.T) {
	data := bytes.Repeat([]byte{0x2a}, 20<<20)
	src := encodeFrames(t,
		wrpc.Frame{Path: []uint32{0}, Data: data},
		wrpc.Frame{Path: []uint32{0}},
		wrpc.Frame{Data: []byte("hello")},
		wrpc.Frame{},
	)
	rd := &countingReader{Reader: bytes.NewReader(src.Bytes())}
	r := wrpc.NewFrameStreamReader(context.Background(), rd, wrpc.NewSubscribePath().Index(0))
	sub := assertIndexReader(t, r, 0)
	time.Sleep(100 * time.Millisecond)
	if n := rd.n.Load(); n > 18<<20 {
		t.Fatalf("expected at most 18MiB to be read before the path is consumed, read %d bytes", n)
	}
	buf, err := io.ReadAll(sub)
	if err != nil {
		t.Fatalf("failed to read path: %s", err)
	}
	if !bytes.Equal(buf, data) {
		t.Fatalf("expected %d bytes of data, got %d", len(data), len(buf))
	}
	assertReadAll(t, r, "hello", "")
}

func TestFrameStreamReaderDrain(t *testing.T) {
	t.Run("empty frames", func(t *testing.T) {
		t.Parallel()
		ctx, ingress := withIngress()
		pr, pw := io.Pipe()
		r := wrpc.NewFrameStreamReader(ctx, pipeReader(pr), wrpc.NewSubscribePath().Index(0))
		sub := assertIndexReader(t, r, 0)
		writeFrames(t, pw, wrpc.Frame{Data: []byte("hello")})
		assertRead(t, r, "hello")
		assertClose(t, sub)
		assertClose(t, r)
		writeFrames(t, pw, wrpc.Frame{Path: []uint32{0}})
		writeFrames(t, pw, wrpc.Frame{})
		endTransmit(t, pw)
		assertIngress(t, ingress, "")
	})

	t.Run("data", func(t *testing.T) {
		t.Parallel()
		ctx, ingress := withIngress()
		pr, pw := io.Pipe()
		r := wrpc.NewFrameStreamReader(ctx, pipeReader(pr))
		assertClose(t, r)
		writeFrames(t, pw, wrpc.Frame{Data: []byte("hello")})
		assertIngress(t, ingress, "closed reader")
		endTransmit(t, pw)
	})

	t.Run("buffered", func(t *testing.T) {
		t.Parallel()
		ctx, ingress := withIngress()
		r := wrpc.NewFrameStreamReader(ctx, closedFrames(t, wrpc.Frame{Data: []byte("hello")}))
		assertRead(t, r, "he")
		assertClose(t, r)
		assertIngress(t, ingress, "not processed")
	})

	t.Run("unread", func(t *testing.T) {
		t.Parallel()
		ctx, ingress := withIngress()
		pr, pw := io.Pipe()
		r := wrpc.NewFrameStreamReader(ctx, pipeReader(pr), wrpc.NewSubscribePath().Index(0))
		sub := assertIndexReader(t, r, 0)
		writeFrames(t, pw, wrpc.Frame{Path: []uint32{0}, Data: []byte("hello")})
		assertRead(t, sub, "he")
		assertClose(t, sub)
		assertIngress(t, ingress, "not processed")
		assertReadAll(t, r, "", "not processed")
		assertClose(t, r)
		endTransmit(t, pw)
	})

	t.Run("first error wins", func(t *testing.T) {
		t.Parallel()
		ctx, ingress := withIngress()
		pr, pw := io.Pipe()
		closing := make(chan struct{})
		closed := make(chan struct{})
		r := wrpc.NewFrameStreamReader(ctx, struct {
			*bufio.Reader
			io.Closer
		}{bufio.NewReader(pr), closerFunc(func() error {
			close(closing)
			<-closed
			return pr.Close()
		})}, wrpc.NewSubscribePath().Index(0))
		sub := assertIndexReader(t, r, 0)
		writeFrames(t, pw, wrpc.Frame{Path: []uint32{0}, Data: []byte("x")})
		writeFrames(t, pw, wrpc.Frame{Path: []uint32{1}, Data: []byte("x")})
		<-closing
		assertClose(t, sub)
		close(closed)
		assertIngress(t, ingress, "unknown path")
		assertClose(t, r)
	})

	t.Run("not indexed", func(t *testing.T) {
		t.Parallel()
		ctx, ingress := withIngress()
		r := wrpc.NewFrameStreamReader(ctx, closedFrames(t,
			wrpc.Frame{Path: []uint32{0}, Data: []byte("hello")},
		), wrpc.NewSubscribePath().Index(0))
		assertReadAll(t, r, "", "")
		assertClose(t, r)
		assertIngress(t, ingress, "not processed")
	})

	for _, skip := range []bool{false, true} {
		t.Run(fmt.Sprintf("skipped=%v", skip), func(t *testing.T) {
			t.Parallel()
			ctx, ingress := withIngress()
			r := wrpc.NewFrameStreamReader(ctx, closedFrames(t,
				wrpc.Frame{Path: []uint32{1}, Data: []byte("x")},
				wrpc.Frame{Data: []byte("r")},
				wrpc.Frame{Path: []uint32{2}, Data: []byte("y")},
				wrpc.Frame{Path: []uint32{1}},
				wrpc.Frame{Path: []uint32{2}},
			), wrpc.NewSubscribePath().Wildcard())
			assertRead(t, r, "r")
			indices := []uint32{1, 2}
			if skip {
				indices = indices[1:]
			}
			for _, i := range indices {
				sub := assertIndexReader(t, r, i)
				if _, err := io.ReadAll(sub); err != nil && (!skip || !strings.Contains(err.Error(), "not processed")) {
					t.Fatalf("failed to read: %s", err)
				}
				assertClose(t, sub)
			}
			if skip {
				assertIngress(t, ingress, "not processed")
			}
			assertClose(t, r)
			if !skip {
				assertIngress(t, ingress, "")
			}
		})
	}

	for _, data := range []bool{false, true} {
		t.Run(fmt.Sprintf("unclaimed element data=%v", data), func(t *testing.T) {
			t.Parallel()
			ctx, ingress := withIngress()
			frames := make([]wrpc.Frame, 0, 65)
			for i := range uint32(65) {
				frames = append(frames, wrpc.Frame{Path: []uint32{i + 1}})
			}
			if data {
				frames[64].Data = []byte("x")
			}
			pr, pw := io.Pipe()
			r := wrpc.NewFrameStreamReader(ctx, pipeReader(pr), wrpc.NewSubscribePath().Wildcard())
			writeFrames(t, pw, frames...)
			time.Sleep(100 * time.Millisecond)
			assertClose(t, r)
			endTransmit(t, pw)
			assertIngress(t, ingress, "released element")
		})
	}

	t.Run("dropped reader race", func(t *testing.T) {
		t.Parallel()
		frame := encodeFrames(t, wrpc.Frame{Path: []uint32{0}, Data: []byte("x")}).Bytes()
		for range 20 {
			ctx, ingress := withIngress()
			pr, pw := io.Pipe()
			r := wrpc.NewFrameStreamReader(ctx, pipeReader(pr), wrpc.NewSubscribePath().Wildcard())
			sub := assertIndexReader(t, r, 0)
			if _, err := pw.Write(frame[:len(frame)-1]); err != nil {
				t.Fatalf("failed to write frame header: %s", err)
			}
			time.Sleep(10 * time.Millisecond)
			assertClose(t, sub)
			if _, err := pw.Write(frame[len(frame)-1:]); err != nil {
				t.Fatalf("failed to write frame data: %s", err)
			}
			assertClose(t, r)
			endTransmit(t, pw)
			assertIngress(t, ingress, "closed reader")
		}
	})
}

func TestFrameStreamReaderIndex(t *testing.T) {
	t.Run("empty path", func(t *testing.T) {
		t.Parallel()
		r := wrpc.NewFrameStreamReader(context.Background(), closedFrames(t, wrpc.Frame{Data: []byte("hello")}),
			wrpc.NewSubscribePath(),
			wrpc.NewSubscribePath().Index(0),
		)
		sub := assertIndexReader(t, r, 0)
		assertReadAll(t, r, "hello", "")
		for _, r := range []wrpc.IndexReader{r, sub} {
			if _, err := r.Index(); err == nil {
				t.Fatal("expected indexing an empty path to fail")
			}
		}
		assertClose(t, sub)
		assertClose(t, r)
	})

	t.Run("not found", func(t *testing.T) {
		t.Parallel()
		r := wrpc.NewFrameStreamReader(context.Background(), closedFrames(t,
			wrpc.Frame{Path: []uint32{0, 1}, Data: []byte("hello")},
			wrpc.Frame{Path: []uint32{0, 1}},
		), wrpc.NewSubscribePath().Index(0).Index(1))
		prefix := assertIndexReader(t, r, 0)
		_, err := prefix.ReadByte()
		assertErr(t, err, "not found")
		sub := assertIndexReader(t, prefix, 1)
		assertReadAll(t, sub, "hello", "")
		for _, path := range [][]uint32{{1}, {0, 1}} {
			_, err := r.Index(path...)
			assertErr(t, err, "not found")
		}
		assertClose(t, sub)
		assertClose(t, prefix)
		assertClose(t, r)
	})
}
