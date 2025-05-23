package wrpc_test

import (
	"bytes"
	"context"
	"io"
	"slices"
	"testing"

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
	t.Run("hello", func(t *testing.T) {
		t.Parallel()
		ctx := context.Background()
		var dst sink
		w := wrpc.NewFrameStreamWriter(ctx, &dst)

		assertWriteString(t, w, "hello")
		assertClose(t, w)

		r := bytes.NewBuffer(dst.Bytes())
		assertReadFrame(t, r, []byte("hello"))

		if n := r.Len(); n > 0 {
			t.Fatalf("%v trailing bytes left in buffer: %s", n, r.String())
		}
	})

	t.Run("nested", func(t *testing.T) {
		t.Parallel()
		ctx := context.Background()
		var dst sink

		w := wrpc.NewFrameStreamWriter(ctx, &dst)

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
}
