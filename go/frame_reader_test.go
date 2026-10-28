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

func TestFrameStreamReader(t *testing.T) {
	t.Run("empty frame", func(t *testing.T) {
		t.Parallel()
		ctx := context.Background()
		var src sink
		for _, frame := range []wrpc.Frame{
			{Data: []byte("hello")},
			{Path: []uint32{0}, Data: []byte("world")},
			{Path: []uint32{0}},
			{Path: []uint32{0}, Data: []byte("!")},
		} {
			if _, err := wrpc.WriteFrame(frame, &src); err != nil {
				t.Fatalf("failed to write frame: %s", err)
			}
		}

		r := wrpc.NewFrameStreamReader(ctx, &src, wrpc.NewSubscribePath().Index(0))
		sub, err := r.Index(0)
		if err != nil {
			t.Fatalf("failed to index reader: %s", err)
		}
		buf, err := io.ReadAll(r)
		if err == nil || !strings.Contains(err.Error(), "closed path") {
			t.Fatalf("expected the stream to fail, got `%v`", err)
		}
		if string(buf) != "hello" {
			t.Fatalf("expected `hello`, got `%s`", buf)
		}
		buf, err = io.ReadAll(sub)
		if err != nil {
			t.Fatalf("expected empty frame to close the path, got `%v`", err)
		}
		if string(buf) != "world" {
			t.Fatalf("expected `world`, got `%s`", buf)
		}
		assertClose(t, sub)
		assertClose(t, r)
	})

	for _, data := range []bool{false, true} {
		t.Run(fmt.Sprintf("repeated empty frame data=%v", data), func(t *testing.T) {
			t.Parallel()
			ctx, ingress := withIngress()
			frames := []wrpc.Frame{
				{Path: []uint32{0}, Data: []byte("world")},
				{Path: []uint32{0}},
				{Path: []uint32{0}},
				{Data: []byte("hello")},
			}
			if data {
				frames = append(frames, wrpc.Frame{Path: []uint32{0}, Data: []byte("!")})
			}
			r := wrpc.NewFrameStreamReader(ctx, encodeFrames(t, frames...), wrpc.NewSubscribePath().Index(0))
			sub, err := r.Index(0)
			if err != nil {
				t.Fatalf("failed to index reader: %s", err)
			}
			buf, err := io.ReadAll(sub)
			if err != nil {
				t.Fatalf("expected empty frame to close the path, got `%v`", err)
			}
			if string(buf) != "world" {
				t.Fatalf("expected `world`, got `%s`", buf)
			}
			buf, err = io.ReadAll(r)
			if data {
				if err == nil || !strings.Contains(err.Error(), "closed path") {
					t.Fatalf("expected the stream to fail, got `%v`", err)
				}
			} else if err != nil {
				t.Fatalf("failed to read: %s", err)
			}
			if string(buf) != "hello" {
				t.Fatalf("expected `hello`, got `%s`", buf)
			}
			assertClose(t, sub)
			assertClose(t, r)
			if data {
				assertIngressFails(t, ingress, "closed path")
			} else {
				assertIngressSucceeds(t, ingress)
			}
		})
	}

	for _, data := range []bool{false, true} {
		t.Run(fmt.Sprintf("undeclared path data=%v", data), func(t *testing.T) {
			t.Parallel()
			ctx, ingress := withIngress()
			frames := []wrpc.Frame{
				{Path: []uint32{1}},
				{Path: []uint32{0, 1}},
				{Path: []uint32{0}, Data: []byte("world")},
				{Data: []byte("hello")},
			}
			if data {
				frames = append(frames, wrpc.Frame{Path: []uint32{1}, Data: []byte("!")})
			}
			r := wrpc.NewFrameStreamReader(ctx, encodeFrames(t, frames...), wrpc.NewSubscribePath().Index(0))
			sub, err := r.Index(0)
			if err != nil {
				t.Fatalf("failed to index reader: %s", err)
			}
			buf, err := io.ReadAll(r)
			if data {
				if err == nil || !strings.Contains(err.Error(), "unknown path") {
					t.Fatalf("expected the stream to fail, got `%v`", err)
				}
			} else if err != nil {
				t.Fatalf("failed to read: %s", err)
			}
			if string(buf) != "hello" {
				t.Fatalf("expected `hello`, got `%s`", buf)
			}
			buf = make([]byte, 5)
			if _, err := io.ReadFull(sub, buf); err != nil {
				t.Fatalf("failed to read: %s", err)
			}
			if string(buf) != "world" {
				t.Fatalf("expected `world`, got `%s`", buf)
			}
			assertClose(t, sub)
			assertClose(t, r)
			if data {
				assertIngressFails(t, ingress, "unknown path")
			} else {
				assertIngressSucceeds(t, ingress)
			}
		})
	}

	t.Run("closed reader", func(t *testing.T) {
		t.Parallel()
		ctx := context.Background()
		pr, pw := io.Pipe()
		r := wrpc.NewFrameStreamReader(ctx, struct {
			*bufio.Reader
			io.Closer
		}{bufio.NewReader(pr), pr},
			wrpc.NewSubscribePath().Index(0),
			wrpc.NewSubscribePath().Index(1),
		)
		for _, i := range []uint32{0, 1} {
			sub, err := r.Index(i)
			if err != nil {
				t.Fatalf("failed to index reader: %s", err)
			}
			assertClose(t, sub)
		}
		var src sink
		for _, frame := range []wrpc.Frame{
			{Path: []uint32{0}},
			{Data: []byte("hello")},
			{Path: []uint32{1}, Data: []byte("world")},
		} {
			if _, err := wrpc.WriteFrame(frame, &src); err != nil {
				t.Fatalf("failed to write frame: %s", err)
			}
		}
		go func() {
			_, err := src.WriteTo(pw)
			pw.CloseWithError(err)
		}()
		buf, err := io.ReadAll(r)
		if err == nil || !strings.Contains(err.Error(), "closed reader") {
			t.Fatalf("expected the stream to fail, got `%v`", err)
		}
		if string(buf) != "hello" {
			t.Fatalf("expected `hello`, got `%s`", buf)
		}
		assertClose(t, r)
	})

	t.Run("close twice", func(t *testing.T) {
		t.Parallel()
		ctx := context.Background()
		pr, pw := io.Pipe()
		r := wrpc.NewFrameStreamReader(ctx, struct {
			*bufio.Reader
			io.Closer
		}{bufio.NewReader(pr), pr},
			wrpc.NewSubscribePath().Index(0),
			wrpc.NewSubscribePath().Index(1),
		)
		sub0, err := r.Index(0)
		if err != nil {
			t.Fatalf("failed to index reader: %s", err)
		}
		sub1, err := r.Index(1)
		if err != nil {
			t.Fatalf("failed to index reader: %s", err)
		}
		assertClose(t, r)
		assertClose(t, sub0)
		assertClose(t, sub0)
		var src sink
		for _, frame := range []wrpc.Frame{
			{Path: []uint32{0}},
			{Path: []uint32{1}, Data: []byte("hello")},
			{Path: []uint32{1}, Data: []byte("world")},
		} {
			if _, err := wrpc.WriteFrame(frame, &src); err != nil {
				t.Fatalf("failed to write frame: %s", err)
			}
		}
		go func() {
			_, err := src.WriteTo(pw)
			pw.CloseWithError(err)
		}()
		buf, err := io.ReadAll(sub1)
		if err != nil {
			t.Fatalf("failed to read: %s", err)
		}
		if string(buf) != "helloworld" {
			t.Fatalf("expected `helloworld`, got `%s`", buf)
		}
		assertClose(t, sub1)
	})

	t.Run("wildcard elements", func(t *testing.T) {
		t.Parallel()
		ctx := context.Background()
		pr, pw := io.Pipe()
		r := wrpc.NewFrameStreamReader(ctx, struct {
			*bufio.Reader
			io.Closer
		}{bufio.NewReader(pr), pr}, wrpc.NewSubscribePath().Index(0).Wildcard())
		write := func(frame wrpc.Frame) {
			var buf bytes.Buffer
			if _, err := wrpc.WriteFrame(frame, &buf); err != nil {
				t.Errorf("failed to encode frame: %s", err)
			}
			if _, err := pw.Write(buf.Bytes()); err != nil {
				t.Errorf("failed to write frame: %s", err)
			}
		}
		write(wrpc.Frame{Path: []uint32{0, 0}, Data: []byte("x")})
		write(wrpc.Frame{Path: []uint32{0, 2}, Data: []byte("b")})
		sub, err := r.Index(0, 2)
		if err != nil {
			t.Fatalf("failed to index reader: %s", err)
		}
		buf := make([]byte, 1)
		if _, err := io.ReadFull(sub, buf); err != nil {
			t.Fatalf("failed to read: %s", err)
		}
		if string(buf) != "b" {
			t.Fatalf("expected `b`, got `%s`", buf)
		}
		for _, i := range []uint32{0, 1} {
			if _, err := r.Index(0, i); err == nil || !strings.Contains(err.Error(), "released") {
				t.Fatalf("expected indexing released element %d to fail, got `%v`", i, err)
			}
		}
		assertClose(t, sub)
		go func() {
			write(wrpc.Frame{Path: []uint32{0, 0}, Data: []byte("y")})
			pw.Close()
		}()
		if _, err := io.ReadAll(r); err == nil || !strings.Contains(err.Error(), "released element") {
			t.Fatalf("expected the stream to fail, got `%v`", err)
		}
		assertClose(t, r)
	})

	t.Run("full buffer", func(t *testing.T) {
		t.Parallel()
		ctx := context.Background()
		var src sink
		for range 100 {
			if _, err := wrpc.WriteFrame(wrpc.Frame{Path: []uint32{0}, Data: []byte("x")}, &src); err != nil {
				t.Fatalf("failed to write frame: %s", err)
			}
		}
		r := wrpc.NewFrameStreamReader(ctx, &src, wrpc.NewSubscribePath().Index(0))
		sub, err := r.Index(0)
		if err != nil {
			t.Fatalf("failed to index reader: %s", err)
		}
		buf, err := io.ReadAll(sub)
		if err != nil {
			t.Fatalf("failed to read: %s", err)
		}
		if string(buf) != strings.Repeat("x", 100) {
			t.Fatalf("expected 100 bytes, got `%s`", buf)
		}
		assertClose(t, sub)
		assertClose(t, r)
	})

	t.Run("unclaimed elements", func(t *testing.T) {
		t.Parallel()
		ctx := context.Background()
		pr, pw := io.Pipe()
		r := wrpc.NewFrameStreamReader(ctx, struct {
			*bufio.Reader
			io.Closer
		}{bufio.NewReader(pr), pr}, wrpc.NewSubscribePath().Wildcard())
		go func() {
			var buf bytes.Buffer
			for i := range uint32(64) {
				if _, err := wrpc.WriteFrame(wrpc.Frame{Path: []uint32{i + 1}, Data: []byte("x")}, &buf); err != nil {
					t.Errorf("failed to encode frame: %s", err)
				}
			}
			for _, frame := range []wrpc.Frame{
				{Path: []uint32{0}, Data: []byte("a")},
				{Path: []uint32{65}, Data: []byte("b")},
			} {
				if _, err := wrpc.WriteFrame(frame, &buf); err != nil {
					t.Errorf("failed to encode frame: %s", err)
				}
			}
			if _, err := pw.Write(buf.Bytes()); err != nil {
				t.Errorf("failed to write frames: %s", err)
			}
			pw.Close()
		}()
		for _, frame := range []struct {
			i    uint32
			data string
		}{{0, "a"}, {65, "b"}} {
			sub, err := r.Index(frame.i)
			if err != nil {
				t.Fatalf("failed to index reader: %s", err)
			}
			buf := make([]byte, 1)
			if _, err := io.ReadFull(sub, buf); err != nil {
				t.Fatalf("failed to read: %s", err)
			}
			if string(buf) != frame.data {
				t.Fatalf("expected `%s`, got `%s`", frame.data, buf)
			}
			assertClose(t, sub)
		}
		if _, err := io.ReadAll(r); err != nil {
			t.Fatalf("failed to read: %s", err)
		}
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
	ctx := context.Background()
	data := bytes.Repeat([]byte{0x2a}, 20<<20)
	var src sink
	for _, frame := range []wrpc.Frame{
		{Path: []uint32{0}, Data: data},
		{Data: []byte("hello")},
	} {
		if _, err := wrpc.WriteFrame(frame, &src); err != nil {
			t.Fatalf("failed to write frame: %s", err)
		}
	}
	rd := &countingReader{Reader: bytes.NewReader(src.Bytes())}
	r := wrpc.NewFrameStreamReader(ctx, rd, wrpc.NewSubscribePath().Index(0))
	sub, err := r.Index(0)
	if err != nil {
		t.Fatalf("failed to index reader: %s", err)
	}
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
	buf, err = io.ReadAll(r)
	if err != nil {
		t.Fatalf("failed to read root: %s", err)
	}
	if string(buf) != "hello" {
		t.Fatalf("expected `hello`, got `%s`", buf)
	}
}

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

func awaitIngress(t *testing.T, ch <-chan error) error {
	t.Helper()
	select {
	case err := <-ch:
		return err
	case <-time.After(5 * time.Second):
		t.Fatal("ingress did not complete")
		return nil
	}
}

func assertIngressFails(t *testing.T, ch <-chan error, msg string) {
	t.Helper()
	if err := awaitIngress(t, ch); err == nil || !strings.Contains(err.Error(), msg) {
		t.Fatalf("expected ingress to fail with `%s`, got `%v`", msg, err)
	}
}

func assertIngressSucceeds(t *testing.T, ch <-chan error) {
	t.Helper()
	if err := awaitIngress(t, ch); err != nil {
		t.Fatalf("expected ingress to succeed, got `%s`", err)
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

func TestFrameStreamReaderDrain(t *testing.T) {
	t.Run("empty frames", func(t *testing.T) {
		t.Parallel()
		ctx, ingress := withIngress()
		pr, pw := io.Pipe()
		r := wrpc.NewFrameStreamReader(ctx, pipeReader(pr), wrpc.NewSubscribePath().Index(0))
		sub, err := r.Index(0)
		if err != nil {
			t.Fatalf("failed to index reader: %s", err)
		}
		writeFrames(t, pw, wrpc.Frame{Data: []byte("hello")})
		buf := make([]byte, 5)
		if _, err := io.ReadFull(r, buf); err != nil {
			t.Fatalf("failed to read: %s", err)
		}
		assertClose(t, sub)
		assertClose(t, r)
		writeFrames(t, pw, wrpc.Frame{Path: []uint32{0}})
		writeFrames(t, pw, wrpc.Frame{})
		pw.Close()
		assertIngressSucceeds(t, ingress)
	})

	t.Run("data", func(t *testing.T) {
		t.Parallel()
		ctx, ingress := withIngress()
		pr, pw := io.Pipe()
		r := wrpc.NewFrameStreamReader(ctx, pipeReader(pr))
		assertClose(t, r)
		writeFrames(t, pw, wrpc.Frame{Data: []byte("hello")})
		assertIngressFails(t, ingress, "closed reader")
		pw.Close()
	})

	t.Run("buffered", func(t *testing.T) {
		t.Parallel()
		ctx, ingress := withIngress()
		r := wrpc.NewFrameStreamReader(ctx, encodeFrames(t, wrpc.Frame{Data: []byte("hello")}))
		buf := make([]byte, 2)
		if _, err := io.ReadFull(r, buf); err != nil {
			t.Fatalf("failed to read: %s", err)
		}
		assertClose(t, r)
		assertIngressFails(t, ingress, "not processed")
	})

	t.Run("not indexed", func(t *testing.T) {
		t.Parallel()
		ctx, ingress := withIngress()
		r := wrpc.NewFrameStreamReader(ctx, encodeFrames(t,
			wrpc.Frame{Path: []uint32{0}, Data: []byte("hello")},
		), wrpc.NewSubscribePath().Index(0))
		if _, err := io.ReadAll(r); err != nil {
			t.Fatalf("failed to read: %s", err)
		}
		assertClose(t, r)
		assertIngressFails(t, ingress, "not processed")
	})

	for _, skip := range []bool{false, true} {
		t.Run(fmt.Sprintf("skipped=%v", skip), func(t *testing.T) {
			t.Parallel()
			ctx, ingress := withIngress()
			r := wrpc.NewFrameStreamReader(ctx, encodeFrames(t,
				wrpc.Frame{Path: []uint32{1}, Data: []byte("x")},
				wrpc.Frame{Data: []byte("r")},
				wrpc.Frame{Path: []uint32{2}, Data: []byte("y")},
			), wrpc.NewSubscribePath().Wildcard())
			buf := make([]byte, 1)
			if _, err := io.ReadFull(r, buf); err != nil {
				t.Fatalf("failed to read: %s", err)
			}
			indices := []uint32{1, 2}
			if skip {
				indices = []uint32{2}
			}
			for _, i := range indices {
				sub, err := r.Index(i)
				if err != nil {
					t.Fatalf("failed to index reader: %s", err)
				}
				if _, err := io.ReadAll(sub); err != nil {
					t.Fatalf("failed to read: %s", err)
				}
				assertClose(t, sub)
			}
			assertClose(t, r)
			if skip {
				assertIngressFails(t, ingress, "not processed")
			} else {
				assertIngressSucceeds(t, ingress)
			}
		})
	}

	for _, data := range []bool{false, true} {
		t.Run(fmt.Sprintf("unclaimed element data=%v", data), func(t *testing.T) {
			t.Parallel()
			ctx, ingress := withIngress()
			frames := make([]wrpc.Frame, 0, 65)
			for i := range uint32(64) {
				frames = append(frames, wrpc.Frame{Path: []uint32{i + 1}})
			}
			last := wrpc.Frame{Path: []uint32{65}}
			if data {
				last.Data = []byte("x")
			}
			pr, pw := io.Pipe()
			r := wrpc.NewFrameStreamReader(ctx, pipeReader(pr), wrpc.NewSubscribePath().Wildcard())
			writeFrames(t, pw, append(frames, last)...)
			time.Sleep(100 * time.Millisecond)
			assertClose(t, r)
			pw.Close()
			if data {
				assertIngressFails(t, ingress, "released element")
			} else {
				assertIngressSucceeds(t, ingress)
			}
		})
	}

	t.Run("dropped reader race", func(t *testing.T) {
		t.Parallel()
		frame := encodeFrames(t, wrpc.Frame{Path: []uint32{0}, Data: []byte("x")}).Bytes()
		for range 20 {
			ctx, ingress := withIngress()
			pr, pw := io.Pipe()
			r := wrpc.NewFrameStreamReader(ctx, pipeReader(pr), wrpc.NewSubscribePath().Wildcard())
			sub, err := r.Index(0)
			if err != nil {
				t.Fatalf("failed to index reader: %s", err)
			}
			if _, err := pw.Write(frame[:len(frame)-1]); err != nil {
				t.Fatalf("failed to write frame header: %s", err)
			}
			time.Sleep(10 * time.Millisecond)
			assertClose(t, sub)
			if _, err := pw.Write(frame[len(frame)-1:]); err != nil {
				t.Fatalf("failed to write frame data: %s", err)
			}
			assertClose(t, r)
			pw.Close()
			assertIngressFails(t, ingress, "closed reader")
		}
	})
}

func TestFrameStreamReaderIndex(t *testing.T) {
	t.Run("empty path", func(t *testing.T) {
		t.Parallel()
		r := wrpc.NewFrameStreamReader(context.Background(), encodeFrames(t), wrpc.NewSubscribePath().Index(0))
		sub, err := r.Index(0)
		if err != nil {
			t.Fatalf("failed to index reader: %s", err)
		}
		for _, r := range []wrpc.IndexReader{r, sub} {
			if _, err := r.Index(); err == nil {
				t.Fatal("expected indexing an empty path to fail")
			}
		}
		assertClose(t, sub)
		assertClose(t, r)
	})

	t.Run("prefix", func(t *testing.T) {
		t.Parallel()
		r := wrpc.NewFrameStreamReader(context.Background(), encodeFrames(t,
			wrpc.Frame{Path: []uint32{0, 1}, Data: []byte("hello")},
		), wrpc.NewSubscribePath().Index(0).Index(1))
		prefix, err := r.Index(0)
		if err != nil {
			t.Fatalf("failed to index prefix: %s", err)
		}
		if _, err := prefix.ReadByte(); err == nil || !strings.Contains(err.Error(), "not found") {
			t.Fatalf("expected reading a prefix to fail, got `%v`", err)
		}
		sub, err := prefix.Index(1)
		if err != nil {
			t.Fatalf("failed to index reader: %s", err)
		}
		buf, err := io.ReadAll(sub)
		if err != nil {
			t.Fatalf("failed to read: %s", err)
		}
		if string(buf) != "hello" {
			t.Fatalf("expected `hello`, got `%s`", buf)
		}
		if _, err := r.Index(1); err == nil || !strings.Contains(err.Error(), "not found") {
			t.Fatalf("expected indexing an unknown path to fail, got `%v`", err)
		}
		assertClose(t, sub)
		assertClose(t, prefix)
		assertClose(t, r)
	})

	t.Run("taken", func(t *testing.T) {
		t.Parallel()
		r := wrpc.NewFrameStreamReader(context.Background(), encodeFrames(t), wrpc.NewSubscribePath().Index(0))
		sub, err := r.Index(0)
		if err != nil {
			t.Fatalf("failed to index reader: %s", err)
		}
		if _, err := r.Index(0); err == nil || !strings.Contains(err.Error(), "not found") {
			t.Fatalf("expected indexing a taken path to fail, got `%v`", err)
		}
		assertClose(t, sub)
		assertClose(t, r)
	})
}
