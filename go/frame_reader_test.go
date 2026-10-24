package wrpc_test

import (
	"context"
	"errors"
	"io"
	"strings"
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
			{},
			{Path: []uint32{0}, Data: []byte("world")},
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
		if err != nil {
			t.Fatalf("expected empty frame to close the path, got `%v`", err)
		}
		if string(buf) != "hello" {
			t.Fatalf("expected `hello`, got `%s`", buf)
		}
		buf, err = io.ReadAll(sub)
		if !errors.Is(err, io.ErrUnexpectedEOF) {
			t.Fatalf("expected the stream to end, got `%v`", err)
		}
		if string(buf) != "world" {
			t.Fatalf("expected `world`, got `%s`", buf)
		}
		assertClose(t, sub)
		assertClose(t, r)
	})

	t.Run("frame after close", func(t *testing.T) {
		t.Parallel()
		ctx := context.Background()
		var src sink
		for _, frame := range []wrpc.Frame{
			{},
			{Data: []byte("hello")},
			{Path: []uint32{0}, Data: []byte("world")},
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
		buf, err := io.ReadAll(sub)
		if err == nil || errors.Is(err, io.ErrUnexpectedEOF) || !strings.Contains(err.Error(), "closed path") {
			t.Fatalf("expected the stream to fail, got `%v`", err)
		}
		if len(buf) > 0 {
			t.Fatalf("expected no data, got `%s`", buf)
		}
		assertClose(t, sub)
		assertClose(t, r)
	})
}

func TestInvokeFramed(t *testing.T) {
	t.Run("empty params", func(t *testing.T) {
		t.Parallel()
		ctx := context.Background()
		var w, r sink
		tx, rx, err := wrpc.InvokeFramed(ctx, &w, &r, "foo", "bar", nil)
		if err != nil {
			t.Fatalf("failed to invoke: %s", err)
		}
		assertClose(t, tx)
		assertClose(t, rx)
		if got, expected := w.String(), "\x00\x03foo\x03bar"; got != expected {
			t.Fatalf("expected `%q`, got `%q`", expected, got)
		}
	})
}
