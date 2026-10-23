package wrpc_test

import (
	"context"
	"errors"
	"io"
	"testing"

	wrpc "wrpc.io/go"
)

func TestFrameStreamReader(t *testing.T) {
	t.Run("empty frame", func(t *testing.T) {
		t.Parallel()
		ctx := context.Background()
		var src sink
		for _, frame := range []wrpc.Frame{
			{Data: []byte("hello")},
			{Path: []uint32{5}},
			{Data: []byte("world")},
		} {
			if _, err := wrpc.WriteFrame(frame, &src); err != nil {
				t.Fatalf("failed to write frame: %s", err)
			}
		}

		r := wrpc.NewFrameStreamReader(ctx, &src)
		buf := make([]byte, 5)
		if _, err := io.ReadFull(r, buf); err != nil {
			t.Fatalf("failed to read: %s", err)
		}
		if string(buf) != "hello" {
			t.Fatalf("expected `hello`, got `%s`", buf)
		}
		if _, err := r.Read(buf); !errors.Is(err, io.ErrUnexpectedEOF) {
			t.Fatalf("expected empty frame to fail the stream, got `%v`", err)
		}
		assertClose(t, r)
	})
}

func TestInvokeFramed(t *testing.T) {
	t.Run("empty params", func(t *testing.T) {
		t.Parallel()
		ctx := context.Background()
		var w, r sink
		_, _, err := wrpc.InvokeFramed(ctx, &w, &r, "foo", "bar", nil)
		if err != nil {
			t.Fatalf("failed to invoke: %s", err)
		}
		if got, expected := w.String(), "\x00\x03foo\x03bar"; got != expected {
			t.Fatalf("expected `%q`, got `%q`", expected, got)
		}
	})
}
